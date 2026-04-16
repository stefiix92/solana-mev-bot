use anyhow::Result;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;

use mev_common::constants::SAVE_SOLEND;

use crate::traits::{LendingAdapter, LendingPosition, LendingProtocol, ObligationState};

// Save (formerly Solend) Obligation layout
// Uses Borsh serialization (not Anchor)
const OBLIGATION_MIN_SIZE: usize = 300;

// Obligation fields:
// version: u8 (1)
// last_update: LastUpdate { slot: u64(8), stale: bool(1) } = 9, padded to 16
// lending_market: Pubkey (32)
// owner: Pubkey (32)
// deposited_value: u128 (16) — in fractional units
// borrowed_value: u128 (16)
// allowed_borrow_value: u128 (16)
// unhealthy_borrow_value: u128 (16)
// padding: [u64; 64] — future use
// deposits: Vec<ObligationCollateral> (u32 len + items)
// borrows: Vec<ObligationLiquidity> (u32 len + items)

const VERSION_OFFSET: usize = 0;
const LAST_UPDATE_OFFSET: usize = 1;
const LENDING_MARKET_OFFSET: usize = LAST_UPDATE_OFFSET + 16;
const OWNER_OFFSET: usize = LENDING_MARKET_OFFSET + 32;
const DEPOSITED_VALUE_OFFSET: usize = OWNER_OFFSET + 32;
const BORROWED_VALUE_OFFSET: usize = DEPOSITED_VALUE_OFFSET + 16;
const ALLOWED_BORROW_OFFSET: usize = BORROWED_VALUE_OFFSET + 16;
const UNHEALTHY_BORROW_OFFSET: usize = ALLOWED_BORROW_OFFSET + 16;
// Padding: 64 * 8 = 512 bytes
const PADDING_SIZE: usize = 512;
const DEPOSITS_LEN_OFFSET: usize = UNHEALTHY_BORROW_OFFSET + 16 + PADDING_SIZE;

// ObligationCollateral: deposit_reserve(32) + deposited_amount(u64=8) + market_value(u128=16)
const COLLATERAL_SIZE: usize = 56;
// ObligationLiquidity: borrow_reserve(32) + cumulative_borrow_rate(u128=16) + borrowed_amount(u128=16) + market_value(u128=16)
const LIQUIDITY_SIZE: usize = 80;

// Save liquidation bonus: typically 5%
const SAVE_LIQUIDATION_BONUS_BPS: u16 = 500;

/// Save (Solend) adapter.
pub struct SaveAdapter;

impl LendingAdapter for SaveAdapter {
    fn protocol(&self) -> LendingProtocol {
        LendingProtocol::Save
    }

    fn program_id(&self) -> Pubkey {
        SAVE_SOLEND
    }

    fn decode_obligation(&self, address: &Pubkey, data: &[u8]) -> Result<Option<ObligationState>> {
        if data.len() < OBLIGATION_MIN_SIZE {
            return Ok(None);
        }

        let version = data[VERSION_OFFSET];
        if version == 0 {
            return Ok(None); // Uninitialized
        }

        let owner = read_pubkey(data, OWNER_OFFSET);
        let deposited_value = read_u128(data, DEPOSITED_VALUE_OFFSET);
        let borrowed_value = read_u128(data, BORROWED_VALUE_OFFSET);

        // Parse deposits if we have enough data
        let mut deposits = Vec::new();
        let mut borrows = Vec::new();

        if DEPOSITS_LEN_OFFSET + 4 <= data.len() {
            let deposits_len = read_u32(data, DEPOSITS_LEN_OFFSET) as usize;
            let mut offset = DEPOSITS_LEN_OFFSET + 4;

            for _ in 0..deposits_len.min(10) {
                if offset + COLLATERAL_SIZE > data.len() {
                    break;
                }
                let reserve = read_pubkey(data, offset);
                let amount = read_u64(data, offset + 32);
                let market_value = read_u128(data, offset + 40);

                deposits.push(LendingPosition {
                    reserve,
                    mint: Pubkey::default(),
                    deposited_amount: amount,
                    borrowed_amount: 0,
                    market_value_usd: (market_value >> 20) as u64,
                });
                offset += COLLATERAL_SIZE;
            }

            // Borrows follow deposits
            if offset + 4 <= data.len() {
                let borrows_len = read_u32(data, offset) as usize;
                offset += 4;

                for _ in 0..borrows_len.min(10) {
                    if offset + LIQUIDITY_SIZE > data.len() {
                        break;
                    }
                    let reserve = read_pubkey(data, offset);
                    let borrowed_amount = read_u128(data, offset + 48);
                    let market_value = read_u128(data, offset + 64);

                    borrows.push(LendingPosition {
                        reserve,
                        mint: Pubkey::default(),
                        deposited_amount: 0,
                        borrowed_amount: (borrowed_amount >> 20) as u64,
                        market_value_usd: (market_value >> 20) as u64,
                    });
                    offset += LIQUIDITY_SIZE;
                }
            }
        }

        let total_deposit = (deposited_value >> 20) as u64;
        let total_borrow = (borrowed_value >> 20) as u64;

        let health_factor = if total_borrow > 0 {
            total_deposit as f64 / total_borrow as f64
        } else {
            f64::INFINITY
        };

        let max_liquidation = borrows.first().map(|b| b.borrowed_amount / 2).unwrap_or(0);

        Ok(Some(ObligationState {
            address: *address,
            protocol: LendingProtocol::Save,
            owner,
            deposits,
            borrows,
            total_deposit_usd: total_deposit,
            total_borrow_usd: total_borrow,
            health_factor,
            max_liquidation_amount: max_liquidation,
            liquidation_bonus_bps: SAVE_LIQUIDATION_BONUS_BPS,
            slot: 0,
        }))
    }

    fn build_liquidation_ix(
        &self,
        obligation: &ObligationState,
        liquidator: &Pubkey,
        repay_amount: u64,
    ) -> Result<Vec<Instruction>> {
        // Solend/Save LiquidateObligationAndRedeemReserveCollateral
        // Instruction index: 14
        let mut ix_data = Vec::with_capacity(1 + 8);
        ix_data.push(14u8); // instruction index
        ix_data.extend_from_slice(&repay_amount.to_le_bytes());

        let accounts = vec![
            AccountMeta::new(*liquidator, true),
            AccountMeta::new(obligation.address, false),
            // Additional: lending_market, repay_reserve, withdraw_reserve,
            // source_liquidity, dest_collateral, repay_reserve_liquidity_supply,
            // withdraw_reserve_collateral_supply, obligation, lending_market_authority,
            // token_program
        ];

        Ok(vec![Instruction {
            program_id: SAVE_SOLEND,
            accounts,
            data: ix_data,
        }])
    }
}

fn read_u32(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
}

fn read_u64(data: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(data[offset..offset + 8].try_into().unwrap())
}

fn read_u128(data: &[u8], offset: usize) -> u128 {
    u128::from_le_bytes(data[offset..offset + 16].try_into().unwrap())
}

fn read_pubkey(data: &[u8], offset: usize) -> Pubkey {
    Pubkey::new_from_array(data[offset..offset + 32].try_into().unwrap())
}
