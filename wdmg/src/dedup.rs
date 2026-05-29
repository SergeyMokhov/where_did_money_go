//! Cross-file deduplication.
//!
//! When the same transaction appears in two different exports (e.g. a card
//! statement overlapping a downloaded range), we keep one copy. But two truly
//! identical rows *within one file* are legitimate (you really did buy two
//! identical coffees), so we keep those.
//!
//! Strategy: build a composite content key from
//! `account_id | ISO date | exact decimal | normalized merchant | time?`. Within
//! a single source file, append an intra-file ordinal so same-key rows stay
//! distinct; across files, identical content keys collapse to the first
//! occurrence. The final [`Transaction::dedup_key`] is the blake3 hash of the
//! per-file-disambiguated key, so it is stable run-to-run (property-tested).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::model::Transaction;

/// A record of one cross-file overlap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Overlap {
    /// One source file involved.
    pub file_a: String,
    /// The other source file involved.
    pub file_b: String,
    /// Number of rows collapsed between this pair.
    pub count: usize,
}

/// Summary of a dedup pass.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DedupReport {
    /// Number of transactions removed as cross-file duplicates.
    pub removed: usize,
    /// Number of transactions kept.
    pub kept: usize,
    /// Cross-file overlaps observed.
    pub overlaps: Vec<Overlap>,
}

/// The content portion of the key (independent of source file & ordinal).
fn content_key(txn: &Transaction) -> String {
    // account_id is `None` in phase 1; include it so later phases stay stable.
    let account = txn.account_id.as_deref().unwrap_or("");
    let date = txn.date.to_string(); // ISO YYYY-MM-DD
    let amount = txn.amount.dedup_repr(); // exact, normalized decimal
    let merchant = &txn.normalized_merchant;
    let time = txn.time.map(|t| t.to_string()).unwrap_or_default();
    format!("{account}|{date}|{amount}|{merchant}|{time}")
}

/// blake3 hex digest of a string.
fn hash(s: &str) -> String {
    blake3::hash(s.as_bytes()).to_hex().to_string()
}

/// Deduplicate `txns` across source files while preserving in-file duplicates.
///
/// Sets [`Transaction::dedup_key`] on every *kept* row and returns the kept rows
/// plus a [`DedupReport`]. Input order is otherwise preserved.
#[must_use]
pub fn dedup(txns: Vec<Transaction>) -> (Vec<Transaction>, DedupReport) {
    // 1. Assign an intra-file ordinal per (source_file, content_key) so legitimate
    //    same-content rows within one file get distinct keys.
    //    intra[(file, content)] = next ordinal.
    let mut intra: HashMap<(String, String), usize> = HashMap::new();

    // 2. Track the first file we saw a given disambiguated key in, to detect
    //    cross-file overlaps and which pair they came from.
    //    seen[disambiguated_key] = source_file of first occurrence.
    let mut seen: HashMap<String, String> = HashMap::new();
    let mut overlap_counts: HashMap<(String, String), usize> = HashMap::new();

    let mut kept = Vec::with_capacity(txns.len());
    let mut removed = 0usize;

    for mut txn in txns {
        let content = content_key(&txn);
        let ordinal = intra
            .entry((txn.source_file.clone(), content.clone()))
            .or_insert(0);
        let disambiguated = format!("{content}|#{ordinal}");
        *ordinal += 1;

        let key = hash(&disambiguated);

        if let Some(first_file) = seen.get(&key) {
            // Same disambiguated key already seen in another file -> cross-file dup.
            if *first_file != txn.source_file {
                removed += 1;
                let pair = ordered_pair(first_file, &txn.source_file);
                *overlap_counts.entry(pair).or_insert(0) += 1;
                continue;
            }
            // Same file + same disambiguated key cannot happen (ordinal differs),
            // so reaching here would be a logic error; fall through and keep.
        }

        seen.insert(key.clone(), txn.source_file.clone());
        txn.dedup_key = key;
        kept.push(txn);
    }

    let mut overlaps: Vec<Overlap> = overlap_counts
        .into_iter()
        .map(|((file_a, file_b), count)| Overlap {
            file_a,
            file_b,
            count,
        })
        .collect();
    overlaps.sort_by(|a, b| {
        (a.file_a.as_str(), a.file_b.as_str()).cmp(&(b.file_a.as_str(), b.file_b.as_str()))
    });

    let report = DedupReport {
        removed,
        kept: kept.len(),
        overlaps,
    };
    (kept, report)
}

/// Order two filenames so overlap pairs are reported deterministically.
fn ordered_pair(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Currency, Money, TxnType};
    use jiff::civil::date;
    use std::str::FromStr;

    fn txn(file: &str, merchant: &str, amount: &str, day: i8) -> Transaction {
        Transaction {
            raw_description: merchant.to_string(),
            normalized_merchant: merchant.to_string(),
            date: date(2025, 1, day),
            time: None,
            amount: Money::from_str(amount).unwrap(),
            currency: Currency::new("CAD"),
            txn_type: TxnType::Spend,
            memo: None,
            cardholder: None,
            profile_id: "test".into(),
            source_file: file.to_string(),
            owner_person: None,
            account_id: None,
            harvested_category: None,
            dedup_key: String::new(),
        }
    }

    #[test]
    fn collapses_cross_file_duplicate() {
        let a = txn("file_a.csv", "SHELL", "-40.00", 2);
        let b = txn("file_b.csv", "SHELL", "-40.00", 2);
        let (kept, report) = dedup(vec![a, b]);
        assert_eq!(kept.len(), 1);
        assert_eq!(report.removed, 1);
        assert_eq!(report.overlaps.len(), 1);
        assert_eq!(report.overlaps[0].count, 1);
    }

    #[test]
    fn keeps_in_file_duplicates() {
        let a = txn("file_a.csv", "TIM HORTONS", "-3.50", 2);
        let b = txn("file_a.csv", "TIM HORTONS", "-3.50", 2);
        let (kept, report) = dedup(vec![a, b]);
        assert_eq!(kept.len(), 2);
        assert_eq!(report.removed, 0);
        // Distinct keys despite identical content.
        assert_ne!(kept[0].dedup_key, kept[1].dedup_key);
    }

    #[test]
    fn key_is_stable_across_runs() {
        let t = txn("file_a.csv", "SHELL", "-40.00", 2);
        let (k1, _) = dedup(vec![t.clone()]);
        let (k2, _) = dedup(vec![t]);
        assert_eq!(k1[0].dedup_key, k2[0].dedup_key);
    }
}
