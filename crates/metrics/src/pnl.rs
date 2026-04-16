use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

/// Tracks P&L for the bot session.
#[derive(Debug)]
pub struct PnlTracker {
    total_profit_lamports: AtomicI64,
    total_tip_lamports: AtomicU64,
    total_opportunities: AtomicU64,
    total_bundles_submitted: AtomicU64,
    total_bundles_landed: AtomicU64,
}

impl PnlTracker {
    pub fn new() -> Self {
        Self {
            total_profit_lamports: AtomicI64::new(0),
            total_tip_lamports: AtomicU64::new(0),
            total_opportunities: AtomicU64::new(0),
            total_bundles_submitted: AtomicU64::new(0),
            total_bundles_landed: AtomicU64::new(0),
        }
    }

    pub fn record_opportunity(&self) {
        self.total_opportunities.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_bundle_submitted(&self) {
        self.total_bundles_submitted.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_bundle_landed(&self, profit_lamports: i64, tip_lamports: u64) {
        self.total_bundles_landed.fetch_add(1, Ordering::Relaxed);
        self.total_profit_lamports.fetch_add(profit_lamports, Ordering::Relaxed);
        self.total_tip_lamports.fetch_add(tip_lamports, Ordering::Relaxed);
    }

    pub fn total_profit_lamports(&self) -> i64 {
        self.total_profit_lamports.load(Ordering::Relaxed)
    }

    pub fn total_opportunities(&self) -> u64 {
        self.total_opportunities.load(Ordering::Relaxed)
    }

    pub fn total_bundles_submitted(&self) -> u64 {
        self.total_bundles_submitted.load(Ordering::Relaxed)
    }

    pub fn total_bundles_landed(&self) -> u64 {
        self.total_bundles_landed.load(Ordering::Relaxed)
    }

    pub fn net_profit_sol(&self) -> f64 {
        self.total_profit_lamports.load(Ordering::Relaxed) as f64 / 1_000_000_000.0
    }
}

impl Default for PnlTracker {
    fn default() -> Self {
        Self::new()
    }
}
