//! Per-category spending trends.
//!
//! For each spend category we compute:
//! - month-over-month delta (last month vs. the prior month),
//! - year-over-year delta (last month vs. the same month a year earlier), and
//! - a `direction` (rising/falling/flat) from the sign of a least-squares slope
//!   over the category's monthly totals.
//!
//! Deltas are exact [`Decimal`] (absolute and percentage); the slope is the only
//! float, used solely to read off a sign (per [`super::stats`]).

use std::collections::BTreeMap;

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::stats::least_squares_slope;
use super::{month_key, Categorized};
use crate::categorize::EffectiveKind;
use crate::model::Money;

/// Slope magnitude below which a trend is considered flat (dollars/month).
const FLAT_SLOPE_EPS: f64 = 1.0;

/// A per-category trend summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trend {
    /// Category name.
    pub category: String,
    /// Month-over-month absolute change (last vs. prior month).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mom_abs: Option<Money>,
    /// Month-over-month percentage change, as a decimal string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mom_pct: Option<String>,
    /// Year-over-year absolute change (last vs. same month a year prior).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub yoy_abs: Option<Money>,
    /// Year-over-year percentage change, as a decimal string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub yoy_pct: Option<String>,
    /// Overall direction from the least-squares slope.
    pub direction: String,
}

/// Compute trends for each spend category.
#[must_use]
pub fn detect_for(items: &[Categorized<'_>]) -> Vec<Trend> {
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
        // Chronological months (BTreeMap key order is chronological for YYYY-MM).
        let ordered: Vec<(String, Decimal)> = months.into_iter().collect();
        if ordered.is_empty() {
            continue;
        }

        let (last_month, last_total) = ordered.last().cloned().unwrap_or_default();

        // MoM: last vs. the immediately preceding month in the series.
        let (mom_abs, mom_pct) = if ordered.len() >= 2 {
            let prev = ordered[ordered.len() - 2].1;
            delta(last_total, prev)
        } else {
            (None, None)
        };

        // YoY: last vs. the same calendar month one year earlier, if present.
        let yoy_key = prior_year_key(&last_month);
        let (yoy_abs, yoy_pct) =
            match yoy_key.and_then(|k| ordered.iter().find(|(m, _)| *m == k).map(|(_, t)| *t)) {
                Some(prior) => delta(last_total, prior),
                None => (None, None),
            };

        // Direction from least-squares slope over the monthly totals.
        let xs: Vec<Decimal> = (0..ordered.len())
            .map(|i| Decimal::from(i as u64))
            .collect();
        let ys: Vec<Decimal> = ordered.iter().map(|(_, t)| *t).collect();
        let direction = match least_squares_slope(&xs, &ys) {
            Some(s) if s > FLAT_SLOPE_EPS => "rising",
            Some(s) if s < -FLAT_SLOPE_EPS => "falling",
            _ => "flat",
        }
        .to_string();

        out.push(Trend {
            category,
            mom_abs,
            mom_pct,
            yoy_abs,
            yoy_pct,
            direction,
        });
    }

    out.sort_by(|a, b| a.category.cmp(&b.category));
    out
}

/// Public entry point used by [`super::compute`].
#[must_use]
pub fn compute(items: &[Categorized<'_>]) -> Vec<Trend> {
    detect_for(items)
}

/// Absolute and percentage delta of `current` over `prior`.
fn delta(current: Decimal, prior: Decimal) -> (Option<Money>, Option<String>) {
    let abs = current - prior;
    let pct = if prior.is_zero() {
        None
    } else {
        (abs / prior * Decimal::from(100u32))
            .round_dp(2)
            .to_string()
            .into()
    };
    (Some(Money::new(abs)), pct)
}

/// Given a `YYYY-MM` key, the same month one year earlier.
fn prior_year_key(month: &str) -> Option<String> {
    let (y, m) = month.split_once('-')?;
    let year: i32 = y.parse().ok()?;
    Some(format!("{:04}-{}", year - 1, m))
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
    fn rising_trend_detected() {
        let pairs = vec![
            spend("Groceries", "-100.00", date(2026, 1, 1)),
            spend("Groceries", "-200.00", date(2026, 2, 1)),
            spend("Groceries", "-300.00", date(2026, 3, 1)),
        ];
        let t = detect_for(&view(&pairs));
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].direction, "rising");
        // MoM: 300 vs 200 -> +100, +50%
        assert_eq!(
            t[0].mom_abs.unwrap().amount(),
            rust_decimal_macros::dec!(100.00)
        );
        assert_eq!(t[0].mom_pct.as_deref(), Some("50.00"));
    }

    #[test]
    fn yoy_delta_when_prior_year_present() {
        let pairs = vec![
            spend("Car", "-100.00", date(2025, 3, 1)),
            spend("Car", "-150.00", date(2026, 3, 1)),
        ];
        let t = detect_for(&view(&pairs));
        assert_eq!(
            t[0].yoy_abs.unwrap().amount(),
            rust_decimal_macros::dec!(50.00)
        );
        assert_eq!(t[0].yoy_pct.as_deref(), Some("50.00"));
    }

    #[test]
    fn flat_trend_when_constant() {
        let pairs = vec![
            spend("Pets", "-50.00", date(2026, 1, 1)),
            spend("Pets", "-50.00", date(2026, 2, 1)),
            spend("Pets", "-50.00", date(2026, 3, 1)),
        ];
        let t = detect_for(&view(&pairs));
        assert_eq!(t[0].direction, "flat");
    }
}
