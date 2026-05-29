//! Decimal-exact statistics helpers.
//!
//! Money is never held as `f64`. Median, MAD, and quantiles are computed by
//! sorting [`Decimal`] values and selecting — no float conversion. The only place
//! we touch `f64` is the *final* dimensionless comparison scalars (the MAD
//! z-score and the coefficient of variation), per the task contract: a ratio used
//! once for a threshold test, never to hold or sum an amount.

use rust_decimal::dec;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;

/// `1.4826`, the constant making MAD a consistent estimator of stddev for normal
/// data. Used only inside the dimensionless z-score scalar.
pub const MAD_SCALE: Decimal = dec!(1.4826);

/// Sum of decimals (exact).
#[must_use]
pub fn sum(xs: &[Decimal]) -> Decimal {
    xs.iter().copied().sum()
}

/// Arithmetic mean as an exact decimal, or `None` for an empty slice.
#[must_use]
pub fn mean(xs: &[Decimal]) -> Option<Decimal> {
    if xs.is_empty() {
        return None;
    }
    let n = Decimal::from(xs.len() as u64);
    sum(xs).checked_div(n)
}

/// Median of a slice (exact for odd counts; the exact average of the two middle
/// values for even counts). `None` for an empty slice.
#[must_use]
pub fn median(xs: &[Decimal]) -> Option<Decimal> {
    if xs.is_empty() {
        return None;
    }
    let mut v = xs.to_vec();
    v.sort();
    let n = v.len();
    if n % 2 == 1 {
        Some(v[n / 2])
    } else {
        let a = v[n / 2 - 1];
        let b = v[n / 2];
        (a + b).checked_div(dec!(2))
    }
}

/// Median absolute deviation: `median(|x - median(x)|)`. `None` for empty input.
#[must_use]
pub fn mad(xs: &[Decimal]) -> Option<Decimal> {
    let m = median(xs)?;
    let devs: Vec<Decimal> = xs.iter().map(|x| (*x - m).abs()).collect();
    median(&devs)
}

/// Linear-interpolation quantile (type-7, the default of NumPy/R) on a sorted
/// copy. `q` in `[0, 1]`. Interpolation is done exactly in [`Decimal`].
#[must_use]
pub fn quantile(xs: &[Decimal], q: Decimal) -> Option<Decimal> {
    if xs.is_empty() {
        return None;
    }
    let mut v = xs.to_vec();
    v.sort();
    let n = v.len();
    if n == 1 {
        return Some(v[0]);
    }
    // position h = (n - 1) * q
    let h = Decimal::from((n - 1) as u64) * q;
    let lower = h.floor();
    let frac = h - lower;
    let lo_idx = lower.to_usize().unwrap_or(0).min(n - 1);
    let hi_idx = (lo_idx + 1).min(n - 1);
    let lo = v[lo_idx];
    let hi = v[hi_idx];
    Some(lo + (hi - lo) * frac)
}

/// First and third quartiles `(Q1, Q3)`.
#[must_use]
pub fn quartiles(xs: &[Decimal]) -> Option<(Decimal, Decimal)> {
    Some((quantile(xs, dec!(0.25))?, quantile(xs, dec!(0.75))?))
}

/// Robust MAD z-score `|x - median| / (MAD_SCALE * MAD)` as an `f64` scalar.
///
/// Returns `None` when MAD is zero (no spread; no meaningful z-score). The `f64`
/// here is a dimensionless ratio compared against a threshold, never a stored
/// amount — exactness of the underlying decimals is preserved up to this point.
#[must_use]
pub fn mad_zscore(x: Decimal, med: Decimal, mad_val: Decimal) -> Option<f64> {
    let denom = MAD_SCALE * mad_val;
    if denom.is_zero() {
        return None;
    }
    let ratio = (x - med).abs().checked_div(denom)?;
    ratio.to_f64()
}

