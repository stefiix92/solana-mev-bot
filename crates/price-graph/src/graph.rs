use mev_common::types::{DexType, PoolState};
use solana_sdk::pubkey::Pubkey;

use crate::tokens::TokenRegistry;

/// A directed edge in the price graph representing a swap through a pool.
#[derive(Debug, Clone)]
pub struct Edge {
    pub source: usize,       // Token index (input)
    pub dest: usize,         // Token index (output)
    pub pool_address: Pubkey,
    pub dex_type: DexType,
    pub source_mint: Pubkey,
    pub dest_mint: Pubkey,
    pub weight: f64,         // -ln(rate * (1 - fee)) — negative means profitable
    pub rate: f64,           // Raw exchange rate (output/input)
    pub liquidity: u64,      // Approximate available liquidity (output side)
    pub last_updated_slot: u64,
}

/// Custom adjacency-list graph optimized for MEV arb detection.
///
/// Design: flat Vec<Edge> + per-node adjacency lists (Vec<usize> indices into edges).
/// No heap allocations on update — edges are modified in place.
/// Bellman-Ford uses pre-allocated buffers.
#[derive(Debug, Clone)]
pub struct PriceGraph {
    pub tokens: TokenRegistry,
    pub edges: Vec<Edge>,
    /// adjacency[node_idx] = vec of edge indices outgoing from that node
    pub adjacency: Vec<Vec<usize>>,
    /// Map from (pool_address, source_mint) → edge index for O(1) updates
    pool_edge_index: std::collections::HashMap<(Pubkey, Pubkey), usize>,
}

impl PriceGraph {
    pub fn new() -> Self {
        Self {
            tokens: TokenRegistry::new(),
            edges: Vec::with_capacity(8_000), // ~4000 pools × 2 directions
            adjacency: Vec::with_capacity(1_000),
            pool_edge_index: std::collections::HashMap::with_capacity(8_000),
        }
    }

    /// Number of tokens (nodes) in the graph.
    pub fn num_tokens(&self) -> usize {
        self.tokens.len()
    }

    /// Number of edges in the graph.
    pub fn num_edges(&self) -> usize {
        self.edges.len()
    }

    /// Ensure adjacency list has capacity for a given node index.
    fn ensure_adjacency(&mut self, node: usize) {
        while self.adjacency.len() <= node {
            self.adjacency.push(Vec::new());
        }
    }

    /// Update the graph with a new pool state.
    /// Creates edges for both swap directions (A→B and B→A).
    /// If edges already exist for this pool, updates them in place.
    pub fn update_pool(&mut self, pool: &PoolState) {
        if pool.token_a_amount == 0 || pool.token_b_amount == 0 {
            return; // Skip pools with no liquidity
        }

        let idx_a = self.tokens.get_or_insert(&pool.token_a_mint);
        let idx_b = self.tokens.get_or_insert(&pool.token_b_mint);
        self.ensure_adjacency(idx_a);
        self.ensure_adjacency(idx_b);

        let fee_rate = pool.fee_rate();

        // Direction A → B
        let rate_a_to_b = (pool.token_b_amount as f64) / (pool.token_a_amount as f64);
        let weight_a_to_b = -(rate_a_to_b * (1.0 - fee_rate)).ln();
        self.upsert_edge(
            idx_a, idx_b,
            pool.address, pool.dex_type,
            pool.token_a_mint, pool.token_b_mint,
            weight_a_to_b, rate_a_to_b,
            pool.token_b_amount, pool.slot,
        );

        // Direction B → A
        let rate_b_to_a = (pool.token_a_amount as f64) / (pool.token_b_amount as f64);
        let weight_b_to_a = -(rate_b_to_a * (1.0 - fee_rate)).ln();
        self.upsert_edge(
            idx_b, idx_a,
            pool.address, pool.dex_type,
            pool.token_b_mint, pool.token_a_mint,
            weight_b_to_a, rate_b_to_a,
            pool.token_a_amount, pool.slot,
        );
    }

