use anyhow::Result;
use solana_sdk::pubkey::Pubkey;
use tracing::{debug, info, trace, warn};

use mev_account_cache::cache::AccountCache;
use mev_common::types::{AccountUpdate, Opportunity, SwapStep, DexType};
use mev_lending_adapters::kamino::KaminoAdapter;
use mev_lending_adapters::marginfi::MarginfiAdapter;
use mev_lending_adapters::save::SaveAdapter;
use mev_lending_adapters::traits::{LendingAdapter, ObligationState};

use crate::traits::Strategy;

/// Liquidation strategy.
///
/// Monitors lending protocol obligation accounts for underwater positions.
/// When health factor < 1.0, constructs a liquidation transaction to capture
/// the liquidation bonus.
pub struct LiquidationStrategy {
    adapters: Vec<Box<dyn LendingAdapter>>,
    cache: AccountCache,
    min_bonus_bps: u16,
    /// Minimum borrow value (USD, scaled 1e6) to liquidate — skip dust positions
    min_borrow_value_usd: u64,
}

impl LiquidationStrategy {
    pub fn new(
        cache: AccountCache,
        min_bonus_bps: u16,
        min_borrow_value_usd: u64,
    ) -> Self {
        Self {
            adapters: vec![
                Box::new(KaminoAdapter),
                Box::new(MarginfiAdapter),
                Box::new(SaveAdapter),
            ],
            cache,
            min_bonus_bps,
            min_borrow_value_usd,
        }
    }

    /// Process an account update and check for liquidation opportunities.
    pub fn process_update(&self, update: &AccountUpdate) -> Option<Opportunity> {
        // Try to decode as an obligation from any lending protocol
        for adapter in &self.adapters {
            if update.owner == adapter.program_id() {
                match adapter.decode_obligation(&update.pubkey, &update.data) {
                    Ok(Some(obligation)) => {
                        return self.evaluate_obligation(&obligation, adapter.as_ref());
                    }
                    Ok(None) => continue, // Not an obligation account
                    Err(e) => {
                        trace!(error = %e, "Failed to decode lending account");
                        continue;
                    }
                }
            }
        }
        None
    }

    fn evaluate_obligation(
        &self,
        obligation: &ObligationState,
        adapter: &dyn LendingAdapter,
    ) -> Option<Opportunity> {
        if !obligation.is_liquidatable() {
            return None;
        }

        // Check minimum bonus
        if obligation.liquidation_bonus_bps < self.min_bonus_bps {
            return None;
        }

        // Check minimum size
        if obligation.total_borrow_usd < self.min_borrow_value_usd {
            return None;
        }

        info!(
            protocol = %obligation.protocol,
            obligation = %obligation.address,
            health = format!("{:.4}", obligation.health_factor),
            borrow_usd = obligation.total_borrow_usd,
            bonus_bps = obligation.liquidation_bonus_bps,
            "Liquidatable position detected"
        );

        // Calculate repay amount (up to max liquidation)
        let repay_amount = obligation.max_liquidation_amount;
        if repay_amount == 0 {
            return None;
        }

        // Estimated profit from liquidation bonus
        let bonus_rate = obligation.liquidation_bonus_bps as f64 / 10_000.0;
        let estimated_profit = (repay_amount as f64 * bonus_rate) as i64;

        // Build liquidation instruction
        // Note: in production, we'd need a proper liquidator pubkey here
        // For now, instructions are empty — built by executor with actual keypair
        let step = SwapStep {
            pool_address: obligation.address, // Using obligation address as "pool"
            dex_type: DexType::RaydiumAmm, // Placeholder — liquidation isn't a DEX swap
            input_mint: obligation.borrows.first().map(|b| b.mint).unwrap_or_default(),
            output_mint: obligation.deposits.first().map(|d| d.mint).unwrap_or_default(),
            amount_in: repay_amount,
            min_amount_out: 0,
            instructions: Vec::new(), // Built by executor
        };

        Some(Opportunity {
            strategy: format!("liquidation_{}", obligation.protocol),
            path: vec![step],
            expected_profit_lamports: estimated_profit,
            estimated_compute_units: 400_000, // Liquidations are compute-heavy
            detected_at_slot: obligation.slot,
        })
    }
}

impl Strategy for LiquidationStrategy {
    fn name(&self) -> &str {
        "liquidation"
    }

    fn evaluate(&self, update: &AccountUpdate) -> Result<Option<Opportunity>> {
        Ok(self.process_update(update))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mev_lending_adapters::traits::LendingPosition;

    #[test]
    fn test_healthy_position_not_liquidated() {
        let obligation = ObligationState {
            address: Pubkey::new_unique(),
            protocol: mev_lending_adapters::traits::LendingProtocol::Kamino,
            owner: Pubkey::new_unique(),
            deposits: vec![LendingPosition {
                reserve: Pubkey::new_unique(),
                mint: Pubkey::new_unique(),
                deposited_amount: 1_000_000,
                borrowed_amount: 0,
                market_value_usd: 2_000_000,
            }],
            borrows: vec![LendingPosition {
                reserve: Pubkey::new_unique(),
                mint: Pubkey::new_unique(),
                deposited_amount: 0,
                borrowed_amount: 500_000,
                market_value_usd: 1_000_000,
            }],
            total_deposit_usd: 2_000_000,
            total_borrow_usd: 1_000_000,
            health_factor: 2.0,
            max_liquidation_amount: 250_000,
            liquidation_bonus_bps: 250,
            slot: 100,
        };
        assert!(!obligation.is_liquidatable());
    }

    #[test]
    fn test_underwater_position_detected() {
        let obligation = ObligationState {
            address: Pubkey::new_unique(),
            protocol: mev_lending_adapters::traits::LendingProtocol::Kamino,
            owner: Pubkey::new_unique(),
            deposits: vec![LendingPosition {
                reserve: Pubkey::new_unique(),
                mint: Pubkey::new_unique(),
                deposited_amount: 500_000,
                borrowed_amount: 0,
                market_value_usd: 900_000,
            }],
            borrows: vec![LendingPosition {
                reserve: Pubkey::new_unique(),
                mint: Pubkey::new_unique(),
                deposited_amount: 0,
                borrowed_amount: 1_000_000,
                market_value_usd: 1_000_000,
            }],
            total_deposit_usd: 900_000,
            total_borrow_usd: 1_000_000,
            health_factor: 0.9,
            max_liquidation_amount: 500_000,
            liquidation_bonus_bps: 250,
            slot: 100,
        };
        assert!(obligation.is_liquidatable());
        assert!(obligation.estimated_profit_usd() > 0);
    }
}
