use prometheus::{IntCounter, IntCounterVec, IntGauge, Histogram, HistogramOpts, Opts, Registry};
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::{error, info};

/// Core metrics for the MEV bot.
#[derive(Clone)]
pub struct BotMetrics {
    pub opportunities_found: IntCounterVec,
    pub bundles_submitted: IntCounterVec,
    pub bundles_landed: IntCounterVec,
    pub bundles_failed: IntCounterVec,
    pub profit_lamports: IntCounter,
    pub tip_lamports: IntCounter,
    pub cache_size: IntGauge,
    pub graph_edges: IntGauge,
    pub graph_tokens: IntGauge,
    pub latest_slot: IntGauge,
    registry: Registry,
}

impl BotMetrics {
    pub fn new() -> Self {
        let registry = Registry::new();

        let opportunities_found = IntCounterVec::new(
            Opts::new("mev_opportunities_found", "Number of arb opportunities detected"),
            &["strategy"],
        ).unwrap();

        let bundles_submitted = IntCounterVec::new(
            Opts::new("mev_bundles_submitted", "Number of Jito bundles submitted"),
            &["strategy"],
        ).unwrap();

        let bundles_landed = IntCounterVec::new(
            Opts::new("mev_bundles_landed", "Number of Jito bundles that landed"),
            &["strategy"],
        ).unwrap();

        let bundles_failed = IntCounterVec::new(
            Opts::new("mev_bundles_failed", "Number of Jito bundles that failed"),
            &["strategy"],
        ).unwrap();

        let profit_lamports = IntCounter::new(
            "mev_profit_lamports_total", "Total profit in lamports",
        ).unwrap();

        let tip_lamports = IntCounter::new(
            "mev_tip_lamports_total", "Total tips paid in lamports",
        ).unwrap();

        let cache_size = IntGauge::new(
            "mev_account_cache_size", "Number of accounts in cache",
        ).unwrap();

        let graph_edges = IntGauge::new(
            "mev_price_graph_edges", "Number of edges in price graph",
        ).unwrap();

        let graph_tokens = IntGauge::new(
            "mev_price_graph_tokens", "Number of tokens in price graph",
        ).unwrap();

        let latest_slot = IntGauge::new(
            "mev_latest_slot", "Latest observed slot",
        ).unwrap();

        registry.register(Box::new(opportunities_found.clone())).unwrap();
        registry.register(Box::new(bundles_submitted.clone())).unwrap();
        registry.register(Box::new(bundles_landed.clone())).unwrap();
        registry.register(Box::new(bundles_failed.clone())).unwrap();
        registry.register(Box::new(profit_lamports.clone())).unwrap();
        registry.register(Box::new(tip_lamports.clone())).unwrap();
        registry.register(Box::new(cache_size.clone())).unwrap();
        registry.register(Box::new(graph_edges.clone())).unwrap();
        registry.register(Box::new(graph_tokens.clone())).unwrap();
        registry.register(Box::new(latest_slot.clone())).unwrap();

        Self {
            opportunities_found,
            bundles_submitted,
            bundles_landed,
            bundles_failed,
            profit_lamports,
            tip_lamports,
            cache_size,
            graph_edges,
            graph_tokens,
            latest_slot,
            registry,
        }
    }

    /// Encode all metrics to Prometheus text format.
    pub fn encode(&self) -> String {
        use prometheus::Encoder;
        let encoder = prometheus::TextEncoder::new();
        let metric_families = self.registry.gather();
        let mut buffer = Vec::new();
        encoder.encode(&metric_families, &mut buffer).unwrap();
        String::from_utf8(buffer).unwrap()
    }
}

impl Default for BotMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Serve Prometheus metrics on an HTTP endpoint.
pub async fn serve_metrics(metrics: BotMetrics, port: u16) {
    let listener = match TcpListener::bind(format!("0.0.0.0:{}", port)).await {
        Ok(l) => l,
        Err(e) => {
            error!(error = %e, port, "Failed to bind metrics server");
            return;
        }
    };

    info!(port, "Prometheus metrics server started");

    loop {
        if let Ok((mut stream, _)) = listener.accept().await {
            let metrics = metrics.clone();
            tokio::spawn(async move {
                // Read the request (we don't care about the content)
                let mut buf = [0u8; 1024];
                let _ = tokio::io::AsyncReadExt::read(&mut stream, &mut buf).await;

                let response = metrics.encode();
                let http_response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain; version=0.0.4\r\nContent-Length: {}\r\n\r\n{}",
                    response.len(),
                    response
                );
                let _ = tokio::io::AsyncWriteExt::write_all(
                    &mut stream,
                    http_response.as_bytes(),
                ).await;
            });
        }
    }
}
