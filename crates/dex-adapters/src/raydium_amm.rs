use anyhow::{bail, Result};
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;

use mev_common::constants::RAYDIUM_AMM_V4;
use mev_common::types::{DexType, PoolState, Quote};

use crate::math::constant_product;
use crate::traits::DexAdapter;

// -- AmmInfo byte offsets (752 bytes total, #[repr(C, packed)]) --
// These match the on-chain Raydium AMM V4 program layout exactly.
const STATUS_OFFSET: usize = 0;
const COIN_DECIMALS_OFFSET: usize = 32;
const PC_DECIMALS_OFFSET: usize = 40;
const SWAP_FEE_NUM_OFFSET: usize = 176;
const SWAP_FEE_DEN_OFFSET: usize = 184;
const COIN_VAULT_OFFSET: usize = 336;
const PC_VAULT_OFFSET: usize = 368;
const COIN_MINT_OFFSET: usize = 400;
const PC_MINT_OFFSET: usize = 432;
const LP_MINT_OFFSET: usize = 464;
const OPEN_ORDERS_OFFSET: usize = 496;
const MARKET_OFFSET: usize = 528;
const MARKET_PROGRAM_OFFSET: usize = 560;
const TARGET_ORDERS_OFFSET: usize = 592;

const AMM_INFO_SIZE: usize = 752;

// AMM status: 1 = initialized, 6 = swapOnly (most common for active pools)
const STATUS_SWAP_ONLY: u64 = 6;
const STATUS_INITIALIZED: u64 = 1;

/// Raydium AMM V4 adapter.
///
/// Note on reserves: The AmmInfo account does NOT store reserve amounts directly.
/// Reserves are held in the coin_vault and pc_vault SPL token accounts.
/// The PoolState.token_a_amount / token_b_amount fields must be populated
/// from vault account data separately (via account cache).
pub struct RaydiumAmmAdapter;

impl DexAdapter for RaydiumAmmAdapter {
    fn dex_type(&self) -> DexType {
        DexType::RaydiumAmm
    }

    fn program_id(&self) -> Pubkey {
        RAYDIUM_AMM_V4
    }

