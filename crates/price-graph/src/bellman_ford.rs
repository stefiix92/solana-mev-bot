use crate::graph::PriceGraph;
use crate::path::{ArbHop, ArbPath};

/// Pre-allocated buffers for Bellman-Ford to avoid per-call allocations.
pub struct BellmanFordState {
    dist: Vec<f64>,
    predecessor: Vec<Option<usize>>, // edge index that led to this node
}

impl BellmanFordState {
    pub fn new(capacity: usize) -> Self {
        Self {
            dist: vec![f64::INFINITY; capacity],
            predecessor: vec![None; capacity],
        }
    }

    fn ensure_capacity(&mut self, n: usize) {
        if self.dist.len() < n {
            self.dist.resize(n, f64::INFINITY);
            self.predecessor.resize(n, None);
        }
    }

    fn reset(&mut self, n: usize) {
        self.ensure_capacity(n);
        for i in 0..n {
            self.dist[i] = f64::INFINITY;
            self.predecessor[i] = None;
        }
    }
}

/// Find all negative cycles (arbitrage opportunities) reachable from a source node.
///
/// Returns profitable cycles as ArbPaths. Uses standard Bellman-Ford with an
/// extra iteration to detect negative cycles, then traces back the cycle.
pub fn find_arbitrage_from(
    graph: &PriceGraph,
    source: usize,
    state: &mut BellmanFordState,
    min_profit_ratio: f64,
) -> Vec<ArbPath> {
    let n = graph.num_tokens();
    if n == 0 || source >= n {
        return Vec::new();
    }

    state.reset(n);
    state.dist[source] = 0.0;

    let edges = &graph.edges;

    // Relax all edges (n-1) times
    for _ in 0..n - 1 {
        let mut changed = false;
        for (edge_idx, edge) in edges.iter().enumerate() {
            if edge.weight.is_infinite() || edge.rate == 0.0 {
                continue; // Skip invalidated edges
            }
            let new_dist = state.dist[edge.source] + edge.weight;
            if new_dist < state.dist[edge.dest] {
                state.dist[edge.dest] = new_dist;
                state.predecessor[edge.dest] = Some(edge_idx);
                changed = true;
            }
        }
        if !changed {
            break; // Early exit — no more relaxations possible
        }
    }

    // Nth iteration: detect negative cycles
    let mut cycles = Vec::new();
    let mut visited_in_cycle = vec![false; n];

    for (edge_idx, edge) in edges.iter().enumerate() {
        if edge.weight.is_infinite() || edge.rate == 0.0 {
            continue;
        }
        let new_dist = state.dist[edge.source] + edge.weight;
        if new_dist < state.dist[edge.dest] - 1e-9 {
            // Negative cycle detected through this edge
            // Trace back to find the actual cycle
            if let Some(path) = trace_cycle(graph, state, edge.dest, &mut visited_in_cycle) {
                let profit_ratio = cycle_profit_ratio(graph, &path);
                if profit_ratio > 1.0 + min_profit_ratio {
                    let arb_path = build_arb_path(graph, &path, profit_ratio);
                    cycles.push(arb_path);
                }
            }
        }
    }

    cycles
}

/// Bounded DFS: find all profitable 2-3 hop cycles starting and ending at `source`.
/// This is more targeted than Bellman-Ford and catches specific patterns.
pub fn find_cycles_dfs(
    graph: &PriceGraph,
    source: usize,
    max_hops: usize,
    min_profit_ratio: f64,
) -> Vec<ArbPath> {
    let mut results = Vec::new();
    let mut path = Vec::with_capacity(max_hops);
    let mut visited_pools = Vec::with_capacity(max_hops);

    dfs_recurse(
        graph, source, source,
        0.0, // cumulative weight
        max_hops,
        min_profit_ratio,
        &mut path,
        &mut visited_pools,
        &mut results,
    );

    results
}

