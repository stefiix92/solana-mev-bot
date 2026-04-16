/// Constant product AMM math: x * y = k
///
/// Used by Raydium AMM V4.

/// Calculate output amount for a swap (base-in direction).
///
/// `reserve_in`: current reserve of the input token
/// `reserve_out`: current reserve of the output token
/// `amount_in`: amount being swapped in (BEFORE fees)
/// `fee_numerator` / `fee_denominator`: swap fee fraction
///
/// Returns (amount_out, fee_amount)
pub fn swap_base_in(
    reserve_in: u64,
    reserve_out: u64,
    amount_in: u64,
    fee_numerator: u64,
    fee_denominator: u64,
) -> Option<(u64, u64)> {
    if reserve_in == 0 || reserve_out == 0 || amount_in == 0 || fee_denominator == 0 {
        return None;
    }

    // Calculate fee
    let fee_amount = (amount_in as u128)
        .checked_mul(fee_numerator as u128)?
        .checked_div(fee_denominator as u128)? as u64;

    let amount_in_after_fee = amount_in.checked_sub(fee_amount)?;

    // Constant product: amount_out = (reserve_out * amount_in_after_fee) / (reserve_in + amount_in_after_fee)
    let numerator = (reserve_out as u128).checked_mul(amount_in_after_fee as u128)?;
    let denominator = (reserve_in as u128).checked_add(amount_in_after_fee as u128)?;

    let amount_out = numerator.checked_div(denominator)? as u64;

    if amount_out >= reserve_out {
        return None; // Cannot drain the pool
    }

    Some((amount_out, fee_amount))
}

/// Calculate the required input amount for a desired output amount (base-out direction).
///
/// Returns (amount_in_with_fee, fee_amount)
pub fn swap_base_out(
    reserve_in: u64,
    reserve_out: u64,
    amount_out: u64,
    fee_numerator: u64,
    fee_denominator: u64,
) -> Option<(u64, u64)> {
    if reserve_in == 0 || reserve_out == 0 || amount_out == 0 || fee_denominator == 0 {
        return None;
    }

    if amount_out >= reserve_out {
        return None;
    }

    // amount_in_before_fee = (reserve_in * amount_out) / (reserve_out - amount_out) + 1 (round up)
    let numerator = (reserve_in as u128).checked_mul(amount_out as u128)?;
    let denominator = (reserve_out as u128).checked_sub(amount_out as u128)?;

    let amount_in_before_fee = numerator
        .checked_div(denominator)?
        .checked_add(1)? as u64; // Round up

    // amount_in_with_fee = amount_in_before_fee / (1 - fee_rate)
    // = amount_in_before_fee * fee_denominator / (fee_denominator - fee_numerator)
    let denom_minus_fee = fee_denominator.checked_sub(fee_numerator)?;
    let amount_in_with_fee = (amount_in_before_fee as u128)
        .checked_mul(fee_denominator as u128)?
        .checked_div(denom_minus_fee as u128)?
        .checked_add(1)? as u64; // Round up

    let fee_amount = amount_in_with_fee.checked_sub(amount_in_before_fee)?;

    Some((amount_in_with_fee, fee_amount))
}

/// Calculate price impact in basis points.
pub fn price_impact_bps(
    reserve_in: u64,
    reserve_out: u64,
    amount_in: u64,
    amount_out: u64,
) -> u16 {
    if reserve_in == 0 || reserve_out == 0 || amount_in == 0 {
        return 0;
    }

    // Spot price = reserve_out / reserve_in
    // Effective price = amount_out / amount_in
    // Impact = 1 - (effective / spot) = 1 - (amount_out * reserve_in) / (amount_in * reserve_out)
    let spot_numerator = (amount_out as u128) * (reserve_in as u128);
    let spot_denominator = (amount_in as u128) * (reserve_out as u128);

    if spot_denominator == 0 {
        return 10_000; // 100%
    }

    let ratio = (spot_numerator * 10_000) / spot_denominator;
    if ratio >= 10_000 {
        return 0;
    }

    (10_000 - ratio) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_swap_base_in_basic() {
        // Pool: 1000 SOL / 100000 USDC, fee 0.25%
        let (amount_out, fee) = swap_base_in(
            1_000_000_000_000, // 1000 SOL (lamports)
            100_000_000_000,   // 100000 USDC (micro-USDC with 6 decimals)
            1_000_000_000,     // 1 SOL in
            25,                // 0.25% fee
            10_000,
        ).unwrap();

        assert!(amount_out > 0);
        assert!(fee > 0);
        // ~99.75 USDC out for 1 SOL in a 1000/100000 pool
        assert!(amount_out < 100_000_000); // Less than 100 USDC
    }

    #[test]
    fn test_swap_zero_input() {
        assert!(swap_base_in(1000, 1000, 0, 25, 10_000).is_none());
    }

    #[test]
    fn test_swap_large_input_never_drains() {
        // With constant product, output asymptotically approaches reserve_out
        // but never reaches it — this is a property of x*y=k
        let (amount_out, _fee) = swap_base_in(100, 100, 100_000, 25, 10_000).unwrap();
        assert!(amount_out < 100, "Should never drain full reserves");
    }

    #[test]
    fn test_swap_base_out_roundtrip() {
        let reserve_in = 1_000_000_000_000u64;
        let reserve_out = 100_000_000_000u64;
        let desired_out = 50_000_000u64; // 50 USDC

        let (amount_in, _fee) = swap_base_out(
            reserve_in, reserve_out, desired_out, 25, 10_000,
        ).unwrap();

        // Verify: swapping amount_in should yield at least desired_out
        let (actual_out, _) = swap_base_in(
            reserve_in, reserve_out, amount_in, 25, 10_000,
        ).unwrap();

        assert!(actual_out >= desired_out);
    }
}
