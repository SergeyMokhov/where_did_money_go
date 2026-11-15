//! Integration tests for profile detection, parsing, normalization, and dedup
//! against realistic fixtures.

use std::path::{Path, PathBuf};
use std::str::FromStr;

use jiff::civil::date;
use rust_decimal::Decimal;

use wdmg::config::Config;
use wdmg::ingest::{detect, load_file};
use wdmg::model::{Money, TxnType};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn cfg() -> Config {
    Config::from_defaults().expect("defaults build")
}

#[test]
fn detects_mastercard_credit() {
    let c = cfg();
    let p = detect(&c, &fixtures().join("mastercard_credit_7001.csv")).unwrap();
    assert_eq!(p.profile.id, "mastercard_credit");
}

#[test]
fn parses_mastercard_credit_row() {
    let c = cfg();
    let txns = load_file(&c, &fixtures().join("mastercard_credit_7001.csv")).unwrap();
    let first = &txns[0];
    assert_eq!(first.date, date(2026, 1, 19));
    assert_eq!(
        first.amount,
        Money::new(Decimal::from_str("-21.72").unwrap())
    );
    assert_eq!(first.txn_type, TxnType::Spend);
    assert_eq!(first.normalized_merchant, "AMAZON");
    assert!(first.time.is_some(), "Mastercard carries a Time column");
    // City/prov stripped on the second row.
    assert_eq!(txns[1].normalized_merchant, "SHELL");
}

#[test]
fn detects_rewards_credit_and_harvests_category() {
    let c = cfg();
    let path = fixtures().join("rewards_credit_7002.csv");
    let p = detect(&c, &path).unwrap();
    assert_eq!(p.profile.id, "rewards_credit");
    let txns = load_file(&c, &path).unwrap();
    assert_eq!(txns[0].date, date(2025, 4, 17));
    assert_eq!(
        txns[0].harvested_category.as_deref(),
        Some("Foreign Currency Spend")
    );
    assert_eq!(txns[0].txn_type, TxnType::Spend);
}

#[test]
fn bank_ledger_handles_both_padded_and_unpadded_dates() {
    let c = cfg();

    let chequing = fixtures().join("ledger_debit_2025.csv");
    assert_eq!(detect(&c, &chequing).unwrap().profile.id, "bank_ledger");
    let chequing_txns = load_file(&c, &chequing).unwrap();
    assert_eq!(chequing_txns[0].date, date(2025, 1, 2));
    assert_eq!(
        chequing_txns[0].amount,
        Money::new(Decimal::from_str("-2425.00").unwrap())
    );

    let sav = fixtures().join("rewards_savings_2020.csv");
    assert_eq!(detect(&c, &sav).unwrap().profile.id, "bank_ledger");
    let sav_txns = load_file(&c, &sav).unwrap();
    // Non-leading-zero date 1/20/2020 parses.
    assert_eq!(sav_txns[0].date, date(2020, 1, 20));
    assert_eq!(
        sav_txns[0].normalized_merchant,
        "EFT WITHDRAWAL TO WATERWORKS"
    );
}

#[test]
fn first_bank_card_bom_blank_rows_leading_space_and_code_prefix() {
    let c = cfg();
    let path = fixtures().join("first_bank_card_7004.csv");
    let p = detect(&c, &path).unwrap();
    assert_eq!(p.profile.id, "first_bank_card");

    let txns = load_file(&c, &path).unwrap();
    assert_eq!(txns.len(), 1, "two blank rows must be skipped");
    let t = &txns[0];
    // %Y%m%d date.
    assert_eq!(t.date, date(2026, 1, 2));
    // -40.0 parses exactly; leading-space header column handled.
    assert_eq!(t.amount, Money::new(Decimal::from_str("-40.0").unwrap()));
    assert_eq!(t.txn_type, TxnType::Spend);
    // [CW] prefix stripped, whitespace collapsed, uppercased.
    assert!(t
        .normalized_merchant
        .starts_with("INTERAC ETRNSFR SENT FLOWERS"));
    assert!(!t.normalized_merchant.contains("[CW]"));
    assert!(!t.normalized_merchant.contains("  "));
    // The leading-apostrophe card number was not the merchant column, but verify
    // raw_description didn't keep stray quotes from the description column.
    assert!(!t.raw_description.starts_with('\''));
}

