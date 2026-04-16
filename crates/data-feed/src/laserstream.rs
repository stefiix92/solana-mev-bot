use std::sync::Arc;

use futures::StreamExt;
use helius_laserstream::grpc::{
    subscribe_update::UpdateOneof, SubscribeRequest,
};
use helius_laserstream::{subscribe, LaserstreamConfig};
use tokio::sync::broadcast;
use tracing::{error, info, warn};

use mev_common::types::AccountUpdate;

use crate::account_update::parse_account_update;
use crate::slot_tracker::SlotTracker;

/// Configuration for the LaserStream data feed.
pub struct DataFeedConfig {
    pub endpoint: String,
    pub api_key: String,
}

/// The main data feed that streams account updates from Helius LaserStream.
pub struct DataFeed {
    config: DataFeedConfig,
    slot_tracker: Arc<SlotTracker>,
}

impl DataFeed {
    pub fn new(config: DataFeedConfig) -> Self {
        Self {
            config,
            slot_tracker: Arc::new(SlotTracker::new()),
        }
    }

    pub fn slot_tracker(&self) -> Arc<SlotTracker> {
        Arc::clone(&self.slot_tracker)
    }

    /// Start streaming account updates. Sends parsed updates on the broadcast channel.
    /// This runs indefinitely until the cancellation token is triggered.
    pub async fn run(
        &self,
        request: SubscribeRequest,
        tx: broadcast::Sender<AccountUpdate>,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) {
        let ls_config = LaserstreamConfig::new(
            self.config.endpoint.clone(),
            self.config.api_key.clone(),
        )
        .with_replay(true);

        info!(
            endpoint = %self.config.endpoint,
            "Connecting to LaserStream"
        );

        let (stream, _handle) = subscribe(ls_config, request);
        let mut stream = Box::pin(stream);

        loop {
            tokio::select! {
                update = stream.next() => {
                    match update {
                        Some(Ok(msg)) => self.handle_update(msg, &tx),
                        Some(Err(e)) => {
                            warn!(error = %e, "LaserStream error (auto-reconnecting)");
                        }
                        None => {
                            error!("LaserStream ended unexpectedly");
                            break;
                        }
                    }
                }
                _ = shutdown.changed() => {
                    info!("Data feed shutting down");
                    break;
                }
            }
        }
    }

    fn handle_update(
        &self,
        msg: helius_laserstream::grpc::SubscribeUpdate,
        tx: &broadcast::Sender<AccountUpdate>,
    ) {
        let Some(update) = msg.update_oneof else {
            return;
        };

        match update {
            UpdateOneof::Account(account_update) => {
                let slot = account_update.slot;
                self.slot_tracker.update(slot);

                if let Some(account_info) = account_update.account {
                    if let Some(parsed) = parse_account_update(
                        &account_info.pubkey,
                        slot,
                        account_info.data,
                        account_info.lamports,
                        &account_info.owner,
                    ) {
                        // Best-effort broadcast — if all receivers are slow, drop the update
                        let _ = tx.send(parsed);
                    }
                }
            }
            UpdateOneof::Slot(slot_update) => {
                self.slot_tracker.update(slot_update.slot);
            }
            _ => {}
        }
    }
}
