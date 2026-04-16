use anyhow::Result;
use solana_sdk::instruction::Instruction;
use solana_sdk::pubkey::Pubkey;

/// A position within a lending protocol obligation.
#[derive(Debug, Clone)]
pub struct LendingPosition {
    /// The reserve/market this position is in
    pub reserve: Pubkey,
    /// Token mint for this position
    pub mint: Pubkey,
    /// Deposited amount (collateral) in native units
    pub deposited_amount: u64,
    /// Borrowed amount (debt) in native units
    pub borrowed_amount: u64,
    /// Current market value in USD (scaled by 1e6)
    pub market_value_usd: u64,
}

/// Decoded obligation/account state from a lending protocol.
#[derive(Debug, Clone)]
pub struct ObligationState {
    pub address: Pubkey,
    pub protocol: LendingProtocol,
    pub owner: Pubkey,
    /// Collateral positions
    pub deposits: Vec<LendingPosition>,
    /// Debt positions
    pub borrows: Vec<LendingPosition>,
    /// Total deposited value in USD (scaled by 1e6)
    pub total_deposit_usd: u64,
    /// Total borrowed value in USD (scaled by 1e6)
    pub total_borrow_usd: u64,
    /// Health factor: deposit_value / borrow_value. < 1.0 means liquidatable.
    pub health_factor: f64,
    /// Maximum liquidation amount in native units of the debt token
    pub max_liquidation_amount: u64,
    /// Liquidation bonus in basis points (e.g., 250 = 2.5%)
    pub liquidation_bonus_bps: u16,
    /// Slot when this was last updated
    pub slot: u64,
}

impl ObligationState {
    /// Is this position eligible for liquidation?
    pub fn is_liquidatable(&self) -> bool {
        self.health_factor < 1.0 && self.total_borrow_usd > 0
    }

    /// Estimated profit from liquidating this position (in USD, scaled 1e6).
    pub fn estimated_profit_usd(&self) -> u64 {
        if !self.is_liquidatable() {
            return 0;
        }
        // Profit = liquidation_amount * bonus_rate
        let bonus_rate = self.liquidation_bonus_bps as f64 / 10_000.0;
        (self.max_liquidation_amount as f64 * bonus_rate) as u64
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LendingProtocol {
    Kamino,
    Marginfi,
    Save,
}

impl std::fmt::Display for LendingProtocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LendingProtocol::Kamino => write!(f, "kamino"),
            LendingProtocol::Marginfi => write!(f, "marginfi"),
            LendingProtocol::Save => write!(f, "save"),
        }
    }
}

/// Unified trait for decoding lending protocol obligation accounts.
pub trait LendingAdapter: Send + Sync {
    fn protocol(&self) -> LendingProtocol;
    fn program_id(&self) -> Pubkey;

    /// Decode raw account bytes into an ObligationState.
    /// Returns None if the data doesn't represent a valid obligation.
    fn decode_obligation(&self, address: &Pubkey, data: &[u8]) -> Result<Option<ObligationState>>;

    /// Build liquidation instruction(s) for an underwater position.
    fn build_liquidation_ix(
        &self,
        obligation: &ObligationState,
        liquidator: &Pubkey,
        repay_amount: u64,
    ) -> Result<Vec<Instruction>>;
}
