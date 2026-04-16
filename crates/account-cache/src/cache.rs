use dashmap::DashMap;
use solana_sdk::pubkey::Pubkey;
use std::sync::Arc;

use mev_common::types::AccountUpdate;

/// Cached account entry with slot-based versioning.
#[derive(Debug, Clone)]
pub struct CachedAccount {
    pub data: Vec<u8>,
    pub lamports: u64,
    pub owner: Pubkey,
    pub slot: u64,
}

/// Thread-safe concurrent account cache.
///
/// Uses DashMap for per-shard locking: readers of different accounts never contend.
/// Updates are slot-gated — stale updates (lower slot) are rejected.
#[derive(Clone)]
pub struct AccountCache {
    inner: Arc<DashMap<Pubkey, CachedAccount>>,
}

impl AccountCache {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(DashMap::new()),
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            inner: Arc::new(DashMap::with_capacity(capacity)),
        }
    }

    /// Insert or update an account. Only applies if the new slot >= cached slot.
    pub fn update(&self, update: &AccountUpdate) {
        self.inner
            .entry(update.pubkey)
            .and_modify(|existing| {
                if update.slot >= existing.slot {
                    existing.data = update.data.clone();
                    existing.lamports = update.lamports;
                    existing.owner = update.owner;
                    existing.slot = update.slot;
                }
            })
            .or_insert_with(|| CachedAccount {
                data: update.data.clone(),
                lamports: update.lamports,
                owner: update.owner,
                slot: update.slot,
            });
    }

    /// Get a snapshot of the cached account data.
    pub fn get(&self, pubkey: &Pubkey) -> Option<CachedAccount> {
        self.inner.get(pubkey).map(|entry| entry.value().clone())
    }

    /// Get raw account data bytes. Returns None if not cached.
    pub fn get_data(&self, pubkey: &Pubkey) -> Option<Vec<u8>> {
        self.inner.get(pubkey).map(|entry| entry.data.clone())
    }

    /// Check if an account is cached.
    pub fn contains(&self, pubkey: &Pubkey) -> bool {
        self.inner.contains_key(pubkey)
    }

    /// Number of cached accounts.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Remove an account from the cache.
    pub fn remove(&self, pubkey: &Pubkey) {
        self.inner.remove(pubkey);
    }

    /// Iterate over all cached accounts. Callback receives (pubkey, cached_account).
    pub fn for_each(&self, f: impl Fn(&Pubkey, &CachedAccount)) {
        for entry in self.inner.iter() {
            f(entry.key(), entry.value());
        }
    }
}

impl Default for AccountCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slot_based_invalidation() {
        let cache = AccountCache::new();
        let pubkey = Pubkey::new_unique();
        let owner = Pubkey::new_unique();

        // Insert at slot 100
        cache.update(&AccountUpdate {
            pubkey,
            slot: 100,
            data: vec![1, 2, 3],
            lamports: 1000,
            owner,
        });
        assert_eq!(cache.get(&pubkey).unwrap().data, vec![1, 2, 3]);

        // Update at slot 101 — should apply
        cache.update(&AccountUpdate {
            pubkey,
            slot: 101,
            data: vec![4, 5, 6],
            lamports: 2000,
            owner,
        });
        assert_eq!(cache.get(&pubkey).unwrap().data, vec![4, 5, 6]);

        // Stale update at slot 99 — should be rejected
        cache.update(&AccountUpdate {
            pubkey,
            slot: 99,
            data: vec![7, 8, 9],
            lamports: 500,
            owner,
        });
        assert_eq!(cache.get(&pubkey).unwrap().data, vec![4, 5, 6]);
        assert_eq!(cache.get(&pubkey).unwrap().slot, 101);
    }
}
