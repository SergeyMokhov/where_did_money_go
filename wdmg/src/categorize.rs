//! Categorization: assign each transaction a `(category, subcategory)`.
//!
//! Patterns are case-insensitive, UNANCHORED regexes matched against the
//! transaction's `normalized_merchant` (falling back to `raw_description`). A
//! subcategory rule only applies when `|amount|` is within its
//! `[amount_min, amount_max]` bounds (when set).
//!
//! Disambiguation (per `default_categories.toml`): **most-specific match wins** —
//! among all matching rules pick the one with the LONGEST matching pattern
//! string; ties break by higher `priority`, then by config order. This is how a
//! bare `UBER` (Rideshare) loses to `UBER EATS` (Restaurants).
//!
//! The matched category's `kind` sets the transaction's effective accounting
//! kind. A profile-declared [`TxnType::Transfer`] (e.g. a PC `PAYMENT`) is sticky:
//! it stays a transfer even when no category rule matched. A spend/income
//! transaction matching no rule is left uncategorized and surfaces in the review
//! queue (collected by the emit step). The Rewards `harvested_category` is only
//! a low-priority fallback *label*; it never overrides an explicit rule and never
//! changes the kind.
//!
//! Per defensive-coding ("parse, don't validate"): all regexes were compiled and
//! validated at config load, so this phase never re-validates raw patterns. We
//! return assignments in a parallel `Vec` rather than mutating [`Transaction`], so
//! the original ingest output stays immutable and re-runnable.

use regex::{Regex, RegexBuilder};
use rust_decimal::Decimal;

use crate::config::{CategoriesDoc, CategoryKind};
use crate::model::{Transaction, TxnType};

/// The effective accounting kind of a transaction after categorization.
///
/// This is the single source of truth downstream: analytics sums [`Spend`] and
/// [`Income`], reconcile consumes [`Transfer`], and [`Unknown`] rows are excluded
/// from flows and surfaced for review.
///
/// [`Spend`]: EffectiveKind::Spend
/// [`Income`]: EffectiveKind::Income
/// [`Transfer`]: EffectiveKind::Transfer
/// [`Unknown`]: EffectiveKind::Unknown
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectiveKind {
    /// Counted as outflow.
    Spend,
    /// Counted as inflow.
    Income,
    /// Netted from both flows; fed to reconciliation.
    Transfer,
    /// Not determinable; excluded from flows, surfaced for review.
    Unknown,
}

impl From<CategoryKind> for EffectiveKind {
    fn from(k: CategoryKind) -> Self {
        match k {
            CategoryKind::Spend => Self::Spend,
            CategoryKind::Income => Self::Income,
            CategoryKind::Transfer => Self::Transfer,
        }
    }
}

/// The categorization outcome for one transaction, aligned by index with the
/// input slice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assignment {
    /// Matched top-level category name, if any.
    pub category: Option<String>,
    /// Matched subcategory name, if any.
    pub subcategory: Option<String>,
    /// Effective accounting kind used by every downstream phase.
    pub kind: EffectiveKind,
    /// True when no rule matched and the harvested label (if any) was the only
    /// signal — these rows go to the review queue.
    pub needs_review: bool,
}

/// A compiled categorization rule: one subcategory pattern with its context.
struct CompiledRule {
    category: String,
    subcategory: String,
    kind: EffectiveKind,
    regex: Regex,
    /// Length of the *pattern string* (not the match) — used for specificity.
    pattern_len: usize,
    amount_min: Option<Decimal>,
    amount_max: Option<Decimal>,
    priority: i32,
    /// Position in config order; lower wins ties after pattern_len & priority.
    order: usize,
}

/// A categorizer: the category rules compiled once, reused across transactions.
pub struct Categorizer {
    rules: Vec<CompiledRule>,
}

/// Strip the look-around groups the `regex` crate cannot compile.
///
/// Category patterns are authored for humans and may contain look-around (e.g.
/// `UBER(?! EATS)`); the `regex` crate rejects it to guarantee linear-time
/// matching. The config layer already validated the sanitized form compiles, and
/// the most-specific-match rule recovers the intended disambiguation (a longer
/// `UBER EATS` beats a bare `UBER`), so stripping look-around here is safe.
fn sanitize_pattern(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len());
    let bytes = pattern.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let is_lookaround = bytes[i] == b'('
            && i + 2 < bytes.len()
            && bytes[i + 1] == b'?'
            && (bytes[i + 2] == b'='
                || bytes[i + 2] == b'!'
                || (bytes[i + 2] == b'<'
                    && i + 3 < bytes.len()
                    && (bytes[i + 3] == b'=' || bytes[i + 3] == b'!')));
        if is_lookaround {
            let mut depth = 0usize;
            let mut j = i;
            while j < bytes.len() {
                match bytes[j] {
                    b'\\' => j += 1,
                    b'(' => depth += 1,
                    b')' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            i = j + 1;
        } else {
            let ch_len = utf8_len(bytes[i]);
            out.push_str(&pattern[i..i + ch_len]);
            i += ch_len;
        }
    }
    out
}

