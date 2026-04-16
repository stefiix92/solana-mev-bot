use anyhow::{bail, Result};
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;

use mev_common::constants::METEORA_DLMM;
use mev_common::types::{DexType, PoolState, Quote};

use crate::math::constant_product;
use crate::traits::DexAdapter;

// Meteora DLMM LbPair account layout (Anchor, Borsh serialized)
// 8-byte discriminator + fields
const DISCRIMINATOR_LEN: usize = 8;
const LB_PAIR_MIN_SIZE: usize = 400; // Conservative minimum

// Field offsets after discriminator (derived from Anchor Borsh ordering of LbPair struct):
// parameters: StaticParameters (packed struct, ~96 bytes)
// v_parameters: VariableParameters (packed struct, ~32 bytes)
// bump_seed: [u8; 1]
// bin_step_seed: [u8; 2]
// pair_type: u8
// active_id: i32
// bin_step: u16
// status: u8
// require_base_factor_seed: u8
// base_factor_seed: [u8; 2]
// padding1: u8
// token_x_mint: Pubkey
// token_y_mint: Pubkey
// reserve_x: Pubkey (vault address)
// reserve_y: Pubkey (vault address)
// ...

// Static parameters sub-struct offsets (within parameters block):
const PARAMS_BASE_FACTOR_OFFSET: usize = DISCRIMINATOR_LEN; // Start of parameters
const PARAMS_SIZE: usize = 96; // Approximate size of StaticParameters

// Variable parameters
const VPARAMS_SIZE: usize = 32;

// After parameters blocks:
const BUMP_SEED_OFFSET: usize = DISCRIMINATOR_LEN + PARAMS_SIZE + VPARAMS_SIZE; // ~136
const BIN_STEP_SEED_OFFSET: usize = BUMP_SEED_OFFSET + 1; // ~137
const PAIR_TYPE_OFFSET: usize = BIN_STEP_SEED_OFFSET + 2; // ~139
const ACTIVE_ID_OFFSET: usize = PAIR_TYPE_OFFSET + 1; // ~140
const BIN_STEP_OFFSET: usize = ACTIVE_ID_OFFSET + 4; // ~144
const STATUS_OFFSET: usize = BIN_STEP_OFFSET + 2; // ~146
// padding/seeds: ~6 bytes
const TOKEN_X_MINT_OFFSET: usize = STATUS_OFFSET + 6; // ~152
const TOKEN_Y_MINT_OFFSET: usize = TOKEN_X_MINT_OFFSET + 32; // ~184
const RESERVE_X_OFFSET: usize = TOKEN_Y_MINT_OFFSET + 32; // ~216
const RESERVE_Y_OFFSET: usize = RESERVE_X_OFFSET + 32; // ~248

// Fee info within StaticParameters:
const BASE_FACTOR_OFFSET: usize = DISCRIMINATOR_LEN; // u16, first field of StaticParameters
const FILTER_PERIOD_OFFSET: usize = BASE_FACTOR_OFFSET + 2; // u16
const DECAY_PERIOD_OFFSET: usize = FILTER_PERIOD_OFFSET + 2; // u16

/// Meteora DLMM adapter.
///
/// Uses bin-based liquidity model: L = price * x + y per bin.
/// Simplified quoting: treats current active bin region as constant-sum
/// with effective rate derived from bin_step and active_id.
pub struct MeteoraDlmmAdapter;

impl DexAdapter for MeteoraDlmmAdapter {
    fn dex_type(&self) -> DexType {
        DexType::MeteoraDlmm
    }

    fn program_id(&self) -> Pubkey {
        METEORA_DLMM
    }

    fn decode_pool(&self, address: &Pubkey, data: &[u8]) -> Result<Option<PoolState>> {
        if data.len() < LB_PAIR_MIN_SIZE {
            return Ok(None);
        }

        // Read key fields
        let active_id = read_i32(data, ACTIVE_ID_OFFSET);
        let bin_step = read_u16(data, BIN_STEP_OFFSET);
        let token_x_mint = read_pubkey(data, TOKEN_X_MINT_OFFSET);
        let token_y_mint = read_pubkey(data, TOKEN_Y_MINT_OFFSET);
        let reserve_x_vault = read_pubkey(data, RESERVE_X_OFFSET);
        let reserve_y_vault = read_pubkey(data, RESERVE_Y_OFFSET);

        if bin_step == 0 {
            return Ok(None);
        }

        // Base fee from StaticParameters
        let base_factor = read_u16(data, BASE_FACTOR_OFFSET);

        // Fee calculation: base_fee = base_factor * bin_step * 10 (in 1e9 precision)
        // Simplified: fee_bps ≈ base_factor * bin_step / 100
        let fee_bps = (base_factor as u64) * (bin_step as u64) / 100;
        let fee_numerator = fee_bps;
        let fee_denominator = 10_000;

        // Derive approximate price from active_id and bin_step:
        // price = (1 + bin_step/10000)^active_id
        // For the graph, we use this to set virtual reserves
        let price = compute_bin_price(active_id, bin_step);

        // Virtual reserves based on price (for graph edge weight calculation)
        // Using a nominal amount to establish the rate
        let virtual_x = 1_000_000_000u64; // 1 unit in base precision
        let virtual_y = (virtual_x as f64 * price) as u64;

        Ok(Some(PoolState {
            address: *address,
            dex_type: DexType::MeteoraDlmm,
            token_a_mint: token_x_mint,
            token_b_mint: token_y_mint,
            token_a_vault: reserve_x_vault,
            token_b_vault: reserve_y_vault,
            token_a_amount: virtual_x,
            token_b_amount: virtual_y.max(1),
            fee_numerator,
            fee_denominator,
            slot: 0,
        }))
    }

