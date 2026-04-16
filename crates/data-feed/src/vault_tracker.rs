use std::collections::HashSet;
use std::sync::{Arc, RwLock};

use solana_sdk::pubkey::Pubkey;
use tracing::debug;

/// Tracks discovered vault pubkeys from decoded pools.
/// Used to populate reserves from the account cache.
///
/// When a pool is decoded, its vault addresses are registered here.
/// The cache updater uses these to parse token account balances.
#[derive(Clone)]
pub struct VaultTracker {
    /// Set of known vault pubkeys (SPL token accounts holding pool reserves)
    vaults: Arc<RwLock<HashSet<Pubkey>>>,
}

impl VaultTracker {
    pub fn new() -> Self {
        Self {
            vaults: Arc::new(RwLock::new(HashSet::with_capacity(10_000))),
        }
    }

    /// Register vault addresses from a decoded pool.
    pub fn register_vaults(&self, vault_a: &Pubkey, vault_b: &Pubkey) {
        let mut vaults = self.vaults.write().unwrap();
        vaults.insert(*vault_a);
        vaults.insert(*vault_b);
    }

    /// Check if a pubkey is a known vault.
    pub fn is_vault(&self, pubkey: &Pubkey) -> bool {
        self.vaults.read().unwrap().contains(pubkey)
    }

    /// Number of tracked vaults.
    pub fn len(&self) -> usize {
        self.vaults.read().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Get all tracked vault pubkeys (for RPC prefetch).
    pub fn all_vaults(&self) -> Vec<Pubkey> {
        self.vaults.read().unwrap().iter().copied().collect()
    }
}

impl Default for VaultTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse SPL Token account data to extract the balance.
/// SPL Token Account layout (165 bytes):
///   [0..32]   mint
///   [32..64]  owner
///   [64..72]  amount (u64 LE)
///   ...
pub fn parse_token_account_amount(data: &[u8]) -> Option<u64> {
    if data.len() < 72 {
        return None;
    }
    let amount = u64::from_le_bytes(data[64..72].try_into().ok()?);
    Some(amount)
}

/// Parse SPL Token account to extract mint.
pub fn parse_token_account_mint(data: &[u8]) -> Option<Pubkey> {
    if data.len() < 32 {
        return None;
    }
    Some(Pubkey::new_from_array(data[0..32].try_into().ok()?))
}
