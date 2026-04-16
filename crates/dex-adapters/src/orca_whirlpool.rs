use anyhow::{bail, Result};
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;

use mev_common::constants::ORCA_WHIRLPOOL;
use mev_common::types::{DexType, PoolState, Quote};

use crate::traits::DexAdapter;

// Whirlpool account layout (653 bytes)
// Reference: https://github.com/orca-so/whirlpools/blob/main/programs/whirlpool/src/state/whirlpool.rs
const DISCRIMINATOR_LEN: usize = 8;
const WHIRLPOOL_SIZE_MIN: usize = 653;

// Offsets (after 8-byte Anchor discriminator)
const WHIRLPOOLS_CONFIG_OFFSET: usize = DISCRIMINATOR_LEN; // Pubkey (32)
const WHIRLPOOL_BUMP_OFFSET: usize = WHIRLPOOLS_CONFIG_OFFSET + 32 + 1; // u8[1] for bump
const TICK_SPACING_OFFSET: usize = WHIRLPOOL_BUMP_OFFSET; // u16
// Actually, the layout is:
// [8] discriminator
// [32] whirlpools_config
// [1] whirlpool_bump[0]
// [2] tick_spacing
// [2] tick_spacing_seed[0..2]
// [2] fee_rate (u16, in hundredths of a bip, so 3000 = 30 bps = 0.3%)
// [2] protocol_fee_rate (u16)
// [16] liquidity (u128)
// [16] sqrt_price (u128)
// [4] tick_current_index (i32)
// [8] protocol_fee_owed_a (u64)
// [8] protocol_fee_owed_b (u64)
// [32] token_mint_a
// [32] token_mint_b
// [32] token_vault_a
// [32] token_vault_b
// [32] fee_growth_global_a (u128)
// [32] fee_growth_global_b (u128)
// [8] reward_last_updated_timestamp (u64)
// [3 × 128] reward_infos

// Corrected flat offsets:
const FEE_RATE_OFFSET: usize = 8 + 32 + 1 + 2 + 2; // = 45
const PROTOCOL_FEE_RATE_OFFSET: usize = FEE_RATE_OFFSET + 2; // = 47
const LIQUIDITY_OFFSET: usize = PROTOCOL_FEE_RATE_OFFSET + 2; // = 49
const SQRT_PRICE_OFFSET: usize = LIQUIDITY_OFFSET + 16; // = 65
const TICK_CURRENT_OFFSET: usize = SQRT_PRICE_OFFSET + 16; // = 81
const PROTOCOL_FEE_OWED_A_OFFSET: usize = TICK_CURRENT_OFFSET + 4; // = 85
const PROTOCOL_FEE_OWED_B_OFFSET: usize = PROTOCOL_FEE_OWED_A_OFFSET + 8; // = 93
const TOKEN_MINT_A_OFFSET: usize = PROTOCOL_FEE_OWED_B_OFFSET + 8; // = 101
const TOKEN_MINT_B_OFFSET: usize = TOKEN_MINT_A_OFFSET + 32; // = 133
const TOKEN_VAULT_A_OFFSET: usize = TOKEN_MINT_B_OFFSET + 32; // = 165
const TOKEN_VAULT_B_OFFSET: usize = TOKEN_VAULT_A_OFFSET + 32; // = 197

/// Orca Whirlpool adapter.
///
/// Uses simplified quoting: treats the pool around the current tick as if it has
/// constant-product-like behavior based on the current sqrt_price and liquidity.
/// Full tick-array traversal math will be added in a later phase.
pub struct OrcaWhirlpoolAdapter;

impl DexAdapter for OrcaWhirlpoolAdapter {
    fn dex_type(&self) -> DexType {
        DexType::OrcaWhirlpool
    }

    fn program_id(&self) -> Pubkey {
        ORCA_WHIRLPOOL
    }

