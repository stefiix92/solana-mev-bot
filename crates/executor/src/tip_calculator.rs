use mev_common::constants::MIN_TIP_LAMPORTS;

/// Calculate the optimal Jito tip for a given expected profit.
///
/// Strategy: give `tip_fraction` of profit as tip to maximize inclusion probability.
/// Floor at MIN_TIP_LAMPORTS, ceiling at max_tip_lamports.
pub fn calculate_tip(
    expected_profit_lamports: i64,
    tip_fraction: f64,
    max_tip_lamports: u64,
) -> u64 {
    if expected_profit_lamports <= 0 {
        return MIN_TIP_LAMPORTS;
    }

    let tip = (expected_profit_lamports as f64 * tip_fraction) as u64;

    tip.max(MIN_TIP_LAMPORTS).min(max_tip_lamports)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tip_calculation() {
        // 100k lamports profit, 50% tip = 50k tip
        assert_eq!(calculate_tip(100_000, 0.5, 5_000_000), 50_000);
    }

    #[test]
    fn test_tip_floor() {
        // Very small profit → floor at MIN_TIP_LAMPORTS
        assert_eq!(calculate_tip(100, 0.5, 5_000_000), MIN_TIP_LAMPORTS);
    }

    #[test]
    fn test_tip_ceiling() {
        // Huge profit → capped at max
        assert_eq!(calculate_tip(100_000_000, 0.5, 5_000_000), 5_000_000);
    }

    #[test]
    fn test_tip_negative_profit() {
        assert_eq!(calculate_tip(-1000, 0.5, 5_000_000), MIN_TIP_LAMPORTS);
    }
}
