use std::sync::Arc;

use anyhow::Result;
use arc_swap::ArcSwap;
use solana_sdk::pubkey::Pubkey;
use tracing::{debug, trace};

use mev_account_cache::cache::AccountCache;
use mev_common::constants;
use mev_common::types::{AccountUpdate, DexType, Opportunity, PoolState, SwapStep};
use mev_data_feed::vault_tracker::VaultTracker;
use mev_dex_adapters::meteora_dlmm::MeteoraDlmmAdapter;
use mev_dex_adapters::orca_whirlpool::OrcaWhirlpoolAdapter;
use mev_dex_adapters::phoenix::PhoenixAdapter;
use mev_dex_adapters::raydium_amm::RaydiumAmmAdapter;
use mev_dex_adapters::raydium_clmm::RaydiumClmmAdapter;
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
    /// Tracks discovered vault pubkeys for reserve population
    vault_tracker: VaultTracker,
    /// Config
    min_profit_lamports: i64,
    max_hops: usize,
    /// Anchor token indices (SOL, USDC, USDT) — arb cycles start/end at these
    anchor_mints: Vec<Pubkey>,
    /// Bellman-Ford pre-allocated state
    bf_state: BellmanFordState,
    /// Counter for batching graph snapshot publishes
    updates_since_publish: u32,
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
                Box::new(RaydiumClmmAdapter),
                Box::new(OrcaWhirlpoolAdapter),
                Box::new(MeteoraDlmmAdapter),
                Box::new(PhoenixAdapter),
            ],
            cache,
            vault_tracker: VaultTracker::new(),
            min_profit_lamports,
            max_hops,
            anchor_mints,
            bf_state: BellmanFordState::new(1_000),
            updates_since_publish: 0,
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
                    // Register vault addresses for tracking
                    self.vault_tracker.register_vaults(&pool.token_a_vault, &pool.token_b_vault);
                    // Populate reserves from vault account cache
                    self.populate_reserves(&mut pool);
                    return Some(pool);
                }
            }
        }

        // Check if this is a vault account update (SPL token account)
        // If so, the vault balance changed — which may create arb opportunities
        // We don't return a pool here, but the cache will be updated by the cache updater
        None
    }

    /// Read vault balances from the account cache to populate pool reserves.
    fn populate_reserves(&self, pool: &mut PoolState) {
        if let Some(vault_a) = self.cache.get_data(&pool.token_a_vault) {
            if let Some(amount) = mev_data_feed::vault_tracker::parse_token_account_amount(&vault_a) {
                pool.token_a_amount = amount;
            }
        }
        if let Some(vault_b) = self.cache.get_data(&pool.token_b_vault) {
            if let Some(amount) = mev_data_feed::vault_tracker::parse_token_account_amount(&vault_b) {
                pool.token_b_amount = amount;
            }
        }
    }

    /// Get the vault tracker (for external integration).
    pub fn vault_tracker(&self) -> &VaultTracker {
        &self.vault_tracker
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

    /// Simulate a cycle with optimal trade sizing.
    ///
    /// Uses ternary search to find the input amount that maximizes profit
    /// after fees and price impact across all hops.
    fn simulate_cycle(
        &self,
        arb_path: &mev_price_graph::path::ArbPath,
        anchor_mint: &Pubkey,
    ) -> Option<Opportunity> {
        use mev_dex_adapters::math::optimizer::{self, HopParams};

        // Collect pool states for each hop
        let mut hop_params = Vec::with_capacity(arb_path.hops.len());
        let mut decoded_pools = Vec::with_capacity(arb_path.hops.len());

        for hop in &arb_path.hops {
            let adapter = self.adapters.iter().find(|a| a.dex_type() == hop.dex_type)?;
            let pool_data = self.cache.get_data(&hop.pool_address)?;
            let mut pool = adapter.decode_pool(&hop.pool_address, &pool_data).ok()??;

            // Populate reserves from vault cache
            self.populate_reserves(&mut pool);

            let (reserve_in, reserve_out) = if hop.input_mint == pool.token_a_mint {
                (pool.token_a_amount, pool.token_b_amount)
            } else {
                (pool.token_b_amount, pool.token_a_amount)
            };

            if reserve_in == 0 || reserve_out == 0 {
                return None;
            }

            hop_params.push(HopParams {
                reserve_in,
                reserve_out,
                fee_numerator: pool.fee_numerator,
                fee_denominator: pool.fee_denominator,
            });

            decoded_pools.push(pool);
        }

        // Find optimal input amount via ternary search
        let max_input = 10_000_000_000u64; // 10 SOL max
        let min_input = 1_000_000u64;      // 0.001 SOL min

        let (optimal_amount, max_profit) = optimizer::optimize_arb_amount(
            &hop_params, max_input, min_input,
        );

        if max_profit <= 0 || optimal_amount == 0 {
            return None;
        }

        // Build the swap steps at the optimal amount
        let mut current_amount = optimal_amount;
        let mut steps = Vec::new();

        for (i, hop) in arb_path.hops.iter().enumerate() {
            let adapter = self.adapters.iter().find(|a| a.dex_type() == hop.dex_type)?;
            let pool = &decoded_pools[i];
            let quote = adapter.quote(pool, &hop.input_mint, current_amount).ok()?;

            if quote.amount_out == 0 {
                return None;
            }

            // 0.5% slippage buffer
            let min_out = quote.amount_out * 995 / 1000;

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

        let profit = current_amount as i64 - optimal_amount as i64;

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

            // Update the mutable graph (zero allocation — in-place edge update)
            self.mutable_graph.update_pool(&pool);

            // Publish graph snapshot for other readers (backrun strategy)
            // Batched: only clone every 100 updates to reduce allocation pressure
            self.updates_since_publish += 1;
            if self.updates_since_publish >= 100 {
                self.graph.store(Arc::new(self.mutable_graph.clone()));
                self.updates_since_publish = 0;
            }

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

// parse_token_account_amount moved to mev_data_feed::vault_tracker
