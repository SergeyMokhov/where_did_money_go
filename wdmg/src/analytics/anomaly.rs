//! Anomaly detection — a battery of complementary, robust, *explainable* methods,
//! tuned by the persona profile (consumer/prosumer vs business/public). Every
//! detector emits a plain-language `reason`; black-box ML is deliberately excluded
//! because the offline report must justify each flag. See
//! `docs/unusual-activity-methods.md` for the method/persona rationale.
//!
//! Amount outliers:
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
//!   that category's monthly history (recurring excluded) — distinguishing one big
//!   purchase from a broadly expensive month.
//!
//! Patterns (all profiles): `duplicate_charge` (same merchant+amount ≤2 days),
//! `first_seen_large` (a large charge from a new payee in the last 90 days),
//! `subscription_increase` (a recurring charge that stepped up), `card_testing` (a
//! burst of tiny charges across ≥3 merchants).
//!
//! Time-series (all profiles): `change_point` (a sustained shift in monthly
//! spending), `seasonal_residual` (a month high even for its season; recurring
//! excluded, capped per category), `cashflow_runway` (a multi-month deficit streak).
//!
//! Forensic/compliance (`business`/`public` profiles only): `structuring`
//! (clustered near-threshold amounts), `round_number` (repeated irregular round
//! transfers to one counterparty), `benford` (leading-digit χ² conformity).
//!
//! Magnitudes are exact [`Decimal`]; only dimensionless ratios (z-scores, χ²) are
//! `f64`. Each anomaly carries `scope` (transaction/month/dataset), a numeric-ish
//! `severity`, and a plain-language `reason` the report shows verbatim.

use std::collections::{BTreeMap, BTreeSet};

use rust_decimal::dec;
use rust_decimal::prelude::{FromPrimitive, ToPrimitive};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::stats::{mad, mad_zscore, median, quartiles};
use super::{month_key, Categorized};
use crate::categorize::EffectiveKind;
use crate::config::AnalysisSettings;
use crate::model::Money;

/// MAD z-score threshold for a single-transaction outlier.
const Z_THRESHOLD: f64 = 3.5;

/// Minimum transactions in a subcategory for a robust within-pool estimate. Below
/// this, the per-subcategory MAD pass can't run; such purchases are checked by the
/// `rare_large` pass instead (the two passes partition the spend by pool size).
const MIN_POOL: usize = 4;

/// Log-space z-score threshold for a rare-subcategory large purchase.
const RARE_LARGE_Z: f64 = 3.0;

/// One detected anomaly. Carries enough context for the report to (a) explain it
/// in plain language (`reason`) and (b) keep it in sync with the active filter:
/// `scope` says what the row represents, and the optional `subcategory`/`person`/
/// `account` mirror the underlying transaction's facets for transaction-scoped
/// anomalies. Aggregate anomalies (`month`, `dataset`) filter by category/month
/// only.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Anomaly {
    /// Detection method id (e.g. `mad_zscore`, `duplicate_charge`, `benford`).
    pub method: String,
    /// Filtering scope: `transaction` (one txn), `month` (a category-month), or
    /// `dataset` (the whole selection).
    #[serde(default)]
    pub scope: String,
    /// Top-level category the anomaly is scoped to (`""` for dataset-wide).
    pub category: String,
    /// Subcategory, for transaction-scoped anomalies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subcategory: Option<String>,
    /// The single transaction date (`YYYY-MM-DD`), a month (`YYYY-MM`), or a span.
    pub period_or_date: String,
    /// The anomalous amount magnitude (or the relevant aggregate).
    pub amount: Money,
    /// The reference baseline (median, fence, expected value, …).
    pub baseline: Money,
    /// Severity: a numeric-ish string (z-score, overage ratio, conformity stat).
    pub severity: String,
    /// Plain-language explanation shown in the report.
    #[serde(default)]
    pub reason: String,
    /// Merchant, for single-transaction anomalies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merchant: Option<String>,
    /// Owner person id, for transaction-scoped anomalies (filter sync).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person: Option<String>,
    /// Account id, for transaction-scoped anomalies (filter sync).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
}

/// Build a transaction-scoped anomaly from a [`Categorized`], copying its facets so
/// the report can filter it exactly like the transaction it describes.
fn txn_anomaly(
    method: &str,
    c: &Categorized<'_>,
    baseline: Decimal,
    severity: String,
    reason: String,
) -> Anomaly {
    Anomaly {
        method: method.into(),
        scope: "transaction".into(),
        category: c.category_label().to_string(),
        subcategory: Some(c.subcategory_label().to_string()),
        period_or_date: c.txn.date.to_string(),
        amount: Money::new(c.txn.amount.amount().abs()),
        baseline: Money::new(baseline),
        severity,
        reason,
        merchant: Some(c.txn.normalized_merchant.clone()),
        person: c.txn.owner_person.clone(),
        account: c.txn.account_id.clone(),
    }
}

