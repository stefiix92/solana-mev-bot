use anyhow::{bail, Result};
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;

use mev_common::constants::RAYDIUM_CLMM;
use mev_common::types::{DexType, PoolState, Quote};

use crate::math::constant_product;
use crate::traits::DexAdapter;

// Raydium CLMM PoolState: 1544 bytes (8 discriminator + 1536 body)
const DISCRIMINATOR_LEN: usize = 8;
const POOL_STATE_SIZE: usize = 1544;

// Offsets after discriminator
const BUMP_OFFSET: usize = DISCRIMINATOR_LEN;                          // 8
const AMM_CONFIG_OFFSET: usize = BUMP_OFFSET + 1;                      // 9
const OWNER_OFFSET: usize = AMM_CONFIG_OFFSET + 32;                    // 41
const TOKEN_MINT_0_OFFSET: usize = OWNER_OFFSET + 32;                  // 73
const TOKEN_MINT_1_OFFSET: usize = TOKEN_MINT_0_OFFSET + 32;           // 105
const TOKEN_VAULT_0_OFFSET: usize = TOKEN_MINT_1_OFFSET + 32;          // 137
const TOKEN_VAULT_1_OFFSET: usize = TOKEN_VAULT_0_OFFSET + 32;         // 169
const OBSERVATION_KEY_OFFSET: usize = TOKEN_VAULT_1_OFFSET + 32;       // 201
const MINT_DECIMALS_0_OFFSET: usize = OBSERVATION_KEY_OFFSET + 32;     // 233
const MINT_DECIMALS_1_OFFSET: usize = MINT_DECIMALS_0_OFFSET + 1;      // 234
const TICK_SPACING_OFFSET: usize = MINT_DECIMALS_1_OFFSET + 1;         // 235
const LIQUIDITY_OFFSET: usize = TICK_SPACING_OFFSET + 2;               // 237
const SQRT_PRICE_X64_OFFSET: usize = LIQUIDITY_OFFSET + 16;            // 253
const TICK_CURRENT_OFFSET: usize = SQRT_PRICE_X64_OFFSET + 16;         // 269
const STATUS_OFFSET: usize = TICK_CURRENT_OFFSET + 4 + 2 + 2;         // 277 (after padding3+4)

// Status flags
const STATUS_OPEN_POSITION: u8 = 1;
const STATUS_SWAP: u8 = 4;

/// Raydium CLMM (concentrated liquidity) adapter.
///
/// Uses virtual reserves derived from sqrt_price and liquidity for approximate quoting.
/// Full tick-array traversal for exact amounts deferred to production optimization.
pub struct RaydiumClmmAdapter;

impl DexAdapter for RaydiumClmmAdapter {
    fn dex_type(&self) -> DexType {
        DexType::RaydiumClmm
    }

    fn program_id(&self) -> Pubkey {
        RAYDIUM_CLMM
    }

    fn decode_pool(&self, address: &Pubkey, data: &[u8]) -> Result<Option<PoolState>> {
        if data.len() < POOL_STATE_SIZE {
            return Ok(None);
        }

        let status = data[STATUS_OFFSET];
        // Must have swap enabled
        if status & STATUS_SWAP == 0 {
            return Ok(None);
        }

        let liquidity = read_u128(data, LIQUIDITY_OFFSET);
        let sqrt_price_x64 = read_u128(data, SQRT_PRICE_X64_OFFSET);

        if liquidity == 0 || sqrt_price_x64 == 0 {
            return Ok(None);
        }

        let token_mint_0 = read_pubkey(data, TOKEN_MINT_0_OFFSET);
        let token_mint_1 = read_pubkey(data, TOKEN_MINT_1_OFFSET);
        let token_vault_0 = read_pubkey(data, TOKEN_VAULT_0_OFFSET);
        let token_vault_1 = read_pubkey(data, TOKEN_VAULT_1_OFFSET);
        let tick_current = read_i32(data, TICK_CURRENT_OFFSET);

        // Derive virtual reserves from concentrated liquidity parameters:
        // reserve_0 ≈ L / sqrt_price
        // reserve_1 ≈ L * sqrt_price
        let sqrt_price_f64 = sqrt_price_x64 as f64 / (1u128 << 64) as f64;
        let liquidity_f64 = liquidity as f64;

        let virtual_0 = if sqrt_price_f64 > 0.0 {
            (liquidity_f64 / sqrt_price_f64) as u64
        } else {
            0
        };
        let virtual_1 = (liquidity_f64 * sqrt_price_f64) as u64;

        if virtual_0 == 0 || virtual_1 == 0 {
            return Ok(None);
        }

        // Fee: read from AmmConfig account (not in pool state directly).
        // Common Raydium CLMM fee tiers: 100 (1bp), 500 (5bp), 2500 (25bp), 10000 (100bp)
        // Default to 25bp (most common for major pairs)
        let fee_numerator = 25u64;
        let fee_denominator = 10_000u64;

        Ok(Some(PoolState {
            address: *address,
            dex_type: DexType::RaydiumClmm,
            token_a_mint: token_mint_0,
            token_b_mint: token_mint_1,
            token_a_vault: token_vault_0,
            token_b_vault: token_vault_1,
            token_a_amount: virtual_0,
            token_b_amount: virtual_1,
            fee_numerator,
            fee_denominator,
            slot: 0,
        }))
    }

