use anyhow::Result;
use solana_client::rpc_client::RpcClient;
use solana_sdk::transaction::Transaction;
use tracing::{debug, info};

/// Fallback: send transaction via Helius staked RPC (no Jito bundle).
/// Used for liquidations and other non-bundle opportunities.
pub struct HeliusSender {
    rpc_client: RpcClient,
}

impl HeliusSender {
    pub fn new(rpc_url: &str) -> Self {
        Self {
            rpc_client: RpcClient::new(rpc_url.to_string()),
        }
    }

    pub fn send_transaction(&self, tx: &Transaction) -> Result<String> {
        let sig = self.rpc_client.send_and_confirm_transaction(tx)?;
        let sig_str = sig.to_string();
        info!(signature = %sig_str, "Transaction sent via Helius staked RPC");
        Ok(sig_str)
    }

    pub fn get_latest_blockhash(&self) -> Result<solana_sdk::hash::Hash> {
        Ok(self.rpc_client.get_latest_blockhash()?)
    }
}
