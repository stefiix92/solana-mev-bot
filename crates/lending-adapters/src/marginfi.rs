use anyhow::Result;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;

use mev_common::constants::MARGINFI;

use crate::traits::{LendingAdapter, LendingPosition, LendingProtocol, ObligationState};

// Marginfi v2 MarginfiAccount layout (Anchor)
const DISCRIMINATOR_LEN: usize = 8;
const MARGINFI_ACCOUNT_MIN_SIZE: usize = 300;

// MarginfiAccount fields after discriminator:
// group: Pubkey (32)
// authority: Pubkey (32) — the owner
// lending_account: LendingAccount — contains balance list
//
// LendingAccount:
//   balances: [Balance; MAX_LENDING_ACCOUNT_BALANCES]  (MAX=16)
//
// Balance:
//   active: bool (1)
//   bank_pk: Pubkey (32)
//   padding: [u8; 7] (7) — alignment
//   asset_shares: WrappedI80F48 (16)
//   liability_shares: WrappedI80F48 (16)
//   emissions_outstanding: WrappedI80F48 (16)
//   last_update: u64 (8)
//   padding2: [u64; 1] (8)
// = 104 bytes per balance

const GROUP_OFFSET: usize = DISCRIMINATOR_LEN;
const AUTHORITY_OFFSET: usize = GROUP_OFFSET + 32;
const BALANCES_OFFSET: usize = AUTHORITY_OFFSET + 32; // Start of Balance array

const BALANCE_SIZE: usize = 104;
const MAX_BALANCES: usize = 16;

// Marginfi liquidation bonus: variable per bank, typically 2.5-5%
const MARGINFI_LIQUIDATION_BONUS_BPS: u16 = 250;

/// Marginfi v2 adapter.
pub struct MarginfiAdapter;

impl LendingAdapter for MarginfiAdapter {
    fn protocol(&self) -> LendingProtocol {
        LendingProtocol::Marginfi
    }

    fn program_id(&self) -> Pubkey {
        MARGINFI
    }

    fn decode_obligation(&self, address: &Pubkey, data: &[u8]) -> Result<Option<ObligationState>> {
        if data.len() < MARGINFI_ACCOUNT_MIN_SIZE {
            return Ok(None);
        }

        let owner = read_pubkey(data, AUTHORITY_OFFSET);

        let mut deposits = Vec::new();
        let mut borrows = Vec::new();

        // Parse balance array (fixed size, 16 slots)
        for i in 0..MAX_BALANCES {
            let offset = BALANCES_OFFSET + i * BALANCE_SIZE;
            if offset + BALANCE_SIZE > data.len() {
                break;
            }

            let active = data[offset] != 0;
            if !active {
                continue;
            }

            let bank_pk = read_pubkey(data, offset + 1);
            // Skip 7 bytes padding
            let asset_shares = read_i80f48(data, offset + 40);
            let liability_shares = read_i80f48(data, offset + 56);

            if asset_shares > 0.0 {
                deposits.push(LendingPosition {
                    reserve: bank_pk,
                    mint: Pubkey::default(),
                    deposited_amount: asset_shares as u64,
                    borrowed_amount: 0,
                    market_value_usd: 0, // Requires bank account to resolve price
                });
            }

            if liability_shares > 0.0 {
                borrows.push(LendingPosition {
                    reserve: bank_pk,
                    mint: Pubkey::default(),
                    deposited_amount: 0,
                    borrowed_amount: liability_shares as u64,
                    market_value_usd: 0,
                });
            }
        }

        // Without bank account data, we can't calculate exact USD values
        // The health factor is approximated — proper calculation requires
        // reading each bank's asset weight, liability weight, and oracle price
        let total_deposit: u64 = deposits.iter().map(|d| d.deposited_amount).sum();
        let total_borrow: u64 = borrows.iter().map(|b| b.borrowed_amount).sum();

        let health_factor = if total_borrow > 0 {
            total_deposit as f64 / total_borrow as f64
        } else {
            f64::INFINITY
        };

        let max_liquidation = borrows.first().map(|b| b.borrowed_amount / 2).unwrap_or(0);

        Ok(Some(ObligationState {
            address: *address,
            protocol: LendingProtocol::Marginfi,
            owner,
            deposits,
            borrows,
            total_deposit_usd: total_deposit,
            total_borrow_usd: total_borrow,
            health_factor,
            max_liquidation_amount: max_liquidation,
            liquidation_bonus_bps: MARGINFI_LIQUIDATION_BONUS_BPS,
            slot: 0,
        }))
    }

    fn build_liquidation_ix(
        &self,
        obligation: &ObligationState,
        liquidator: &Pubkey,
        repay_amount: u64,
    ) -> Result<Vec<Instruction>> {
        // marginfi liquidate instruction
        // Anchor discriminator for "liquidate"
        let discriminator: [u8; 8] = [223, 179, 226, 125, 48, 132, 98, 174];

        let mut ix_data = Vec::with_capacity(8 + 8);
        ix_data.extend_from_slice(&discriminator);
        ix_data.extend_from_slice(&repay_amount.to_le_bytes());

        let accounts = vec![
            AccountMeta::new(obligation.address, false),  // Liquidatee marginfi account
            AccountMeta::new_readonly(*liquidator, true),  // Signer
            // Additional: liquidator_marginfi_account, bank accounts, oracles
        ];

        Ok(vec![Instruction {
            program_id: MARGINFI,
            accounts,
            data: ix_data,
        }])
    }
}

/// Read an I80F48 fixed-point number as f64.
/// I80F48 = 128-bit signed integer where lower 48 bits are fraction.
fn read_i80f48(data: &[u8], offset: usize) -> f64 {
    let raw = i128::from_le_bytes(data[offset..offset + 16].try_into().unwrap());
    raw as f64 / (1i128 << 48) as f64
}

fn read_pubkey(data: &[u8], offset: usize) -> Pubkey {
    Pubkey::new_from_array(data[offset..offset + 32].try_into().unwrap())
}
