use solana_sdk::pubkey::Pubkey;
use std::collections::HashSet;
use std::str::FromStr;
use std::sync::RwLock;
use tracing::{info, warn};

use mev_common::types::Opportunity;

/// Token and pool blacklist for filtering honeypots and known exploits.
/// Thread-safe and hot-reloadable.
pub struct Blacklist {
    token_mints: RwLock<HashSet<Pubkey>>,
    pool_addresses: RwLock<HashSet<Pubkey>>,
}

impl Blacklist {
    pub fn new() -> Self {
        Self {
            token_mints: RwLock::new(HashSet::new()),
            pool_addresses: RwLock::new(HashSet::new()),
        }
    }

    /// Initialize from config string lists.
    pub fn from_config(token_mints: &[String], pool_addresses: &[String]) -> Self {
        let bl = Self::new();

        for mint in token_mints {
            if let Ok(pubkey) = Pubkey::from_str(mint) {
                bl.add_token(&pubkey);
            }
        }

        for pool in pool_addresses {
            if let Ok(pubkey) = Pubkey::from_str(pool) {
                bl.add_pool(&pubkey);
            }
        }

        info!(
            tokens = bl.token_count(),
            pools = bl.pool_count(),
            "Blacklist initialized"
        );

        bl
    }

    pub fn add_token(&self, mint: &Pubkey) {
        self.token_mints.write().unwrap().insert(*mint);
    }

    pub fn add_pool(&self, address: &Pubkey) {
        self.pool_addresses.write().unwrap().insert(*address);
    }

    pub fn remove_token(&self, mint: &Pubkey) {
        self.token_mints.write().unwrap().remove(mint);
    }

    pub fn remove_pool(&self, address: &Pubkey) {
        self.pool_addresses.write().unwrap().remove(address);
    }

    pub fn is_token_blacklisted(&self, mint: &Pubkey) -> bool {
        self.token_mints.read().unwrap().contains(mint)
    }

    pub fn is_pool_blacklisted(&self, address: &Pubkey) -> bool {
        self.pool_addresses.read().unwrap().contains(address)
    }

    /// Check if an opportunity involves any blacklisted tokens or pools.
    pub fn check_opportunity(&self, opportunity: &Opportunity) -> bool {
        for step in &opportunity.path {
            if self.is_pool_blacklisted(&step.pool_address) {
                warn!(pool = %step.pool_address, "Opportunity blocked: blacklisted pool");
                return false;
            }
            if self.is_token_blacklisted(&step.input_mint) || self.is_token_blacklisted(&step.output_mint) {
                warn!(
                    input = %step.input_mint,
                    output = %step.output_mint,
                    "Opportunity blocked: blacklisted token"
                );
                return false;
            }
        }
        true
    }

    pub fn token_count(&self) -> usize {
        self.token_mints.read().unwrap().len()
    }

    pub fn pool_count(&self) -> usize {
        self.pool_addresses.read().unwrap().len()
    }

    /// Reload blacklist from config file.
    pub fn reload(&self, token_mints: &[String], pool_addresses: &[String]) {
        {
            let mut tokens = self.token_mints.write().unwrap();
            tokens.clear();
            for mint in token_mints {
                if let Ok(pubkey) = Pubkey::from_str(mint) {
                    tokens.insert(pubkey);
                }
            }
        }
        {
            let mut pools = self.pool_addresses.write().unwrap();
            pools.clear();
            for pool in pool_addresses {
                if let Ok(pubkey) = Pubkey::from_str(pool) {
                    pools.insert(pubkey);
                }
            }
        }
        info!(
            tokens = self.token_count(),
            pools = self.pool_count(),
            "Blacklist reloaded"
        );
    }
}

impl Default for Blacklist {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mev_common::types::{DexType, SwapStep};

    #[test]
    fn test_blacklist_token() {
        let bl = Blacklist::new();
        let mint = Pubkey::new_unique();
        assert!(!bl.is_token_blacklisted(&mint));
        bl.add_token(&mint);
        assert!(bl.is_token_blacklisted(&mint));
        bl.remove_token(&mint);
        assert!(!bl.is_token_blacklisted(&mint));
    }

    #[test]
    fn test_blacklist_blocks_opportunity() {
        let bl = Blacklist::new();
        let bad_mint = Pubkey::new_unique();
        bl.add_token(&bad_mint);

        let opp = Opportunity {
            strategy: "test".to_string(),
            path: vec![SwapStep {
                pool_address: Pubkey::new_unique(),
                dex_type: DexType::RaydiumAmm,
                input_mint: bad_mint,
                output_mint: Pubkey::new_unique(),
                amount_in: 1000,
                min_amount_out: 900,
                instructions: vec![],
            }],
            expected_profit_lamports: 50_000,
            estimated_compute_units: 200_000,
            detected_at_slot: 100,
        };

        assert!(!bl.check_opportunity(&opp));
    }
}