/// Coefficient of variation `stddev / |mean|` as an `f64` scalar, using a
/// population stddev computed exactly in [`Decimal`] then converted once for the
/// ratio. `None` when the mean is zero or input is empty.
#[must_use]
pub fn coefficient_of_variation(xs: &[Decimal]) -> Option<f64> {
    let m = mean(xs)?;
    if m.is_zero() {
        return None;
    }
    let n = Decimal::from(xs.len() as u64);
    let var = xs
        .iter()
        .map(|x| {
            let d = *x - m;
            d * d
        })
        .sum::<Decimal>()
        .checked_div(n)?;
    // stddev = sqrt(var); do the sqrt in f64 (dimensionless ratio numerator).
    let var_f = var.to_f64()?;
    let mean_f = m.to_f64()?;
    if mean_f == 0.0 {
        return None;
    }
    Some(var_f.sqrt() / mean_f.abs())
}

/// Least-squares slope of `ys` against `xs` (both equal length), as an `f64`
/// scalar used only to read off a trend *direction*. `None` if degenerate.
#[must_use]
pub fn least_squares_slope(xs: &[Decimal], ys: &[Decimal]) -> Option<f64> {
    if xs.len() != ys.len() || xs.len() < 2 {
        return None;
    }
    let n = Decimal::from(xs.len() as u64);
    let sx = sum(xs);
    let sy = sum(ys);
    let sxx: Decimal = xs.iter().map(|x| *x * *x).sum();
    let sxy: Decimal = xs.iter().zip(ys).map(|(x, y)| *x * *y).sum();
    let denom = n * sxx - sx * sx;
    if denom.is_zero() {
        return None;
    }
    let slope = (n * sxy - sx * sy).checked_div(denom)?;
    slope.to_f64()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_odd_and_even() {
        assert_eq!(median(&[dec!(3), dec!(1), dec!(2)]), Some(dec!(2)));
        assert_eq!(
            median(&[dec!(1), dec!(2), dec!(3), dec!(4)]),
            Some(dec!(2.5))
        );
        assert_eq!(median(&[]), None);
    }

    #[test]
    fn mean_is_exact() {
        assert_eq!(mean(&[dec!(1.00), dec!(2.00)]), Some(dec!(1.5)));
        assert_eq!(mean(&[]), None);
    }

    #[test]
    fn mad_of_constant_is_zero() {
        assert_eq!(mad(&[dec!(5), dec!(5), dec!(5)]), Some(dec!(0)));
    }

    #[test]
    fn quantile_matches_type7() {
        // For 1..=5, Q1 = 2, Q3 = 4 under type-7 interpolation.
        let xs = [dec!(1), dec!(2), dec!(3), dec!(4), dec!(5)];
        assert_eq!(quantile(&xs, dec!(0.25)), Some(dec!(2)));
        assert_eq!(quantile(&xs, dec!(0.75)), Some(dec!(4)));
        assert_eq!(quantile(&xs, dec!(0.5)), Some(dec!(3)));
    }

    #[test]
    fn quartiles_simple() {
        let xs = [dec!(1), dec!(2), dec!(3), dec!(4), dec!(5)];
        assert_eq!(quartiles(&xs), Some((dec!(2), dec!(4))));
    }

    #[test]
    fn zscore_none_on_zero_mad() {
        assert_eq!(mad_zscore(dec!(10), dec!(5), dec!(0)), None);
    }

    #[test]
    fn zscore_flags_outlier() {
        // values mostly ~10, one at 100.
        let xs = [dec!(10), dec!(10), dec!(11), dec!(9), dec!(100)];
        let med = median(&xs).unwrap();
        let m = mad(&xs).unwrap();
        let z = mad_zscore(dec!(100), med, m).unwrap();
        assert!(z > 3.5, "z = {z}");
    }

    #[test]
    fn cov_of_stable_series_is_small() {
        let xs = [dec!(9.99), dec!(9.99), dec!(9.99), dec!(10.00)];
        let cov = coefficient_of_variation(&xs).unwrap();
        assert!(cov < 0.15, "cov = {cov}");
    }

    #[test]
    fn slope_sign_detects_rising() {
        let xs = [dec!(0), dec!(1), dec!(2), dec!(3)];
        let ys = [dec!(10), dec!(20), dec!(30), dec!(40)];
        let s = least_squares_slope(&xs, &ys).unwrap();
        assert!(s > 0.0);
    }
}
