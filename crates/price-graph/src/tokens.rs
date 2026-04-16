use solana_sdk::pubkey::Pubkey;
use std::collections::HashMap;

/// Maps token mints to compact numeric indices for the graph.
/// This avoids hashing Pubkeys on every graph operation.
#[derive(Debug, Clone)]
pub struct TokenRegistry {
    mint_to_index: HashMap<Pubkey, usize>,
    index_to_mint: Vec<Pubkey>,
}

impl TokenRegistry {
    pub fn new() -> Self {
        Self {
            mint_to_index: HashMap::new(),
            index_to_mint: Vec::new(),
        }
    }

    /// Get or assign an index for a token mint.
    pub fn get_or_insert(&mut self, mint: &Pubkey) -> usize {
        if let Some(&idx) = self.mint_to_index.get(mint) {
            return idx;
        }
        let idx = self.index_to_mint.len();
        self.index_to_mint.push(*mint);
        self.mint_to_index.insert(*mint, idx);
        idx
    }

    /// Look up index for a mint. Returns None if not registered.
    pub fn get_index(&self, mint: &Pubkey) -> Option<usize> {
        self.mint_to_index.get(mint).copied()
    }

    /// Look up mint for an index.
    pub fn get_mint(&self, index: usize) -> Option<&Pubkey> {
        self.index_to_mint.get(index)
    }

    /// Number of registered tokens.
    pub fn len(&self) -> usize {
        self.index_to_mint.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index_to_mint.is_empty()
    }
}

impl Default for TokenRegistry {
    fn default() -> Self {
        Self::new()
    }
}
