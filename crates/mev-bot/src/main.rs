use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use anyhow::Result;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use tokio::sync::{broadcast, mpsc, watch};
use tracing::{error, info, warn};

use mev_account_cache::cache::AccountCache;
use mev_account_cache::invalidation::run_cache_updater;
use mev_common::config::BotConfig;
use mev_common::types::{AccountUpdate, Opportunity};
use mev_data_feed::filters::{build_dex_subscription, build_lending_subscription};
use mev_data_feed::laserstream::{DataFeed, DataFeedConfig};
use mev_executor::bundle::JitoBundle;
use mev_executor::helius_sender::HeliusSender;
use mev_executor::jito_client::{Executor, JitoBundleClient};
use mev_executor::status_tracker::{BundleOutcome, BundleStatusTracker};
use mev_executor::tx_builder;
use mev_metrics::prometheus_exporter::{serve_metrics, BotMetrics};
use mev_metrics::pnl::PnlTracker;
use mev_risk::blacklist::Blacklist;
use mev_risk::circuit_breaker::CircuitBreaker;
use mev_risk::limits::RiskLimits;
use mev_strategies::dex_arb::DexArbStrategy;
use mev_strategies::liquidation::LiquidationStrategy;

#[tokio::main]
async fn main() -> Result<()> {
    let config_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("config/default.toml"));
    let overlay_path = std::env::args().nth(2).map(PathBuf::from);

    let config = BotConfig::load_with_overlay(&config_path, overlay_path.as_deref())?;
    init_tracing(&config.runtime.log_level);

    info!("Starting MEV Bot v0.1.0");
    info!(dry_run = config.runtime.dry_run, strategies = ?config.strategies.enabled, "Config loaded");

    let api_key = std::env::var("HELIUS_API_KEY").unwrap_or_else(|_| {
        error!("HELIUS_API_KEY not set");
        std::process::exit(1);
    });

    let keypair = load_keypair(&config.wallet.keypair_path)?;
    info!(wallet = %keypair.pubkey(), "Wallet loaded");

    // Shutdown coordination
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // Broadcast channel for account updates
    let (update_tx, _) = broadcast::channel::<AccountUpdate>(16_384);

    // Subscribe all receivers BEFORE moving update_tx
    let cache_rx = update_tx.subscribe();
    let strategy_rx = update_tx.subscribe();
    let liquidation_rx = update_tx.subscribe();

    // Opportunity channel: strategy → executor
    let (opp_tx, mut opp_rx) = mpsc::channel::<Opportunity>(256);

    // Core components
    let cache = AccountCache::with_capacity(100_000);
    let metrics = BotMetrics::new();
    let pnl = Arc::new(PnlTracker::new());

    let risk_limits = RiskLimits {
        max_position_lamports: config.risk.max_position_lamports,
        min_profit_lamports: config.strategies.dex_arb.as_ref()
            .map(|c| c.min_profit_lamports as i64)
            .unwrap_or(50_000),
        max_tip_lamports: config.risk.max_tip_lamports,
        tip_fraction: config.risk.tip_fraction,
    };

    let anchor_mints: Vec<Pubkey> = config.strategies.dex_arb.as_ref()
        .map(|c| c.anchor_mints.iter().filter_map(|s| Pubkey::from_str(s).ok()).collect())
        .unwrap_or_else(|| vec![mev_common::constants::SOL_MINT, mev_common::constants::USDC_MINT]);

    let max_hops = config.strategies.dex_arb.as_ref()
        .map(|c| c.max_hops as usize)
        .unwrap_or(3);

    // Blacklist
    let blacklist = Arc::new(Blacklist::from_config(
        &config.risk.blacklist.token_mints,
        &config.risk.blacklist.pool_addresses,
    ));

    // Circuit breaker
    let circuit_breaker = Arc::new(CircuitBreaker::new(
        config.risk.circuit_breaker_window_secs,
        config.risk.circuit_breaker_max_loss_lamports as i64,
    ));

    let rpc_url = format!("{}/?api-key={}", config.helius.rpc_endpoint, api_key);

    let data_feed = DataFeed::new(DataFeedConfig {
        endpoint: config.helius.laserstream_endpoint.clone(),
        api_key: api_key.clone(),
    });
    let slot_tracker = data_feed.slot_tracker();
    let mut subscribe_request = build_dex_subscription();

    // Add lending protocol subscriptions if liquidation strategy is enabled
    if config.strategies.enabled.contains(&"liquidation".to_string()) {
        let lending_filters = build_lending_subscription();
        subscribe_request.accounts.extend(lending_filters);
        info!("Lending protocol subscriptions added (Kamino, Marginfi, Save)");
    }

    let jito_url = if config.jito.use_helius_proxy {
        format!("{}/?api-key={}", config.helius.rpc_endpoint, api_key)
    } else {
        config.jito.block_engine_url.clone()
    };
    let executor = Executor::new(&jito_url, config.runtime.dry_run);

    // ========== SPAWN TASKS ==========

    // 1. Cache updater
    {
        let cache = cache.clone();
        let shutdown = shutdown_rx.clone();
        tokio::spawn(async move {
            run_cache_updater(cache, cache_rx, shutdown).await;
        });
    }

    // 2. Data feed (takes ownership of update_tx)
    {
        let shutdown = shutdown_rx.clone();
        tokio::spawn(async move {
            data_feed.run(subscribe_request, update_tx, shutdown).await;
        });
    }

    // 3. Metrics server
    {
        let metrics = metrics.clone();
        let port = config.metrics.prometheus_port;
        tokio::spawn(async move {
            serve_metrics(metrics, port).await;
        });
    }

    // 4. Strategy task — processes account updates, finds arbs, sends to executor
    {
        let cache = cache.clone();
        let metrics = metrics.clone();
        let pnl = pnl.clone();
        let blacklist = blacklist.clone();
        let circuit_breaker = circuit_breaker.clone();
        let mut shutdown = shutdown_rx.clone();
        let risk = risk_limits.clone();
        let mut strategy = DexArbStrategy::new(cache, risk.min_profit_lamports, max_hops, anchor_mints);
        let mut rx = strategy_rx;
        let opp_tx = opp_tx.clone();

        tokio::spawn(async move {
            info!("DEX arb strategy started (5 DEXs: Raydium AMM/CLMM, Orca, Meteora, Phoenix)");
            loop {
                tokio::select! {
                    result = rx.recv() => {
                        match result {
                            Ok(update) => {
                                // Check circuit breaker before processing
                                if !circuit_breaker.is_allowed() {
                                    continue;
                                }

                                let opportunities = strategy.process_update(&update);
                                for opp in opportunities {
                                    pnl.record_opportunity();
                                    metrics.opportunities_found.with_label_values(&["dex_arb"]).inc();

                                    // Check blacklist
                                    if !blacklist.check_opportunity(&opp) {
                                        continue;
                                    }

                                    if risk.check(&opp).is_approved() {
                                        if opp_tx.send(opp).await.is_err() {
                                            warn!("Executor channel full, dropping opportunity");
                                        }
                                    }
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(n)) => {
                                warn!(skipped = n, "Strategy lagged");
                            }
                            Err(broadcast::error::RecvError::Closed) => break,
                        }
                    }
                    _ = shutdown.changed() => break,
                }
            }
            info!("DEX arb strategy stopped");
        });
    }

    // 5. Liquidation strategy task
    if config.strategies.enabled.contains(&"liquidation".to_string()) {
        let cache = cache.clone();
        let metrics = metrics.clone();
        let pnl = pnl.clone();
        let blacklist = blacklist.clone();
        let circuit_breaker = circuit_breaker.clone();
        let mut shutdown = shutdown_rx.clone();
        let risk = risk_limits.clone();
        let opp_tx = opp_tx.clone();
        let mut rx = liquidation_rx;

        let min_bonus = config.strategies.liquidation.as_ref()
            .map(|c| c.min_bonus_bps)
            .unwrap_or(200);

        let liquidation_strategy = LiquidationStrategy::new(
            cache,
            min_bonus,
            100_000, // Min $0.10 borrow to liquidate
        );

        tokio::spawn(async move {
            info!("Liquidation strategy started (Kamino, Marginfi, Save)");
            loop {
                tokio::select! {
                    result = rx.recv() => {
                        match result {
                            Ok(update) => {
                                if !circuit_breaker.is_allowed() {
                                    continue;
                                }
                                if let Some(opp) = liquidation_strategy.process_update(&update) {
                                    pnl.record_opportunity();
                                    metrics.opportunities_found.with_label_values(&["liquidation"]).inc();

                                    if blacklist.check_opportunity(&opp) && risk.check(&opp).is_approved() {
                                        if opp_tx.send(opp).await.is_err() {
                                            warn!("Executor channel full, dropping liquidation");
                                        }
                                    }
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(n)) => {
                                warn!(skipped = n, "Liquidation strategy lagged");
                            }
                            Err(broadcast::error::RecvError::Closed) => break,
                        }
                    }
                    _ = shutdown.changed() => break,
                }
            }
            info!("Liquidation strategy stopped");
        });
    }

    // 6. Executor task — receives opportunities, builds bundles, submits to Jito, tracks status
    {
        let pnl = pnl.clone();
        let circuit_breaker = circuit_breaker.clone();
        let metrics = metrics.clone();
        let risk = risk_limits.clone();
        let mut shutdown = shutdown_rx.clone();
        let helius = HeliusSender::new(&rpc_url);
        let jito_status_client = JitoBundleClient::new(&jito_url);
        let mut status_tracker = BundleStatusTracker::new(64, 30);

        tokio::spawn(async move {
            info!("Executor started");
            let mut poll_interval = tokio::time::interval(std::time::Duration::from_secs(5));

            loop {
                tokio::select! {
                    Some(opp) = opp_rx.recv() => {
                        let strategy_label = opp.strategy.clone();
                        info!(
                            strategy = %opp.strategy,
                            profit = opp.expected_profit_lamports,
                            hops = opp.path.len(),
                            "Executing opportunity"
                        );

                        let tip = risk.calculate_tip(opp.expected_profit_lamports);

                        let blockhash = match helius.get_latest_blockhash() {
                            Ok(h) => h,
                            Err(e) => {
                                warn!(error = %e, "Failed to get blockhash");
                                continue;
                            }
                        };

                        let tx = match tx_builder::build_arb_transaction(
                            &opp, &keypair, tip, 0, blockhash,
                        ) {
                            Ok(tx) => tx,
                            Err(e) => {
                                warn!(error = %e, "Failed to build transaction");
                                continue;
                            }
                        };

                        let mut bundle = JitoBundle::new(opp.strategy.clone());
                        bundle.add_transaction(tx);
                        bundle.expected_profit_lamports = opp.expected_profit_lamports;
                        bundle.tip_lamports = tip;

                        pnl.record_bundle_submitted();
                        metrics.bundles_submitted.with_label_values(&[&strategy_label]).inc();

                        match executor.execute(bundle).await {
                            Ok(Some(bundle_id)) => {
                                info!(bundle_id = %bundle_id, "Bundle submitted");
                                status_tracker.track(
                                    bundle_id,
                                    strategy_label,
                                    opp.expected_profit_lamports,
                                    tip,
                                );
                            }
                            Ok(None) => {} // Dry run
                            Err(e) => {
                                warn!(error = %e, "Bundle submission failed");
                                metrics.bundles_failed.with_label_values(&[&strategy_label]).inc();
                            }
                        }
                    }

                    // Poll bundle statuses every 5 seconds
                    _ = poll_interval.tick() => {
                        if status_tracker.pending_count() > 0 {
                            let outcomes = status_tracker.poll(&jito_status_client).await;
                            for outcome in outcomes {
                                match outcome {
                                    BundleOutcome::Landed { strategy, profit_lamports, tip_lamports, .. } => {
                                        pnl.record_bundle_landed(profit_lamports, tip_lamports);
                                        metrics.bundles_landed.with_label_values(&[&strategy]).inc();
                                        circuit_breaker.record_trade(profit_lamports);
                                    }
                                    BundleOutcome::Failed { strategy, reason, .. } => {
                                        metrics.bundles_failed.with_label_values(&[&strategy]).inc();
                                        // Record as zero P&L (revert protection = no loss)
                                        circuit_breaker.record_trade(0);
                                    }
                                    BundleOutcome::Pending => {}
                                }
                            }
                        }
                    }

                    _ = shutdown.changed() => break,
                }
            }
            info!("Executor stopped");
        });
    }

    // 6. Periodic stats logging
    {
        let cache = cache.clone();
        let slot = slot_tracker.clone();
        let metrics = metrics.clone();
        let pnl = pnl.clone();
        let mut shutdown = shutdown_rx.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        let cached = cache.len();
                        let latest = slot.latest();

                        metrics.cache_size.set(cached as i64);
                        metrics.latest_slot.set(latest as i64);

                        info!(
                            cached_accounts = cached,
                            latest_slot = latest,
                            opportunities = pnl.total_opportunities(),
                            bundles = pnl.total_bundles_submitted(),
                            profit_sol = format!("{:.6}", pnl.net_profit_sol()),
                            "Status"
                        );
                    }
                    _ = shutdown.changed() => break,
                }
            }
        });
    }

    info!("Bot running. Ctrl+C to stop.");
    tokio::signal::ctrl_c().await?;

    info!("Shutdown signal received");
    let _ = shutdown_tx.send(true);

    // Give tasks time to finish
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    info!(
        profit_sol = format!("{:.6}", pnl.net_profit_sol()),
        opportunities = pnl.total_opportunities(),
        bundles_submitted = pnl.total_bundles_submitted(),
        "Final stats"
    );

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
        .init();
}

fn load_keypair(path: &str) -> Result<Keypair> {
    let raw = std::fs::read(path)
        .map_err(|e| anyhow::anyhow!("Failed to read keypair from {}: {}", path, e))?;

    // Try JSON array format (solana-keygen output)
    if let Ok(bytes) = serde_json::from_slice::<Vec<u8>>(&raw) {
        return Ok(Keypair::from_bytes(&bytes)?);
    }

    // Try raw bytes
    Ok(Keypair::from_bytes(&raw)?)
}