#[test]
fn dedup_collapses_cross_file_keeps_in_file() {
    let c = cfg();
    let mut all = Vec::new();
    // ledger_debit_2025 has SHELL -40.00 on 01/03; overlap file repeats it (cross-file
    // dup) plus two identical TIM HORTONS rows (in-file dups to keep).
    all.extend(load_file(&c, &fixtures().join("ledger_debit_2025.csv")).unwrap());
    all.extend(load_file(&c, &fixtures().join("ledger_debit_overlap.csv")).unwrap());

    let before = all.len();
    let (kept, report) = wdmg::dedup::dedup(all);
    // One SHELL row removed as a cross-file duplicate.
    assert_eq!(report.removed, 1);
    assert_eq!(kept.len(), before - 1);
    // The two in-file TIM HORTONS rows both survive.
    let tims = kept
        .iter()
        .filter(|t| t.normalized_merchant.starts_with("TIM HORTONS"))
        .count();
    assert_eq!(tims, 2);
    // Overlap reported with the right pair.
    assert_eq!(report.overlaps.len(), 1);
    assert_eq!(report.overlaps[0].count, 1);
    assert_eq!(report.overlaps[0].file_a, "ledger_debit_2025.csv");
    assert_eq!(report.overlaps[0].file_b, "ledger_debit_overlap.csv");
}

#[test]
fn load_dir_skips_non_csv_and_loads_all_profiles() {
    let c = cfg();
    let (txns, report) = wdmg::ingest::load_dir(&c, &fixtures()).unwrap();
    assert!(!txns.is_empty());
    // notes.txt is skipped, not errored.
    assert!(report.skipped.iter().any(|n| n == "notes.txt"));
    assert!(report.errors.is_empty(), "errors: {:?}", report.errors);
    // All four profiles exercised across the fixtures.
    let profiles: std::collections::BTreeSet<_> =
        report.loaded.iter().map(|(_, p, _)| p.clone()).collect();
    for want in [
        "mastercard_credit",
        "rewards_credit",
        "bank_ledger",
        "first_bank_card",
    ] {
        assert!(profiles.contains(want), "missing profile {want}");
    }
}