    fn decode_pool(&self, address: &Pubkey, data: &[u8]) -> Result<Option<PoolState>> {
        if data.len() < AMM_INFO_SIZE {
            return Ok(None);
        }

        let status = read_u64(data, STATUS_OFFSET);

        // Only decode active pools
        if status != STATUS_INITIALIZED && status != STATUS_SWAP_ONLY {
            return Ok(None);
        }

        let coin_mint = read_pubkey(data, COIN_MINT_OFFSET);
        let pc_mint = read_pubkey(data, PC_MINT_OFFSET);
        let coin_vault = read_pubkey(data, COIN_VAULT_OFFSET);
        let pc_vault = read_pubkey(data, PC_VAULT_OFFSET);
        let swap_fee_numerator = read_u64(data, SWAP_FEE_NUM_OFFSET);
        let swap_fee_denominator = read_u64(data, SWAP_FEE_DEN_OFFSET);

        Ok(Some(PoolState {
            address: *address,
            dex_type: DexType::RaydiumAmm,
            token_a_mint: coin_mint,
            token_b_mint: pc_mint,
            token_a_vault: coin_vault,
            token_b_vault: pc_vault,
            // Reserves must be populated from vault accounts
            token_a_amount: 0,
            token_b_amount: 0,
            fee_numerator: swap_fee_numerator,
            fee_denominator: swap_fee_denominator,
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
            bail!("Pool {} has zero reserves", pool.address);
        }

        let (amount_out, fee_amount) = constant_product::swap_base_in(
            reserve_in,
            reserve_out,
            amount_in,
            pool.fee_numerator,
            pool.fee_denominator,
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
        // SwapBaseIn instruction discriminator = 9
        let mut ix_data = Vec::with_capacity(17);
        ix_data.push(9u8); // discriminator
        ix_data.extend_from_slice(&amount_in.to_le_bytes());
        ix_data.extend_from_slice(&min_amount_out.to_le_bytes());

        // Determine vault order based on swap direction
        let (source_vault, dest_vault) = if *input_mint == pool.token_a_mint {
            (pool.token_a_vault, pool.token_b_vault)
        } else {
            (pool.token_b_vault, pool.token_a_vault)
        };

        // We need to read additional pubkeys from the pool account data.
        // For now, derive the authority PDA.
        let (authority, _nonce) = Pubkey::find_program_address(
            &[
                // Raydium AMM uses specific seeds for authority
                &[97, 109, 109, 32, 97, 117, 116, 104, 111, 114, 105, 116, 121], // "amm authority"
            ],
            &RAYDIUM_AMM_V4,
        );

        // Simplified SwapBaseIn accounts (V2 — no orderbook, 9 accounts)
        let accounts = vec![
            AccountMeta::new_readonly(spl_token::id(), false),        // Token program
            AccountMeta::new(pool.address, false),                     // AMM account
            AccountMeta::new_readonly(authority, false),                // AMM authority
            AccountMeta::new(source_vault, false),                     // Source vault
            AccountMeta::new(dest_vault, false),                       // Dest vault
            AccountMeta::new(*user_source_ata, false),                 // User source token
            AccountMeta::new(*user_dest_ata, false),                   // User dest token
            AccountMeta::new_readonly(*user_wallet, true),             // User wallet (signer)
        ];

        Ok(vec![Instruction {
            program_id: RAYDIUM_AMM_V4,
            accounts,
            data: ix_data,
        }])
    }
}

// -- Byte reading helpers --

fn read_u64(data: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(data[offset..offset + 8].try_into().unwrap())
}

fn read_pubkey(data: &[u8], offset: usize) -> Pubkey {
    Pubkey::new_from_array(data[offset..offset + 32].try_into().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_pool_too_short() {
        let adapter = RaydiumAmmAdapter;
        let pubkey = Pubkey::new_unique();
        let result = adapter.decode_pool(&pubkey, &[0u8; 100]).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_decode_pool_inactive_status() {
        let adapter = RaydiumAmmAdapter;
        let pubkey = Pubkey::new_unique();
        // Status 0 = uninitialized
        let data = vec![0u8; AMM_INFO_SIZE];
        let result = adapter.decode_pool(&pubkey, &data).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_decode_pool_active() {
        let adapter = RaydiumAmmAdapter;
        let pool_address = Pubkey::new_unique();
        let mut data = vec![0u8; AMM_INFO_SIZE];

        // Set status = 6 (swap only)
        data[STATUS_OFFSET..STATUS_OFFSET + 8]
            .copy_from_slice(&6u64.to_le_bytes());

        // Set fee = 25/10000
        data[SWAP_FEE_NUM_OFFSET..SWAP_FEE_NUM_OFFSET + 8]
            .copy_from_slice(&25u64.to_le_bytes());
        data[SWAP_FEE_DEN_OFFSET..SWAP_FEE_DEN_OFFSET + 8]
            .copy_from_slice(&10_000u64.to_le_bytes());

        // Set mints
        let mint_a = Pubkey::new_unique();
        let mint_b = Pubkey::new_unique();
        data[COIN_MINT_OFFSET..COIN_MINT_OFFSET + 32]
            .copy_from_slice(mint_a.as_ref());
        data[PC_MINT_OFFSET..PC_MINT_OFFSET + 32]
            .copy_from_slice(mint_b.as_ref());

        let result = adapter.decode_pool(&pool_address, &data).unwrap();
        assert!(result.is_some());
        let pool = result.unwrap();
        assert_eq!(pool.token_a_mint, mint_a);
        assert_eq!(pool.token_b_mint, mint_b);
        assert_eq!(pool.fee_numerator, 25);
        assert_eq!(pool.fee_denominator, 10_000);
    }
}
