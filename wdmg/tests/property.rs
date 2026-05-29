//! Property-based round-trip tests (per testing-strategy: round-trip props catch
//! input classes example tests miss).
//!
//! (a) Money: `parse(format(x)) == x`.
//! (b) Config docs: `from_str(to_string(cfg)) == cfg` for profiles/categories/people,
//!     guaranteeing future `add-*` edits are lossless.
//! (c) Dedup key stability: the same transaction always yields the same key.

use std::str::FromStr;

use proptest::prelude::*;
use rust_decimal::Decimal;

use wdmg::config::{CategoriesDoc, PeopleDoc, ProfilesDoc};
use wdmg::model::Money;

// ---- (a) Money round-trip --------------------------------------------------

proptest! {
    #[test]
    fn money_parse_format_roundtrip(units in -1_000_000_000_i64..1_000_000_000, scale in 0u32..6) {
        let d = Decimal::new(units, scale);
        let m = Money::new(d);
        let s = m.to_string();
        prop_assert_eq!(Money::from_str(&s).unwrap(), m);
    }

    #[test]
    fn money_json_roundtrip(units in -1_000_000_000_i64..1_000_000_000, scale in 0u32..6) {
        let m = Money::new(Decimal::new(units, scale));
        let json = serde_json::to_string(&m).unwrap();
        let back: Money = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(back, m);
    }
}

// ---- (b) Config TOML round-trips on the shipped defaults --------------------

#[test]
fn profiles_doc_roundtrips() {
    let doc: ProfilesDoc = toml::from_str(wdmg::config::DEFAULT_PROFILES).unwrap();
    let s = toml::to_string(&doc).unwrap();
    let back: ProfilesDoc = toml::from_str(&s).unwrap();
    assert_eq!(doc, back);
}

#[test]
fn categories_doc_roundtrips() {
    let doc: CategoriesDoc = toml::from_str(wdmg::config::DEFAULT_CATEGORIES).unwrap();
    let s = toml::to_string(&doc).unwrap();
    let back: CategoriesDoc = toml::from_str(&s).unwrap();
    assert_eq!(doc, back);
}

#[test]
fn people_doc_roundtrips() {
    let doc: PeopleDoc = toml::from_str(wdmg::config::DEFAULT_PEOPLE).unwrap();
    let s = toml::to_string(&doc).unwrap();
    let back: PeopleDoc = toml::from_str(&s).unwrap();
    assert_eq!(doc, back);
}

// A generated-config round-trip: synthesize categories docs and ensure they
// survive a serialize/parse cycle (models future `add-category` edits).
proptest! {
    #[test]
    fn synthesized_categories_roundtrip(
        names in proptest::collection::vec("[A-Za-z][A-Za-z ]{0,10}", 1..4),
        min_units in 0i64..100_000,
    ) {
        let mut doc = CategoriesDoc {
            schema_version: 1,
            categories: Vec::new(),
        };
        for (i, name) in names.into_iter().enumerate() {
            doc.categories.push(wdmg::config::Category {
                name: format!("{name}{i}"),
                kind: wdmg::config::CategoryKind::Spend,
                subcategories: vec![wdmg::config::Subcategory {
                    name: format!("sub{i}"),
                    patterns: vec!["FOO".into(), "BAR".into()],
                    amount_min: Some(Decimal::new(min_units, 2)),
                    amount_max: None,
                    priority: i as i32,
                }],
            });
        }
        let s = toml::to_string(&doc).unwrap();
        let back: CategoriesDoc = toml::from_str(&s).unwrap();
        prop_assert_eq!(doc, back);
    }
}

// ---- (c) Dedup key stability -----------------------------------------------

proptest! {
    #[test]
    fn dedup_key_is_stable(
        merchant in "[A-Z ]{1,20}",
        units in -1_000_000i64..1_000_000,
        day in 1i8..=28,
    ) {
        use jiff::civil::date;
        use wdmg::model::{Currency, Transaction, TxnType};

        let make = || Transaction {
            raw_description: merchant.clone(),
            normalized_merchant: merchant.clone(),
            date: date(2025, 1, day),
            time: None,
            amount: Money::new(Decimal::new(units, 2)),
            currency: Currency::new("CAD"),
            txn_type: TxnType::Spend,
            memo: None,
            cardholder: None,
            profile_id: "p".into(),
            source_file: "f.csv".into(),
            owner_person: None,
            account_id: None,
            harvested_category: None,
            dedup_key: String::new(),
        };
        let (k1, _) = wdmg::dedup::dedup(vec![make()]);
        let (k2, _) = wdmg::dedup::dedup(vec![make()]);
        prop_assert_eq!(&k1[0].dedup_key, &k2[0].dedup_key);
        prop_assert!(!k1[0].dedup_key.is_empty());
    }
}
