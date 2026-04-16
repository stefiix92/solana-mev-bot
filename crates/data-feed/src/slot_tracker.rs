use std::sync::atomic::{AtomicU64, Ordering};

/// Tracks the latest observed slot across the data feed.
#[derive(Debug)]
pub struct SlotTracker {
    latest_slot: AtomicU64,
}

impl SlotTracker {
    pub fn new() -> Self {
        Self {
            latest_slot: AtomicU64::new(0),
        }
    }

    /// Update the latest slot if the new value is greater.
    pub fn update(&self, slot: u64) {
        self.latest_slot.fetch_max(slot, Ordering::Relaxed);
    }

    /// Get the latest observed slot.
    pub fn latest(&self) -> u64 {
        self.latest_slot.load(Ordering::Relaxed)
    }
}

impl Default for SlotTracker {
    fn default() -> Self {
        Self::new()
    }
}
