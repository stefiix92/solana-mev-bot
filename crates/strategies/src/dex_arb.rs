use std::sync::Arc;

use anyhow::Result;
use arc_swap::ArcSwap;
use solana_sdk::pubkey::Pubkey;
use tracing::{debug, trace};

use mev_account_cache::cache::AccountCache;
use mev_common::constants;
use mev_common::types::{AccountUpdate, DexType, Opportunity, PoolState, SwapStep};
use mev_dex_adapters::meteora_dlmm::MeteoraDlmmAdapter;
use mev_dex_adapters::orca_whirlpool::OrcaWhirlpoolAdapter;
use mev_dex_adapters::phoenix::PhoenixAdapter;
use mev_dex_adapters::raydium_amm::RaydiumAmmAdapter;
use mev_dex_adapters::traits::DexAdapter;
use mev_price_graph::bellman_ford::{find_cycles_dfs, BellmanFordState};
use mev_price_graph::graph::PriceGraph;

use crate::traits::Strategy;

/// DEX arbitrage strategy.
///
/// On each pool account update:
/// 1. Decode the pool state
/// 2. Update the price graph
/// 3. Run arb detection (DFS for 2-3 hop cycles)
/// 4. For profitable cycles, simulate exact amounts and emit an Opportunity
pub struct DexArbStrategy {
    /// Shared price graph (ArcSwap for lock-free reads/writes from single thread)
    graph: Arc<ArcSwap<PriceGraph>>,
    /// Mutable graph for updates (owned by this strategy's task)
    mutable_graph: PriceGraph,
    /// DEX adapters
    adapters: Vec<Box<dyn DexAdapter>>,
    /// Account cache for reading vault balances
    cache: AccountCache,
    /// Config
    min_profit_lamports: i64,
    max_hops: usize,
    /// Anchor token indices (SOL, USDC, USDT) — arb cycles start/end at these
    anchor_mints: Vec<Pubkey>,
    /// Bellman-Ford pre-allocated state
    bf_state: BellmanFordState,
}

impl DexArbStrategy {
    pub fn new(
        cache: AccountCache,
        min_profit_lamports: i64,
        max_hops: usize,
        anchor_mints: Vec<Pubkey>,
    ) -> Self {
        let graph = PriceGraph::new();

        Self {
            graph: Arc::new(ArcSwap::from_pointee(graph.clone())),
            mutable_graph: graph,
            adapters: vec![
                Box::new(RaydiumAmmAdapter),
                Box::new(OrcaWhirlpoolAdapter),
                Box::new(MeteoraDlmmAdapter),
                Box::new(PhoenixAdapter),
            ],
            cache,
            min_profit_lamports,
            max_hops,
            anchor_mints,
            bf_state: BellmanFordState::new(1_000),
        }
    }

    /// Get a shared reference to the price graph (for other components).
    pub fn shared_graph(&self) -> Arc<ArcSwap<PriceGraph>> {
        Arc::clone(&self.graph)
    }

    /// Try to decode an account update as a pool from any known DEX.
    fn try_decode_pool(&self, update: &AccountUpdate) -> Option<PoolState> {
        for adapter in &self.adapters {
            if update.owner == adapter.program_id() {
                if let Ok(Some(mut pool)) = adapter.decode_pool(&update.pubkey, &update.data) {
                    pool.slot = update.slot;
                    // Try to populate reserves from vault account cache
                    self.populate_reserves(&mut pool);
                    return Some(pool);
                }
            }
        }
        None
    }

    /// Read vault balances from the account cache to populate pool reserves.
    fn populate_reserves(&self, pool: &mut PoolState) {
        if let Some(vault_a) = self.cache.get_data(&pool.token_a_vault) {
            if let Some(amount) = parse_token_account_amount(&vault_a) {
                pool.token_a_amount = amount;
            }
        }
        if let Some(vault_b) = self.cache.get_data(&pool.token_b_vault) {
            if let Some(amount) = parse_token_account_amount(&vault_b) {
                pool.token_b_amount = amount;
            }
        }
    }

