//! Anomaly detection — three complementary, robust methods.
//!
//! - `mad_zscore`: a single transaction whose magnitude is a robust outlier
//!   within its **subcategory**, `|x - median| / (1.4826 * MAD) > 3.5`. Robust
//!   because the median/MAD are not dragged by the very outlier being detected.
//!   Scoped per subcategory (not the broad category) so like is compared with
//!   like — a $50 oil change is not "anomalous" against a pool of $16 gas
//!   fill-ups. Known **recurring/subscription** merchants are excluded: a stable
//!   repeated charge (rent, telecom, Netflix) is the opposite of an anomaly.
//! - `rare_large`: a large one-off purchase in a subcategory too small for the
//!   MAD pass (< `MIN_POOL` transactions) — a jewellery or travel splurge with no
//!   peers to be an outlier against. Flagged when it's a strong upper outlier of
//!   the **global** non-recurring spend distribution, judged in **log space** so
//!   the heavy right skew of purchase sizes doesn't dominate the fence.
//! - `monthly_iqr`: a whole month whose category spend exceeds `Q3 + 1.5*IQR` of
//!   that category's monthly history — distinguishing one big purchase from a
//!   broadly expensive month.
//!
//! Magnitudes are exact [`Decimal`]; only the z-score ratio is `f64` (per
//! [`super::stats`]). Each anomaly carries its `method`, `baseline`, and a
//! `severity` (the z-score, or the overage ratio over the IQR fence).

use std::collections::{BTreeMap, BTreeSet};

use rust_decimal::dec;
use rust_decimal::prelude::{FromPrimitive, ToPrimitive};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::stats::{mad, mad_zscore, median, quartiles};
use super::{month_key, Categorized};
use crate::categorize::EffectiveKind;
use crate::model::Money;

/// MAD z-score threshold for a single-transaction outlier.
const Z_THRESHOLD: f64 = 3.5;

/// Minimum transactions in a subcategory for a robust within-pool estimate. Below
/// this, the per-subcategory MAD pass can't run; such purchases are checked by the
/// `rare_large` pass instead (the two passes partition the spend by pool size).
const MIN_POOL: usize = 4;

/// Log-space z-score threshold for a rare-subcategory large purchase.
const RARE_LARGE_Z: f64 = 3.0;

/// One detected anomaly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Anomaly {
    /// Detection method (`mad_zscore` | `rare_large` | `monthly_iqr`).
    pub method: String,
    /// Category the anomaly is scoped to.
    pub category: String,
    /// The single transaction date (`mad_zscore`) or month (`monthly_iqr`).
    pub period_or_date: String,
    /// The anomalous amount magnitude.
    pub amount: Money,
    /// The reference baseline (category median for z-score, IQR fence for monthly).
    pub baseline: Money,
    /// Severity: z-score, or the ratio `amount / fence` for the monthly method.
    pub severity: String,
    /// Merchant, for single-transaction anomalies (aids the review/report).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merchant: Option<String>,
}

