use mev_common::types::DexType;
use solana_sdk::pubkey::Pubkey;

/// A profitable arbitrage path through the graph.
#[derive(Debug, Clone)]
pub struct ArbPath {
    /// Sequence of hops: (pool_address, dex_type, input_mint, output_mint)
    pub hops: Vec<ArbHop>,
    /// Expected profit ratio: e.g., 1.005 means 0.5% profit before gas/tips
    pub profit_ratio: f64,
    /// Estimated profit in the anchor token (usually SOL lamports)
    pub estimated_profit_lamports: i64,
}

#[derive(Debug, Clone)]
pub struct ArbHop {
    pub pool_address: Pubkey,
    pub dex_type: DexType,
    pub input_mint: Pubkey,
    pub output_mint: Pubkey,
    /// Edge index in the graph (for fast lookups)
    pub edge_index: usize,
}

impl ArbPath {
    /// Number of hops in this path.
    pub fn num_hops(&self) -> usize {
        self.hops.len()
    }

    /// Is this path profitable above a minimum threshold?
    pub fn is_profitable(&self, min_profit_lamports: i64) -> bool {
        self.estimated_profit_lamports > min_profit_lamports
    }
}
