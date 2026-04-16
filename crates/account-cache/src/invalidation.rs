use mev_common::types::AccountUpdate;
use tokio::sync::broadcast;
use tracing::{debug, info};

use crate::cache::AccountCache;

/// Task that listens for account updates and keeps the cache in sync.
pub async fn run_cache_updater(
    cache: AccountCache,
    mut rx: broadcast::Receiver<AccountUpdate>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let mut update_count: u64 = 0;

    info!("Cache updater started");

    loop {
        tokio::select! {
            result = rx.recv() => {
                match result {
                    Ok(update) => {
                        cache.update(&update);
                        update_count += 1;
                        if update_count % 10_000 == 0 {
                            debug!(
                                updates = update_count,
                                cache_size = cache.len(),
                                "Cache stats"
                            );
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        debug!(skipped = n, "Cache updater lagged, skipped updates");
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        info!("Broadcast channel closed, stopping cache updater");
                        break;
                    }
                }
            }
            _ = shutdown.changed() => {
                info!("Cache updater shutting down");
                break;
            }
        }
    }
}
