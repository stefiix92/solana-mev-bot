use thiserror::Error;

#[derive(Error, Debug)]
pub enum MevError {
    #[error("Config error: {0}")]
    Config(String),

    #[error("Pool decode error: {0}")]
    PoolDecode(String),

    #[error("Quote error: {0}")]
    Quote(String),

    #[error("Insufficient liquidity: need {need}, have {have}")]
    InsufficientLiquidity { need: u64, have: u64 },

    #[error("Transaction build error: {0}")]
    TxBuild(String),

    #[error("Bundle submission error: {0}")]
    BundleSubmit(String),

    #[error("Bundle not landed: {bundle_id}")]
    BundleNotLanded { bundle_id: String },

    #[error("Risk limit exceeded: {0}")]
    RiskLimit(String),

    #[error("Circuit breaker triggered: {0}")]
    CircuitBreaker(String),

    #[error("Data feed error: {0}")]
    DataFeed(String),

    #[error("Account cache miss: {0}")]
    CacheMiss(String),

    #[error("RPC error: {0}")]
    Rpc(String),

    #[error("Blacklisted: {0}")]
    Blacklisted(String),

    #[error(transparent)]
    Solana(#[from] solana_sdk::signature::SignerError),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub type MevResult<T> = Result<T, MevError>;