/// Detect anomalies across the transactions, honoring the persona profile.
///
/// `recurring_merchants` is the set of normalized-merchant names already flagged as
/// recurring/subscription; their transactions are excluded from the
/// single-transaction outlier passes (a stable repeated charge is not an anomaly).
/// Forensic/compliance detectors (Benford, structuring, round-number) run only for
/// the business/public profiles.
#[must_use]
pub fn detect(
    items: &[Categorized<'_>],
    recurring_merchants: &BTreeSet<String>,
    settings: &AnalysisSettings,
) -> Vec<Anomaly> {
    let mut out = Vec::new();
    // Amount outliers.
    out.extend(mad_zscore_anomalies(items, recurring_merchants));
    out.extend(rare_large_anomalies(items, recurring_merchants));
    out.extend(monthly_iqr_anomalies(items, recurring_merchants));
    // Tier 1: patterns useful for everyone.
    out.extend(duplicate_charge_anomalies(items, recurring_merchants));
    out.extend(first_seen_large_anomalies(items, recurring_merchants));
    out.extend(subscription_price_increase_anomalies(items));
    // Tier 2: card-testing is a universal fraud signal.
    out.extend(card_testing_anomalies(items, recurring_merchants));
    // Tier 2 forensic/compliance — business & public profiles only.
    if settings.profile.includes_forensic() {
        out.extend(structuring_anomalies(items, settings.structuring_threshold));
        out.extend(round_number_transfer_anomalies(items, recurring_merchants));
        out.extend(benford_anomalies(items));
    }
    // Tier 3: time-series structure.
    out.extend(change_point_anomalies(items));
    out.extend(seasonal_residual_anomalies(items, recurring_merchants));
    out.extend(cashflow_runway_anomalies(items));
    // Prefer the season-aware flag: drop a `monthly_iqr` month already surfaced by
    // `seasonal_residual` for the same category, so a seasonal peak isn't listed twice.
    let seasonal: BTreeSet<(String, String)> = out
        .iter()
        .filter(|a| a.method == "seasonal_residual")
        .map(|a| (a.category.clone(), a.period_or_date.clone()))
        .collect();
    out.retain(|a| {
        !(a.method == "monthly_iqr"
            && seasonal.contains(&(a.category.clone(), a.period_or_date.clone())))
    });
    // Deterministic ordering: method, category, period/date, then amount.
    out.sort_by(|a, b| {
        a.method
            .cmp(&b.method)
            .then_with(|| a.category.cmp(&b.category))
            .then_with(|| a.period_or_date.cmp(&b.period_or_date))
            .then_with(|| b.amount.amount().cmp(&a.amount.amount()))
            .then_with(|| a.merchant.cmp(&b.merchant))
    });
    out
}

/// Per-subcategory single-transaction MAD z-score outliers.
///
/// Pools are scoped per subcategory (not the broad category) so the median/MAD
/// describe like-for-like spend; transactions from `recurring_merchants` are
/// skipped so a stable repeated charge does not get flagged on every occurrence.
fn mad_zscore_anomalies<'a>(
    items: &'a [Categorized<'a>],
    recurring_merchants: &BTreeSet<String>,
) -> Vec<Anomaly> {
    // subcategory -> the categorized spend rows in it
    let mut by_sub: BTreeMap<String, Vec<&'a Categorized<'a>>> = BTreeMap::new();
    for c in items {
        if c.assignment.kind != EffectiveKind::Spend
            || recurring_merchants.contains(&c.txn.normalized_merchant)
        {
            continue;
        }
        by_sub
            .entry(c.subcategory_label().to_string())
            .or_default()
            .push(c);
    }

    let mut out = Vec::new();
    for (subcategory, rows) in by_sub {
        if rows.len() < MIN_POOL {
            // Too few points for a meaningful robust estimate (the `rare_large`
            // pass handles large one-off purchases in these small subcategories).
            continue;
        }
        let mags: Vec<Decimal> = rows.iter().map(|c| c.txn.amount.amount().abs()).collect();
        let med = match median(&mags) {
            Some(m) => m,
            None => continue,
        };
        let mad_val = match mad(&mags) {
            Some(m) => m,
            None => continue,
        };
        for c in &rows {
            let mag = c.txn.amount.amount().abs();
            if let Some(z) = mad_zscore(mag, med, mad_val) {
                if z > Z_THRESHOLD {
                    let reason = format!(
                        "{} of {} is {:.1}× the usual {} charge (~{})",
                        c.txn.normalized_merchant,
                        Money::new(mag),
                        if med.is_zero() {
                            0.0
                        } else {
                            (mag / med).to_f64().unwrap_or(0.0)
                        },
                        subcategory,
                        Money::new(med),
                    );
                    out.push(txn_anomaly("mad_zscore", c, med, format!("{z:.2}"), reason));
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
            let reason = format!(
                "{} of {} is a large one-off in {}, a category you rarely use (typical purchase ~{})",
                c.txn.normalized_merchant,
                Money::new(mag),
                sub,
                Money::new(typical),
            );
            out.push(txn_anomaly(
                "rare_large",
                c,
                typical,
                format!("{z:.2}"),
                reason,
            ));
        }
    }
    out
}

/// Per-category months exceeding the IQR fence of that category's monthly history.
///
/// Recurring merchants are excluded from the monthly totals so a steady bill (rent,
/// telecom) — or a one-time *shift* in such a bill — doesn't masquerade as an
/// unusually expensive month; the discretionary spend is what we're testing.
fn monthly_iqr_anomalies(items: &[Categorized<'_>], recurring: &BTreeSet<String>) -> Vec<Anomaly> {
    // category -> month -> total magnitude
    let mut by_cat: BTreeMap<String, BTreeMap<String, Decimal>> = BTreeMap::new();
    for c in items {
        if c.assignment.kind != EffectiveKind::Spend
            || recurring.contains(&c.txn.normalized_merchant)
        {
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
                let reason = format!(
                    "{} spending of {} in {} is well above the usual month (fence {})",
                    category,
                    Money::new(*total),
                    month,
                    Money::new(fence),
                );
                out.push(Anomaly {
                    method: "monthly_iqr".into(),
                    scope: "month".into(),
                    category: category.clone(),
                    subcategory: None,
                    period_or_date: month.clone(),
                    amount: Money::new(*total),
                    baseline: Money::new(fence),
                    severity: format!("{ratio:.2}"),
                    reason,
                    merchant: None,
                    person: None,
                    account: None,
                });
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Whole days from `a` to `b` (negative if `b` is earlier).
fn day_diff(a: jiff::civil::Date, b: jiff::civil::Date) -> i64 {
    b.since(a).map(|s| s.get_days() as i64).unwrap_or(0)
}

/// The `p` (0..1) percentile of a pre-sorted `Decimal` slice (nearest-rank).
fn percentile(sorted: &[Decimal], p: f64) -> Option<Decimal> {
    if sorted.is_empty() {
        return None;
    }
    let idx = ((p * (sorted.len() as f64 - 1.0)).round() as usize).min(sorted.len() - 1);
    Some(sorted[idx])
}

/// All non-recurring spend magnitudes (sorted ascending), for global fences.
fn global_spend_mags(items: &[Categorized<'_>], recurring: &BTreeSet<String>) -> Vec<Decimal> {
    let mut mags: Vec<Decimal> = items
        .iter()
        .filter(|c| {
            c.assignment.kind == EffectiveKind::Spend
                && !recurring.contains(&c.txn.normalized_merchant)
                && !c.txn.amount.amount().is_zero()
        })
        .map(|c| c.txn.amount.amount().abs())
        .collect();
    mags.sort();
    mags
}

/// Latest transaction date across `items`, if any.
fn max_date(items: &[Categorized<'_>]) -> Option<jiff::civil::Date> {
    items.iter().map(|c| c.txn.date).max()
}

/// True when the dated rows recur on a steady cadence (a standing arrangement, not
/// an irregular series): ≥3 rows, a 6–40 day median interval, and a low coefficient
/// of variation across intervals.
fn is_regular_cadence(rows: &[&Categorized<'_>]) -> bool {
    if rows.len() < 3 {
        return false;
    }
    let mut dates: Vec<jiff::civil::Date> = rows.iter().map(|c| c.txn.date).collect();
    dates.sort();
    let intervals: Vec<f64> = dates
        .windows(2)
        .map(|w| day_diff(w[0], w[1]) as f64)
        .collect();
    let mean = intervals.iter().sum::<f64>() / intervals.len() as f64;
    if mean <= 0.0 {
        return false;
    }
    let var = intervals.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / intervals.len() as f64;
    let cov = var.sqrt() / mean;
    (6.0..=40.0).contains(&mean) && cov < 0.35
}

// ---------------------------------------------------------------------------
// Tier 1: duplicate charge, first-seen large, subscription price increase
// ---------------------------------------------------------------------------

/// Two non-recurring charges from the same merchant for the **same amount** within
/// ≤2 days — almost always a double-bill or an accidental re-charge. Flags the
/// later of each such pair. Universally useful, and the highest signal-per-line of
/// the new detectors.
fn duplicate_charge_anomalies<'a>(
    items: &'a [Categorized<'a>],
    recurring: &BTreeSet<String>,
) -> Vec<Anomaly> {
    let mut groups: BTreeMap<(String, Decimal), Vec<&'a Categorized<'a>>> = BTreeMap::new();
    for c in items {
        if c.assignment.kind != EffectiveKind::Spend
            || recurring.contains(&c.txn.normalized_merchant)
        {
            continue;
        }
        let mag = c.txn.amount.amount().abs();
        if mag.is_zero() {
            continue;
        }
        groups
            .entry((c.txn.normalized_merchant.clone(), mag))
            .or_default()
            .push(c);
    }
    let mut out = Vec::new();
    for ((merchant, mag), mut rows) in groups {
        if rows.len() < 2 {
            continue;
        }
        rows.sort_by_key(|c| c.txn.date);
        for w in rows.windows(2) {
            let d = day_diff(w[0].txn.date, w[1].txn.date);
            if (0..=2).contains(&d) {
                let reason = format!(
                    "Possible duplicate: {} charged {} on {} and again on {} ({} day{} apart)",
                    merchant,
                    Money::new(mag),
                    w[0].txn.date,
                    w[1].txn.date,
                    d,
                    if d == 1 { "" } else { "s" },
                );
                out.push(txn_anomaly(
                    "duplicate_charge",
                    w[1],
                    mag,
                    format!("{d}"),
                    reason,
                ));
            }
        }
    }
    out
}

/// A **large** charge from a merchant seen for the first time **recently** — the
/// classic "unfamiliar new payee" cue (new vendor, or fraud). Scoped to the last
/// ~90 days of data and the top of the non-recurring spend distribution (≥P95) so
/// it stays a short, high-signal list. One row per merchant.
fn first_seen_large_anomalies<'a>(
    items: &'a [Categorized<'a>],
    recurring: &BTreeSet<String>,
) -> Vec<Anomaly> {
    let mags = global_spend_mags(items, recurring);
    let p95 = match percentile(&mags, 0.95) {
        Some(v) if !v.is_zero() => v,
        _ => return Vec::new(),
    };
    let cutoff = match max_date(items) {
        Some(d) => d.checked_sub(jiff::Span::new().days(90)).unwrap_or(d),
        None => return Vec::new(),
    };
    // First appearance date per merchant (across all spend, recurring included, so a
    // recurring charge's debut doesn't masquerade as "new").
    let mut first_seen: BTreeMap<String, jiff::civil::Date> = BTreeMap::new();
    for c in items {
        if c.assignment.kind != EffectiveKind::Spend {
            continue;
        }
        first_seen
            .entry(c.txn.normalized_merchant.clone())
            .and_modify(|d| {
                if c.txn.date < *d {
                    *d = c.txn.date;
                }
            })
            .or_insert(c.txn.date);
    }
    let mut flagged: BTreeSet<String> = BTreeSet::new();
    let mut out = Vec::new();
    for c in items {
        if c.assignment.kind != EffectiveKind::Spend
            || recurring.contains(&c.txn.normalized_merchant)
        {
            continue;
        }
        let m = &c.txn.normalized_merchant;
        let mag = c.txn.amount.amount().abs();
        let is_debut = first_seen.get(m) == Some(&c.txn.date);
        if is_debut && c.txn.date >= cutoff && mag >= p95 && flagged.insert(m.clone()) {
            let reason = format!(
                "First charge from {} — and it's a large {} (a new payee in the last 90 days)",
                m,
                Money::new(mag),
            );
            out.push(txn_anomaly(
                "first_seen_large",
                c,
                p95,
                format!("{}", Money::new(mag)),
                reason,
            ));
        }
    }
    out
}

/// A recurring charge whose amount **stepped up** — a subscription price increase
/// or "creep". Detects a clean two-plateau level shift (early-half median vs
/// late-half median ≥15% higher) for a merchant billed on a regular monthly+
/// cadence. One row per merchant, on its latest charge.
fn subscription_price_increase_anomalies<'a>(items: &'a [Categorized<'a>]) -> Vec<Anomaly> {
    let mut by_merchant: BTreeMap<String, Vec<&'a Categorized<'a>>> = BTreeMap::new();
    for c in items {
        if c.assignment.kind == EffectiveKind::Spend {
            by_merchant
                .entry(c.txn.normalized_merchant.clone())
                .or_default()
                .push(c);
        }
    }
    let mut out = Vec::new();
    for (merchant, mut rows) in by_merchant {
        if rows.len() < 4 {
            continue;
        }
        rows.sort_by_key(|c| c.txn.date);
        // Regular monthly+ cadence (median interval 20..400 days) ⇒ subscription-like.
        let mut intervals: Vec<i64> = rows
            .windows(2)
            .map(|w| day_diff(w[0].txn.date, w[1].txn.date))
            .collect();
        intervals.sort_unstable();
        let med_int = intervals[intervals.len() / 2];
        if !(20..=400).contains(&med_int) {
            continue;
        }
        let amounts: Vec<Decimal> = rows.iter().map(|c| c.txn.amount.amount().abs()).collect();
        let mid = amounts.len() / 2;
        let mut early: Vec<Decimal> = amounts[..mid].to_vec();
        let mut late: Vec<Decimal> = amounts[mid..].to_vec();
        early.sort();
        late.sort();
        let (early_med, late_med) = match (median(&early), median(&late)) {
            (Some(a), Some(b)) if !a.is_zero() => (a, b),
            _ => continue,
        };
        let ratio = (late_med / early_med).to_f64().unwrap_or(1.0);
        if ratio < 1.15 {
            continue;
        }
        // Clean plateaus: the early half stays near its median, the late half too.
        let early_max = *early.last().unwrap();
        let late_min = late[0];
        if early_max > early_med * dec!(1.15) || late_min < late_med * dec!(0.85) {
            continue;
        }
        let last = *rows.last().unwrap();
        let pct = (ratio - 1.0) * 100.0;
        let reason = format!(
            "{} went from ~{} to ~{} (+{:.0}%) — a recurring price increase",
            merchant,
            Money::new(early_med),
            Money::new(late_med),
            pct,
        );
        out.push(txn_anomaly(
            "subscription_increase",
            last,
            early_med,
            format!("{pct:.1}"),
            reason,
        ));
    }
    out
}

// ---------------------------------------------------------------------------
// Tier 2: card-testing velocity, structuring, round-number, Benford
// ---------------------------------------------------------------------------

/// A burst of tiny charges in a short window — the fingerprint of automated card
/// testing. ≥3 small (≤ max($5, 0.2·median) ) non-recurring charges within 2 days
/// on one account. Flags the last charge of each burst.
fn card_testing_anomalies<'a>(
    items: &'a [Categorized<'a>],
    recurring: &BTreeSet<String>,
) -> Vec<Anomaly> {
    let mags = global_spend_mags(items, recurring);
    let med = median(&mags).unwrap_or(Decimal::ZERO);
    let small = dec!(5).max(med * dec!(0.2));
    // account -> small charges
    let mut by_acct: BTreeMap<String, Vec<&'a Categorized<'a>>> = BTreeMap::new();
    for c in items {
        if c.assignment.kind != EffectiveKind::Spend
            || recurring.contains(&c.txn.normalized_merchant)
        {
            continue;
        }
        let mag = c.txn.amount.amount().abs();
        if mag.is_zero() || mag > small {
            continue;
        }
        let acct = c
            .txn
            .account_id
            .clone()
            .unwrap_or_else(|| "unassigned".into());
        by_acct.entry(acct).or_default().push(c);
    }
    let mut out = Vec::new();
    for (_acct, mut rows) in by_acct {
        if rows.len() < 3 {
            continue;
        }
        rows.sort_by_key(|c| c.txn.date);
        // Greedy clusters: consecutive charges ≤2 days apart.
        let mut i = 0;
        while i < rows.len() {
            let mut j = i;
            while j + 1 < rows.len() && day_diff(rows[j].txn.date, rows[j + 1].txn.date) <= 2 {
                j += 1;
            }
            let cluster = &rows[i..=j];
            // Card testing cycles through *different* merchants/sites; require ≥3
            // distinct merchants so a legitimate repeat (three coffees at one shop)
            // isn't mistaken for a testing burst.
            let distinct: BTreeSet<&str> = cluster
                .iter()
                .map(|c| c.txn.normalized_merchant.as_str())
                .collect();
            if cluster.len() >= 3 && distinct.len() >= 3 {
                let total: Decimal = cluster.iter().map(|c| c.txn.amount.amount().abs()).sum();
                let last = cluster[cluster.len() - 1];
                let reason = format!(
                    "{} small charges across {} different merchants totalling {} between {} and {} — looks like card testing",
                    cluster.len(),
                    distinct.len(),
                    Money::new(total),
                    cluster[0].txn.date,
                    last.txn.date,
                );
                out.push(txn_anomaly(
                    "card_testing",
                    last,
                    small,
                    format!("{}", cluster.len()),
                    reason,
                ));
            }
            i = j + 1;
        }
    }
    out
}

/// Transactions clustering **just below** a reporting threshold — the shape of
/// "structuring" (splitting amounts to stay under a report). Flags each member of a
/// cluster of ≥2 near-threshold (`[0.9·T, T)`) transactions by the same person
/// within 30 days. Business/public profiles only.
fn structuring_anomalies<'a>(items: &'a [Categorized<'a>], threshold: Decimal) -> Vec<Anomaly> {
    if threshold <= Decimal::ZERO {
        return Vec::new();
    }
    let low = threshold * dec!(0.9);
    let mut by_person: BTreeMap<String, Vec<&'a Categorized<'a>>> = BTreeMap::new();
    for c in items {
        let mag = c.txn.amount.amount().abs();
        if mag >= low && mag < threshold {
            let p = c
                .txn
                .owner_person
                .clone()
                .unwrap_or_else(|| "unassigned".into());
            by_person.entry(p).or_default().push(c);
        }
    }
    let mut out = Vec::new();
    for (_p, mut rows) in by_person {
        if rows.len() < 2 {
            continue;
        }
        rows.sort_by_key(|c| c.txn.date);
        // A txn is "in a cluster" if it has a peer within 30 days.
        for (i, c) in rows.iter().enumerate() {
            let has_peer = rows
                .iter()
                .enumerate()
                .any(|(k, o)| k != i && day_diff(o.txn.date, c.txn.date).abs() <= 30);
            if has_peer {
                let mag = c.txn.amount.amount().abs();
                let reason = format!(
                    "{} sits just under the {} reporting threshold — repeated near-threshold amounts can indicate structuring",
                    Money::new(mag),
                    Money::new(threshold),
                );
                let ratio = (mag / threshold).to_f64().unwrap_or(0.0);
                out.push(txn_anomaly(
                    "structuring",
                    c,
                    threshold,
                    format!("{ratio:.3}"),
                    reason,
                ));
            }
        }
    }
    out
}

/// Counterparties that receive **repeated** round-number transfers (`≥ $500`,
/// divisible by 100) — informal payments or gifts that draw scrutiny for public
/// figures. Aggregated per counterparty (≥3 such transfers) and excluding recurring
/// payees, so a single round transfer or a known monthly bill isn't noise. One row
/// per counterparty, on its latest transfer. Business/public only.
fn round_number_transfer_anomalies<'a>(
    items: &'a [Categorized<'a>],
    recurring: &BTreeSet<String>,
) -> Vec<Anomaly> {
    let mut by: BTreeMap<String, Vec<&'a Categorized<'a>>> = BTreeMap::new();
    for c in items {
        if c.assignment.kind != EffectiveKind::Transfer
            || recurring.contains(&c.txn.normalized_merchant)
        {
            continue;
        }
        let mag = c.txn.amount.amount().abs();
        if mag >= dec!(500) && (mag % dec!(100)).is_zero() {
            by.entry(c.txn.normalized_merchant.clone())
                .or_default()
                .push(c);
        }
    }
    let mut out = Vec::new();
    for (merchant, mut rows) in by {
        if rows.len() < 3 {
            continue;
        }
        rows.sort_by_key(|c| c.txn.date);
        // Skip steady standing transfers (a regular monthly/biweekly cadence is a
        // known arrangement — rent, a savings sweep, a card payment — not an
        // irregular round payment worth scrutiny). recurring detection doesn't see
        // transfers, so judge the cadence here.
        if is_regular_cadence(&rows) {
            continue;
        }
        let total: Decimal = rows.iter().map(|c| c.txn.amount.amount().abs()).sum();
        let last = *rows.last().unwrap();
        let reason = format!(
            "{} received {} irregular round-number transfers totalling {} — round amounts to one counterparty are worth a second look",
            merchant,
            rows.len(),
            Money::new(total),
        );
        out.push(txn_anomaly(
            "round_number",
            last,
            dec!(0),
            format!("{}", rows.len()),
            reason,
        ));
    }
    out
}

/// Benford's-law conformity of leading digits over spend magnitudes. Fabricated or
/// heavily-rounded amounts skew the natural `1`→30% … `9`→4.6% curve. Dataset-wide;
/// needs ≥100 values. Emits one anomaly when the χ² statistic clears a conservative
/// bar (well above the p=0.05 critical 15.51) and names the most over-represented
/// digit. Business/public only.
fn benford_anomalies(items: &[Categorized<'_>]) -> Vec<Anomaly> {
    let mut counts = [0u64; 10];
    let mut total = 0u64;
    for c in items {
        if c.assignment.kind != EffectiveKind::Spend {
            continue;
        }
        let mut f = match c.txn.amount.amount().abs().to_f64() {
            Some(f) if f > 0.0 => f,
            _ => continue,
        };
        while f >= 10.0 {
            f /= 10.0;
        }
        while f < 1.0 {
            f *= 10.0;
        }
        let d = f.floor() as usize;
        if (1..=9).contains(&d) {
            counts[d] += 1;
            total += 1;
        }
    }
    if total < 100 {
        return Vec::new();
    }
    let n = total as f64;
    let mut chi2 = 0.0;
    let mut worst = (0usize, 0.0f64); // (digit, observed-minus-expected fraction)
    for (d, &count) in counts.iter().enumerate().take(10).skip(1) {
        let expected_frac = (1.0 + 1.0 / d as f64).log10();
        let expected = expected_frac * n;
        let obs = count as f64;
        chi2 += (obs - expected).powi(2) / expected;
        let over = obs / n - expected_frac;
        if over > worst.1 {
            worst = (d, over);
        }
    }
    // Conservative bar: p=0.05 χ²(8 dof) is 15.51; require clearly more to avoid
    // false alarms on naturally-non-Benford personal data.
    if chi2 < 20.0 {
        return Vec::new();
    }
    let d = worst.0;
    let obs_pct = counts[d] as f64 / n * 100.0;
    let exp_pct = (1.0 + 1.0 / d as f64).log10() * 100.0;
    let reason = format!(
        "Leading-digit distribution deviates from Benford's law (χ²={chi2:.0} over {total} amounts); digit {d} appears {obs_pct:.0}% vs an expected {exp_pct:.0}% — worth checking for fabricated or rounded amounts"
    );
    vec![Anomaly {
        method: "benford".into(),
        scope: "dataset".into(),
        category: String::new(),
        subcategory: None,
        period_or_date: "all".into(),
        amount: Money::new(Decimal::from(total)),
        baseline: Money::new(Decimal::ZERO),
        severity: format!("{chi2:.1}"),
        reason,
        merchant: None,
        person: None,
        account: None,
    }]
}

// ---------------------------------------------------------------------------
// Tier 3: change-point, seasonal residual, cash-flow runway
// ---------------------------------------------------------------------------

/// Monthly totals (sorted) of `mags` for items matching `kind`.
fn monthly_totals(items: &[Categorized<'_>], kind: EffectiveKind) -> Vec<(String, Decimal)> {
    let mut by_month: BTreeMap<String, Decimal> = BTreeMap::new();
    for c in items {
        if c.assignment.kind == kind {
            *by_month
                .entry(month_key(c.txn.date))
                .or_insert(Decimal::ZERO) += c.txn.amount.amount().abs();
        }
    }
    by_month.into_iter().collect()
}

/// A sustained shift in overall monthly spending (a regime change, not a one-month
/// spike): the split that maximises the gap between the mean before and after, when
/// that gap is ≥25% and each side has ≥3 months. Emits the single biggest change.
fn change_point_anomalies(items: &[Categorized<'_>]) -> Vec<Anomaly> {
    let series = monthly_totals(items, EffectiveKind::Spend);
    if series.len() < 8 {
        return Vec::new();
    }
    let vals: Vec<Decimal> = series.iter().map(|(_, v)| *v).collect();
    let mean = |s: &[Decimal]| -> Decimal {
        if s.is_empty() {
            Decimal::ZERO
        } else {
            s.iter().copied().sum::<Decimal>() / Decimal::from(s.len() as u64)
        }
    };
    let mut best: Option<(usize, Decimal, Decimal, f64)> = None; // (k, before, after, |rel|)
    for k in 3..=series.len() - 3 {
        let before = mean(&vals[..k]);
        let after = mean(&vals[k..]);
        if before.is_zero() {
            continue;
        }
        let rel = ((after - before) / before).to_f64().unwrap_or(0.0);
        if best.is_none() || rel.abs() > best.unwrap().3 {
            best = Some((k, before, after, rel.abs()));
        }
    }
    let (k, before, after, rel) = match best {
        Some(b) if b.3 >= 0.25 => b,
        _ => return Vec::new(),
    };
    let month = series[k].0.clone();
    let dir = if after > before { "up" } else { "down" };
    let reason = format!(
        "Monthly spending shifted {} around {}: from ~{}/mo to ~{}/mo ({:+.0}%)",
        dir,
        month,
        Money::new(before),
        Money::new(after),
        rel * 100.0 * if after > before { 1.0 } else { -1.0 },
    );
    vec![Anomaly {
        method: "change_point".into(),
        scope: "month".into(),
        category: String::new(),
        subcategory: None,
        period_or_date: month,
        amount: Money::new(after),
        baseline: Money::new(before),
        severity: format!("{:.2}", rel),
        reason,
        merchant: None,
        person: None,
        account: None,
    }]
}

/// Season-adjusted month outliers: a month that's high **even for that time of
/// year**. Builds a month-of-year index (median per calendar month across years),
/// deseasonalises, and flags residuals beyond a robust 3.5σ. Needs ≥24 months of a
/// category's history. Complements `monthly_iqr`, which is season-blind.
fn seasonal_residual_anomalies(
    items: &[Categorized<'_>],
    recurring: &BTreeSet<String>,
) -> Vec<Anomaly> {
    // category -> month-key -> total (discretionary only — recurring excluded so a
    // rent/bill level-shift doesn't read as a string of seasonal anomalies).
    let mut by_cat: BTreeMap<String, BTreeMap<String, Decimal>> = BTreeMap::new();
    for c in items {
        if c.assignment.kind == EffectiveKind::Spend
            && !recurring.contains(&c.txn.normalized_merchant)
        {
            *by_cat
                .entry(c.category_label().to_string())
                .or_default()
                .entry(month_key(c.txn.date))
                .or_insert(Decimal::ZERO) += c.txn.amount.amount().abs();
        }
    }
    let mut out = Vec::new();
    for (category, months) in by_cat {
        if months.len() < 24 {
            continue;
        }
        // Month-of-year (01..12) → values across years.
        let mut by_moy: BTreeMap<String, Vec<Decimal>> = BTreeMap::new();
        for (mk, v) in &months {
            let moy = mk.split('-').nth(1).unwrap_or("00").to_string();
            by_moy.entry(moy).or_default().push(*v);
        }
        let seasonal: BTreeMap<String, Decimal> = by_moy
            .into_iter()
            .filter_map(|(moy, mut vs)| {
                vs.sort();
                median(&vs).map(|m| (moy, m))
            })
            .collect();
        // Residuals = value − month-of-year index.
        let mut resid: Vec<(String, Decimal, Decimal)> = Vec::new(); // (month, value, residual)
        for (mk, v) in &months {
            let moy = mk.split('-').nth(1).unwrap_or("00");
            if let Some(idx) = seasonal.get(moy) {
                resid.push((mk.clone(), *v, *v - *idx));
            }
        }
        let rvals: Vec<Decimal> = resid.iter().map(|(_, _, r)| *r).collect();
        let (rmed, rmad) = match (median(&rvals), mad(&rvals)) {
            (Some(m), Some(d)) if !d.is_zero() => (m, d),
            _ => continue,
        };
        // Collect candidates, then keep only the top 2 by z per category — a
        // sustained level shift would otherwise flag every month of the new regime
        // (that's a change-point, reported separately), drowning real seasonal spikes.
        let mut cands: Vec<(String, Decimal, Decimal, f64)> = Vec::new(); // (month, value, idx, z)
        for (mk, v, r) in &resid {
            if let Some(z) = mad_zscore(*r, rmed, rmad) {
                if z > 3.5 && *r > Decimal::ZERO {
                    cands.push((mk.clone(), *v, v - r, z));
                }
            }
        }
        cands.sort_by(|a, b| b.3.partial_cmp(&a.3).unwrap_or(std::cmp::Ordering::Equal));
        cands.truncate(2);
        for (mk, v, idx, z) in cands {
            let reason = format!(
                "{} spending of {} in {} is high even for that time of year (seasonal norm ~{})",
                category,
                Money::new(v),
                mk,
                Money::new(idx),
            );
            out.push(Anomaly {
                method: "seasonal_residual".into(),
                scope: "month".into(),
                category: category.clone(),
                subcategory: None,
                period_or_date: mk,
                amount: Money::new(v),
                baseline: Money::new(idx),
                severity: format!("{z:.2}"),
                reason,
                merchant: None,
                person: None,
                account: None,
            });
        }
    }
    out
}

/// A run of consecutive months where spending exceeded income — a cash-flow /
/// runway warning. Emits the longest deficit streak of ≥3 months.
fn cashflow_runway_anomalies(items: &[Categorized<'_>]) -> Vec<Anomaly> {
    let mut by_month: BTreeMap<String, (Decimal, Decimal)> = BTreeMap::new(); // (income, spend)
    for c in items {
        let e = by_month
            .entry(month_key(c.txn.date))
            .or_insert((Decimal::ZERO, Decimal::ZERO));
        match c.assignment.kind {
            EffectiveKind::Income => e.0 += c.txn.amount.amount().abs(),
            EffectiveKind::Spend => e.1 += c.txn.amount.amount().abs(),
            _ => {}
        }
    }
    let series: Vec<(String, Decimal)> = by_month
        .into_iter()
        .map(|(m, (inc, spend))| (m, inc - spend))
        .collect();
    if series.len() < 3 {
        return Vec::new();
    }
    // Longest run of consecutive negative-net months.
    let (mut best_start, mut best_len) = (0usize, 0usize);
    let (mut cur_start, mut cur_len) = (0usize, 0usize);
    for (i, (_, net)) in series.iter().enumerate() {
        if *net < Decimal::ZERO {
            if cur_len == 0 {
                cur_start = i;
            }
            cur_len += 1;
            if cur_len > best_len {
                best_len = cur_len;
                best_start = cur_start;
            }
        } else {
            cur_len = 0;
        }
    }
    if best_len < 3 {
        return Vec::new();
    }
    let start = series[best_start].0.clone();
    let end = series[best_start + best_len - 1].0.clone();
    let deficit: Decimal = series[best_start..best_start + best_len]
        .iter()
        .map(|(_, n)| -*n)
        .sum();
    let reason = format!(
        "Spending exceeded income for {} months in a row ({}–{}), a cumulative {} shortfall",
        best_len,
        start,
        end,
        Money::new(deficit),
    );
    vec![Anomaly {
        method: "cashflow_runway".into(),
        scope: "dataset".into(),
        category: String::new(),
        subcategory: None,
        period_or_date: format!("{start}…{end}"),
        amount: Money::new(deficit),
        baseline: Money::new(Decimal::ZERO),
        severity: format!("{best_len}"),
        reason,
        merchant: None,
        person: None,
        account: None,
    }]
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

    /// Consumer-profile settings (forensic detectors off) for the default tests.
    fn settings() -> AnalysisSettings {
        AnalysisSettings::default()
    }

    /// Public-profile settings (all detectors on) for forensic tests.
    fn settings_public() -> AnalysisSettings {
        AnalysisSettings {
            profile: crate::config::AnalysisProfile::Public,
            structuring_threshold: dec!(10000),
        }
    }

    /// Build a transfer transaction (for structuring / round-number tests).
    fn transfer(merchant: &str, amount: &str, d: Date) -> (Transaction, Assignment) {
        let (mut t, mut a) = spend_full("Transfers", "Move", merchant, amount, d);
        t.txn_type = TxnType::Transfer;
        a.kind = EffectiveKind::Transfer;
        (t, a)
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
        let a = detect(&view(&pairs), &no_recurring(), &settings());
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
        let a = detect(&view(&pairs), &no_recurring(), &settings());
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
        let a = detect(&view(&pairs), &recurring, &settings());
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
        let a = detect(&view(&pairs), &no_recurring(), &settings());
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
        let a = detect(&view(&pairs), &no_recurring(), &settings());
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
        let a = detect(&view(&pairs), &no_recurring(), &settings());
        assert!(a
            .iter()
            .any(|x| x.method == "monthly_iqr" && x.period_or_date == "2026-05"));
    }

    #[test]
    fn no_amount_outlier_in_uniform_data() {
        // Uniform amounts spaced a month apart (so the duplicate-charge pass, which
        // legitimately fires on identical same-week charges, stays quiet). The
        // amount-outlier passes must find nothing.
        let mut pairs = vec![];
        for m in 1..=10i8 {
            pairs.push(spend("Groceries", "-50.00", date(2026, m, 15)));
        }
        let a = detect(&view(&pairs), &no_recurring(), &settings());
        assert!(
            !a.iter().any(|x| {
                matches!(
                    x.method.as_str(),
                    "mad_zscore" | "rare_large" | "monthly_iqr"
                )
            }),
            "uniform data must not produce an amount outlier, got: {a:?}"
        );
    }

    #[test]
    fn flags_duplicate_charge() {
        let mut pairs = vec![
            spend_named("Shop", "Online", "ACME", "-42.00", date(2026, 3, 10)),
            spend_named("Shop", "Online", "ACME", "-42.00", date(2026, 3, 11)), // dup next day
            spend_named("Shop", "Online", "ACME", "-9.00", date(2026, 3, 20)),
        ];
        pairs.push(spend_named(
            "Shop",
            "Online",
            "OTHER",
            "-42.00",
            date(2026, 3, 11),
        ));
        let a = detect(&view(&pairs), &no_recurring(), &settings());
        assert!(
            a.iter()
                .any(|x| x.method == "duplicate_charge" && x.merchant.as_deref() == Some("ACME")),
            "duplicate ACME charge should be flagged, got: {a:?}"
        );
    }

    #[test]
    fn flags_subscription_price_increase() {
        // Monthly charges that step from $10 to $15 halfway through.
        let mut pairs = vec![];
        for m in 1..=4i8 {
            pairs.push(spend_named(
                "Subs",
                "Streaming",
                "STREAMR",
                "-10.00",
                date(2026, m, 1),
            ));
        }
        for m in 5..=8i8 {
            pairs.push(spend_named(
                "Subs",
                "Streaming",
                "STREAMR",
                "-15.00",
                date(2026, m, 1),
            ));
        }
        let a = detect(&view(&pairs), &no_recurring(), &settings());
        assert!(
            a.iter()
                .any(|x| x.method == "subscription_increase"
                    && x.merchant.as_deref() == Some("STREAMR")),
            "price increase should be flagged, got: {a:?}"
        );
    }

    #[test]
    fn flags_card_testing_burst() {
        let mut pairs = vec![];
        // Normal spend pool so the "small" threshold is meaningful.
        for i in 1..=12 {
            pairs.push(spend_named(
                "Food",
                "Groceries",
                "MART",
                "-40.00",
                date(2026, 1, i),
            ));
        }
        // A burst of tiny charges over two days on the same (default None) account.
        for (i, m) in ["A", "B", "C", "D"].iter().enumerate() {
            pairs.push(spend_named(
                "Misc",
                "Tiny",
                m,
                "-1.00",
                date(2026, 2, 1 + (i as i8) / 2),
            ));
        }
        let a = detect(&view(&pairs), &no_recurring(), &settings());
        assert!(
            a.iter().any(|x| x.method == "card_testing"),
            "card-testing burst should be flagged, got: {a:?}"
        );
    }

    #[test]
    fn structuring_only_for_forensic_profile_and_needs_a_cluster() {
        let pairs = vec![
            transfer("WIRE", "-9500.00", date(2026, 1, 5)),
            transfer("WIRE", "-9600.00", date(2026, 1, 20)),
        ];
        // Consumer profile: forensic detectors off.
        assert!(
            !detect(&view(&pairs), &no_recurring(), &settings())
                .iter()
                .any(|x| x.method == "structuring"),
            "consumer profile must not run structuring"
        );
        // Public profile: the near-threshold cluster is flagged.
        let a = detect(&view(&pairs), &no_recurring(), &settings_public());
        assert!(
            a.iter().filter(|x| x.method == "structuring").count() >= 2,
            "public profile flags the near-threshold cluster, got: {a:?}"
        );
        // A single near-threshold transfer is NOT structuring.
        let lone = vec![transfer("WIRE", "-9500.00", date(2026, 1, 5))];
        assert!(
            !detect(&view(&lone), &no_recurring(), &settings_public())
                .iter()
                .any(|x| x.method == "structuring"),
            "a lone near-threshold txn is not structuring"
        );
    }

    #[test]
    fn flags_cashflow_deficit_streak() {
        // Income one month, then four straight months of pure spending.
        let mut pairs = vec![spend_full(
            "Income",
            "Salary",
            "ACME PAY",
            "5000.00",
            date(2026, 1, 1),
        )];
        if let Some(p) = pairs.last_mut() {
            p.0.txn_type = TxnType::Income;
            p.1.kind = EffectiveKind::Income;
        }
        for m in 2..=6i8 {
            pairs.push(spend("Food", "-800.00", date(2026, m, 10)));
        }
        let a = detect(&view(&pairs), &no_recurring(), &settings());
        assert!(
            a.iter().any(|x| x.method == "cashflow_runway"),
            "a multi-month deficit streak should be flagged, got: {a:?}"
        );
    }
}
