//! Emit: assemble and write `analysis.json` and `review_queue.json`.
//!
//! [`Analysis`] mirrors `references/analytics.md`: period, currency, flow totals
//! and series, the category/merchant breakdowns, recurring/anomalies/trends,
//! per-person/-account breakdowns, the transfer reconciliation, and the dedup
//! report. [`ReviewQueue`] is everything the host LLM needs to categorize unknown
//! merchants and assign unresolved transfer counterparties in one shot.
//!
//! Output is pretty-printed with **deterministic ordering**: every collection is
//! sorted by a stable key upstream (analytics/reconcile) or here, so re-running
//! the pipeline on the same inputs yields byte-identical files. The currency
//! guard runs before any sum, failing closed on mixed-currency inputs.

use std::collections::BTreeMap;
use std::path::Path;

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::analytics::{Analytics, Categorized};
use crate::categorize::EffectiveKind;
use crate::config::LifeEvent;
use crate::dedup::DedupReport;
use crate::error::AnalyticsError;
use crate::model::Money;
use crate::reconcile::Reconciliation;

/// The complete `analysis.json` document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Analysis {
    /// The computed analytics (period, currency, flows, breakdowns, etc.).
    #[serde(flatten)]
    pub analytics: Analytics,
    /// Transfer reconciliation (pairs, external, unresolved, graph).
    pub transfers: Reconciliation,
    /// Provenance: cross-file dedup summary.
    pub dedup_report: DedupReport,
    /// Significant life events to overlay on the cash-flow chart (annotation
    /// only). Defaults to empty when reading an older `analysis.json`.
    #[serde(default)]
    pub events: Vec<LifeEvent>,
}

/// One unknown-merchant entry for the review queue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnknownMerchant {
    /// The normalized merchant string.
    pub normalized: String,
    /// Up to a few raw-description samples.
    pub samples: Vec<String>,
    /// Number of transactions for this merchant.
    pub count: usize,
    /// Total magnitude.
    pub total: Money,
    /// Average ticket.
    pub avg: Money,
    /// `(earliest, latest)` ISO dates.
    pub date_range: (String, String),
    /// Most common resolved owner, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// Most common resolved account, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
}

/// One unassigned-transfer-counterparty entry for the review queue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnassignedTransfer {
    /// Owning account/person.
    pub account: String,
    /// Signed amount.
    pub amount: Money,
    /// Leg date.
    pub date: String,
    /// Normalized description.
    pub description: String,
}

/// The complete `review_queue.json` document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewQueue {
    /// Spend/income transactions whose merchant matched no category rule.
    pub unknown_merchants: Vec<UnknownMerchant>,
    /// Transfer legs that were neither paired nor assigned.
    pub unassigned_transfers: Vec<UnassignedTransfer>,
}

/// Build the [`Analysis`] document from the pipeline's outputs.
#[must_use]
pub fn build_analysis(
    analytics: Analytics,
    transfers: Reconciliation,
    dedup_report: DedupReport,
    events: Vec<LifeEvent>,
) -> Analysis {
    Analysis {
        analytics,
        transfers,
        dedup_report,
        events,
    }
}

/// Accumulator for one unknown merchant while scanning.
#[derive(Default)]
struct UnknownAcc {
    samples: Vec<String>,
    count: usize,
    total: Decimal,
    min_date: Option<jiff::civil::Date>,
    max_date: Option<jiff::civil::Date>,
    owners: BTreeMap<String, usize>,
    accounts: BTreeMap<String, usize>,
}

