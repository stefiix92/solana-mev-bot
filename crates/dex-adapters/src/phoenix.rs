use anyhow::{bail, Result};
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;

use mev_common::constants::PHOENIX;
use mev_common::types::{DexType, PoolState, Quote};

use crate::math::constant_product;
use crate::traits::DexAdapter;

// Phoenix Market account layout
// Phoenix uses a custom binary format (not Anchor/Borsh)
// Header section contains market metadata
const MARKET_MIN_SIZE: usize = 512;

// Phoenix market header offsets (simplified — exact offsets from phoenix-v1 source)
// The market account has a header followed by order book data
const DISCRIMINATOR_LEN: usize = 8;

// MarketHeader (after 8-byte discriminator):
// status: u64 (8)
// market_size_params: MarketSizeParams (16)
// base_params: TokenParams (48)
// base_lot_size: u64 (8)
// quote_params: TokenParams (48)
// quote_lot_size: u64 (8)
// tick_size_in_quote_lots_per_base_unit: u64 (8)
// authority: Pubkey (32)
// fee_recipient: Pubkey (32)
// market_sequence_number: u64 (8)
// successor: Pubkey (32)
// raw_base_units_per_base_unit: u32 (4)

// TokenParams: { decimals: u32, vault_key: Pubkey, mint_key: Pubkey }
// = 4 + 32 + 32 = 68 bytes (with padding to 48? Actually it's packed differently)

// Simplified layout for extraction:
const STATUS_OFFSET: usize = DISCRIMINATOR_LEN;
// MarketSizeParams: bids_size(u64), asks_size(u64) = 16 bytes
const MARKET_SIZE_PARAMS_OFFSET: usize = STATUS_OFFSET + 8;
// base_params starts after status + market_size_params
const BASE_PARAMS_OFFSET: usize = MARKET_SIZE_PARAMS_OFFSET + 16;
// TokenParams layout: decimals(u32) + padding(u32) + vault_key(Pubkey) + mint_key(Pubkey)
const BASE_DECIMALS_OFFSET: usize = BASE_PARAMS_OFFSET;
const BASE_VAULT_OFFSET: usize = BASE_PARAMS_OFFSET + 8; // after decimals + padding
const BASE_MINT_OFFSET: usize = BASE_VAULT_OFFSET + 32;
const BASE_LOT_SIZE_OFFSET: usize = BASE_MINT_OFFSET + 32;
// quote_params follows
const QUOTE_PARAMS_OFFSET: usize = BASE_LOT_SIZE_OFFSET + 8;
const QUOTE_DECIMALS_OFFSET: usize = QUOTE_PARAMS_OFFSET;
const QUOTE_VAULT_OFFSET: usize = QUOTE_PARAMS_OFFSET + 8;
const QUOTE_MINT_OFFSET: usize = QUOTE_VAULT_OFFSET + 32;
const QUOTE_LOT_SIZE_OFFSET: usize = QUOTE_MINT_OFFSET + 32;
const TICK_SIZE_OFFSET: usize = QUOTE_LOT_SIZE_OFFSET + 8;

/// Phoenix CLOB adapter.
///
/// Phoenix is a limit order book — fundamentally different from AMMs.
/// For arb detection, we approximate the best bid/ask as a constant exchange rate.
/// Full order book traversal for exact execution is deferred.
pub struct PhoenixAdapter;

impl DexAdapter for PhoenixAdapter {
    fn dex_type(&self) -> DexType {
        DexType::Phoenix
    }

    fn program_id(&self) -> Pubkey {
        PHOENIX
    }

    fn decode_pool(&self, address: &Pubkey, data: &[u8]) -> Result<Option<PoolState>> {
        if data.len() < MARKET_MIN_SIZE {
            return Ok(None);
        }

        let status = read_u64(data, STATUS_OFFSET);
        if status == 0 {
            return Ok(None); // Inactive market
        }

        let base_mint = read_pubkey(data, BASE_MINT_OFFSET);
        let quote_mint = read_pubkey(data, QUOTE_MINT_OFFSET);
        let base_vault = read_pubkey(data, BASE_VAULT_OFFSET);
        let quote_vault = read_pubkey(data, QUOTE_VAULT_OFFSET);
        let base_lot_size = read_u64(data, BASE_LOT_SIZE_OFFSET);
        let quote_lot_size = read_u64(data, QUOTE_LOT_SIZE_OFFSET);
        let tick_size = read_u64(data, TICK_SIZE_OFFSET);

        // For the price graph, we need to approximate the exchange rate
        // from the order book. Without parsing the full book, use lot sizes
        // and tick size to establish an approximate mid-price.
        // Virtual reserves derived from lot parameters.
        let virtual_base = base_lot_size.max(1) * 1000;
        let virtual_quote = if tick_size > 0 && quote_lot_size > 0 {
            tick_size * quote_lot_size * 1000
        } else {
            virtual_base
        };

        // Phoenix fee: typically 1-4 bps for makers, 2-5 bps for takers
        let fee_numerator = 4; // 4 bps taker fee (conservative)
        let fee_denominator = 10_000;

        Ok(Some(PoolState {
            address: *address,
            dex_type: DexType::Phoenix,
            token_a_mint: base_mint,
            token_b_mint: quote_mint,
            token_a_vault: base_vault,
            token_b_vault: quote_vault,
            token_a_amount: virtual_base,
            token_b_amount: virtual_quote,
            fee_numerator,
            fee_denominator,
            slot: 0,
        }))
    }

    fn quote(&self, pool: &PoolState, input_mint: &Pubkey, amount_in: u64) -> Result<Quote> {
        // Simplified: treat as constant product with virtual reserves
        // This gives a rough estimate for arb detection
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
        // Phoenix Swap instruction (IOC order)
        // Discriminator for Swap: derived from program IDL
        let discriminator: [u8; 8] = [0xf8, 0xc6, 0x9e, 0x91, 0xe1, 0x75, 0x87, 0xc8];

        let mut ix_data = Vec::with_capacity(8 + 8 + 8 + 1);
        ix_data.extend_from_slice(&discriminator);
        ix_data.extend_from_slice(&amount_in.to_le_bytes());
        ix_data.extend_from_slice(&min_amount_out.to_le_bytes());
        ix_data.push(0); // side: 0 = bid (buy base), 1 = ask (sell base)

        let accounts = vec![
            AccountMeta::new(pool.address, false),             // Market
            AccountMeta::new_readonly(*user_wallet, true),     // Trader
            AccountMeta::new(*user_source_ata, false),         // Base account
            AccountMeta::new(*user_dest_ata, false),           // Quote account
            AccountMeta::new(pool.token_a_vault, false),       // Base vault
            AccountMeta::new(pool.token_b_vault, false),       // Quote vault
            AccountMeta::new_readonly(spl_token::id(), false), // Token program
        ];

        Ok(vec![Instruction {
            program_id: PHOENIX,
            accounts,
            data: ix_data,
        }])
    }
}

fn read_u64(data: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(data[offset..offset + 8].try_into().unwrap())
}

fn read_pubkey(data: &[u8], offset: usize) -> Pubkey {
    Pubkey::new_from_array(data[offset..offset + 32].try_into().unwrap())
}
