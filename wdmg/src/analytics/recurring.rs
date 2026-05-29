//! Recurring-charge / subscription detection.
//!
//! A normalized-merchant group is flagged recurring when all hold:
//! - at least 3 occurrences,
//! - the median inter-transaction interval (in days) clusters near a known
//!   cadence in `{7, 14, 30/31, 365}` within a tolerance, and
//! - the amount is stable: coefficient of variation `< 0.15`.
//!
//! Amounts stay in [`Decimal`]; the CoV is the only float (a dimensionless ratio,
//! per [`super::stats`]). Spend magnitudes are used so a $-9.99 subscription is
//! treated as $9.99.

use jiff::civil::Date;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::stats::{coefficient_of_variation, median};
use super::Categorized;
use crate::categorize::EffectiveKind;
use crate::model::Money;

/// CoV ceiling for "stable amount".
const COV_CEILING: f64 = 0.15;

/// Known cadences as `(label, days, tolerance_days)`.
const CADENCES: &[(&str, i64, i64)] = &[
    ("weekly", 7, 2),
    ("biweekly", 14, 3),
    ("monthly", 30, 4),
    ("yearly", 365, 21),
];

/// One detected recurring charge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recurring {
    /// Normalized merchant.
    pub merchant: String,
    /// Cadence label (`weekly`/`biweekly`/`monthly`/`yearly`).
    pub cadence: String,
    /// Median interval in days.
    pub interval_days: i64,
    /// Typical (median) amount magnitude.
    pub typical_amount: Money,
    /// Number of occurrences observed.
    pub occurrences: usize,
    /// Amount normalized to a monthly figure (`typical * 30 / interval`).
    pub monthly_equivalent: Money,
    /// Predicted next charge date (`last + median interval`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub predicted_next_date: Option<String>,
}

/// Detect recurring charges among spend items.
#[must_use]
pub fn detect(items: &[Categorized<'_>]) -> Vec<Recurring> {
    // Group spend by normalized merchant: collect (date, magnitude).
    let mut groups: std::collections::BTreeMap<String, Vec<(Date, Decimal)>> =
        std::collections::BTreeMap::new();
    for c in items {
        if c.assignment.kind != EffectiveKind::Spend {
            continue;
        }
        groups
            .entry(c.txn.normalized_merchant.clone())
            .or_default()
            .push((c.txn.date, c.txn.amount.amount().abs()));
    }

    let mut out = Vec::new();
    for (merchant, mut occ) in groups {
        if occ.len() < 3 {
            continue;
        }
        occ.sort_by_key(|(d, _)| *d);

        // Inter-transaction intervals in days.
        let intervals: Vec<Decimal> = occ
            .windows(2)
            .map(|w| {
                let days = w[1]
                    .0
                    .since(w[0].0)
                    .map(|s| s.get_days() as i64)
                    .unwrap_or(0);
                Decimal::from(days)
            })
            .collect();
        let median_interval = match median(&intervals) {
            Some(m) => m,
            None => continue,
        };
        let median_days = median_interval.round().try_into().unwrap_or(0i64);

        // Snap to a known cadence within tolerance.
        let cadence = CADENCES
            .iter()
            .find(|(_, days, tol)| (median_days - days).abs() <= *tol);
        let (label, _, _) = match cadence {
            Some(c) => *c,
            None => continue,
        };

        // Amount stability.
        let amounts: Vec<Decimal> = occ.iter().map(|(_, a)| *a).collect();
        let cov = match coefficient_of_variation(&amounts) {
            Some(c) => c,
            None => continue,
        };
        if cov >= COV_CEILING {
            continue;
        }

        let typical = median(&amounts).unwrap_or(Decimal::ZERO);
        let monthly_equiv = if median_days > 0 {
            (typical * Decimal::from(30u32) / Decimal::from(median_days as u64)).round_dp(2)
        } else {
            typical
        };
        let predicted_next = occ
            .last()
            .and_then(|(last, _)| last.checked_add(jiff::Span::new().days(median_days)).ok())
            .map(|d| d.to_string());

        out.push(Recurring {
            merchant,
            cadence: label.to_string(),
            interval_days: median_days,
            typical_amount: Money::new(typical),
            occurrences: occ.len(),
            monthly_equivalent: Money::new(monthly_equiv),
            predicted_next_date: predicted_next,
        });
    }

    // Deterministic: by descending monthly_equivalent, then merchant.
    out.sort_by(|a, b| {
        b.monthly_equivalent
            .amount()
            .cmp(&a.monthly_equivalent.amount())
            .then_with(|| a.merchant.cmp(&b.merchant))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::categorize::Assignment;
    use crate::model::{Currency, Transaction, TxnType};
    use jiff::civil::date;
    use std::str::FromStr;

    fn spend(merchant: &str, amount: &str, d: Date) -> (Transaction, Assignment) {
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
            category: Some("Subscriptions".into()),
            subcategory: Some("Digital".into()),
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
    fn detects_monthly_subscription() {
        let pairs = vec![
            spend("NETFLIX", "-16.99", date(2026, 1, 5)),
            spend("NETFLIX", "-16.99", date(2026, 2, 5)),
            spend("NETFLIX", "-16.99", date(2026, 3, 5)),
            spend("NETFLIX", "-16.99", date(2026, 4, 5)),
        ];
        let r = detect(&view(&pairs));
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].merchant, "NETFLIX");
        assert_eq!(r[0].cadence, "monthly");
        assert_eq!(r[0].occurrences, 4);
        assert_eq!(
            r[0].typical_amount.amount(),
            rust_decimal_macros::dec!(16.99)
        );
        assert!(r[0].predicted_next_date.is_some());
    }

    #[test]
    fn ignores_irregular_amounts() {
        // Same cadence but wildly varying amounts -> not flagged.
        let pairs = vec![
            spend("SHOP", "-5.00", date(2026, 1, 5)),
            spend("SHOP", "-500.00", date(2026, 2, 5)),
            spend("SHOP", "-50.00", date(2026, 3, 5)),
        ];
        assert!(detect(&view(&pairs)).is_empty());
    }

    #[test]
    fn ignores_too_few_occurrences() {
        let pairs = vec![
            spend("RARE", "-9.99", date(2026, 1, 5)),
            spend("RARE", "-9.99", date(2026, 2, 5)),
        ];
        assert!(detect(&view(&pairs)).is_empty());
    }
}
