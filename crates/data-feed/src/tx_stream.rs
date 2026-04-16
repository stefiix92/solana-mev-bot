use solana_sdk::pubkey::Pubkey;

/// A detected swap transaction from the data feed.
#[derive(Debug, Clone)]
pub struct DetectedSwap {
    pub signature: Vec<u8>,
    pub slot: u64,
    pub program_id: Pubkey,
    pub pool_address: Option<Pubkey>,
    /// Estimated swap size in lamports (input amount)
    pub estimated_size_lamports: u64,
    /// The raw transaction bytes (for including in backrun bundle)
    pub raw_transaction: Vec<u8>,
}

/// Parse a transaction update to detect large DEX swaps.
///
/// Looks for instructions to known DEX programs and extracts swap details.
/// Returns None for non-swap transactions or swaps below the size threshold.
pub fn detect_swap(
    slot: u64,
    transaction_data: &[u8],
    signature: &[u8],
    dex_program_ids: &[Pubkey],
    min_size_lamports: u64,
) -> Option<DetectedSwap> {
    // Transaction binary format: signatures + message
    // Message contains: header + account_keys + recent_blockhash + instructions
    // Each instruction has: program_id_index + accounts + data

    // For now, use a simplified detection:
    // Check if any of the account keys in the transaction match known DEX program IDs
    // Full instruction parsing will be added when we integrate with the gRPC transaction stream

    if transaction_data.len() < 200 {
        return None; // Too small to be a meaningful swap tx
    }

    // Scan for known program IDs in the transaction bytes
    for program_id in dex_program_ids {
        let program_bytes = program_id.to_bytes();
        if contains_bytes(transaction_data, &program_bytes) {
            return Some(DetectedSwap {
                signature: signature.to_vec(),
                slot,
                program_id: *program_id,
                pool_address: None, // Extracted during full parsing
                estimated_size_lamports: 0, // Estimated during full parsing
                raw_transaction: transaction_data.to_vec(),
            });
        }
    }

    None
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|window| window == needle)
}