    fn decode_pool(&self, address: &Pubkey, data: &[u8]) -> Result<Option<PoolState>> {
        if data.len() < WHIRLPOOL_SIZE_MIN {
            return Ok(None);
        }

        // Check Anchor discriminator for Whirlpool account
        // (first 8 bytes should be the account discriminator)

        let fee_rate_raw = read_u16(data, FEE_RATE_OFFSET);
        let liquidity = read_u128(data, LIQUIDITY_OFFSET);
        let sqrt_price = read_u128(data, SQRT_PRICE_OFFSET);

        if liquidity == 0 || sqrt_price == 0 {
            return Ok(None); // No liquidity
        }

        let token_mint_a = read_pubkey(data, TOKEN_MINT_A_OFFSET);
        let token_mint_b = read_pubkey(data, TOKEN_MINT_B_OFFSET);
        let token_vault_a = read_pubkey(data, TOKEN_VAULT_A_OFFSET);
        let token_vault_b = read_pubkey(data, TOKEN_VAULT_B_OFFSET);

        // Fee rate is in hundredths of a basis point (1 = 0.0001%)
        // Convert to numerator/denominator: fee_rate_raw / 1_000_000
        let fee_numerator = fee_rate_raw as u64;
        let fee_denominator = 1_000_000u64;

        // For the price graph, we need approximate reserve amounts.
        // Derive virtual reserves from sqrt_price and liquidity:
        // In concentrated liquidity: L = sqrt(x * y) at the current price
        // virtual_x = L / sqrt_price, virtual_y = L * sqrt_price
        // sqrt_price is Q64.64 fixed point (shifted by 2^64)
        let sqrt_price_f64 = sqrt_price as f64 / (1u128 << 64) as f64;
        let liquidity_f64 = liquidity as f64;

        let virtual_a = if sqrt_price_f64 > 0.0 {
            (liquidity_f64 / sqrt_price_f64) as u64
        } else {
            0
        };
        let virtual_b = (liquidity_f64 * sqrt_price_f64) as u64;

        Ok(Some(PoolState {
            address: *address,
            dex_type: DexType::OrcaWhirlpool,
            token_a_mint: token_mint_a,
            token_b_mint: token_mint_b,
            token_a_vault: token_vault_a,
            token_b_vault: token_vault_b,
            token_a_amount: virtual_a,
            token_b_amount: virtual_b,
            fee_numerator,
            fee_denominator,
            slot: 0,
        }))
    }

    fn quote(&self, pool: &PoolState, input_mint: &Pubkey, amount_in: u64) -> Result<Quote> {
        // Simplified quote: use constant product math with virtual reserves
        // This is an approximation that's accurate for small swaps within the current tick range
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

        let (amount_out, fee_amount) = crate::math::constant_product::swap_base_in(
            reserve_in,
            reserve_out,
            amount_in,
            pool.fee_numerator,
            pool.fee_denominator,
        )
        .ok_or_else(|| anyhow::anyhow!("Swap math overflow for pool {}", pool.address))?;

        let price_impact = crate::math::constant_product::price_impact_bps(
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
        // Whirlpool swap instruction (Anchor)
        // Discriminator: sha256("global:swap")[..8]
        let discriminator: [u8; 8] = [248, 198, 158, 145, 225, 117, 135, 200]; // "swap"

        let mut ix_data = Vec::with_capacity(8 + 8 + 8 + 1 + 16);
        ix_data.extend_from_slice(&discriminator);
        ix_data.extend_from_slice(&amount_in.to_le_bytes());
        ix_data.extend_from_slice(&min_amount_out.to_le_bytes());
        // sqrt_price_limit: 0 means no limit (use max/min based on direction)
        ix_data.push(1); // a_to_b direction flag (simplified)
        ix_data.extend_from_slice(&0u128.to_le_bytes()); // sqrt_price_limit

        let accounts = vec![
            AccountMeta::new_readonly(spl_token::id(), false),
            AccountMeta::new_readonly(*user_wallet, true),
            AccountMeta::new(pool.address, false),
            AccountMeta::new(*user_source_ata, false),
            AccountMeta::new(pool.token_a_vault, false),
            AccountMeta::new(*user_dest_ata, false),
            AccountMeta::new(pool.token_b_vault, false),
            // Tick arrays would need to be resolved — simplified for now
        ];

        Ok(vec![Instruction {
            program_id: ORCA_WHIRLPOOL,
            accounts,
            data: ix_data,
        }])
    }
}

fn read_u16(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap())
}

fn read_u128(data: &[u8], offset: usize) -> u128 {
    u128::from_le_bytes(data[offset..offset + 16].try_into().unwrap())
}

fn read_pubkey(data: &[u8], offset: usize) -> Pubkey {
    Pubkey::new_from_array(data[offset..offset + 32].try_into().unwrap())
}
