use anyhow::Result;
use mev_common::types::{AccountUpdate, Opportunity};

/// Trait implemented by all MEV strategies.
pub trait Strategy: Send + Sync {
    /// Name of this strategy (for logging/metrics).
    fn name(&self) -> &str;

    /// Evaluate an account update. Returns an opportunity if one is detected.
    /// Called on every relevant account update — must be fast.
    fn evaluate(&self, update: &AccountUpdate) -> Result<Option<Opportunity>>;
}