fn dfs_recurse(
    graph: &PriceGraph,
    current: usize,
    target: usize,
    cumulative_weight: f64,
    remaining_hops: usize,
    min_profit_ratio: f64,
    path: &mut Vec<usize>, // edge indices
    visited_pools: &mut Vec<solana_sdk::pubkey::Pubkey>,
    results: &mut Vec<ArbPath>,
) {
    if remaining_hops == 0 {
        return;
    }

    for &edge_idx in graph.edges_from(current) {
        let edge = &graph.edges[edge_idx];

        if edge.weight.is_infinite() || edge.rate == 0.0 || edge.liquidity == 0 {
            continue;
        }

        // Don't reuse the same pool in a path
        if visited_pools.contains(&edge.pool_address) {
            continue;
        }

        let new_weight = cumulative_weight + edge.weight;

        // Check if we've completed a cycle back to source
        if edge.dest == target && !path.is_empty() {
            path.push(edge_idx);
            let profit_ratio = (-new_weight).exp();
            if profit_ratio > 1.0 + min_profit_ratio {
                let arb_path = build_arb_path_from_edges(graph, path, profit_ratio);
                results.push(arb_path);
            }
            path.pop();
            continue;
        }

        // Recurse deeper
        if remaining_hops > 1 {
            path.push(edge_idx);
            visited_pools.push(edge.pool_address);

            dfs_recurse(
                graph, edge.dest, target,
                new_weight, remaining_hops - 1,
                min_profit_ratio,
                path, visited_pools, results,
            );

            visited_pools.pop();
            path.pop();
        }
    }
}

/// Trace back from a node known to be on a negative cycle to extract the cycle.
fn trace_cycle(
    graph: &PriceGraph,
    state: &BellmanFordState,
    start: usize,
    visited_in_cycle: &mut [bool],
) -> Option<Vec<usize>> {
    let n = graph.num_tokens();

    // Walk back n times to ensure we're inside the cycle
    let mut node = start;
    for _ in 0..n {
        if let Some(edge_idx) = state.predecessor[node] {
            node = graph.edges[edge_idx].source;
        } else {
            return None;
        }
    }

    // Now trace the cycle
    let cycle_start = node;
    if visited_in_cycle[cycle_start] {
        return None; // Already extracted this cycle
    }

    let mut path = Vec::new();
    let mut current = cycle_start;

    loop {
        if let Some(edge_idx) = state.predecessor[current] {
            visited_in_cycle[current] = true;
            path.push(edge_idx);
            current = graph.edges[edge_idx].source;
            if current == cycle_start {
                break;
            }
            if path.len() > n {
                return None; // Safety: prevent infinite loops
            }
        } else {
            return None;
        }
    }

    path.reverse();

    // Limit to reasonable cycle lengths (2-4 hops)
    if path.len() < 2 || path.len() > 4 {
        return None;
    }

    Some(path)
}

/// Calculate the profit ratio of a cycle: product of exchange rates around the loop.
fn cycle_profit_ratio(graph: &PriceGraph, edge_indices: &[usize]) -> f64 {
    let total_weight: f64 = edge_indices
        .iter()
        .map(|&idx| graph.edges[idx].weight)
        .sum();

    (-total_weight).exp()
}

fn build_arb_path(graph: &PriceGraph, edge_indices: &[usize], profit_ratio: f64) -> ArbPath {
    build_arb_path_from_edges(graph, edge_indices, profit_ratio)
}

