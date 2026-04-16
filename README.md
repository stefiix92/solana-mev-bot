# Solana MEV Bot

High-performance Solana MEV bot built in Rust. Targets DEX arbitrage, backrunning, and lending liquidations using Helius LaserStream for real-time data and Jito bundles for atomic execution.

## Strategies

| Strategy | Description | DEXs / Protocols |
|---|---|---|
| **DEX Arbitrage** | Detect cross-DEX price discrepancies via Bellman-Ford negative cycle detection | Raydium AMM V4, Orca Whirlpool, Meteora DLMM, Phoenix |
| **Backrunning** | Detect large swaps, exploit post-trade price imbalances | All supported DEXs |
| **Liquidation** | Monitor underwater lending positions, capture liquidation bonus | Kamino KLend, Marginfi, Save/Solend |

## Architecture

```
Helius LaserStream (gRPC)
        │
    data-feed ──broadcast──┬──────────┬────────────┐
        │                  │          │            │
    account-cache      dex_arb    backrun    liquidation
     (DashMap)         strategy   strategy    strategy
                           │
                      price-graph
                     (bellman-ford)
                           │
                   ┌───────┴────────┐
                   │  risk pipeline │
                   │  blacklist     │
                   │  circuit break │
                   └───────┬────────┘
                           │
                       executor
                    ┌──────┴──────┐
              Jito Bundles    Helius RPC
               (atomic)       (fallback)
```

## Quick Start

```bash
# Prerequisites
rustc --version  # >= 1.89

# Generate a bot wallet
solana-keygen new -o bot-keypair.json

# Fund it with SOL (mainnet or devnet)
solana transfer bot-keypair.json 1 --allow-unfunded-recipient

# Run (dry-run mode by default in devnet config)
HELIUS_API_KEY=your_key cargo run -p mev-bot -- config/default.toml

# Run with devnet overlay
HELIUS_API_KEY=your_key cargo run -p mev-bot -- config/default.toml config/devnet.toml

# Prometheus metrics
curl http://localhost:9090/metrics
```

## Configuration

All parameters are in `config/default.toml`. Key settings:

```toml
[strategies]
enabled = ["dex_arb", "backrun", "liquidation"]

[strategies.dex_arb]
min_profit_lamports = 50_000       # ~$0.007 min profit per arb
max_hops = 3                       # 2-3 hop cycles

[risk]
max_position_lamports = 10_000_000_000  # 10 SOL max per trade
tip_fraction = 0.50                     # 50% of profit as Jito tip
daily_loss_limit_lamports = 1_000_000_000
circuit_breaker_window_secs = 300       # 5 min rolling window
circuit_breaker_max_loss_lamports = 500_000_000

[runtime]
dry_run = false
```

Environment variables:
- `HELIUS_API_KEY` — Required. Your Helius API key.
- `RUST_LOG` — Optional. Override log level (e.g., `debug`, `info,mev_strategies=trace`).

## Project Structure

```
crates/
├── common/            # Types, constants (program IDs), config, errors
├── data-feed/         # Helius LaserStream gRPC, subscription filters
├── account-cache/     # DashMap concurrent cache, slot-based invalidation
├── dex-adapters/      # DEX pool decoding + AMM math + swap instructions
│   ├── raydium_amm    # Constant product (x*y=k)
│   ├── orca_whirlpool # Concentrated liquidity (simplified)
│   ├── meteora_dlmm   # Bin-based liquidity
│   └── phoenix        # Central limit order book
├── lending-adapters/  # Obligation decoding for liquidation
│   ├── kamino         # KLend obligations
│   ├── marginfi       # MarginfiAccount balances
│   └── save           # Solend obligations
├── price-graph/       # Token graph + Bellman-Ford arb detection
├── strategies/        # dex_arb, backrun, liquidation
├── executor/          # Jito bundle submission, tx building, tip calc
├── risk/              # Limits, blacklist, circuit breaker, gas estimator
├── metrics/           # Prometheus exporter, P&L tracking
└── mev-bot/           # Binary entrypoint, orchestrator
```

## How It Works

### DEX Arbitrage
1. LaserStream delivers real-time pool state updates
2. Each update triggers pool decoding via the appropriate DEX adapter
3. The price graph updates the edge weight: `w = -ln(rate × (1 - fee))`
4. Bellman-Ford detects negative cycles (profitable arb paths)
5. Profitable cycles are simulated with actual amounts through pool math
6. Approved opportunities are submitted as Jito bundles (atomic, revert-protected)

### Backrunning
1. Large swaps are detected in the transaction stream
2. Post-swap price imbalance is evaluated across the price graph
3. If profitable, a backrun transaction is bundled after the target swap

### Liquidation
1. Lending obligation accounts are monitored via LaserStream
2. Health factor is computed from collateral/debt positions
3. Underwater positions (health < 1.0) trigger liquidation instructions
4. Liquidation bonus (2.5-5% depending on protocol) is captured as profit

## Risk Management

- **Min profit threshold** — Skip opportunities below configured minimum
- **Max position size** — Cap SOL exposure per trade
- **Token/pool blacklist** — Block honeypots and known exploits (hot-reloadable)
- **Circuit breaker** — Auto-pause all strategies if rolling loss exceeds threshold
- **Jito revert protection** — Failed bundles cost nothing (no gas wasted)
- **Dry-run mode** — Full pipeline without actual execution

## Key Dependencies

| Crate | Purpose |
|---|---|
| `helius-laserstream` | Real-time gRPC data from Helius |
| `solana-sdk` | Transaction building, keypairs |
| `jito-sdk-rust` | Jito bundle submission |
| `dashmap` | Lock-free concurrent account cache |
| `arc-swap` | Atomic pointer swap for price graph |
| `prometheus` | Metrics export |
| `tokio` | Async runtime |

## Metrics

Prometheus metrics served at `http://localhost:9090/metrics`:

- `mev_opportunities_found{strategy}` — Detected opportunities
- `mev_bundles_submitted{strategy}` — Bundles sent to Jito
- `mev_bundles_landed{strategy}` — Successfully landed bundles
- `mev_profit_lamports_total` — Cumulative profit
- `mev_account_cache_size` — Cached accounts
- `mev_price_graph_edges` — Graph edges (pool directions)
- `mev_latest_slot` — Latest observed slot

## Testing

```bash
cargo test --workspace
```

32 tests covering: constant product math, pool decoding, arb detection, risk limits, blacklist, circuit breaker, liquidation detection, tip calculation.

## Profitability Notes

Based on 2025-2026 research:
- Solana MEV revenue: **$720M in 2025**
- Average arb profit: **$1.58/trade** — volume is everything
- Need **~60+ successful trades/day** to cover infrastructure
- Infrastructure is the edge, not strategy
- **Alpenglow** (Q3 2026) will compress MEV window from ~600ms to ~150ms

## License

Private — not for redistribution.
