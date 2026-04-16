use std::path::PathBuf;

use anyhow::Result;
use tokio::sync::{broadcast, watch};
use tracing::{error, info};

use mev_account_cache::cache::AccountCache;
use mev_account_cache::invalidation::run_cache_updater;
use mev_common::config::BotConfig;
use mev_common::types::AccountUpdate;
use mev_data_feed::filters::build_dex_subscription;
use mev_data_feed::laserstream::{DataFeed, DataFeedConfig};

#[tokio::main]
async fn main() -> Result<()> {
    // Load config
    let config_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("config/default.toml"));

    let overlay_path = std::env::args().nth(2).map(PathBuf::from);

    let config = BotConfig::load_with_overlay(
        &config_path,
        overlay_path.as_deref(),
    )?;

    // Initialize logging
    init_tracing(&config.runtime.log_level);

    info!("Starting MEV Bot");
    info!(
        dry_run = config.runtime.dry_run,
        strategies = ?config.strategies.enabled,
        "Configuration loaded"
    );

    // Shutdown signal
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // Account update broadcast channel (capacity for bursts)
    let (update_tx, _) = broadcast::channel::<AccountUpdate>(16_384);

    // Initialize account cache
    let cache = AccountCache::with_capacity(100_000);

    // Initialize data feed
    let api_key = std::env::var("HELIUS_API_KEY")
        .unwrap_or_else(|_| {
            error!("HELIUS_API_KEY environment variable not set");
            std::process::exit(1);
        });

    let data_feed = DataFeed::new(DataFeedConfig {
        endpoint: config.helius.laserstream_endpoint.clone(),
        api_key,
    });

    let slot_tracker = data_feed.slot_tracker();

    // Build subscription request
    let subscribe_request = build_dex_subscription();

    // Spawn cache updater task
    let cache_for_updater = cache.clone();
    let shutdown_for_cache = shutdown_rx.clone();
    let update_rx_for_cache = update_tx.subscribe();
    tokio::spawn(async move {
        run_cache_updater(cache_for_updater, update_rx_for_cache, shutdown_for_cache).await;
    });

    // Spawn data feed task
    let shutdown_for_feed = shutdown_rx.clone();
    let feed_handle = tokio::spawn(async move {
        data_feed.run(subscribe_request, update_tx, shutdown_for_feed).await;
    });

    // Log periodic stats
    let cache_for_stats = cache.clone();
    let slot_tracker_for_stats = slot_tracker.clone();
    let mut shutdown_for_stats = shutdown_rx.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    info!(
                        cached_accounts = cache_for_stats.len(),
                        latest_slot = slot_tracker_for_stats.latest(),
                        "Status"
                    );
                }
                _ = shutdown_for_stats.changed() => break,
            }
        }
    });

    info!("Bot running. Press Ctrl+C to stop.");

    // Wait for shutdown signal
    tokio::signal::ctrl_c().await?;
    info!("Shutdown signal received");
    let _ = shutdown_tx.send(true);

    // Wait for data feed to finish
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        feed_handle,
    ).await;

    info!("Bot stopped");
    Ok(())
}

fn init_tracing(level: &str) {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(level));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .with_thread_ids(true)
        .with_file(false)
        .with_line_number(false)
        .init();
}