/// Byte length of a UTF-8 char given its lead byte.
fn utf8_len(lead: u8) -> usize {
    if lead < 0x80 {
        1
    } else if lead >> 5 == 0b110 {
        2
    } else if lead >> 4 == 0b1110 {
        3
    } else {
        4
    }
}

impl Categorizer {
    /// Compile every subcategory pattern in `categories` into a flat rule list.
    ///
    /// Patterns were already validated at config load; compilation here cannot
    /// realistically fail, but any that does is silently skipped rather than
    /// panicking (it simply cannot match), keeping this on the infallible path.
    #[must_use]
    pub fn new(categories: &CategoriesDoc) -> Self {
        let mut rules = Vec::new();
        let mut order = 0usize;
        for cat in &categories.categories {
            let kind = EffectiveKind::from(cat.kind);
            for sub in &cat.subcategories {
                for pat in &sub.patterns {
                    let sanitized = sanitize_pattern(pat);
                    if let Ok(regex) = RegexBuilder::new(&sanitized).case_insensitive(true).build()
                    {
                        rules.push(CompiledRule {
                            category: cat.name.clone(),
                            subcategory: sub.name.clone(),
                            kind,
                            regex,
                            pattern_len: pat.len(),
                            amount_min: sub.amount_min,
                            amount_max: sub.amount_max,
                            priority: sub.priority,
                            order,
                        });
                    }
                    order += 1;
                }
            }
        }
        Self { rules }
    }

    /// Categorize one transaction.
    #[must_use]
    pub fn assign(&self, txn: &Transaction) -> Assignment {
        let abs_amount = txn.amount.amount().abs();

        // Find the most-specific matching rule.
        let mut best: Option<&CompiledRule> = None;
        for rule in &self.rules {
            // Amount bounds gate (inclusive).
            if let Some(min) = rule.amount_min {
                if abs_amount < min {
                    continue;
                }
            }
            if let Some(max) = rule.amount_max {
                if abs_amount > max {
                    continue;
                }
            }
            // Match normalized merchant first, then raw description.
            if !rule.regex.is_match(&txn.normalized_merchant)
                && !rule.regex.is_match(&txn.raw_description)
            {
                continue;
            }
            best = Some(match best {
                None => rule,
                Some(cur) => {
                    // Most-specific wins: longer pattern, then higher priority,
                    // then earlier config order.
                    let better = (
                        rule.pattern_len,
                        rule.priority,
                        std::cmp::Reverse(rule.order),
                    ) > (cur.pattern_len, cur.priority, std::cmp::Reverse(cur.order));
                    if better {
                        rule
                    } else {
                        cur
                    }
                }
            });
        }

        match best {
            Some(rule) => {
                // Reconcile the rule's kind with the ingest-declared kind.
                //
                // The merchant pattern is authoritative for the *label* but not
                // always for the *sign*: an ingest REFUND (Income) of an Uber ride
                // matches the spend pattern "UBER", yet it is still money coming
                // back, not spending. So:
                //   - a `transfer` category always wins (CC-payment detection is
                //     authoritative: a CREDIT "PAYMENT - THANK YOU" the ledger
                //     mapped to income is really a transfer);
                //   - an ingest `Transfer` is sticky and is never flipped to
                //     spend/income by a merchant pattern;
                //   - a definite ingest Spend/Income that conflicts with the
                //     rule's kind keeps the ingest kind (the sign is authoritative)
                //     but adopts the rule's category/subcategory label;
                //   - otherwise the rule's kind sets the effective kind.
                let kind =
                    if rule.kind == EffectiveKind::Transfer || txn.txn_type == TxnType::Transfer {
                        // A transfer category, or an ingest-declared transfer, is
                        // authoritative and never flipped to spend/income.
                        EffectiveKind::Transfer
                    } else {
                        match txn.txn_type {
                            TxnType::Spend => EffectiveKind::Spend,
                            TxnType::Income => EffectiveKind::Income,
                            // Unknown (or matching) ingest: trust the category kind.
                            _ => rule.kind,
                        }
                    };
                Assignment {
                    category: Some(rule.category.clone()),
                    subcategory: Some(rule.subcategory.clone()),
                    kind,
                    needs_review: false,
                }
            }
            None => {
                // No rule matched. A profile-declared transfer stays a transfer.
                if txn.txn_type == TxnType::Transfer {
                    return Assignment {
                        category: None,
                        subcategory: None,
                        kind: EffectiveKind::Transfer,
                        needs_review: false,
                    };
                }
                // Fall back to the ingest-declared kind. Spend/income with no
                // rule needs review; the harvested category is only a label.
                let kind = match txn.txn_type {
                    TxnType::Spend => EffectiveKind::Spend,
                    TxnType::Income => EffectiveKind::Income,
                    TxnType::Transfer => EffectiveKind::Transfer,
                    TxnType::Unknown => EffectiveKind::Unknown,
                };
                let needs_review = matches!(kind, EffectiveKind::Spend | EffectiveKind::Income)
                    || kind == EffectiveKind::Unknown;
                Assignment {
                    category: txn
                        .harvested_category
                        .clone()
                        .map(|_| "Uncategorized".into()),
                    subcategory: txn.harvested_category.clone(),
                    kind,
                    needs_review,
                }
            }
        }
    }

