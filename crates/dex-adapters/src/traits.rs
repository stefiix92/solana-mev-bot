use anyhow::Result;
use solana_sdk::instruction::Instruction;
use solana_sdk::pubkey::Pubkey;

use mev_common::types::{DexType, PoolState, Quote};

/// Unified trait for decoding DEX pool accounts and building swap instructions.
pub trait DexAdapter: Send + Sync {
    /// The DEX type this adapter handles.
    fn dex_type(&self) -> DexType;

    /// The on-chain program ID for this DEX.
    fn program_id(&self) -> Pubkey;

    /// Decode raw account bytes into a normalized PoolState.
    /// Returns None if the account data doesn't represent a valid pool
    /// (e.g., wrong discriminator, wrong status).
    fn decode_pool(&self, address: &Pubkey, data: &[u8]) -> Result<Option<PoolState>>;

    /// Calculate the output amount for a given input amount through this pool.
    /// This performs the pool's AMM math locally (no RPC calls).
    fn quote(&self, pool: &PoolState, input_mint: &Pubkey, amount_in: u64) -> Result<Quote>;

    /// Build the swap instruction(s) for this DEX.
    fn build_swap_ix(
        &self,
        pool: &PoolState,
        input_mint: &Pubkey,
        amount_in: u64,
        min_amount_out: u64,
        user_wallet: &Pubkey,
        user_source_ata: &Pubkey,
        user_dest_ata: &Pubkey,
    ) -> Result<Vec<Instruction>>;

    /// Return the vault pubkeys that hold the pool's reserves.
    /// These accounts need to be monitored for balance changes.
    fn vault_pubkeys(&self, pool: &PoolState) -> Vec<Pubkey> {
        vec![pool.token_a_vault, pool.token_b_vault]
    }
}
