//! Anomaly detection — two complementary, robust methods.
//!
//! - `mad_zscore`: a single transaction whose magnitude is a robust outlier
//!   within its category, `|x - median| / (1.4826 * MAD) > 3.5`. Robust because
//!   the median/MAD are not dragged by the very outlier being detected.
//! - `monthly_iqr`: a whole month whose category spend exceeds `Q3 + 1.5*IQR` of
//!   that category's monthly history — distinguishing one big purchase from a
//!   broadly expensive month.
//!
//! Magnitudes are exact [`Decimal`]; only the z-score ratio is `f64` (per
//! [`super::stats`]). Each anomaly carries its `method`, `baseline`, and a
//! `severity` (the z-score, or the overage ratio over the IQR fence).

use std::collections::BTreeMap;

use rust_decimal::dec;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::stats::{mad, mad_zscore, median, quartiles};
use super::{month_key, Categorized};
use crate::categorize::EffectiveKind;
use crate::model::Money;

/// MAD z-score threshold for a single-transaction outlier.
const Z_THRESHOLD: f64 = 3.5;

/// One detected anomaly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Anomaly {
    /// Detection method (`mad_zscore` | `monthly_iqr`).
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
#[must_use]
pub fn detect(items: &[Categorized<'_>]) -> Vec<Anomaly> {
    let mut out = Vec::new();
    out.extend(mad_zscore_anomalies(items));
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

/// Per-category single-transaction MAD z-score outliers.
fn mad_zscore_anomalies(items: &[Categorized<'_>]) -> Vec<Anomaly> {
    // category -> Vec<(magnitude, date, merchant)>
    let mut by_cat: BTreeMap<String, Vec<(Decimal, jiff::civil::Date, String)>> = BTreeMap::new();
    for c in items {
        if c.assignment.kind != EffectiveKind::Spend {
            continue;
        }
        by_cat
            .entry(c.category_label().to_string())
            .or_default()
            .push((
                c.txn.amount.amount().abs(),
                c.txn.date,
                c.txn.normalized_merchant.clone(),
            ));
    }

    let mut out = Vec::new();
    for (category, rows) in by_cat {
        if rows.len() < 4 {
            // Too few points for a meaningful robust estimate.
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
                        category: category.clone(),
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

    fn spend(cat: &str, amount: &str, d: Date) -> (Transaction, Assignment) {
        let t = Transaction {
            raw_description: "M".into(),
            normalized_merchant: "M".into(),
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
            subcategory: Some("S".into()),
            kind: EffectiveKind::Spend,
            needs_review: false,
        };
        (t, a)
    }

    fn view<'a>(p: &'a [(Transaction, Assignment)]) -> Vec<Categorized<'a>> {
        p.iter()
            .map(|(t, a)| Categorized {
                txn: t,
                assignment: a,
            })
            .collect()
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
        let a = detect(&view(&pairs));
        assert!(a
            .iter()
            .any(|x| x.method == "mad_zscore"
                && x.amount.amount() == rust_decimal_macros::dec!(5000.00)));
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
        let a = detect(&view(&pairs));
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
        assert!(detect(&view(&pairs)).is_empty());
    }
}
