use solana_sdk::transaction::Transaction;

/// A Jito bundle: 1-5 transactions that execute atomically.
/// All succeed or none do. Revert protection = no cost on failure.
#[derive(Debug)]
pub struct JitoBundle {
    pub transactions: Vec<Transaction>,
    pub strategy: String,
    pub expected_profit_lamports: i64,
    pub tip_lamports: u64,
}

impl JitoBundle {
    pub fn new(strategy: String) -> Self {
        Self {
            transactions: Vec::with_capacity(5),
            strategy,
            expected_profit_lamports: 0,
            tip_lamports: 0,
        }
    }

    pub fn add_transaction(&mut self, tx: Transaction) {
        assert!(self.transactions.len() < 5, "Jito bundles support max 5 txs");
        self.transactions.push(tx);
    }

    pub fn num_transactions(&self) -> usize {
        self.transactions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.transactions.is_empty()
    }
}
