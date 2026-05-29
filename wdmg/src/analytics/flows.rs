//! Flow totals and the category / merchant / entity breakdowns.
//!
//! All sums are exact [`Decimal`]. Spend is stored as a positive magnitude in the
//! totals (`total_outflow`) and category totals, matching the report's
//! expectation; the raw transaction amount is negative for outflows, so we take
//! its absolute value. Income is summed as its positive amount.

use std::collections::BTreeMap;

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::stats::mean;
use super::{month_key, week_key, Categorized};
use crate::categorize::EffectiveKind;
use crate::model::Money;

/// Top-level flow figures plus the monthly/weekly series.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Flows {
    /// Sum of income amounts (positive).
    pub total_inflow: Money,
    /// Sum of spend magnitudes (positive).
    pub total_outflow: Money,
    /// `total_inflow - total_outflow`.
    pub net: Money,
    /// `(inflow - outflow) / inflow`, or `null` when there is no income.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub savings_rate: Option<String>,
    /// Per-calendar-month series.
    pub monthly: Vec<PeriodFlow>,
    /// Per-ISO-week series.
    pub weekly: Vec<PeriodFlow>,
}

/// One bucket of the monthly/weekly series with a rolling-mean baseline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeriodFlow {
    /// Bucket key (`YYYY-MM` or `YYYY-Www`).
    pub period: String,
    /// Income in the bucket.
    pub inflow: Money,
    /// Spend magnitude in the bucket.
    pub outflow: Money,
    /// `inflow - outflow`.
    pub net: Money,
    /// Rolling mean of `net` over the trailing window (3 months / 4 weeks),
    /// `null` until enough history accumulates.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline: Option<Money>,
}

/// A `(category, subcategory)` breakdown row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategoryBreakdown {
    /// Top-level category.
    pub category: String,
    /// Subcategory.
    pub subcategory: String,
    /// Total magnitude (positive).
    pub total: Money,
    /// Number of contributing transactions.
    pub count: usize,
    /// Percentage of the side's total (spend or income), as a decimal string.
    pub pct_of_spend: String,
    /// Per-month totals for this row.
    pub monthly: Vec<MonthAmount>,
}

/// A merchant breakdown row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MerchantBreakdown {
    /// Normalized merchant string.
    pub merchant: String,
    /// Total magnitude (positive).
    pub total: Money,
    /// Number of transactions.
    pub count: usize,
    /// Average ticket (`total / count`).
    pub avg: Money,
}

/// A per-month `(period, total)` pair.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonthAmount {
    /// `YYYY-MM`.
    pub period: String,
    /// Magnitude in the month.
    pub total: Money,
}

/// Spend/income/net for one person or account.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityBreakdown {
    /// Person id or account id (or `"unassigned"`).
    pub id: String,
    /// Spend magnitude.
    pub spend: Money,
    /// Income.
    pub income: Money,
    /// `income - spend`.
    pub net: Money,
}

/// Spend magnitude of a flow item (absolute amount for spend, else zero).
fn spend_mag(c: &Categorized<'_>) -> Decimal {
    if c.assignment.kind == EffectiveKind::Spend {
        c.txn.amount.amount().abs()
    } else {
        Decimal::ZERO
    }
}

/// Income amount of a flow item (the amount for income, else zero).
fn income_amt(c: &Categorized<'_>) -> Decimal {
    if c.assignment.kind == EffectiveKind::Income {
        c.txn.amount.amount()
    } else {
        Decimal::ZERO
    }
}

