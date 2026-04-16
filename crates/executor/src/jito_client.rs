use anyhow::Result;
use jito_sdk_rust::JitoJsonRpcSDK;
use serde_json::json;
use solana_sdk::transaction::Transaction;
use tracing::{debug, error, info, warn};

use crate::bundle::JitoBundle;
use crate::tx_builder::serialize_tx_base58;

/// Client for submitting bundles to Jito block engine.
pub struct JitoBundleClient {
    sdk: JitoJsonRpcSDK,
}

impl JitoBundleClient {
    pub fn new(block_engine_url: &str) -> Self {
        Self {
            sdk: JitoJsonRpcSDK::new(block_engine_url, None),
        }
    }

    /// Submit a bundle to the Jito block engine.
    /// Returns the bundle UUID on success.
    pub async fn submit_bundle(&self, bundle: &JitoBundle) -> Result<String> {
        if bundle.is_empty() {
            anyhow::bail!("Cannot submit empty bundle");
        }

        let serialized_txs: Vec<String> = bundle
            .transactions
            .iter()
            .map(serialize_tx_base58)
            .collect();

        debug!(
            strategy = %bundle.strategy,
            num_txs = bundle.num_transactions(),
            tip = bundle.tip_lamports,
            profit = bundle.expected_profit_lamports,
            "Submitting Jito bundle"
        );

        let params = json!([serialized_txs]);

        match self.sdk.send_bundle(Some(params), None).await {
            Ok(response) => {
                if let Some(result) = response.get("result") {
                    let bundle_id = result.as_str().unwrap_or("unknown").to_string();
                    info!(
                        bundle_id = %bundle_id,
                        strategy = %bundle.strategy,
                        "Bundle submitted"
                    );
                    Ok(bundle_id)
                } else if let Some(error) = response.get("error") {
                    let err_msg = error.to_string();
                    warn!(error = %err_msg, "Bundle submission error");
                    anyhow::bail!("Jito error: {}", err_msg)
                } else {
                    anyhow::bail!("Unexpected Jito response: {}", response)
                }
            }
            Err(e) => {
                error!(error = %e, "Bundle submission failed");
                Err(e)
            }
        }
    }

    /// Check the status of submitted bundles.
    pub async fn get_bundle_statuses(&self, bundle_ids: Vec<String>) -> Result<serde_json::Value> {
        self.sdk
            .get_bundle_statuses(bundle_ids)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get bundle statuses: {}", e))
    }
}

/// Executor that receives opportunities and submits them as Jito bundles.
pub struct Executor {
    jito_client: JitoBundleClient,
    dry_run: bool,
}

impl Executor {
    pub fn new(block_engine_url: &str, dry_run: bool) -> Self {
        Self {
            jito_client: JitoBundleClient::new(block_engine_url),
            dry_run,
        }
    }

    /// Execute a bundle. In dry-run mode, just logs the opportunity.
    pub async fn execute(&self, bundle: JitoBundle) -> Result<Option<String>> {
        if self.dry_run {
            info!(
                strategy = %bundle.strategy,
                profit = bundle.expected_profit_lamports,
                tip = bundle.tip_lamports,
                num_txs = bundle.num_transactions(),
                "[DRY RUN] Would submit bundle"
            );
            return Ok(None);
        }

        let bundle_id = self.jito_client.submit_bundle(&bundle).await?;
        Ok(Some(bundle_id))
    }
}