    fn quote(&self, pool: &PoolState, input_mint: &Pubkey, amount_in: u64) -> Result<Quote> {
        // Simplified: use constant product with virtual reserves
        // Accurate for small swaps within the active bin range
        let (reserve_in, reserve_out, output_mint) = if *input_mint == pool.token_a_mint {
            (pool.token_a_amount, pool.token_b_amount, pool.token_b_mint)
        } else if *input_mint == pool.token_b_mint {
            (pool.token_b_amount, pool.token_a_amount, pool.token_a_mint)
        } else {
            bail!("Input mint {} not in pool {}", input_mint, pool.address);
        };

        if reserve_in == 0 || reserve_out == 0 {
            bail!("Pool {} has zero reserves", pool.address);
        }

        let (amount_out, fee_amount) = constant_product::swap_base_in(
            reserve_in, reserve_out, amount_in,
            pool.fee_numerator, pool.fee_denominator,
        )
        .ok_or_else(|| anyhow::anyhow!("Swap math overflow for pool {}", pool.address))?;

        let price_impact = constant_product::price_impact_bps(
            reserve_in, reserve_out, amount_in, amount_out,
        );

        Ok(Quote {
            input_mint: *input_mint,
            output_mint,
            amount_in,
            amount_out,
            fee_amount,
            price_impact_bps: price_impact,
        })
    }

    fn build_swap_ix(
        &self,
        pool: &PoolState,
        _input_mint: &Pubkey,
        amount_in: u64,
        min_amount_out: u64,
        user_wallet: &Pubkey,
        user_source_ata: &Pubkey,
        user_dest_ata: &Pubkey,
    ) -> Result<Vec<Instruction>> {
        // Swap2 discriminator: [65, 75, 63, 76, 235, 91, 91, 136]
        let discriminator: [u8; 8] = [65, 75, 63, 76, 235, 91, 91, 136];

        let mut ix_data = Vec::with_capacity(8 + 8 + 8);
        ix_data.extend_from_slice(&discriminator);
        ix_data.extend_from_slice(&amount_in.to_le_bytes());
        ix_data.extend_from_slice(&min_amount_out.to_le_bytes());

        let accounts = vec![
            AccountMeta::new(pool.address, false),
            AccountMeta::new_readonly(spl_token::id(), false),
            AccountMeta::new(*user_source_ata, false),
            AccountMeta::new(*user_dest_ata, false),
            AccountMeta::new(pool.token_a_vault, false),
            AccountMeta::new(pool.token_b_vault, false),
            AccountMeta::new_readonly(*user_wallet, true),
            // Bin arrays would need to be resolved for production
        ];

        Ok(vec![Instruction {
            program_id: METEORA_DLMM,
            accounts,
            data: ix_data,
        }])
    }
}

/// Compute the price at a given bin: price = (1 + bin_step/10000)^active_id
fn compute_bin_price(active_id: i32, bin_step: u16) -> f64 {
    let base = 1.0 + (bin_step as f64) / 10_000.0;
    base.powi(active_id)
}

fn read_u16(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap())
}

fn read_i32(data: &[u8], offset: usize) -> i32 {
    i32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
}

fn read_pubkey(data: &[u8], offset: usize) -> Pubkey {
    Pubkey::new_from_array(data[offset..offset + 32].try_into().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bin_price_at_zero() {
        let price = compute_bin_price(0, 100);
        assert!((price - 1.0).abs() < 0.0001, "Price at bin 0 should be ~1.0");
    }

    #[test]
    fn test_bin_price_positive() {
        let price = compute_bin_price(100, 100);
        // (1 + 0.01)^100 ≈ 2.7048
        assert!(price > 2.7 && price < 2.71, "Price should be ~2.7048, got {}", price);
    }

    #[test]
    fn test_bin_price_negative() {
        let price = compute_bin_price(-100, 100);
        // (1.01)^(-100) ≈ 0.3697
        assert!(price > 0.36 && price < 0.38, "Price should be ~0.37, got {}", price);
    }
}