/// Build the [`ReviewQueue`] from categorized items and the reconciliation.
///
/// Unknown merchants are the spend/income rows whose assignment `needs_review`.
/// Unassigned transfers are taken directly from [`Reconciliation::unresolved`].
#[must_use]
pub fn build_review_queue(items: &[Categorized<'_>], transfers: &Reconciliation) -> ReviewQueue {
    let mut groups: BTreeMap<String, UnknownAcc> = BTreeMap::new();

    for c in items {
        if !c.assignment.needs_review {
            continue;
        }
        // Only spend/income rows are "unknown merchants"; transfers/unknowns that
        // need attention are handled via the reconciliation's unresolved list.
        if !matches!(
            c.assignment.kind,
            EffectiveKind::Spend | EffectiveKind::Income
        ) {
            continue;
        }
        let acc = groups.entry(c.txn.normalized_merchant.clone()).or_default();
        if acc.samples.len() < 3 && !acc.samples.contains(&c.txn.raw_description) {
            acc.samples.push(c.txn.raw_description.clone());
        }
        acc.count += 1;
        acc.total += c.txn.amount.amount().abs();
        let d = c.txn.date;
        acc.min_date = Some(acc.min_date.map_or(d, |m| m.min(d)));
        acc.max_date = Some(acc.max_date.map_or(d, |m| m.max(d)));
        if let Some(o) = &c.txn.owner_person {
            *acc.owners.entry(o.clone()).or_insert(0) += 1;
        }
        if let Some(a) = &c.txn.account_id {
            *acc.accounts.entry(a.clone()).or_insert(0) += 1;
        }
    }

    let mut unknown_merchants: Vec<UnknownMerchant> = groups
        .into_iter()
        .map(|(normalized, acc)| {
            let avg = if acc.count == 0 {
                Decimal::ZERO
            } else {
                (acc.total / Decimal::from(acc.count as u64)).round_dp(2)
            };
            let date_range = (
                acc.min_date.map(|d| d.to_string()).unwrap_or_default(),
                acc.max_date.map(|d| d.to_string()).unwrap_or_default(),
            );
            UnknownMerchant {
                normalized,
                samples: acc.samples,
                count: acc.count,
                total: Money::new(acc.total),
                avg: Money::new(avg),
                date_range,
                owner: most_common(&acc.owners),
                account: most_common(&acc.accounts),
            }
        })
        .collect();

    // Deterministic: by descending total, then merchant.
    unknown_merchants.sort_by(|a, b| {
        b.total
            .amount()
            .cmp(&a.total.amount())
            .then_with(|| a.normalized.cmp(&b.normalized))
    });

    let unassigned_transfers = transfers
        .unresolved
        .iter()
        .map(|u| UnassignedTransfer {
            account: u.account.clone(),
            amount: u.amount,
            date: u.date.clone(),
            description: u.description.clone(),
        })
        .collect();

    ReviewQueue {
        unknown_merchants,
        unassigned_transfers,
    }
}

/// The most frequent key in a counter map, ties broken by key order.
fn most_common(counts: &BTreeMap<String, usize>) -> Option<String> {
    counts
        .iter()
        .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
        .map(|(k, _)| k.clone())
}

/// Serialize a value to pretty JSON, mapping errors to a typed [`AnalyticsError`].
fn to_pretty<T: Serialize>(value: &T, what: &str) -> Result<String, AnalyticsError> {
    serde_json::to_string_pretty(value).map_err(|source| AnalyticsError::Serialize {
        what: what.to_string(),
        source,
    })
}

/// Write `analysis.json` and `review_queue.json` into `out_dir` (created if
/// missing). Deterministic and idempotent: same inputs produce byte-identical
/// files.
pub fn write_outputs(
    out_dir: &Path,
    analysis: &Analysis,
    review: &ReviewQueue,
) -> Result<(), AnalyticsError> {
    std::fs::create_dir_all(out_dir).map_err(|source| AnalyticsError::Write {
        path: out_dir.to_path_buf(),
        source,
    })?;

    let analysis_json = to_pretty(analysis, "analysis.json")?;
    let review_json = to_pretty(review, "review_queue.json")?;

    let analysis_path = out_dir.join("analysis.json");
    let review_path = out_dir.join("review_queue.json");

    std::fs::write(&analysis_path, analysis_json.as_bytes()).map_err(|source| {
        AnalyticsError::Write {
            path: analysis_path.clone(),
            source,
        }
    })?;
    std::fs::write(&review_path, review_json.as_bytes()).map_err(|source| {
        AnalyticsError::Write {
            path: review_path,
            source,
        }
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::categorize::Assignment;
    use crate::model::{Currency, Transaction, TxnType};
    use jiff::civil::date;
    use std::str::FromStr;

    fn unknown(merchant: &str, raw: &str, amount: &str) -> (Transaction, Assignment) {
        let t = Transaction {
            raw_description: raw.into(),
            normalized_merchant: merchant.into(),
            date: date(2026, 1, 2),
            time: None,
            amount: Money::from_str(amount).unwrap(),
            currency: Currency::new("CAD"),
            txn_type: TxnType::Spend,
            memo: None,
            cardholder: None,
            profile_id: "t".into(),
            source_file: "f.csv".into(),
            owner_person: Some("me".into()),
            account_id: Some("acct".into()),
            harvested_category: None,
            dedup_key: String::new(),
        };
        let a = Assignment {
            category: None,
            subcategory: None,
            kind: EffectiveKind::Spend,
            needs_review: true,
        };
        (t, a)
    }

    fn empty_recon() -> Reconciliation {
        Reconciliation {
            matched_pairs: vec![],
            external: vec![],
            unresolved: vec![],
            graph: crate::reconcile::Graph {
                nodes: vec![],
                edges: vec![],
            },
        }
    }

    #[test]
    fn review_queue_groups_unknown_merchants() {
        let pairs = [
            unknown("WEIRD SHOP", "WEIRD SHOP #1", "-10.00"),
            unknown("WEIRD SHOP", "WEIRD SHOP #2", "-20.00"),
        ];
        let view: Vec<Categorized<'_>> = pairs
            .iter()
            .map(|(t, a)| Categorized {
                txn: t,
                assignment: a,
            })
            .collect();
        let rq = build_review_queue(&view, &empty_recon());
        assert_eq!(rq.unknown_merchants.len(), 1);
        assert_eq!(rq.unknown_merchants[0].count, 2);
        assert_eq!(
            rq.unknown_merchants[0].total.amount(),
            rust_decimal_macros::dec!(30.00)
        );
        assert_eq!(rq.unknown_merchants[0].owner.as_deref(), Some("me"));
        assert_eq!(rq.unknown_merchants[0].samples.len(), 2);
    }
}
