use anyhow::Result;
use solana_sdk::compute_budget::ComputeBudgetInstruction;
use solana_sdk::hash::Hash;
use solana_sdk::instruction::Instruction;
use solana_sdk::message::Message;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::system_instruction;
use solana_sdk::transaction::Transaction;

use mev_common::constants;
use mev_common::types::Opportunity;

/// Build a complete transaction from an opportunity.
///
/// Layout:
/// 1. SetComputeUnitLimit
/// 2. SetComputeUnitPrice (priority fee)
/// 3. Swap instruction(s) from the opportunity
/// 4. Jito tip transfer (to random tip account)
pub fn build_arb_transaction(
    opportunity: &Opportunity,
    payer: &Keypair,
    tip_lamports: u64,
    priority_fee_microlamports: u64,
    recent_blockhash: Hash,
) -> Result<Transaction> {
    let mut instructions: Vec<Instruction> = Vec::new();

    // 1. Compute budget
    instructions.push(ComputeBudgetInstruction::set_compute_unit_limit(
        opportunity.estimated_compute_units,
    ));

    if priority_fee_microlamports > 0 {
        instructions.push(ComputeBudgetInstruction::set_compute_unit_price(
            priority_fee_microlamports,
        ));
    }

    // 2. Swap instructions from each hop
    for step in &opportunity.path {
        instructions.extend_from_slice(&step.instructions);
    }

    // 3. Jito tip (transfer SOL to random tip account)
    let tip_account = random_tip_account();
    instructions.push(system_instruction::transfer(
        &payer.pubkey(),
        &tip_account,
        tip_lamports,
    ));

    // Build and sign transaction
    let message = Message::new(&instructions, Some(&payer.pubkey()));
    let mut tx = Transaction::new_unsigned(message);
    tx.sign(&[payer], recent_blockhash);

    Ok(tx)
}

/// Pick a random Jito tip account.
fn random_tip_account() -> Pubkey {
    use rand::Rng;
    let accounts = constants::jito_tip_accounts();
    let idx = rand::thread_rng().gen_range(0..accounts.len());
    accounts[idx]
}

/// Serialize a transaction to base58 for Jito bundle submission.
pub fn serialize_tx_base58(tx: &Transaction) -> String {
    bs58::encode(bincode::serialize(tx).unwrap()).into_string()
}

/// Serialize a transaction to base64.
pub fn serialize_tx_base64(tx: &Transaction) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bincode::serialize(tx).unwrap())
}
