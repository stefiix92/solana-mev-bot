use anyhow::Result;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;

use mev_common::constants::KAMINO_KLEND;

use crate::traits::{LendingAdapter, LendingPosition, LendingProtocol, ObligationState};

// Kamino KLend Obligation account layout (Anchor)
// 8-byte discriminator + fields
const DISCRIMINATOR_LEN: usize = 8;
const OBLIGATION_MIN_SIZE: usize = 400;

// KLend Obligation layout (approximate offsets after discriminator):
// tag: u64 (8)
// last_update: LastUpdate { slot: u64, stale: u8 } (9, padded to 16)
// lending_market: Pubkey (32)
// owner: Pubkey (32)
// deposits: Vec<ObligationCollateral> — prefixed with u32 length
// borrows: Vec<ObligationLiquidity> — prefixed with u32 length
// deposited_value_sf: u128 (scaled fraction, 16 bytes)
// borrowed_value_sf: u128 (16)
// allowed_borrow_value_sf: u128 (16)
// unhealthy_borrow_value_sf: u128 (16)
// ...

const TAG_OFFSET: usize = DISCRIMINATOR_LEN;
const LAST_UPDATE_OFFSET: usize = TAG_OFFSET + 8;
const LENDING_MARKET_OFFSET: usize = LAST_UPDATE_OFFSET + 16;
const OWNER_OFFSET: usize = LENDING_MARKET_OFFSET + 32;
// After owner: deposits_len (u32), then N × ObligationCollateral
const DEPOSITS_LEN_OFFSET: usize = OWNER_OFFSET + 32;

// ObligationCollateral: deposit_reserve(32) + deposited_amount(u64=8) + market_value_sf(u128=16) + padding
const OBLIGATION_COLLATERAL_SIZE: usize = 56; // 32 + 8 + 16

// After deposits: borrows_len (u32), then N × ObligationLiquidity
// ObligationLiquidity: borrow_reserve(32) + cumulative_borrow_rate_bsf(u128=16) + borrowed_amount_sf(u128=16) + market_value_sf(u128=16) + padding
const OBLIGATION_LIQUIDITY_SIZE: usize = 80; // 32 + 16 + 16 + 16

// Kamino liquidation bonus: 2.5% to liquidator, 2.5% to insurance fund
const KAMINO_LIQUIDATION_BONUS_BPS: u16 = 250;

/// Kamino KLend adapter.
pub struct KaminoAdapter;

impl LendingAdapter for KaminoAdapter {
    fn protocol(&self) -> LendingProtocol {
        LendingProtocol::Kamino
    }

    fn program_id(&self) -> Pubkey {
        KAMINO_KLEND
    }

    fn decode_obligation(&self, address: &Pubkey, data: &[u8]) -> Result<Option<ObligationState>> {
        if data.len() < OBLIGATION_MIN_SIZE {
            return Ok(None);
        }

        let owner = read_pubkey(data, OWNER_OFFSET);

        // Parse deposits
        let deposits_len = read_u32(data, DEPOSITS_LEN_OFFSET) as usize;
        if deposits_len > 10 {
            return Ok(None); // Sanity check
        }

        let mut deposits = Vec::with_capacity(deposits_len);
        let mut offset = DEPOSITS_LEN_OFFSET + 4;

        for _ in 0..deposits_len {
            if offset + OBLIGATION_COLLATERAL_SIZE > data.len() {
                break;
            }
            let reserve = read_pubkey(data, offset);
            let amount = read_u64(data, offset + 32);
            let market_value = read_u128(data, offset + 40) as u64; // Scaled fraction → approximate

            deposits.push(LendingPosition {
                reserve,
                mint: Pubkey::default(), // Resolved from reserve account
                deposited_amount: amount,
                borrowed_amount: 0,
                market_value_usd: market_value / (1u64 << 20), // Rough SF→USD conversion
            });

            offset += OBLIGATION_COLLATERAL_SIZE;
        }

        // Parse borrows
        let borrows_len = if offset + 4 <= data.len() {
            read_u32(data, offset) as usize
        } else {
            0
        };
        offset += 4;

        let mut borrows = Vec::with_capacity(borrows_len);
        for _ in 0..borrows_len.min(10) {
            if offset + OBLIGATION_LIQUIDITY_SIZE > data.len() {
                break;
            }
            let reserve = read_pubkey(data, offset);
            let borrowed_sf = read_u128(data, offset + 48); // borrowed_amount_sf
            let market_value = read_u128(data, offset + 64) as u64;

            borrows.push(LendingPosition {
                reserve,
                mint: Pubkey::default(),
                deposited_amount: 0,
                borrowed_amount: (borrowed_sf >> 20) as u64, // Rough SF conversion
                market_value_usd: market_value / (1u64 << 20),
            });

            offset += OBLIGATION_LIQUIDITY_SIZE;
        }

        let total_deposit: u64 = deposits.iter().map(|d| d.market_value_usd).sum();
        let total_borrow: u64 = borrows.iter().map(|b| b.market_value_usd).sum();

        let health_factor = if total_borrow > 0 {
            total_deposit as f64 / total_borrow as f64
        } else {
            f64::INFINITY
        };

        // Max liquidation: up to 50% of the borrow (close factor)
        let max_liquidation = borrows.first().map(|b| b.borrowed_amount / 2).unwrap_or(0);

        Ok(Some(ObligationState {
            address: *address,
            protocol: LendingProtocol::Kamino,
            owner,
            deposits,
            borrows,
            total_deposit_usd: total_deposit,
            total_borrow_usd: total_borrow,
            health_factor,
            max_liquidation_amount: max_liquidation,
            liquidation_bonus_bps: KAMINO_LIQUIDATION_BONUS_BPS,
            slot: 0,
        }))
    }

    fn build_liquidation_ix(
        &self,
        obligation: &ObligationState,
        liquidator: &Pubkey,
        repay_amount: u64,
    ) -> Result<Vec<Instruction>> {
        // KLend liquidateObligationAndRedeemReserveCollateral
        // Anchor discriminator
        let discriminator: [u8; 8] = [117, 76, 214, 245, 87, 163, 24, 63];

        let mut ix_data = Vec::with_capacity(8 + 8 + 8);
        ix_data.extend_from_slice(&discriminator);
        ix_data.extend_from_slice(&repay_amount.to_le_bytes());
        ix_data.extend_from_slice(&0u64.to_le_bytes()); // min_acceptable_received

        // Simplified account list — full list requires reserve accounts
        let accounts = vec![
            AccountMeta::new_readonly(*liquidator, true),
            AccountMeta::new(obligation.address, false),
            // Additional accounts (lending_market, reserves, vaults, oracles)
            // would be resolved from the reserve account data
        ];

        Ok(vec![Instruction {
            program_id: KAMINO_KLEND,
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
