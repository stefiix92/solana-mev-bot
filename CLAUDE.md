# CLAUDE.md

## Project Overview

Solana MEV bot — Rust workspace, 11 crates. Arbitrage, backrunning, and liquidation strategies. Helius LaserStream for real-time data, Jito bundles for atomic execution.

## Build & Test

```bash
cargo check --workspace     # Type check
cargo test --workspace      # 37 tests
cargo build -p mev-bot      # Build binary
cargo run -p mev-bot -- config/default.toml  # Run (needs HELIUS_API_KEY env)
```

Rust 1.89+ required. Edition 2021.

## Architecture

```
crates/
  common/           → types, constants (program IDs), config (TOML), errors
  data-feed/        → Helius LaserStream gRPC, subscription filters, vault tracker
  account-cache/    → DashMap concurrent cache, slot-based invalidation
  dex-adapters/     → 6 DEX adapters + AMM math + trade optimizer
  lending-adapters/ → 3 lending protocol obligation decoders
  price-graph/      → token graph + Bellman-Ford + DFS cycle detection
  strategies/       → dex_arb, backrun, liquidation
  executor/         → Jito bundle submission, tx builder, tip calc, status tracker
  risk/             → limits, blacklist, circuit breaker, gas estimator
  metrics/          → Prometheus exporter, P&L tracker
  mev-bot/          → binary entrypoint, orchestrator
```

## Key Patterns

- **Workspace deps**: All dependency versions in root `Cargo.toml` `[workspace.dependencies]`. Crates use `{ workspace = true }`.
- **Internal crates**: Referenced as `mev-common`, `mev-data-feed`, etc.
- **Account decoding**: Manual byte-offset reads (`read_u64`, `read_pubkey`) — NOT borsh/anchor derive. Each adapter defines its own offsets matching on-chain layout.
- **Price graph**: Custom adjacency list, NOT petgraph. Edge weight = `-ln(rate * (1 - fee))`. Negative cycle = profitable arb.
- **Concurrency**: `broadcast::channel` for fan-out, `mpsc::channel` for strategy→executor, `DashMap` for cache, `ArcSwap` for graph snapshots.
- **Trade sizing**: Ternary search optimizer in `dex-adapters/src/math/optimizer.rs` — finds input amount maximizing profit.
- **Graph publishing**: Batched every 100 updates to reduce clone allocations.

## DEX Adapters (implement `DexAdapter` trait)

| Adapter | Program ID | Math |
|---|---|---|
| `raydium_amm` | `675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8` | Constant product x*y=k |
| `raydium_clmm` | `CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK` | Concentrated liquidity (virtual reserves) |
| `orca_whirlpool` | `whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc` | Concentrated liquidity (simplified) |
| `meteora_dlmm` | `LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo` | Bin-based, price = (1+step/10000)^id |
| `phoenix` | `PhoeNiXZ8ByJGLkxNfZRnkUfjvmuYqLR89jjFHGqdXY` | CLOB (approximated) |

## Lending Adapters (implement `LendingAdapter` trait)

| Adapter | Program ID |
|---|---|
| `kamino` | `KLend2g3cP87fffoy8q1mQqGKjrxjC8boSyAYavgmjD` |
| `marginfi` | `MFv2hWf31Z9kbCa1snEPYctwafyhdvnV7FZnsebVacA` |
| `save` | `So1endDq2YkqhipRh3WViPa8hFMJ7zuYHKBw5e5hfTo` |

## Config

`config/default.toml` — all tunable params. Overlay with `config/devnet.toml` for devnet. Env vars substituted via `${VAR_NAME}` syntax.

Key env vars:
- `HELIUS_API_KEY` — required
- `RUST_LOG` — optional log level override

## Adding a New DEX Adapter

1. Create `crates/dex-adapters/src/new_dex.rs`
2. Implement `DexAdapter` trait: `decode_pool`, `quote`, `build_swap_ix`
3. Add `pub mod new_dex;` to `crates/dex-adapters/src/lib.rs`
4. Add program ID to `crates/common/src/constants.rs`
5. Add subscription filter in `crates/data-feed/src/filters.rs`
6. Register adapter in `crates/strategies/src/dex_arb.rs` `DexArbStrategy::new()`
7. Write tests with known account data snapshots

## Common Gotchas

- Account layouts are `#[repr(C, packed)]` or Anchor Borsh — offsets must match on-chain program exactly. Test with real mainnet data.
- Concentrated liquidity adapters (Orca, Raydium CLMM) use simplified virtual-reserve quoting. Full tick-array traversal not yet implemented — quotes are approximate for large swaps.
- Vault balances (SPL token accounts) are separate from pool accounts. The `VaultTracker` registers vaults as pools are decoded, but vault accounts need to be in the cache for reserves to be non-zero.
- Jito tip accounts — randomly selected per bundle from 8 hardcoded addresses in `constants.rs`.
- Circuit breaker requires manual `reset()` after tripping.
