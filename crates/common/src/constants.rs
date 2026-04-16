use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

// -- DEX Program IDs --

pub const RAYDIUM_AMM_V4: Pubkey =
    solana_sdk::pubkey!("675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8");

pub const RAYDIUM_CLMM: Pubkey =
    solana_sdk::pubkey!("CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK");

pub const ORCA_WHIRLPOOL: Pubkey =
    solana_sdk::pubkey!("whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc");

pub const METEORA_DLMM: Pubkey =
    solana_sdk::pubkey!("LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo");

pub const PHOENIX: Pubkey =
    solana_sdk::pubkey!("PhoeNiXZ8ByJGLkxNfZRnkUfjvmuYqLR89jjFHGqdXY");

// -- Lending Program IDs --

pub const KAMINO_KLEND: Pubkey =
    solana_sdk::pubkey!("KLend2g3cP87fffoy8q1mQqGKjrxjC8boSyAYavgmjD");

pub const MARGINFI: Pubkey =
    solana_sdk::pubkey!("MFv2hWf31Z9kbCa1snEPYctwafyhdvnV7FZnsebVacA");

pub const SAVE_SOLEND: Pubkey =
    solana_sdk::pubkey!("So1endDq2YkqhipRh3WViPa8hFMJ7zuYHKBw5e5hfTo");

// -- Common Token Mints --

pub const SOL_MINT: Pubkey =
    solana_sdk::pubkey!("So11111111111111111111111111111111111111112");

pub const USDC_MINT: Pubkey =
    solana_sdk::pubkey!("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v");

pub const USDT_MINT: Pubkey =
    solana_sdk::pubkey!("Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB");

// -- Jito Tip Accounts --

pub fn jito_tip_accounts() -> [Pubkey; 8] {
    [
        Pubkey::from_str("96gYZGLnJYVFmbjzopPSU6QiEV5fGqZNyN9nmNhvrZU5").unwrap(),
        Pubkey::from_str("HFqU5x63VTqvQss8hp11i4bPYoTAYn472HQLBtDQSxKe").unwrap(),
        Pubkey::from_str("Cw8CFyM9FkoMi7K7Crf6HNQqf4uEMzpKw6QNghXLvLkY").unwrap(),
        Pubkey::from_str("ADaUMid9yfUytqMBgopwjb2DTLSLGPnqJW7T3qx4Pczd").unwrap(),
        Pubkey::from_str("DfXygSm4jCyNCybVYYK6DwvWqjKee8pbDmJGcLWNDXjh").unwrap(),
        Pubkey::from_str("ADuUkR4vqLUMWXxW9gh6D6L8pMSawimctcNZ5pGwDcEt").unwrap(),
        Pubkey::from_str("DttWaMuVvTiduZRnguLF7jNxTgiMBZ1hyAumKUiL6d3k").unwrap(),
        Pubkey::from_str("3AVi9Tg9Uo68tJfuvoKvqKNWKkC5wPdSSdeBnizKZ6jT").unwrap(),
    ]
}

// -- Compute Budget --

pub const DEFAULT_COMPUTE_UNITS: u32 = 200_000;
pub const MIN_TIP_LAMPORTS: u64 = 10_000;
