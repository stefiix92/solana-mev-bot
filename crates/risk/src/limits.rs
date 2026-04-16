use mev_common::types::Opportunity;
use tracing::debug;

/// Risk limits for opportunity filtering.
#[derive(Debug, Clone)]
pub struct RiskLimits {
    pub max_position_lamports: u64,
    pub min_profit_lamports: i64,
    pub max_tip_lamports: u64,
    pub tip_fraction: f64,
}

impl RiskLimits {
    /// Check if an opportunity passes all risk checks.
    pub fn check(&self, opportunity: &Opportunity) -> RiskCheckResult {
        // Check minimum profit
        if opportunity.expected_profit_lamports < self.min_profit_lamports {
            return RiskCheckResult::Rejected("Below min profit threshold");
        }

        // Check max position size (first hop amount_in)
        if let Some(first_step) = opportunity.path.first() {
            if first_step.amount_in > self.max_position_lamports {
                return RiskCheckResult::Rejected("Exceeds max position size");
            }
        }

        RiskCheckResult::Approved
    }

    /// Calculate the tip for an approved opportunity.
    pub fn calculate_tip(&self, profit_lamports: i64) -> u64 {
        use mev_common::constants::MIN_TIP_LAMPORTS;

        if profit_lamports <= 0 {
            return MIN_TIP_LAMPORTS;
        }

        let tip = (profit_lamports as f64 * self.tip_fraction) as u64;
        tip.max(MIN_TIP_LAMPORTS).min(self.max_tip_lamports)
    }
}

#[derive(Debug)]
pub enum RiskCheckResult {
    Approved,
    Rejected(&'static str),
}

impl RiskCheckResult {
    pub fn is_approved(&self) -> bool {
        matches!(self, RiskCheckResult::Approved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mev_common::types::{DexType, SwapStep};
    use solana_sdk::pubkey::Pubkey;

    fn make_opportunity(profit: i64, amount_in: u64) -> Opportunity {
        Opportunity {
            strategy: "test".to_string(),
            path: vec![SwapStep {
                pool_address: Pubkey::new_unique(),
                dex_type: DexType::RaydiumAmm,
                input_mint: Pubkey::new_unique(),
                output_mint: Pubkey::new_unique(),
                amount_in,
                min_amount_out: 0,
                instructions: vec![],
            }],
            expected_profit_lamports: profit,
            estimated_compute_units: 200_000,
            detected_at_slot: 100,
        }
    }

    #[test]
    fn test_approved_opportunity() {
        let limits = RiskLimits {
            max_position_lamports: 10_000_000_000,
            min_profit_lamports: 50_000,
            max_tip_lamports: 5_000_000,
            tip_fraction: 0.5,
        };
        let opp = make_opportunity(100_000, 1_000_000_000);
        assert!(limits.check(&opp).is_approved());
    }

    #[test]
    fn test_rejected_low_profit() {
        let limits = RiskLimits {
            max_position_lamports: 10_000_000_000,
            min_profit_lamports: 50_000,
            max_tip_lamports: 5_000_000,
            tip_fraction: 0.5,
        };
        let opp = make_opportunity(10_000, 1_000_000_000);
        assert!(!limits.check(&opp).is_approved());
    }

    #[test]
    fn test_rejected_position_too_large() {
        let limits = RiskLimits {
            max_position_lamports: 1_000_000_000,
            min_profit_lamports: 50_000,
            max_tip_lamports: 5_000_000,
            tip_fraction: 0.5,
        };
        let opp = make_opportunity(100_000, 5_000_000_000);
        assert!(!limits.check(&opp).is_approved());
    }
}