    /// Find arb opportunities after a graph update.
    fn find_opportunities(&mut self) -> Vec<Opportunity> {
        let mut opportunities = Vec::new();

        // Search from each anchor mint
        for anchor in &self.anchor_mints {
            if let Some(anchor_idx) = self.mutable_graph.tokens.get_index(anchor) {
                let cycles = find_cycles_dfs(
                    &self.mutable_graph,
                    anchor_idx,
                    self.max_hops,
                    0.0001, // min 0.01% profit ratio threshold (before amount simulation)
                );

                for cycle in cycles {
                    // Simulate with actual amounts to get real profit
                    if let Some(opp) = self.simulate_cycle(&cycle, anchor) {
                        if opp.expected_profit_lamports >= self.min_profit_lamports {
                            opportunities.push(opp);
                        }
                    }
                }
            }
        }

        opportunities
    }

    /// Simulate a cycle with actual amounts through the pool math.
    fn simulate_cycle(
        &self,
        arb_path: &mev_price_graph::path::ArbPath,
        anchor_mint: &Pubkey,
    ) -> Option<Opportunity> {
        // Start with a test amount (1 SOL = 1_000_000_000 lamports)
        let start_amount: u64 = 1_000_000_000;
        let mut current_amount = start_amount;
        let mut steps = Vec::new();

        for hop in &arb_path.hops {
            let adapter = self.adapters.iter().find(|a| a.dex_type() == hop.dex_type)?;

            // Get current pool state from graph edge
            let edge = &self.mutable_graph.edges[hop.edge_index];

            // Build a PoolState for quoting
            // We need the actual pool state from the cache or the graph
            let pool_data = self.cache.get_data(&hop.pool_address)?;
            let pool = adapter.decode_pool(&hop.pool_address, &pool_data).ok()??;

            let quote = adapter.quote(&pool, &hop.input_mint, current_amount).ok()?;

            if quote.amount_out == 0 {
                return None;
            }

            // Apply 0.5% slippage buffer for min_amount_out
            let min_out = quote.amount_out * 995 / 1000;

            steps.push(SwapStep {
                pool_address: hop.pool_address,
                dex_type: hop.dex_type,
                input_mint: hop.input_mint,
                output_mint: hop.output_mint,
                amount_in: current_amount,
                min_amount_out: min_out,
                instructions: Vec::new(), // Built by executor
            });

            current_amount = quote.amount_out;
        }

        let profit = current_amount as i64 - start_amount as i64;

        Some(Opportunity {
            strategy: "dex_arb".to_string(),
            path: steps,
            expected_profit_lamports: profit,
            estimated_compute_units: 200_000 * arb_path.num_hops() as u32,
            detected_at_slot: 0,
        })
    }
}

impl Strategy for DexArbStrategy {
    fn name(&self) -> &str {
        "dex_arb"
    }

    fn evaluate(&self, update: &AccountUpdate) -> Result<Option<Opportunity>> {
        // This is called from the hot path — we need mutable access to the graph
        // In a real implementation, this would use interior mutability (RefCell or similar)
        // For now, return None and use the run loop pattern instead
        Ok(None)
    }
}

impl DexArbStrategy {
    /// Main processing loop: receive updates, update graph, find arbs.
    /// Returns opportunities when found.
    pub fn process_update(&mut self, update: &AccountUpdate) -> Vec<Opportunity> {
        // Try to decode as a pool
        if let Some(pool) = self.try_decode_pool(update) {
            trace!(
                pool = %pool.address,
                dex = %pool.dex_type,
                reserve_a = pool.token_a_amount,
                reserve_b = pool.token_b_amount,
                "Pool state updated"
            );

            // Update the mutable graph
            self.mutable_graph.update_pool(&pool);

            // Publish updated graph for other readers
            self.graph.store(Arc::new(self.mutable_graph.clone()));

            // Search for arb opportunities
            return self.find_opportunities();
        }

        Vec::new()
    }

    /// Get current graph stats.
    pub fn graph_stats(&self) -> (usize, usize) {
        (self.mutable_graph.num_tokens(), self.mutable_graph.num_edges())
    }
}

/// Parse an SPL token account to extract the token amount.
/// SPL Token account layout: ... amount at offset 64 (u64 LE).
fn parse_token_account_amount(data: &[u8]) -> Option<u64> {
    if data.len() < 72 {
        return None;
    }
    let amount_bytes: [u8; 8] = data[64..72].try_into().ok()?;
    Some(u64::from_le_bytes(amount_bytes))
}
