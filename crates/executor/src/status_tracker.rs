use std::collections::VecDeque;
use std::time::{Duration, Instant};

use tracing::{debug, info, warn};

use crate::jito_client::JitoBundleClient;

/// Tracks submitted bundles and polls for their status.
pub struct BundleStatusTracker {
    /// Pending bundles: (bundle_id, strategy, expected_profit, tip, submitted_at)
    pending: VecDeque<PendingBundle>,
    /// Max bundles to track simultaneously
    max_pending: usize,
    /// How long to keep polling before giving up
    timeout: Duration,
}

struct PendingBundle {
    bundle_id: String,
    strategy: String,
    expected_profit_lamports: i64,
    tip_lamports: u64,
    submitted_at: Instant,
}

/// Result of checking bundle status.
#[derive(Debug)]
pub enum BundleOutcome {
    /// Bundle landed on-chain
    Landed {
        bundle_id: String,
        strategy: String,
        profit_lamports: i64,
        tip_lamports: u64,
    },
    /// Bundle failed (dropped, expired, or reverted)
    Failed {
        bundle_id: String,
        strategy: String,
        reason: String,
    },
    /// Still pending — check again later
    Pending,
}

impl BundleStatusTracker {
    pub fn new(max_pending: usize, timeout_secs: u64) -> Self {
        Self {
            pending: VecDeque::with_capacity(max_pending),
            max_pending,
            timeout: Duration::from_secs(timeout_secs),
        }
    }

    /// Record a newly submitted bundle.
    pub fn track(
        &mut self,
        bundle_id: String,
        strategy: String,
        expected_profit_lamports: i64,
        tip_lamports: u64,
    ) {
        // Evict oldest if at capacity
        if self.pending.len() >= self.max_pending {
            self.pending.pop_front();
        }

        self.pending.push_back(PendingBundle {
            bundle_id,
            strategy,
            expected_profit_lamports,
            tip_lamports,
            submitted_at: Instant::now(),
        });
    }

    /// Poll Jito for statuses of all pending bundles.
    /// Returns outcomes for bundles that have resolved (landed or failed).
    pub async fn poll(&mut self, client: &JitoBundleClient) -> Vec<BundleOutcome> {
        if self.pending.is_empty() {
            return Vec::new();
        }

        let now = Instant::now();
        let mut outcomes = Vec::new();
        let mut still_pending = VecDeque::new();

        // Collect bundle IDs to check
        let ids: Vec<String> = self.pending.iter().map(|b| b.bundle_id.clone()).collect();

        // Batch status check
        let statuses = match client.get_bundle_statuses(ids).await {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, "Failed to poll bundle statuses");
                return Vec::new();
            }
        };

        for bundle in self.pending.drain(..) {
            // Check if timed out
            if now.duration_since(bundle.submitted_at) > self.timeout {
                outcomes.push(BundleOutcome::Failed {
                    bundle_id: bundle.bundle_id,
                    strategy: bundle.strategy,
                    reason: "timeout".to_string(),
                });
                continue;
            }

            // Check status from Jito response
            let status = statuses["result"]["value"]
                .as_array()
                .and_then(|arr| {
                    arr.iter().find(|s| {
                        s["bundle_id"].as_str() == Some(&bundle.bundle_id)
                    })
                });

            match status {
                Some(s) => {
                    let confirmation = s["confirmation_status"].as_str().unwrap_or("");
                    match confirmation {
                        "confirmed" | "finalized" => {
                            info!(
                                bundle_id = %bundle.bundle_id,
                                strategy = %bundle.strategy,
                                profit = bundle.expected_profit_lamports,
                                "Bundle LANDED"
                            );
                            outcomes.push(BundleOutcome::Landed {
                                bundle_id: bundle.bundle_id,
                                strategy: bundle.strategy,
                                profit_lamports: bundle.expected_profit_lamports,
                                tip_lamports: bundle.tip_lamports,
                            });
                        }
                        "failed" | "rejected" => {
                            let err = s["err"].as_str().unwrap_or("unknown").to_string();
                            warn!(
                                bundle_id = %bundle.bundle_id,
                                error = %err,
                                "Bundle FAILED"
                            );
                            outcomes.push(BundleOutcome::Failed {
                                bundle_id: bundle.bundle_id,
                                strategy: bundle.strategy,
                                reason: err,
                            });
                        }
                        _ => {
                            // Still processing
                            still_pending.push_back(bundle);
                        }
                    }
                }
                None => {
                    // Not found in response — still pending or dropped
                    still_pending.push_back(bundle);
                }
            }
        }

        self.pending = still_pending;
        outcomes
    }

    /// Number of bundles currently being tracked.
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
}
