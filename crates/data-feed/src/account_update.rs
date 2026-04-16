use mev_common::types::AccountUpdate;
use solana_sdk::pubkey::Pubkey;

/// Convert raw gRPC account update bytes into our AccountUpdate type.
pub fn parse_account_update(
    pubkey_bytes: &[u8],
    slot: u64,
    data: Vec<u8>,
    lamports: u64,
    owner_bytes: &[u8],
) -> Option<AccountUpdate> {
    if pubkey_bytes.len() != 32 || owner_bytes.len() != 32 {
        return None;
    }

    let pubkey = Pubkey::new_from_array(pubkey_bytes.try_into().ok()?);
    let owner = Pubkey::new_from_array(owner_bytes.try_into().ok()?);

    Some(AccountUpdate {
        pubkey,
        slot,
        data,
        lamports,
        owner,
    })
}
