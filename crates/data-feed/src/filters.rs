use helius_laserstream::grpc::{SubscribeRequest, SubscribeRequestFilterAccounts};
use mev_common::constants;
use std::collections::HashMap;

/// Build the default subscription request for all monitored DEX programs.
pub fn build_dex_subscription() -> SubscribeRequest {
    let mut request = SubscribeRequest::default();

    // Raydium AMM V4
    request.accounts.insert(
        "raydium_amm".to_string(),
        SubscribeRequestFilterAccounts {
            owner: vec![constants::RAYDIUM_AMM_V4.to_string()],
            account: vec![],
            nonempty_txn_signature: None,
            ..Default::default()
        },
    );

    // Raydium CLMM
    request.accounts.insert(
        "raydium_clmm".to_string(),
        SubscribeRequestFilterAccounts {
            owner: vec![constants::RAYDIUM_CLMM.to_string()],
            account: vec![],
            nonempty_txn_signature: None,
            ..Default::default()
        },
    );

    // Orca Whirlpool
    request.accounts.insert(
        "orca_whirlpool".to_string(),
        SubscribeRequestFilterAccounts {
            owner: vec![constants::ORCA_WHIRLPOOL.to_string()],
            account: vec![],
            nonempty_txn_signature: None,
            ..Default::default()
        },
    );

    // Meteora DLMM
    request.accounts.insert(
        "meteora_dlmm".to_string(),
        SubscribeRequestFilterAccounts {
            owner: vec![constants::METEORA_DLMM.to_string()],
            account: vec![],
            nonempty_txn_signature: None,
            ..Default::default()
        },
    );

    // Phoenix
    request.accounts.insert(
        "phoenix".to_string(),
        SubscribeRequestFilterAccounts {
            owner: vec![constants::PHOENIX.to_string()],
            account: vec![],
            nonempty_txn_signature: None,
            ..Default::default()
        },
    );

    // Confirmed commitment
    request.commitment = Some(1);

    request
}

/// Build subscription for lending protocols (Phase 4).
pub fn build_lending_subscription() -> HashMap<String, SubscribeRequestFilterAccounts> {
    let mut filters = HashMap::new();

    filters.insert(
        "kamino".to_string(),
        SubscribeRequestFilterAccounts {
            owner: vec![constants::KAMINO_KLEND.to_string()],
            account: vec![],
            nonempty_txn_signature: None,
            ..Default::default()
        },
    );

    filters.insert(
        "marginfi".to_string(),
        SubscribeRequestFilterAccounts {
            owner: vec![constants::MARGINFI.to_string()],
            account: vec![],
            nonempty_txn_signature: None,
            ..Default::default()
        },
    );

    filters.insert(
        "save".to_string(),
        SubscribeRequestFilterAccounts {
            owner: vec![constants::SAVE_SOLEND.to_string()],
            account: vec![],
            nonempty_txn_signature: None,
            ..Default::default()
        },
    );

    filters
}
