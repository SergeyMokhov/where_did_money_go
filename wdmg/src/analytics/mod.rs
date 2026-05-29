//! Analytics: turn categorized, identity-resolved transactions into the exact
//! `analysis.json` field set described in `references/analytics.md`.
//!
//! Everything monetary is computed in [`rust_decimal::Decimal`] and serialized as
//! a string (see [`crate::model::Money`]). Floats appear only as dimensionless
//! threshold scalars inside [`stats`] (MAD z-score, coefficient of variation,
//! trend slope), never to hold or sum amounts.
//!
//! Transfers are excluded from every flow, category, merchant, recurring,
//! anomaly, and trend computation — they are reconciled separately by
//! [`crate::reconcile`]. The single source of truth for a transaction's
//! accounting kind is its [`crate::categorize::Assignment`], not the raw ingest
//! [`crate::model::TxnType`].

pub mod anomaly;
pub mod flows;
pub mod recurring;
pub mod stats;
pub mod trends;

use serde::{Deserialize, Serialize};

use crate::categorize::{Assignment, EffectiveKind};
use crate::error::AnalyticsError;
use crate::model::{Currency, Money, Transaction};

/// A transaction paired with its categorization, the unit analytics operates on.
#[derive(Debug, Clone, Copy)]
pub struct Categorized<'a> {
    /// The underlying transaction.
    pub txn: &'a Transaction,
    /// Its categorization assignment.
    pub assignment: &'a Assignment,
}

impl Categorized<'_> {
    /// The category label, defaulting to `"Uncategorized"` for spend/income with
    /// no rule so they still appear in breakdowns rather than vanishing.
    #[must_use]
    pub fn category_label(&self) -> &str {
        self.assignment
            .category
            .as_deref()
            .unwrap_or("Uncategorized")
    }

    /// The subcategory label, defaulting to `"Uncategorized"`.
    #[must_use]
    pub fn subcategory_label(&self) -> &str {
        self.assignment
            .subcategory
            .as_deref()
            .unwrap_or("Uncategorized")
    }
}

/// The covered date range.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Period {
    /// Earliest transaction date (ISO `YYYY-MM-DD`), if any.
    pub start: Option<String>,
    /// Latest transaction date (ISO `YYYY-MM-DD`), if any.
    pub end: Option<String>,
}

/// A compact, per-transaction record embedded in `analysis.json` so the HTML
/// report can slice-and-dice (filter by person / account / category / month /
/// kind) and recompute every aggregate **client-side** from the same exact,
/// Rust-emitted figures. Money stays an exact decimal string (never f64); the
/// browser sums in integer cents so the unfiltered view matches Rust exactly.
///
/// This carries only fields needed for filtering and aggregation — no memo,
/// cardholder, or source-file provenance — keeping the island lean.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportTxn {
    /// ISO `YYYY-MM-DD`.
    pub date: String,
    /// Calendar-month key `YYYY-MM` (precomputed for convenient client grouping).
    pub month: String,
    /// Signed amount (negative = outflow), exact decimal string.
    pub amount: Money,
    /// Effective accounting kind: `spend` | `income` | `transfer` | `unknown`.
    pub kind: String,
    /// Top-level category label (`"Uncategorized"` when none).
    pub category: String,
    /// Subcategory label (`"Uncategorized"` when none).
    pub subcategory: String,
    /// Normalized merchant string.
    pub merchant: String,
    /// Resolved owner person id (`"unassigned"` when none).
    pub person: String,
    /// Resolved account id (`"unassigned"` when none).
    pub account: String,
}

/// Stable string tag for an [`EffectiveKind`], matching the client filter values.
fn kind_tag(kind: EffectiveKind) -> &'static str {
    match kind {
        EffectiveKind::Spend => "spend",
        EffectiveKind::Income => "income",
        EffectiveKind::Transfer => "transfer",
        EffectiveKind::Unknown => "unknown",
    }
}

/// The computed analytics, minus transfers/dedup/review which the emit phase
/// assembles from [`crate::reconcile`] and the review queue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Analytics {
    /// Covered period.
    pub period: Period,
    /// Single currency of all inputs.
    pub currency: String,
    /// Flow totals and series.
    #[serde(flatten)]
    pub flows: flows::Flows,
    /// Spend grouped by `(category, subcategory)`.
    pub spend_by_category: Vec<flows::CategoryBreakdown>,
    /// Income grouped by `(category, subcategory)`.
    pub income_by_source: Vec<flows::CategoryBreakdown>,
    /// Top merchants by absolute spend.
    pub top_merchants: Vec<flows::MerchantBreakdown>,
    /// Recurring/subscription groups.
    pub recurring: Vec<recurring::Recurring>,
    /// Detected anomalies.
    pub anomalies: Vec<anomaly::Anomaly>,
    /// Per-category trends.
    pub trends: Vec<trends::Trend>,
    /// Spend/income/net by resolved owner.
    pub by_person: Vec<flows::EntityBreakdown>,
    /// Spend/income/net by resolved account.
    pub by_account: Vec<flows::EntityBreakdown>,
    /// Every (non-deduped) transaction in a compact form, so the report can
    /// filter and recompute aggregates client-side. Includes transfers and
    /// unknowns (with their `kind`) so the browser can replicate the Rust rules.
    pub transactions: Vec<ReportTxn>,
}