/// Detect anomalies among spend items.
///
/// `recurring_merchants` is the set of normalized-merchant names already flagged
/// as recurring/subscription; their transactions are excluded from the
/// single-transaction outlier pass (a stable repeated charge is not an anomaly).
#[must_use]
pub fn detect(items: &[Categorized<'_>], recurring_merchants: &BTreeSet<String>) -> Vec<Anomaly> {
    let mut out = Vec::new();
    out.extend(mad_zscore_anomalies(items, recurring_merchants));
    out.extend(rare_large_anomalies(items, recurring_merchants));
    out.extend(monthly_iqr_anomalies(items));
    // Deterministic ordering: method, category, period/date, then amount.
    out.sort_by(|a, b| {
        a.method
            .cmp(&b.method)
            .then_with(|| a.category.cmp(&b.category))
            .then_with(|| a.period_or_date.cmp(&b.period_or_date))
            .then_with(|| b.amount.amount().cmp(&a.amount.amount()))
    });
    out
}

/// Per-subcategory single-transaction MAD z-score outliers.
///
/// Pools are scoped per subcategory (not the broad category) so the median/MAD
/// describe like-for-like spend; transactions from `recurring_merchants` are
/// skipped so a stable repeated charge does not get flagged on every occurrence.
fn mad_zscore_anomalies(
    items: &[Categorized<'_>],
    recurring_merchants: &BTreeSet<String>,
) -> Vec<Anomaly> {
    // subcategory -> Vec<(magnitude, date, merchant)>
    let mut by_sub: BTreeMap<String, Vec<(Decimal, jiff::civil::Date, String)>> = BTreeMap::new();
    for c in items {
        if c.assignment.kind != EffectiveKind::Spend {
            continue;
        }
        if recurring_merchants.contains(&c.txn.normalized_merchant) {
            continue;
        }
        by_sub
            .entry(c.subcategory_label().to_string())
            .or_default()
            .push((
                c.txn.amount.amount().abs(),
                c.txn.date,
                c.txn.normalized_merchant.clone(),
            ));
    }

    let mut out = Vec::new();
    for (subcategory, rows) in by_sub {
        if rows.len() < MIN_POOL {
            // Too few points for a meaningful robust estimate (the `rare_large`
            // pass handles large one-off purchases in these small subcategories).
            continue;
        }
        let mags: Vec<Decimal> = rows.iter().map(|(m, _, _)| *m).collect();
        let med = match median(&mags) {
            Some(m) => m,
            None => continue,
        };
        let mad_val = match mad(&mags) {
            Some(m) => m,
            None => continue,
        };
        for (mag, date, merchant) in &rows {
            if let Some(z) = mad_zscore(*mag, med, mad_val) {
                if z > Z_THRESHOLD {
                    out.push(Anomaly {
                        method: "mad_zscore".into(),
                        category: subcategory.clone(),
                        period_or_date: date.to_string(),
                        amount: Money::new(*mag),
                        baseline: Money::new(med),
                        severity: format!("{z:.2}"),
                        merchant: Some(merchant.clone()),
                    });
                }
            }
        }
    }
    out
}

/// Median of a slice of `f64` (sorted copy made internally). Returns `0.0` for an
/// empty slice; callers guard on the population size before relying on the result.
fn f64_median(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

/// Large one-off purchases in a *rare* subcategory (fewer than [`MIN_POOL`]
/// transactions), which the per-subcategory pass can't evaluate.
///
/// Such a purchase has no peers to be an outlier against, yet a large charge in a
/// subcategory you almost never use — a jewellery or travel splurge — is exactly
/// the "unusual activity" worth surfacing. We flag it when its magnitude is a
/// strong **upper** outlier of the global non-recurring spend distribution, judged
/// in **log space**: purchase sizes are heavily right-skewed, so a log scale keeps
/// the robust fence from being dominated by a few big-ticket categories (per the
/// stats lesson on skew). `mad == 0` ⇒ fail closed (no anomaly). The z-score is a
/// dimensionless ratio (`f64`); the displayed magnitude/baseline stay `Decimal`.
fn rare_large_anomalies(
    items: &[Categorized<'_>],
    recurring_merchants: &BTreeSet<String>,
) -> Vec<Anomaly> {
    // Per-subcategory non-recurring spend counts, and the global log-magnitudes.
    let mut sub_count: BTreeMap<String, usize> = BTreeMap::new();
    let mut logs: Vec<f64> = Vec::new();
    for c in items {
        if c.assignment.kind != EffectiveKind::Spend
            || recurring_merchants.contains(&c.txn.normalized_merchant)
        {
            continue;
        }
        *sub_count
            .entry(c.subcategory_label().to_string())
            .or_default() += 1;
        if let Some(f) = c.txn.amount.amount().abs().to_f64() {
            if f > 0.0 {
                logs.push(f.ln());
            }
        }
    }
    // Need a reasonable global population for a stable robust estimate.
    if logs.len() < 10 {
        return Vec::new();
    }
    let med = f64_median(&logs);
    let devs: Vec<f64> = logs.iter().map(|x| (x - med).abs()).collect();
    let mad_log = f64_median(&devs);
    if mad_log <= 0.0 {
        return Vec::new(); // undefined spread ⇒ fail closed
    }
    let scale = 1.4826 * mad_log;
    let typical = Decimal::from_f64(med.exp()).unwrap_or_default().round_dp(2);

    let mut out = Vec::new();
    for c in items {
        if c.assignment.kind != EffectiveKind::Spend
            || recurring_merchants.contains(&c.txn.normalized_merchant)
        {
            continue;
        }
        let sub = c.subcategory_label().to_string();
        if sub_count.get(&sub).copied().unwrap_or(0) >= MIN_POOL {
            continue; // large enough pool ⇒ owned by the MAD pass
        }
        let mag = c.txn.amount.amount().abs();
        let f = match mag.to_f64() {
            Some(f) if f > 0.0 => f,
            _ => continue,
        };
        let z = (f.ln() - med) / scale; // positive ⇒ larger than typical (upper tail)
        if z > RARE_LARGE_Z {
            out.push(Anomaly {
                method: "rare_large".into(),
                category: sub,
                period_or_date: c.txn.date.to_string(),
                amount: Money::new(mag),
                baseline: Money::new(typical),
                severity: format!("{z:.2}"),
                merchant: Some(c.txn.normalized_merchant.clone()),
            });
        }
    }
    out
}

/// Per-category months exceeding the IQR fence of that category's monthly history.
fn monthly_iqr_anomalies(items: &[Categorized<'_>]) -> Vec<Anomaly> {
    // category -> month -> total magnitude
    let mut by_cat: BTreeMap<String, BTreeMap<String, Decimal>> = BTreeMap::new();
    for c in items {
        if c.assignment.kind != EffectiveKind::Spend {
            continue;
        }
        *by_cat
            .entry(c.category_label().to_string())
            .or_default()
            .entry(month_key(c.txn.date))
            .or_insert(Decimal::ZERO) += c.txn.amount.amount().abs();
    }

    let mut out = Vec::new();
    for (category, months) in by_cat {
        if months.len() < 4 {
            // Need enough months for a stable quartile fence.
            continue;
        }
        let totals: Vec<Decimal> = months.values().copied().collect();
        let (q1, q3) = match quartiles(&totals) {
            Some(q) => q,
            None => continue,
        };
        let iqr = q3 - q1;
        let fence = q3 + dec!(1.5) * iqr;
        if fence.is_zero() {
            continue;
        }
        for (month, total) in &months {
            if *total > fence {
                let ratio = total
                    .checked_div(fence)
                    .and_then(|r| r.to_f64())
                    .unwrap_or(0.0);
                out.push(Anomaly {
                    method: "monthly_iqr".into(),
                    category: category.clone(),
                    period_or_date: month.clone(),
                    amount: Money::new(*total),
                    baseline: Money::new(fence),
                    severity: format!("{ratio:.2}"),
                    merchant: None,
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::categorize::Assignment;
    use crate::model::{Currency, Transaction, TxnType};
    use jiff::civil::{date, Date};
    use std::str::FromStr;

    fn spend_full(
        cat: &str,
        sub: &str,
        merchant: &str,
        amount: &str,
        d: Date,
    ) -> (Transaction, Assignment) {
        let t = Transaction {
            raw_description: merchant.into(),
            normalized_merchant: merchant.into(),
            date: d,
            time: None,
            amount: Money::from_str(amount).unwrap(),
            currency: Currency::new("CAD"),
            txn_type: TxnType::Spend,
            memo: None,
            cardholder: None,
            profile_id: "t".into(),
            source_file: "f.csv".into(),
            owner_person: None,
            account_id: None,
            harvested_category: None,
            dedup_key: String::new(),
        };
        let a = Assignment {
            category: Some(cat.into()),
            subcategory: Some(sub.into()),
            kind: EffectiveKind::Spend,
            needs_review: false,
        };
        (t, a)
    }

    // Default merchant "M", default subcategory "S" — most tests only vary amount.
    fn spend(cat: &str, amount: &str, d: Date) -> (Transaction, Assignment) {
        spend_full(cat, "S", "M", amount, d)
    }
    fn spend_sub(cat: &str, sub: &str, amount: &str, d: Date) -> (Transaction, Assignment) {
        spend_full(cat, sub, "M", amount, d)
    }
    fn spend_named(
        cat: &str,
        sub: &str,
        merchant: &str,
        amount: &str,
        d: Date,
    ) -> (Transaction, Assignment) {
        spend_full(cat, sub, merchant, amount, d)
    }

    fn view<'a>(p: &'a [(Transaction, Assignment)]) -> Vec<Categorized<'a>> {
        p.iter()
            .map(|(t, a)| Categorized {
                txn: t,
                assignment: a,
            })
            .collect()
    }

    fn no_recurring() -> BTreeSet<String> {
        BTreeSet::new()
    }

    #[test]
    fn flags_single_transaction_outlier() {
        // A realistic spread so MAD is non-zero; one clear outlier at 5000.
        let amounts = [
            "-48.00", "-52.00", "-50.00", "-55.00", "-45.00", "-51.00", "-49.00", "-53.00",
        ];
        let mut pairs = vec![];
        for (i, a) in amounts.iter().enumerate() {
            pairs.push(spend("Groceries", a, date(2026, 1, (i + 1) as i8)));
        }
        pairs.push(spend("Groceries", "-5000.00", date(2026, 1, 15)));
        let a = detect(&view(&pairs), &no_recurring());
        assert!(a
            .iter()
            .any(|x| x.method == "mad_zscore"
                && x.amount.amount() == rust_decimal_macros::dec!(5000.00)));
    }

    /// A pricey-but-normal item in its own subcategory must not be flagged just
    /// because it dwarfs a different, cheaper subcategory in the same category.
    #[test]
    fn subcategory_scoping_avoids_cross_subcategory_false_positive() {
        let mut pairs = vec![];
        // Many small "Gas" charges (subcategory S1) ...
        for i in 1..=10 {
            pairs.push(spend_sub("Car", "S1", "-16.00", date(2026, 1, i)));
        }
        // ... and a handful of larger, steady "Maintenance" charges (subcategory S2).
        let maint = ["-180.00", "-200.00", "-190.00", "-210.00", "-195.00"];
        for (i, a) in maint.iter().enumerate() {
            pairs.push(spend_sub("Car", "S2", a, date(2026, 2, (i + 1) as i8)));
        }
        let a = detect(&view(&pairs), &no_recurring());
        // Pooled per subcategory, none of the maintenance charges are outliers.
        assert!(
            !a.iter().any(|x| x.method == "mad_zscore"),
            "no single-txn anomaly expected, got: {a:?}"
        );
    }

    /// A stable recurring merchant must not be flagged on every occurrence.
    #[test]
    fn excludes_recurring_merchant() {
        let mut pairs = vec![];
        // Small, varied subcategory baseline so MAD is non-zero.
        let base = ["-20.00", "-25.00", "-22.00", "-28.00", "-24.00"];
        for (i, amt) in base.iter().enumerate() {
            pairs.push(spend_named(
                "Bills",
                "S",
                "CORNER STORE",
                amt,
                date(2026, 1, (i + 1) as i8),
            ));
        }
        // A big, *recurring* mortgage in the same subcategory, every month.
        for m in 1..=6i8 {
            pairs.push(spend_named(
                "Bills",
                "S",
                "RMG MTG",
                "-950.00",
                date(2026, m, 1),
            ));
        }
        let mut recurring = BTreeSet::new();
        recurring.insert("RMG MTG".to_string());
        let a = detect(&view(&pairs), &recurring);
        assert!(
            !a.iter()
                .any(|x| x.method == "mad_zscore" && x.merchant.as_deref() == Some("RMG MTG")),
            "recurring merchant should be excluded, got: {a:?}"
        );
    }

    /// A large one-off purchase alone in its subcategory (too few peers for the
    /// MAD pass) is still flagged by `rare_large` against the global distribution.
    #[test]
    fn flags_rare_large_one_off_purchase() {
        let mut pairs = vec![];
        // A broad pool of small, everyday purchases across normal subcategories.
        let everyday = [
            "-20.00", "-35.00", "-12.00", "-48.00", "-25.00", "-60.00", "-18.00", "-40.00",
            "-30.00", "-22.00", "-55.00", "-15.00",
        ];
        for (i, a) in everyday.iter().enumerate() {
            pairs.push(spend_sub(
                "Food",
                "Groceries",
                a,
                date(2026, 1, (i + 1) as i8),
            ));
        }
        // A single $3,000 jewellery purchase — alone in its subcategory.
        pairs.push(spend_named(
            "Indulgence",
            "Gifts",
            "JEWELLER",
            "-3000.00",
            date(2026, 2, 14),
        ));
        let a = detect(&view(&pairs), &no_recurring());
        assert!(
            a.iter().any(|x| x.method == "rare_large"
                && x.merchant.as_deref() == Some("JEWELLER")
                && x.amount.amount() == rust_decimal_macros::dec!(3000.00)),
            "rare large purchase should be flagged, got: {a:?}"
        );
    }

    /// A *small* one-off purchase in a rare subcategory must NOT be flagged — only
    /// genuinely large outliers of the global distribution qualify.
    #[test]
    fn does_not_flag_rare_small_purchase() {
        let mut pairs = vec![];
        let everyday = [
            "-20.00", "-35.00", "-12.00", "-48.00", "-25.00", "-60.00", "-18.00", "-40.00",
            "-30.00", "-22.00", "-55.00", "-15.00",
        ];
        for (i, a) in everyday.iter().enumerate() {
            pairs.push(spend_sub(
                "Food",
                "Groceries",
                a,
                date(2026, 1, (i + 1) as i8),
            ));
        }
        // A lone $28 purchase in a rare subcategory — close to typical, not large.
        pairs.push(spend_named(
            "Misc",
            "Oddment",
            "CORNER",
            "-28.00",
            date(2026, 2, 2),
        ));
        let a = detect(&view(&pairs), &no_recurring());
        assert!(
            !a.iter().any(|x| x.method == "rare_large"),
            "a small rare purchase should not be flagged, got: {a:?}"
        );
    }

    #[test]
    fn flags_expensive_month() {
        // Four normal months, one with a huge total spread over many txns.
        let mut pairs = vec![];
        for m in 1..=4i8 {
            for d in 1..=3 {
                pairs.push(spend("Groceries", "-50.00", date(2026, m, d)));
            }
        }
        for d in 1..=10 {
            pairs.push(spend("Groceries", "-300.00", date(2026, 5, d)));
        }
        let a = detect(&view(&pairs), &no_recurring());
        assert!(a
            .iter()
            .any(|x| x.method == "monthly_iqr" && x.period_or_date == "2026-05"));
    }

    #[test]
    fn no_anomaly_in_uniform_data() {
        let mut pairs = vec![];
        for i in 1..=10 {
            pairs.push(spend("Groceries", "-50.00", date(2026, 1, i)));
        }
        assert!(detect(&view(&pairs), &no_recurring()).is_empty());
    }
}