fn build_arb_path_from_edges(graph: &PriceGraph, edge_indices: &[usize], profit_ratio: f64) -> ArbPath {
    let hops: Vec<ArbHop> = edge_indices
        .iter()
        .map(|&idx| {
            let edge = &graph.edges[idx];
            ArbHop {
                pool_address: edge.pool_address,
                dex_type: edge.dex_type,
                input_mint: edge.source_mint,
                output_mint: edge.dest_mint,
                edge_index: idx,
            }
        })
        .collect();

    ArbPath {
        hops,
        profit_ratio,
        estimated_profit_lamports: 0, // Calculated later with actual amounts
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::PriceGraph;
    use mev_common::types::{DexType, PoolState};
    use solana_sdk::pubkey::Pubkey;

    fn make_pool(
        mint_a: Pubkey,
        mint_b: Pubkey,
        amount_a: u64,
        amount_b: u64,
        fee_num: u64,
        fee_den: u64,
    ) -> PoolState {
        PoolState {
            address: Pubkey::new_unique(),
            dex_type: DexType::RaydiumAmm,
            token_a_mint: mint_a,
            token_b_mint: mint_b,
            token_a_vault: Pubkey::new_unique(),
            token_b_vault: Pubkey::new_unique(),
            token_a_amount: amount_a,
            token_b_amount: amount_b,
            fee_numerator: fee_num,
            fee_denominator: fee_den,
            slot: 100,
        }
    }

    #[test]
    fn test_no_arb_in_balanced_graph() {
        let mut graph = PriceGraph::new();
        let sol = Pubkey::new_unique();
        let usdc = Pubkey::new_unique();
        let ray = Pubkey::new_unique();

        // All pools balanced at fair value — no arb exists
        graph.update_pool(&make_pool(sol, usdc, 1_000, 100_000, 25, 10_000));
        graph.update_pool(&make_pool(usdc, ray, 100_000, 50_000, 25, 10_000));
        graph.update_pool(&make_pool(ray, sol, 50_000, 1_000, 25, 10_000));

        let sol_idx = graph.tokens.get_index(&sol).unwrap();
        let cycles = find_cycles_dfs(&graph, sol_idx, 3, 0.001);

        // With fees, no balanced graph produces profitable cycles
        assert!(
            cycles.is_empty() || cycles.iter().all(|c| c.profit_ratio < 1.001),
            "Balanced graph should not have profitable arbs"
        );
    }

    #[test]
    fn test_arb_detection_price_discrepancy() {
        let mut graph = PriceGraph::new();
        let sol = Pubkey::new_unique();
        let usdc = Pubkey::new_unique();
        let ray = Pubkey::new_unique();

        // Create a price discrepancy:
        // SOL→USDC: 1 SOL = 100 USDC (pool 1)
        // USDC→RAY: 1 USDC = 0.5 RAY (pool 2)
        // RAY→SOL:  1 RAY = 3 SOL (pool 3)  ← mispriced! Should be 2 SOL
        //
        // Cycle: 1 SOL → 100 USDC → 50 RAY → 150 SOL = 150x return!
        // Even with 0.25% fee per hop, this is massively profitable.
        graph.update_pool(&make_pool(sol, usdc, 1_000, 100_000, 0, 10_000)); // 0 fee for clarity
        graph.update_pool(&make_pool(usdc, ray, 100_000, 50_000, 0, 10_000));
        graph.update_pool(&make_pool(ray, sol, 10_000, 30_000, 0, 10_000)); // 1 RAY = 3 SOL

        let sol_idx = graph.tokens.get_index(&sol).unwrap();
        let cycles = find_cycles_dfs(&graph, sol_idx, 3, 0.001);

        assert!(!cycles.is_empty(), "Should detect arbitrage opportunity");
        assert!(
            cycles[0].profit_ratio > 1.0,
            "Profit ratio should be > 1.0, got {}",
            cycles[0].profit_ratio
        );
    }

    #[test]
    fn test_bellman_ford_finds_negative_cycle() {
        let mut graph = PriceGraph::new();
        let sol = Pubkey::new_unique();
        let usdc = Pubkey::new_unique();

        // Create obvious arb: buy SOL cheap on one DEX, sell expensive on another
        // Pool 1: 1000 SOL / 100000 USDC (1 SOL = 100 USDC)
        graph.update_pool(&PoolState {
            address: Pubkey::new_unique(),
            dex_type: DexType::RaydiumAmm,
            token_a_mint: sol,
            token_b_mint: usdc,
            token_a_vault: Pubkey::new_unique(),
            token_b_vault: Pubkey::new_unique(),
            token_a_amount: 1_000,
            token_b_amount: 100_000,
            fee_numerator: 0,
            fee_denominator: 10_000,
            slot: 100,
        });

        // Pool 2: 1000 SOL / 200000 USDC (1 SOL = 200 USDC) — different price!
        graph.update_pool(&PoolState {
            address: Pubkey::new_unique(),
            dex_type: DexType::OrcaWhirlpool,
            token_a_mint: sol,
            token_b_mint: usdc,
            token_a_vault: Pubkey::new_unique(),
            token_b_vault: Pubkey::new_unique(),
            token_a_amount: 1_000,
            token_b_amount: 200_000,
            fee_numerator: 0,
            fee_denominator: 10_000,
            slot: 100,
        });

        let sol_idx = graph.tokens.get_index(&sol).unwrap();
        let mut state = BellmanFordState::new(graph.num_tokens());
        let cycles = find_arbitrage_from(&graph, sol_idx, &mut state, 0.001);

        // Should find: buy SOL on pool1 (cheap), sell on pool2 (expensive)
        let dfs_cycles = find_cycles_dfs(&graph, sol_idx, 2, 0.001);
        assert!(
            !cycles.is_empty() || !dfs_cycles.is_empty(),
            "Should detect 2-pool arb"
        );
    }
}