    fn quote(&self, pool: &PoolState, input_mint: &Pubkey, amount_in: u64) -> Result<Quote> {
        let (reserve_in, reserve_out, output_mint) = if *input_mint == pool.token_a_mint {
            (pool.token_a_amount, pool.token_b_amount, pool.token_b_mint)
        } else if *input_mint == pool.token_b_mint {
            (pool.token_b_amount, pool.token_a_amount, pool.token_a_mint)
        } else {
            bail!("Input mint {} not in pool {}", input_mint, pool.address);
        };

        if reserve_in == 0 || reserve_out == 0 {
            bail!("Pool {} has zero virtual reserves", pool.address);
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
        input_mint: &Pubkey,
        amount_in: u64,
        min_amount_out: u64,
        user_wallet: &Pubkey,
        user_source_ata: &Pubkey,
        user_dest_ata: &Pubkey,
    ) -> Result<Vec<Instruction>> {
        // swap_v2 instruction
        // Discriminator from Anchor: sha256("global:swap_v2")[..8]
        let discriminator: [u8; 8] = [43, 4, 237, 11, 26, 201, 30, 98];

        let is_base_input = true;
        let sqrt_price_limit: u128 = 0; // No limit

        let mut ix_data = Vec::with_capacity(8 + 8 + 8 + 16 + 1);
        ix_data.extend_from_slice(&discriminator);
        ix_data.extend_from_slice(&amount_in.to_le_bytes());
        ix_data.extend_from_slice(&min_amount_out.to_le_bytes());
        ix_data.extend_from_slice(&sqrt_price_limit.to_le_bytes());
        ix_data.push(is_base_input as u8);

        // Determine vault order
        let (input_vault, output_vault) = if *input_mint == pool.token_a_mint {
            (pool.token_a_vault, pool.token_b_vault)
        } else {
            (pool.token_b_vault, pool.token_a_vault)
        };

        // Need: amm_config and observation_key from pool data (stored in account)
        // For now, use placeholders — resolved from cached pool account data in production
        let accounts = vec![
            AccountMeta::new_readonly(*user_wallet, true),          // payer
            AccountMeta::new(pool.address, false),                   // pool_state
            AccountMeta::new(*user_source_ata, false),               // input_token_account
            AccountMeta::new(*user_dest_ata, false),                 // output_token_account
            AccountMeta::new(input_vault, false),                    // input_vault
            AccountMeta::new(output_vault, false),                   // output_vault
            AccountMeta::new_readonly(spl_token::id(), false),       // token_program
        ];

        Ok(vec![Instruction {
            program_id: RAYDIUM_CLMM,
            accounts,
            data: ix_data,
        }])
    }
}

fn read_u128(data: &[u8], offset: usize) -> u128 {
    u128::from_le_bytes(data[offset..offset + 16].try_into().unwrap())
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
    fn test_decode_clmm_too_small() {
        let adapter = RaydiumClmmAdapter;
        let pubkey = Pubkey::new_unique();
        assert!(adapter.decode_pool(&pubkey, &[0u8; 100]).unwrap().is_none());
    }

    #[test]
    fn test_decode_clmm_active_pool() {
        let adapter = RaydiumClmmAdapter;
        let pool_addr = Pubkey::new_unique();
        let mut data = vec![0u8; POOL_STATE_SIZE];

        // Set status = swap enabled (bit 2)
        data[STATUS_OFFSET] = STATUS_SWAP;

        // Set liquidity (non-zero)
        let liquidity: u128 = 1_000_000_000_000;
        data[LIQUIDITY_OFFSET..LIQUIDITY_OFFSET + 16]
            .copy_from_slice(&liquidity.to_le_bytes());

        // Set sqrt_price_x64 (represents price ~100 USDC/SOL)
        // sqrt(100) * 2^64 ≈ 10 * 18446744073709551616 ≈ 1.84e20
        let sqrt_price: u128 = 184_467_440_737_095_516_160;
        data[SQRT_PRICE_X64_OFFSET..SQRT_PRICE_X64_OFFSET + 16]
            .copy_from_slice(&sqrt_price.to_le_bytes());

        // Set mints
        let mint0 = Pubkey::new_unique();
        let mint1 = Pubkey::new_unique();
        data[TOKEN_MINT_0_OFFSET..TOKEN_MINT_0_OFFSET + 32]
            .copy_from_slice(mint0.as_ref());
        data[TOKEN_MINT_1_OFFSET..TOKEN_MINT_1_OFFSET + 32]
            .copy_from_slice(mint1.as_ref());

        let result = adapter.decode_pool(&pool_addr, &data).unwrap();
        assert!(result.is_some());

        let pool = result.unwrap();
        assert_eq!(pool.token_a_mint, mint0);
        assert_eq!(pool.token_b_mint, mint1);
        assert!(pool.token_a_amount > 0);
        assert!(pool.token_b_amount > 0);
    }
}