#[test]
fn missing_column_is_a_typed_error_not_a_panic() {
    use std::io::Write;
    let dir = std::env::temp_dir().join("wdmg-missing-col-test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("pc_broken.csv");
    // Has the mastercard_credit header_first_cell + contains markers, but drop "Amount".
    let mut f = std::fs::File::create(&path).unwrap();
    writeln!(
        f,
        r#""Description","Type","Card Holder Name","Date","Time""#
    )
    .unwrap();
    writeln!(f, r#""X","PURCHASE","FRY","01/01/2026","10:00 AM""#).unwrap();
    drop(f);

    let c = cfg();
    let p = detect(&c, &path).unwrap();
    let err = wdmg::ingest::read_file(p, &path).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("Amount"),
        "expected missing-column error, got: {msg}"
    );
}

#[test]
fn rewards_card_export_with_only_a_payment_is_detected() {
    // A real export can hold a single payment row with an empty memo, so the
    // profile must not depend on the "Rewards earned ~ Category:" memo.
    let csv = "Transaction date,Transaction,Name,Memo,Amount\n\
               03/02/2026,CREDIT,PAYMENT - THANK YOU,,54.10\n";
    let cfg = cfg();
    let p = wdmg::ingest::detect_text(&cfg, Path::new("1234 xxxx xxxx 5678.CSV"), csv)
        .expect("detected");
    assert_eq!(p.profile.id, "rewards_credit");
}

/// Defaults plus one extra profile appended (as the browser's column mapper does).
fn cfg_with(extra: &str) -> Result<Config, wdmg::error::ConfigError> {
    let mut texts = std::collections::BTreeMap::new();
    texts.insert(
        "profiles.toml".to_string(),
        format!("{}\n{extra}", wdmg::config::defaults::profiles()),
    );
    Config::from_texts(&texts)
}

const MAPPED: &str = r#"
[[profile]]
id = "mapped_card"
[profile.match]
header_first_cell = 'Posted'
header_contains   = ["Posted", "Details", "Value"]
[profile.date]
column  = "Posted"
formats = ["%Y-%m-%d"]
[profile.amount]
column = "Value"
sign   = "outflow_positive"
[profile.merchant]
column = "Details"
[profile.type.by_sign]
negative = "spend"
positive = "transfer"
[profile.account]
kind     = "credit_card"
currency = "CAD"
"#;

#[test]
fn profile_without_type_column_classifies_by_sign() {
    let cfg = cfg_with(MAPPED).expect("mapped profile builds");
    let csv = "Posted,Details,Value\n2026-11-01,COFFEE,4.50\n2026-11-02,PAYMENT,-100.00\n";
    let txns =
        wdmg::ingest::load_bytes(&cfg, Path::new("card.csv"), csv.as_bytes()).expect("parses");
    assert_eq!(txns[0].profile_id, "mapped_card");
    // outflow_positive: 4.50 is money out -> spend; -100 is money in -> transfer.
    assert_eq!(txns[0].txn_type, TxnType::Spend);
    assert!(txns[0].amount.amount().is_sign_negative());
    assert_eq!(txns[1].txn_type, TxnType::Transfer);
}

#[test]
fn by_sign_rejects_unknown_kinds() {
    let bad = MAPPED.replace("positive = \"transfer\"", "positive = \"refund\"");
    let err = cfg_with(&bad).unwrap_err().to_string();
    assert!(err.contains("by_sign") && err.contains("refund"), "{err}");
}

fn cfg_profiles_only(text: &str) -> Config {
    let mut texts = std::collections::BTreeMap::new();
    texts.insert("profiles.toml".to_string(), text.to_string());
    Config::from_texts(&texts).expect("config builds")
}

#[test]
fn user_profiles_add_to_builtins() {
    // A profiles.toml holding only a mapped format still recognises built-ins.
    let cfg = cfg_profiles_only(&format!("schema_version = 1\n{MAPPED}"));
    assert_eq!(cfg.profiles[0].profile.id, "mapped_card");
    let card_export = "Transaction date,Transaction,Name,Memo,Amount\n\
                     03/02/2026,CREDIT,PAYMENT - THANK YOU,,54.10\n";
    let p = wdmg::ingest::detect_text(&cfg, Path::new("x.csv"), card_export).expect("built-in");
    assert_eq!(p.profile.id, "rewards_credit");
}

#[test]
fn user_profile_overrides_builtin_with_same_id() {
    let over = MAPPED.replace("id = \"mapped_card\"", "id = \"rewards_credit\"");
    let cfg = cfg_profiles_only(&format!("schema_version = 1\n{over}"));
    let n = cfg
        .profiles
        .iter()
        .filter(|p| p.profile.id == "rewards_credit")
        .count();
    assert_eq!(n, 1, "the user's copy replaces the built-in");
    assert_eq!(cfg.profiles[0].profile.date.column, "Posted");
}

const MULTI_DESC: &str = r#"
[[profile]]
id = "some_bank_savings"
[profile.match]
header_first_cell = 'ID'
header_contains   = ["ID", "Date", "Account Name", "Description1", "Description2", "Description3", "Amount", "Balance"]
[profile.date]
column  = "Date"
formats = ["%Y-%m-%d", "%Y-%-m-%-d"]
[profile.amount]
column = "Amount"
sign   = "signed"
[profile.merchant]
column = "Description1"
extra_columns = ["Description2", "Description3"]
[profile.type.by_sign]
negative = "spend"
positive = "income"
[profile.account]
kind     = "debit"
currency = "CAD"
"#;

#[test]
fn dates_with_a_time_suffix_and_multi_column_descriptions() {
    let cfg = cfg_with(MULTI_DESC).expect("profile builds");
    let csv = "ID,Date,Account Name,Description1,Description2,Description3,Amount,Balance\n\
               1001,2026-03-31 12:00:00 AM,Savings - 0,\"Interest Credit\",\"Interest Amount 0.07\",,0.07,140.07\n\
               1002,2026-11-02T08:15:00Z,Savings - 0,Transfer out,,ref 77,-5.00,135.07\n\
               1003,2026-11-03,Savings - 0,Plain,,,1.00,21.94\n";
    let t =
        wdmg::ingest::load_bytes(&cfg, Path::new("savings.csv"), csv.as_bytes()).expect("parses");
    assert_eq!(t.len(), 3);
    assert_eq!(t[0].date.to_string(), "2026-03-31");
    assert_eq!(t[1].date.to_string(), "2026-11-02");
    assert_eq!(t[0].raw_description, "Interest Credit Interest Amount 0.07");
    assert_eq!(t[1].raw_description, "Transfer out ref 77");
    assert_eq!(t[2].raw_description, "Plain");
    assert_eq!(t[0].txn_type, TxnType::Income);
}

#[test]
fn a_time_suffix_never_rescues_a_bad_date() {
    let cfg = cfg_with(MULTI_DESC).expect("profile builds");
    let csv = "ID,Date,Account Name,Description1,Description2,Description3,Amount,Balance\n\
               1,2026-13-31 12:00:00 AM,S,X,,,1.00,1\n";
    assert!(wdmg::ingest::load_bytes(&cfg, Path::new("s.csv"), csv.as_bytes()).is_err());
}

#[test]
fn split_description_ledger_is_a_builtin_format() {
    let cfg = wdmg::config::Config::from_defaults().expect("defaults");
    let csv = " ID, Date, Account Name, Description1, Description2, Description3, Amount, Balance\n\
               100000001,2026-03-31 12:00:00 AM,Savings - 0,\"Interest Credit\",\"Interest Amount 0.07\",,0.07,140.07\n\
               100000002,2026-11-02 12:00:00 AM,Savings - 0,\"Transfer Out\",\"To chequing\",,-5.00,135.07\n";
    let t = wdmg::ingest::load_bytes(&cfg, Path::new("download.csv"), csv.as_bytes())
        .expect("recognised");
    assert_eq!(t[0].profile_id, "split_description_ledger");
    assert_eq!(t[0].date.to_string(), "2026-03-31");
    assert_eq!(t[0].raw_description, "Interest Credit Interest Amount 0.07");
    assert_eq!(t[0].txn_type, TxnType::Income);
    assert_eq!(t[1].txn_type, TxnType::Spend);
}
