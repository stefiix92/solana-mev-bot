use solana_sdk::instruction::Instruction;
use solana_sdk::pubkey::Pubkey;

/// Identifies which DEX a pool belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DexType {
    RaydiumAmm,
    RaydiumClmm,
    OrcaWhirlpool,
    MeteoraDlmm,
    Phoenix,
}

impl std::fmt::Display for DexType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DexType::RaydiumAmm => write!(f, "raydium_amm"),
            DexType::RaydiumClmm => write!(f, "raydium_clmm"),
            DexType::OrcaWhirlpool => write!(f, "orca_whirlpool"),
            DexType::MeteoraDlmm => write!(f, "meteora_dlmm"),
            DexType::Phoenix => write!(f, "phoenix"),
        }
    }
}

/// Raw account update from the data feed.
#[derive(Debug, Clone)]
pub struct AccountUpdate {
    pub pubkey: Pubkey,
    pub slot: u64,
    pub data: Vec<u8>,
    pub lamports: u64,
    pub owner: Pubkey,
}

/// Decoded pool state — normalized across all DEXs.
#[derive(Debug, Clone)]
pub struct PoolState {
    pub address: Pubkey,
    pub dex_type: DexType,
    pub token_a_mint: Pubkey,
    pub token_b_mint: Pubkey,
    pub token_a_vault: Pubkey,
    pub token_b_vault: Pubkey,
    pub token_a_amount: u64,
    pub token_b_amount: u64,
    pub fee_numerator: u64,
    pub fee_denominator: u64,
    pub slot: u64,
}

impl PoolState {
    /// Fee as a fraction (0.0 to 1.0).
    pub fn fee_rate(&self) -> f64 {
        self.fee_numerator as f64 / self.fee_denominator as f64
    }
}

/// Result of quoting a swap through a pool.
#[derive(Debug, Clone)]
pub struct Quote {
    pub input_mint: Pubkey,
    pub output_mint: Pubkey,
    pub amount_in: u64,
    pub amount_out: u64,
    pub fee_amount: u64,
    pub price_impact_bps: u16,
}

/// A detected MEV opportunity ready for execution.
#[derive(Debug, Clone)]
pub struct Opportunity {
    pub strategy: String,
    pub path: Vec<SwapStep>,
    pub expected_profit_lamports: i64,
    pub estimated_compute_units: u32,
    pub detected_at_slot: u64,
}

/// One hop in a multi-swap path.
#[derive(Debug, Clone)]
pub struct SwapStep {
    pub pool_address: Pubkey,
    pub dex_type: DexType,
    pub input_mint: Pubkey,
    pub output_mint: Pubkey,
    pub amount_in: u64,
    pub min_amount_out: u64,
    pub instructions: Vec<Instruction>,
}
