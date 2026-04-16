use anyhow::Result;
use reqwest::Client;
use serde::Deserialize;
use tracing::{debug, warn};

/// Estimates priority fees via the Helius Priority Fee API.
pub struct GasEstimator {
    client: Client,
    helius_url: String,
}

#[derive(Debug, Deserialize)]
struct PriorityFeeResponse {
    #[serde(rename = "priorityFeeLevels")]
    priority_fee_levels: Option<PriorityFeeLevels>,
}

#[derive(Debug, Deserialize)]
struct PriorityFeeLevels {
    min: Option<f64>,
    low: Option<f64>,
    medium: Option<f64>,
    high: Option<f64>,
    #[serde(rename = "veryHigh")]
    very_high: Option<f64>,
    #[serde(rename = "unsafeMax")]
    unsafe_max: Option<f64>,
}

impl GasEstimator {
    pub fn new(helius_url: &str) -> Self {
        Self {
            client: Client::new(),
            helius_url: helius_url.to_string(),
        }
    }

    /// Get the recommended priority fee in microlamports per CU.
    /// Uses the "high" tier for competitive MEV execution.
    pub async fn get_priority_fee(&self, account_keys: &[String]) -> Result<u64> {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getPriorityFeeEstimate",
            "params": [{
                "accountKeys": account_keys,
                "options": {
                    "includeAllPriorityFeeLevels": true,
                    "recommended": true
                }
            }]
        });

        let response = self
            .client
            .post(&self.helius_url)
            .json(&body)
            .send()
            .await?;

        let result: serde_json::Value = response.json().await?;

        // Extract recommended fee or fall back to a default
        if let Some(fee) = result["result"]["priorityFeeEstimate"].as_f64() {
            let fee_u64 = fee as u64;
            debug!(priority_fee = fee_u64, "Priority fee estimated");
            return Ok(fee_u64);
        }

        // Try to get the "high" level from detailed response
        if let Some(high) = result["result"]["priorityFeeLevels"]["high"].as_f64() {
            return Ok(high as u64);
        }

        warn!("Could not estimate priority fee, using default");
        Ok(1_000) // Default: 1000 microlamports per CU
    }
}
