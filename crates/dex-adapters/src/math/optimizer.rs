use crate::math::constant_product;

/// Find the optimal input amount that maximizes profit for a multi-hop arb cycle.
///
/// Uses ternary search on the profit curve: profit = f(amount_in) is unimodal
/// (increases then decreases due to price impact). Ternary search finds the peak.
///
/// Returns (optimal_amount_in, max_profit) in lamports.
pub fn optimize_arb_amount(
    hops: &[HopParams],
    max_input: u64,
    min_input: u64,
) -> (u64, i64) {
    if hops.is_empty() || max_input <= min_input {
        return (0, 0);
    }

    // Ternary search: O(log(max_input/precision)) iterations
    let mut lo = min_input;
    let mut hi = max_input;

    // ~40 iterations for full u64 range (log3(2^64) ≈ 40)
    for _ in 0..50 {
        if hi - lo < 1000 {
            break; // Precision: 1000 lamports (~$0.0001)
        }

        let m1 = lo + (hi - lo) / 3;
        let m2 = hi - (hi - lo) / 3;

        let profit1 = simulate_cycle(hops, m1);
        let profit2 = simulate_cycle(hops, m2);

        if profit1 < profit2 {
            lo = m1;
        } else {
            hi = m2;
        }
    }

    let optimal = (lo + hi) / 2;
    let profit = simulate_cycle(hops, optimal);

    (optimal, profit)
}

/// Parameters for one hop in the arb cycle.
#[derive(Debug, Clone)]
pub struct HopParams {
    pub reserve_in: u64,
    pub reserve_out: u64,
    pub fee_numerator: u64,
    pub fee_denominator: u64,
}

/// Simulate a complete cycle: start with `amount_in`, pass through each hop,
/// return profit (output - input). Negative = loss.
pub fn simulate_cycle(hops: &[HopParams], amount_in: u64) -> i64 {
    if amount_in == 0 {
        return 0;
    }

    let mut current = amount_in;

    for hop in hops {
        match constant_product::swap_base_in(
            hop.reserve_in,
            hop.reserve_out,
            current,
            hop.fee_numerator,
            hop.fee_denominator,
        ) {
            Some((out, _fee)) => {
                if out == 0 {
                    return i64::MIN; // Dead path
                }
                current = out;
            }
            None => return i64::MIN, // Overflow or invalid
        }
    }

    current as i64 - amount_in as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_optimize_simple_arb() {
        // 2-hop cycle with price discrepancy:
        // Pool 1: SOL→USDC: 1000 SOL / 150000 USDC (rate: 150 USDC/SOL)
        // Pool 2: USDC→SOL: 100000 USDC / 1100 SOL (rate: 0.011 SOL/USDC = 91 USDC/SOL)
        // Arb: sell SOL for USDC on pool1 (expensive), buy SOL with USDC on pool2 (cheap)
        // 1 SOL → 150 USDC → 1.65 SOL ≈ 65% profit before fees
        let hops = vec![
            HopParams {
                reserve_in: 1_000_000_000_000,   // 1000 SOL (in)
                reserve_out: 150_000_000_000,    // 150000 USDC (out) — SOL is expensive here
                fee_numerator: 25,
                fee_denominator: 10_000,
            },
            HopParams {
                reserve_in: 100_000_000_000,     // 100000 USDC (in)
                reserve_out: 1_100_000_000_000,  // 1100 SOL (out) — SOL is cheap here
                fee_numerator: 25,
                fee_denominator: 10_000,
            },
        ];

        let (optimal, profit) = optimize_arb_amount(
            &hops,
            100_000_000_000, // max 100 SOL
            1_000_000,       // min 0.001 SOL
        );

        assert!(optimal > 0, "Should find non-zero optimal amount");
        assert!(profit > 0, "Should be profitable, got {}", profit);
    }

    #[test]
    fn test_no_arb_balanced_pools() {
        // Same price on both pools — no arb after fees
        let hops = vec![
            HopParams {
                reserve_in: 1_000_000_000,
                reserve_out: 100_000_000,
                fee_numerator: 25,
                fee_denominator: 10_000,
            },
            HopParams {
                reserve_in: 100_000_000,
                reserve_out: 1_000_000_000,
                fee_numerator: 25,
                fee_denominator: 10_000,
            },
        ];

        let (_, profit) = optimize_arb_amount(&hops, 100_000_000, 1_000);
        assert!(profit <= 0, "Balanced pools should not be profitable after fees");
    }

    #[test]
    fn test_ternary_search_finds_peak() {
        // Same profitable setup as test_optimize_simple_arb
        let hops = vec![
            HopParams {
                reserve_in: 1_000_000_000_000,
                reserve_out: 150_000_000_000,
                fee_numerator: 25,
                fee_denominator: 10_000,
            },
            HopParams {
                reserve_in: 100_000_000_000,
                reserve_out: 1_100_000_000_000,
                fee_numerator: 25,
                fee_denominator: 10_000,
            },
        ];

        let (optimal, max_profit) = optimize_arb_amount(&hops, 100_000_000_000, 1_000_000);
        assert!(max_profit > 0, "Must be profitable for peak test");

        // Check that the optimal is within 0.1% of the best nearby value
        let profit_below = simulate_cycle(&hops, optimal.saturating_sub(100_000_000));
        let profit_above = simulate_cycle(&hops, optimal.saturating_add(100_000_000));
        let best_nearby = profit_below.max(profit_above);

        // Allow 0.1% tolerance — ternary search converges to ~1000 lamport precision
        let tolerance = (max_profit.abs() as f64 * 0.001) as i64;
        assert!(
            max_profit >= best_nearby - tolerance,
            "Optimal should be near peak: opt={}, best_nearby={}, tolerance={}",
            max_profit, best_nearby, tolerance,
        );
    }
}
