use solana_client::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use tracing::{info, warn};

use mev_common::types::AccountUpdate;

use crate::cache::AccountCache;

/// Prefetch a list of known accounts via RPC and populate the cache.
/// Used at startup to seed the cache before the gRPC stream delivers updates.
pub fn prefetch_accounts(
    cache: &AccountCache,
    rpc_client: &RpcClient,
    pubkeys: &[Pubkey],
) -> usize {
    if pubkeys.is_empty() {
        return 0;
    }

    info!(count = pubkeys.len(), "Prefetching accounts via RPC");

    let mut loaded = 0;

    // getMultipleAccounts supports max 100 accounts per call
    for chunk in pubkeys.chunks(100) {
        match rpc_client.get_multiple_accounts(chunk) {
            Ok(accounts) => {
                for (i, maybe_account) in accounts.iter().enumerate() {
                    if let Some(account) = maybe_account {
                        cache.update(&AccountUpdate {
                            pubkey: chunk[i],
                            slot: 0, // Will be overwritten by first gRPC update
                            data: account.data.clone(),
                            lamports: account.lamports,
                            owner: account.owner,
                        });
                        loaded += 1;
                    }
                }
            }
            Err(e) => {
                warn!(error = %e, "Failed to prefetch account batch");
            }
        }
    }

    info!(loaded, total = pubkeys.len(), "Prefetch complete");
    loaded
}