    fn upsert_edge(
        &mut self,
        source: usize,
        dest: usize,
        pool_address: Pubkey,
        dex_type: DexType,
        source_mint: Pubkey,
        dest_mint: Pubkey,
        weight: f64,
        rate: f64,
        liquidity: u64,
        slot: u64,
    ) {
        let key = (pool_address, source_mint);

        if let Some(&edge_idx) = self.pool_edge_index.get(&key) {
            // Update existing edge in place (zero allocation)
            let edge = &mut self.edges[edge_idx];
            edge.weight = weight;
            edge.rate = rate;
            edge.liquidity = liquidity;
            edge.last_updated_slot = slot;
        } else {
            // Insert new edge
            let edge_idx = self.edges.len();
            self.edges.push(Edge {
                source,
                dest,
                pool_address,
                dex_type,
                source_mint,
                dest_mint,
                weight,
                rate,
                liquidity,
                last_updated_slot: slot,
            });
            self.adjacency[source].push(edge_idx);
            self.pool_edge_index.insert(key, edge_idx);
        }
    }

    /// Get all outgoing edges from a token.
    pub fn edges_from(&self, token_idx: usize) -> &[usize] {
        if token_idx < self.adjacency.len() {
            &self.adjacency[token_idx]
        } else {
            &[]
        }
    }

    /// Remove all edges for a specific pool (e.g., when pool becomes inactive).
    pub fn remove_pool(&mut self, pool_address: &Pubkey) {
        // Mark edges with infinite weight (effectively removes them from arb detection)
        // Actual removal would require reindexing, so we just invalidate
        for edge in &mut self.edges {
            if edge.pool_address == *pool_address {
                edge.weight = f64::INFINITY;
                edge.rate = 0.0;
                edge.liquidity = 0;
            }
        }
    }
}

impl Default for PriceGraph {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_pool(
        addr: Pubkey,
        mint_a: Pubkey,
        mint_b: Pubkey,
        amount_a: u64,
        amount_b: u64,
    ) -> PoolState {
        PoolState {
            address: addr,
            dex_type: DexType::RaydiumAmm,
            token_a_mint: mint_a,
            token_b_mint: mint_b,
            token_a_vault: Pubkey::new_unique(),
            token_b_vault: Pubkey::new_unique(),
            token_a_amount: amount_a,
            token_b_amount: amount_b,
            fee_numerator: 25,
            fee_denominator: 10_000,
            slot: 100,
        }
    }

    #[test]
    fn test_graph_construction() {
        let mut graph = PriceGraph::new();
        let sol = Pubkey::new_unique();
        let usdc = Pubkey::new_unique();
        let pool = make_pool(Pubkey::new_unique(), sol, usdc, 1_000_000, 100_000_000);

        graph.update_pool(&pool);

        assert_eq!(graph.num_tokens(), 2);
        assert_eq!(graph.num_edges(), 2); // Both directions
    }

    #[test]
    fn test_edge_update_in_place() {
        let mut graph = PriceGraph::new();
        let sol = Pubkey::new_unique();
        let usdc = Pubkey::new_unique();
        let pool_addr = Pubkey::new_unique();

        let pool1 = make_pool(pool_addr, sol, usdc, 1_000_000, 100_000_000);
        graph.update_pool(&pool1);
        assert_eq!(graph.num_edges(), 2);

        // Update same pool with new reserves
        let mut pool2 = pool1.clone();
        pool2.token_a_amount = 2_000_000;
        pool2.slot = 101;
        graph.update_pool(&pool2);

        // Should still have 2 edges (updated in place, not duplicated)
        assert_eq!(graph.num_edges(), 2);
    }

    #[test]
    fn test_zero_liquidity_skipped() {
        let mut graph = PriceGraph::new();
        let pool = make_pool(
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            0, 0,
        );
        graph.update_pool(&pool);
        assert_eq!(graph.num_edges(), 0);
    }
}
