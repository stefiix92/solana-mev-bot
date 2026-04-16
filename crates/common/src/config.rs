use serde::Deserialize;
use std::path::Path;

use crate::errors::{MevError, MevResult};

#[derive(Debug, Deserialize, Clone)]
pub struct BotConfig {
    pub helius: HeliusConfig,
    pub jito: JitoConfig,
    pub wallet: WalletConfig,
    pub strategies: StrategiesConfig,
    pub risk: RiskConfig,
    pub metrics: MetricsConfig,
    pub runtime: RuntimeConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct HeliusConfig {
    pub rpc_endpoint: String,
    pub laserstream_endpoint: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct JitoConfig {
    pub block_engine_url: String,
    pub use_helius_proxy: bool,
}

#[derive(Debug, Deserialize, Clone)]
pub struct WalletConfig {
    pub keypair_path: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct StrategiesConfig {
    pub enabled: Vec<String>,
    pub dex_arb: Option<DexArbConfig>,
    pub backrun: Option<BackrunConfig>,
    pub liquidation: Option<LiquidationConfig>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct DexArbConfig {
    pub min_profit_lamports: u64,
    pub max_hops: u8,
    pub anchor_mints: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct BackrunConfig {
    pub min_trade_size_lamports: u64,
    pub max_slippage_bps: u16,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LiquidationConfig {
    pub min_bonus_bps: u16,
    pub protocols: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RiskConfig {
    pub max_position_lamports: u64,
    pub max_tip_lamports: u64,
    pub tip_fraction: f64,
    pub daily_loss_limit_lamports: u64,
    pub circuit_breaker_window_secs: u64,
    pub circuit_breaker_max_loss_lamports: u64,
    pub blacklist: BlacklistConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct BlacklistConfig {
    pub token_mints: Vec<String>,
    pub pool_addresses: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct MetricsConfig {
    pub prometheus_port: u16,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RuntimeConfig {
    pub worker_threads: usize,
    pub dry_run: bool,
    pub log_level: String,
}

impl BotConfig {
    /// Load config from a TOML file, with env var substitution for secrets.
    pub fn load(config_path: &Path) -> MevResult<Self> {
        let content = std::fs::read_to_string(config_path)
            .map_err(|e| MevError::Config(format!("Failed to read {}: {}", config_path.display(), e)))?;

        // Substitute env vars: any ${VAR_NAME} in the TOML
        let content = substitute_env_vars(&content);

        let config: BotConfig = toml::from_str(&content)
            .map_err(|e| MevError::Config(format!("Failed to parse config: {}", e)))?;

        Ok(config)
    }

    /// Load with an overlay (e.g., devnet.toml on top of default.toml).
    pub fn load_with_overlay(base_path: &Path, overlay_path: Option<&Path>) -> MevResult<Self> {
        let mut base = Self::load(base_path)?;

        if let Some(overlay) = overlay_path {
            if overlay.exists() {
                let overlay_config = Self::load(overlay)?;
                base.merge(overlay_config);
            }
        }

        Ok(base)
    }

    fn merge(&mut self, other: BotConfig) {
        // Overlay replaces top-level sections if present
        self.helius = other.helius;
        self.risk = other.risk;
        self.runtime = other.runtime;
    }
}

fn substitute_env_vars(content: &str) -> String {
    let mut result = content.to_string();
    // Find all ${VAR_NAME} patterns and replace with env values
    while let Some(start) = result.find("${") {
        if let Some(end) = result[start..].find('}') {
            let var_name = &result[start + 2..start + end];
            let value = std::env::var(var_name).unwrap_or_default();
            result = format!("{}{}{}", &result[..start], value, &result[start + end + 1..]);
        } else {
            break;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_default_config() {
        let config_path = Path::new("../../config/default.toml");
        if config_path.exists() {
            let config = BotConfig::load(config_path);
            assert!(config.is_ok(), "Failed to load default config: {:?}", config.err());
            let config = config.unwrap();
            assert_eq!(config.metrics.prometheus_port, 9090);
            assert!(!config.strategies.enabled.is_empty());
        }
    }
}