/// Compute top-level flows and the monthly/weekly series.
#[must_use]
pub fn compute(items: &[Categorized<'_>]) -> Flows {
    let total_outflow: Decimal = items.iter().map(spend_mag).sum();
    let total_inflow: Decimal = items.iter().map(income_amt).sum();
    let net = total_inflow - total_outflow;

    let savings_rate = if total_inflow.is_zero() {
        None
    } else {
        (total_inflow - total_outflow)
            .checked_div(total_inflow)
            .map(|r| r.round_dp(6).to_string())
    };

    let monthly = series(items, month_key, 3);
    let weekly = series(items, week_key, 4);

    Flows {
        total_inflow: Money::new(total_inflow),
        total_outflow: Money::new(total_outflow),
        net: Money::new(net),
        savings_rate,
        monthly,
        weekly,
    }
}

/// Build a sorted period series with a trailing rolling-mean baseline of `net`.
fn series<F>(items: &[Categorized<'_>], key: F, window: usize) -> Vec<PeriodFlow>
where
    F: Fn(jiff::civil::Date) -> String,
{
    let mut buckets: BTreeMap<String, (Decimal, Decimal)> = BTreeMap::new();
    for c in items {
        let k = key(c.txn.date);
        let entry = buckets.entry(k).or_insert((Decimal::ZERO, Decimal::ZERO));
        entry.0 += income_amt(c);
        entry.1 += spend_mag(c);
    }

    // BTreeMap iterates in sorted (chronological for these zero-padded keys) order.
    let ordered: Vec<(String, Decimal, Decimal)> = buckets
        .into_iter()
        .map(|(k, (inflow, outflow))| (k, inflow, outflow))
        .collect();

    let nets: Vec<Decimal> = ordered.iter().map(|(_, i, o)| *i - *o).collect();

    ordered
        .iter()
        .enumerate()
        .map(|(idx, (period, inflow, outflow))| {
            let net = *inflow - *outflow;
            // Trailing window EXCLUDING the current bucket, so the baseline is a
            // genuine prior expectation rather than self-referential.
            let lo = idx.saturating_sub(window);
            let baseline = if idx == 0 {
                None
            } else {
                mean(&nets[lo..idx]).map(Money::new)
            };
            PeriodFlow {
                period: period.clone(),
                inflow: Money::new(*inflow),
                outflow: Money::new(*outflow),
                net: Money::new(net),
                baseline,
            }
        })
        .collect()
}

/// Group spend or income by `(category, subcategory)`, sorted by descending total
/// then name for determinism. `side_total` is the denominator for `pct`.
#[must_use]
pub fn by_category(
    items: &[Categorized<'_>],
    kind: EffectiveKind,
    side_total: &Money,
) -> Vec<CategoryBreakdown> {
    // (category, subcategory) -> (total, count, per-month totals)
    type CatAgg = (Decimal, usize, BTreeMap<String, Decimal>);
    let mut groups: BTreeMap<(String, String), CatAgg> = BTreeMap::new();

    for c in items {
        if c.assignment.kind != kind {
            continue;
        }
        let mag = match kind {
            EffectiveKind::Spend => c.txn.amount.amount().abs(),
            EffectiveKind::Income => c.txn.amount.amount(),
            _ => continue,
        };
        let key = (
            c.category_label().to_string(),
            c.subcategory_label().to_string(),
        );
        let entry = groups
            .entry(key)
            .or_insert((Decimal::ZERO, 0, BTreeMap::new()));
        entry.0 += mag;
        entry.1 += 1;
        *entry
            .2
            .entry(month_key(c.txn.date))
            .or_insert(Decimal::ZERO) += mag;
    }

    let denom = side_total.amount();
    let mut rows: Vec<CategoryBreakdown> = groups
        .into_iter()
        .map(|((category, subcategory), (total, count, months))| {
            let pct = if denom.is_zero() {
                Decimal::ZERO
            } else {
                (total / denom * Decimal::from(100u32)).round_dp(2)
            };
            let monthly = months
                .into_iter()
                .map(|(period, total)| MonthAmount {
                    period,
                    total: Money::new(total),
                })
                .collect();
            CategoryBreakdown {
                category,
                subcategory,
                total: Money::new(total),
                count,
                pct_of_spend: pct.to_string(),
                monthly,
            }
        })
        .collect();

    rows.sort_by(|a, b| {
        b.total
            .amount()
            .cmp(&a.total.amount())
            .then_with(|| a.category.cmp(&b.category))
            .then_with(|| a.subcategory.cmp(&b.subcategory))
    });
    rows
}

/// Top `n` merchants by spend magnitude (income excluded).
#[must_use]
pub fn top_merchants(items: &[Categorized<'_>], n: usize) -> Vec<MerchantBreakdown> {
    let mut groups: BTreeMap<String, (Decimal, usize)> = BTreeMap::new();
    for c in items {
        if c.assignment.kind != EffectiveKind::Spend {
            continue;
        }
        let entry = groups
            .entry(c.txn.normalized_merchant.clone())
            .or_insert((Decimal::ZERO, 0));
        entry.0 += c.txn.amount.amount().abs();
        entry.1 += 1;
    }

    let mut rows: Vec<MerchantBreakdown> = groups
        .into_iter()
        .map(|(merchant, (total, count))| {
            let avg = if count == 0 {
                Decimal::ZERO
            } else {
                (total / Decimal::from(count as u64)).round_dp(2)
            };
            MerchantBreakdown {
                merchant,
                total: Money::new(total),
                count,
                avg: Money::new(avg),
            }
        })
        .collect();

    rows.sort_by(|a, b| {
        b.total
            .amount()
            .cmp(&a.total.amount())
            .then_with(|| a.merchant.cmp(&b.merchant))
    });
    rows.truncate(n);
    rows
}

/// Group spend/income/net by resolved owner person.
#[must_use]
pub fn by_person(items: &[Categorized<'_>]) -> Vec<EntityBreakdown> {
    by_entity(items, |c| {
        c.txn
            .owner_person
            .clone()
            .unwrap_or_else(|| "unassigned".into())
    })
}

/// Group spend/income/net by resolved account.
#[must_use]
pub fn by_account(items: &[Categorized<'_>]) -> Vec<EntityBreakdown> {
    by_entity(items, |c| {
        c.txn
            .account_id
            .clone()
            .unwrap_or_else(|| "unassigned".into())
    })
}

/// Shared grouping by an extracted key.
fn by_entity<F>(items: &[Categorized<'_>], key: F) -> Vec<EntityBreakdown>
where
    F: Fn(&Categorized<'_>) -> String,
{
    let mut groups: BTreeMap<String, (Decimal, Decimal)> = BTreeMap::new();
    for c in items {
        let entry = groups
            .entry(key(c))
            .or_insert((Decimal::ZERO, Decimal::ZERO));
        entry.0 += spend_mag(c);
        entry.1 += income_amt(c);
    }
    groups
        .into_iter()
        .map(|(id, (spend, income))| EntityBreakdown {
            id,
            spend: Money::new(spend),
            income: Money::new(income),
            net: Money::new(income - spend),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::categorize::Assignment;
    use crate::model::{Currency, Transaction, TxnType};
    use jiff::civil::date;
    use rust_decimal_macros::dec;
    use std::str::FromStr;

    fn mk(merchant: &str, amount: &str, kind: EffectiveKind, day: i8) -> (Transaction, Assignment) {
        let t = Transaction {
            raw_description: merchant.into(),
            normalized_merchant: merchant.into(),
            date: date(2026, 1, day),
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
            category: Some("Cat".into()),
            subcategory: Some("Sub".into()),
            kind,
            needs_review: false,
        };
        (t, a)
    }

    fn view<'a>(pairs: &'a [(Transaction, Assignment)]) -> Vec<Categorized<'a>> {
        pairs
            .iter()
            .map(|(t, a)| Categorized {
                txn: t,
                assignment: a,
            })
            .collect()
    }

    #[test]
    fn totals_exclude_sign_and_compute_net() {
        let pairs = vec![
            mk("A", "-40.00", EffectiveKind::Spend, 2),
            mk("B", "100.00", EffectiveKind::Income, 3),
        ];
        let f = compute(&view(&pairs));
        assert_eq!(f.total_outflow.amount(), dec!(40.00));
        assert_eq!(f.total_inflow.amount(), dec!(100.00));
        assert_eq!(f.net.amount(), dec!(60.00));
        // savings_rate = (100 - 40)/100 = 0.6
        assert_eq!(f.savings_rate.as_deref(), Some("0.60"));
    }

    #[test]
    fn category_pct_sums_to_100() {
        let pairs = vec![
            mk("A", "-40.00", EffectiveKind::Spend, 2),
            mk("B", "-60.00", EffectiveKind::Spend, 3),
        ];
        let v = view(&pairs);
        let f = compute(&v);
        let rows = by_category(&v, EffectiveKind::Spend, &f.total_outflow);
        let pct_sum: Decimal = rows
            .iter()
            .map(|r| Decimal::from_str(&r.pct_of_spend).unwrap())
            .sum();
        assert_eq!(pct_sum, dec!(100.00));
    }

    #[test]
    fn top_merchants_sorted_desc() {
        let pairs = vec![
            mk("SMALL", "-5.00", EffectiveKind::Spend, 2),
            mk("BIG", "-500.00", EffectiveKind::Spend, 3),
        ];
        let m = top_merchants(&view(&pairs), 10);
        assert_eq!(m[0].merchant, "BIG");
        assert_eq!(m[0].avg.amount(), dec!(500.00));
    }

    #[test]
    fn by_person_groups_net() {
        let pairs = vec![
            mk("A", "-40.00", EffectiveKind::Spend, 2),
            mk("B", "100.00", EffectiveKind::Income, 3),
        ];
        let p = by_person(&view(&pairs));
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].id, "me");
        assert_eq!(p[0].net.amount(), dec!(60.00));
    }

    #[test]
    fn monthly_series_has_baseline_after_first() {
        let pairs = vec![
            mk("A", "-10.00", EffectiveKind::Spend, 2),
            (
                {
                    let mut t = mk("B", "-20.00", EffectiveKind::Spend, 2).0;
                    t.date = date(2026, 2, 2);
                    t
                },
                Assignment {
                    category: Some("Cat".into()),
                    subcategory: Some("Sub".into()),
                    kind: EffectiveKind::Spend,
                    needs_review: false,
                },
            ),
        ];
        let f = compute(&view(&pairs));
        assert_eq!(f.monthly.len(), 2);
        assert!(f.monthly[0].baseline.is_none());
        assert!(f.monthly[1].baseline.is_some());
    }
}