/// Verify all transactions share one currency; return it, or a typed error
/// naming the offenders. Fails closed (per defensive-coding) rather than summing
/// across currencies.
pub fn single_currency(txns: &[Transaction]) -> Result<Currency, AnalyticsError> {
    let mut iter = txns.iter();
    let first = match iter.next() {
        Some(t) => t.currency.clone(),
        None => return Ok(Currency::new("CAD")), // empty input: neutral default
    };
    for t in iter {
        if t.currency != first {
            return Err(AnalyticsError::MixedCurrency {
                first: first.to_string(),
                other: t.currency.to_string(),
            });
        }
    }
    Ok(first)
}

/// Compute the full analytics from categorized transactions.
///
/// Transfers are excluded from flows/categories/merchants/recurring/anomalies/
/// trends. The caller supplies the same `(txn, assignment)` pairing used
/// everywhere downstream.
pub fn compute(items: &[Categorized<'_>], currency: &Currency) -> Analytics {
    // Spend/income view excludes transfers and unknown rows.
    let flow_items: Vec<Categorized<'_>> = items
        .iter()
        .copied()
        .filter(|c| {
            matches!(
                c.assignment.kind,
                EffectiveKind::Spend | EffectiveKind::Income
            )
        })
        .collect();

    let period = period_of(items);
    let flows = flows::compute(&flow_items);
    let spend_by_category =
        flows::by_category(&flow_items, EffectiveKind::Spend, &flows.total_outflow);
    let income_by_source =
        flows::by_category(&flow_items, EffectiveKind::Income, &flows.total_inflow);
    let top_merchants = flows::top_merchants(&flow_items, 20);
    let recurring = recurring::detect(&flow_items);
    // Recurring/subscription merchants are excluded from single-transaction
    // anomaly detection — a stable repeated charge is not an anomaly.
    let recurring_merchants: std::collections::BTreeSet<String> =
        recurring.iter().map(|r| r.merchant.clone()).collect();
    let anomalies = anomaly::detect(&flow_items, &recurring_merchants);
    let trends = trends::compute(&flow_items);
    let by_person = flows::by_person(&flow_items);
    let by_account = flows::by_account(&flow_items);

    // Per-transaction export for the client-side filter. Built from ALL items
    // (transfers/unknowns included, tagged by kind) in the deterministic order
    // the pipeline produced, so re-runs stay byte-identical.
    let transactions: Vec<ReportTxn> = items
        .iter()
        .map(|c| ReportTxn {
            date: c.txn.date.to_string(),
            month: month_key(c.txn.date),
            amount: c.txn.amount,
            kind: kind_tag(c.assignment.kind).to_string(),
            category: c.category_label().to_string(),
            subcategory: c.subcategory_label().to_string(),
            merchant: c.txn.normalized_merchant.clone(),
            person: c
                .txn
                .owner_person
                .clone()
                .unwrap_or_else(|| "unassigned".into()),
            account: c
                .txn
                .account_id
                .clone()
                .unwrap_or_else(|| "unassigned".into()),
        })
        .collect();

    Analytics {
        period,
        currency: currency.to_string(),
        flows,
        spend_by_category,
        income_by_source,
        top_merchants,
        recurring,
        anomalies,
        trends,
        by_person,
        by_account,
        transactions,
    }
}

/// Min/max transaction date across all items (transfers included, so the period
/// reflects the whole dataset).
fn period_of(items: &[Categorized<'_>]) -> Period {
    let mut start = None;
    let mut end = None;
    for c in items {
        let d = c.txn.date;
        start = Some(match start {
            Some(s) if s <= d => s,
            _ => d,
        });
        end = Some(match end {
            Some(e) if e >= d => e,
            _ => d,
        });
    }
    Period {
        start: start.map(|d| d.to_string()),
        end: end.map(|d| d.to_string()),
    }
}

/// Calendar-month key `YYYY-MM` for a date.
#[must_use]
pub fn month_key(date: jiff::civil::Date) -> String {
    format!("{:04}-{:02}", date.year(), date.month())
}

/// ISO-week key `YYYY-Www` for a date.
#[must_use]
pub fn week_key(date: jiff::civil::Date) -> String {
    let iw = date.iso_week_date();
    format!("{:04}-W{:02}", iw.year(), iw.week())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Currency, Money, TxnType};
    use jiff::civil::date;
    use std::str::FromStr;

    fn txn(cur: &str) -> Transaction {
        Transaction {
            raw_description: "X".into(),
            normalized_merchant: "X".into(),
            date: date(2026, 1, 2),
            time: None,
            amount: Money::from_str("-10.00").unwrap(),
            currency: Currency::new(cur),
            txn_type: TxnType::Spend,
            memo: None,
            cardholder: None,
            profile_id: "t".into(),
            source_file: "f.csv".into(),
            owner_person: None,
            account_id: None,
            harvested_category: None,
            dedup_key: String::new(),
        }
    }

    #[test]
    fn single_currency_accepts_uniform() {
        let txns = vec![txn("CAD"), txn("CAD")];
        assert_eq!(single_currency(&txns).unwrap(), Currency::new("CAD"));
    }

    #[test]
    fn single_currency_rejects_mixed() {
        let txns = vec![txn("CAD"), txn("USD")];
        assert!(single_currency(&txns).is_err());
    }

    #[test]
    fn month_and_week_keys() {
        assert_eq!(month_key(date(2026, 1, 2)), "2026-01");
        assert_eq!(week_key(date(2026, 1, 15)), "2026-W03");
    }
}
