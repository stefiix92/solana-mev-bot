use std::sync::Arc;

use anyhow::Result;
use arc_swap::ArcSwap;
use solana_sdk::pubkey::Pubkey;
use tracing::{debug, info, trace};

use mev_account_cache::cache::AccountCache;
use mev_common::constants;
use mev_common::types::{DexType, Opportunity, SwapStep};
use mev_dex_adapters::traits::DexAdapter;
use mev_dex_adapters::raydium_amm::RaydiumAmmAdapter;
use mev_dex_adapters::orca_whirlpool::OrcaWhirlpoolAdapter;
use mev_dex_adapters::meteora_dlmm::MeteoraDlmmAdapter;
use mev_dex_adapters::phoenix::PhoenixAdapter;
use mev_price_graph::bellman_ford::find_cycles_dfs;
use mev_price_graph::graph::PriceGraph;

use crate::traits::Strategy;

/// Backrunning strategy.
///
/// Detects large swaps that create price imbalances, then constructs an
/// arbitrage transaction to execute immediately after the target swap.
///
/// Flow:
/// 1. Detect a large swap in the transaction stream
/// 2. Simulate the post-swap pool state
/// 3. Check if the price imbalance creates an arb opportunity
/// 4. If profitable, construct a bundle: [target_tx, our_arb_tx]
pub struct BackrunStrategy {
    /// Shared price graph (read-only, updated by dex_arb strategy)
    graph: Arc<ArcSwap<PriceGraph>>,
    /// DEX adapters for quoting
    adapters: Vec<Box<dyn DexAdapter>>,
    /// Account cache
    cache: AccountCache,
    /// Config
    min_trade_size_lamports: u64,
    min_profit_lamports: i64,
    max_slippage_bps: u16,
    /// Anchor mints for arb detection
    anchor_mints: Vec<Pubkey>,
    /// Known DEX program IDs
    dex_programs: Vec<Pubkey>,
}

impl BackrunStrategy {
    pub fn new(
        graph: Arc<ArcSwap<PriceGraph>>,
        cache: AccountCache,
        min_trade_size_lamports: u64,
        min_profit_lamports: i64,
        max_slippage_bps: u16,
        anchor_mints: Vec<Pubkey>,
    ) -> Self {
        Self {
            graph,
            adapters: vec![
                Box::new(RaydiumAmmAdapter),
                Box::new(OrcaWhirlpoolAdapter),
                Box::new(MeteoraDlmmAdapter),
                Box::new(PhoenixAdapter),
            ],
            cache,
            min_trade_size_lamports,
            min_profit_lamports,
            max_slippage_bps,
            anchor_mints,
            dex_programs: vec![
                constants::RAYDIUM_AMM_V4,
                constants::RAYDIUM_CLMM,
                constants::ORCA_WHIRLPOOL,
                constants::METEORA_DLMM,
                constants::PHOENIX,
            ],
        }
    }

    /// Known DEX program IDs for swap detection.
    pub fn dex_programs(&self) -> &[Pubkey] {
        &self.dex_programs
    }

    /// Evaluate a detected swap for backrunning opportunity.
    ///
    /// Given a large swap that just happened on a DEX, check if the resulting
    /// price imbalance creates an arb opportunity across other DEXs.
    pub fn evaluate_swap(
        &self,
        swap_pool: &Pubkey,
        swap_program: &Pubkey,
        estimated_size: u64,
    ) -> Option<Opportunity> {
        if estimated_size < self.min_trade_size_lamports {
            return None;
        }

        trace!(
            pool = %swap_pool,
            program = %swap_program,
            size = estimated_size,
            "Evaluating swap for backrun"
        );

        // Load current graph snapshot
        let graph = self.graph.load();

        // Check if the swap pool is in our graph
        // After the swap, the pool's prices will be skewed
        // Look for arb cycles that include this pool's token pair

        // For each anchor mint, search for profitable cycles
        for anchor in &self.anchor_mints {
            if let Some(anchor_idx) = graph.tokens.get_index(anchor) {
                let cycles = find_cycles_dfs(&graph, anchor_idx, 3, 0.0001);

                for cycle in cycles {
                    // Check if any hop in this cycle involves the swapped pool
                    let involves_pool = cycle.hops.iter().any(|h| h.pool_address == *swap_pool);

                    // Even if it doesn't directly involve the pool, a large swap
                    // can create ripple effects across related pools
                    if cycle.profit_ratio > 1.0 {
                        // Simulate with actual amounts
                        if let Some(opp) = self.simulate_backrun_cycle(&graph, &cycle, anchor) {
                            if opp.expected_profit_lamports >= self.min_profit_lamports {
                                info!(
                                    profit = opp.expected_profit_lamports,
                                    hops = opp.path.len(),
                                    involves_swap_pool = involves_pool,
                                    "Backrun opportunity detected"
                                );
                                return Some(opp);
                            }
                        }
                    }
                }
            }
        }

        None
    }

    fn simulate_backrun_cycle(
        &self,
        graph: &PriceGraph,
        arb_path: &mev_price_graph::path::ArbPath,
        anchor_mint: &Pubkey,
    ) -> Option<Opportunity> {
        let start_amount: u64 = 1_000_000_000; // 1 SOL
        let mut current_amount = start_amount;
        let mut steps = Vec::new();

        for hop in &arb_path.hops {
            let adapter = self.adapters.iter().find(|a| a.dex_type() == hop.dex_type)?;
            let pool_data = self.cache.get_data(&hop.pool_address)?;
            let pool = adapter.decode_pool(&hop.pool_address, &pool_data).ok()??;
            let quote = adapter.quote(&pool, &hop.input_mint, current_amount).ok()?;

            if quote.amount_out == 0 {
                return None;
            }

            let slippage_factor = 10_000 - self.max_slippage_bps as u64;
            let min_out = quote.amount_out * slippage_factor / 10_000;

            steps.push(SwapStep {
                pool_address: hop.pool_address,
                dex_type: hop.dex_type,
                input_mint: hop.input_mint,
                output_mint: hop.output_mint,
                amount_in: current_amount,
                min_amount_out: min_out,
                instructions: Vec::new(),
            });

            current_amount = quote.amount_out;
        }

        let profit = current_amount as i64 - start_amount as i64;

        Some(Opportunity {
            strategy: "backrun".to_string(),
            path: steps,
            expected_profit_lamports: profit,
            estimated_compute_units: 300_000 * arb_path.num_hops() as u32,
            detected_at_slot: 0,
        })
    }
}

impl Strategy for BackrunStrategy {
    fn name(&self) -> &str {
        "backrun"
    }

    fn evaluate(&self, _update: &mev_common::types::AccountUpdate) -> Result<Option<Opportunity>> {
        // Backrunning uses transaction stream, not account updates directly
        Ok(None)
    }
}