    /// Categorize a whole slice, returning a parallel `Vec` of assignments.
    #[must_use]
    pub fn assign_all(&self, txns: &[Transaction]) -> Vec<Assignment> {
        txns.iter().map(|t| self.assign(t)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::model::{Currency, Money, TxnType};
    use jiff::civil::date;
    use std::str::FromStr;

    fn cfg() -> Config {
        Config::from_defaults().unwrap()
    }

    fn txn(merchant: &str, amount: &str, ty: TxnType) -> Transaction {
        Transaction {
            raw_description: merchant.to_string(),
            normalized_merchant: merchant.to_string(),
            date: date(2026, 1, 2),
            time: None,
            amount: Money::from_str(amount).unwrap(),
            currency: Currency::new("CAD"),
            txn_type: ty,
            memo: None,
            cardholder: None,
            profile_id: "test".into(),
            source_file: "f.csv".into(),
            owner_person: None,
            account_id: None,
            harvested_category: None,
            dedup_key: String::new(),
        }
    }

    #[test]
    fn uber_eats_beats_bare_uber() {
        let c = Categorizer::new(&cfg().categories);
        // "UBER EATS" matches both Rideshare (UBER) and Restaurants (UBER EATS);
        // the longer pattern wins.
        let a = c.assign(&txn("UBER EATS", "-25.00", TxnType::Spend));
        assert_eq!(a.category.as_deref(), Some("Takeout & Restaurants"));
        assert_eq!(a.subcategory.as_deref(), Some("Restaurants"));
        // A bare UBER ride still lands in Rideshare.
        let b = c.assign(&txn("UBER TRIP", "-25.00", TxnType::Spend));
        assert_eq!(b.subcategory.as_deref(), Some("Rideshare"));
    }

    #[test]
    fn amount_bounds_gate_coffee_vs_uncategorized() {
        let c = Categorizer::new(&cfg().categories);
        // Coffee caps at 20.00; a $50 "COFFEE" charge fails the gate -> no Coffee.
        let big = c.assign(&txn("COFFEE", "-50.00", TxnType::Spend));
        assert_ne!(big.subcategory.as_deref(), Some("Coffee"));
        // A $5 coffee matches.
        let small = c.assign(&txn("STARBUCKS", "-5.00", TxnType::Spend));
        assert_eq!(small.subcategory.as_deref(), Some("Coffee"));
    }

    #[test]
    fn gas_has_no_amount_gate() {
        let c = Categorizer::new(&cfg().categories);
        // Gas is intentionally ungated: a small pay-at-pump top-up and a full
        // fill-up both land in Gas (see default_categories.toml rationale).
        let small = c.assign(&txn("SHELL", "-5.00", TxnType::Spend));
        assert_eq!(small.subcategory.as_deref(), Some("Gas"));
        let big = c.assign(&txn("SHELL", "-60.00", TxnType::Spend));
        assert_eq!(big.subcategory.as_deref(), Some("Gas"));
    }

    #[test]
    fn category_kind_overrides_effective_kind() {
        let c = Categorizer::new(&cfg().categories);
        // A CC payment description maps to the Transfers category (kind=transfer).
        let a = c.assign(&txn("PAYMENT - THANK YOU", "92.40", TxnType::Income));
        assert_eq!(a.kind, EffectiveKind::Transfer);
        assert_eq!(a.subcategory.as_deref(), Some("Credit Card Payment"));
    }

    #[test]
    fn refund_keeps_income_kind_but_takes_category_label() {
        let c = Categorizer::new(&cfg().categories);
        // A REFUND (ingest Income) for an Uber ride matches the "UBER" spend
        // pattern; it must stay Income (money back) but be labelled Rideshare.
        let a = c.assign(&txn("UBER *TRIP", "54.06", TxnType::Income));
        assert_eq!(a.kind, EffectiveKind::Income);
        assert_eq!(a.subcategory.as_deref(), Some("Rideshare"));
        assert!(!a.needs_review);
    }

    #[test]
    fn profile_transfer_is_sticky_without_rule() {
        let c = Categorizer::new(&cfg().categories);
        // Unrecognized merchant but ingest said Transfer (PC PAYMENT) -> stays transfer.
        let a = c.assign(&txn(
            "ZZZ UNKNOWN COUNTERPARTY",
            "100.00",
            TxnType::Transfer,
        ));
        assert_eq!(a.kind, EffectiveKind::Transfer);
        assert!(!a.needs_review);
        assert!(a.category.is_none());
    }

    #[test]
    fn unmatched_spend_needs_review() {
        let c = Categorizer::new(&cfg().categories);
        let a = c.assign(&txn("SOME RANDOM MERCHANT XYZ", "-12.34", TxnType::Spend));
        assert!(a.needs_review);
        assert_eq!(a.kind, EffectiveKind::Spend);
        assert!(a.category.is_none());
    }
}
