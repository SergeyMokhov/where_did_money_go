//! Statements in layouts no built-in format describes: the engine works the
//! columns out from the file. One case per common shape — accounting-import
//! layouts, US/Canadian/UK/EU bank and card exports — with made-up data.

use std::str::FromStr;

use rust_decimal::Decimal;
use wdmg::config::Config;
use wdmg::ingest::load_files;
use wdmg::model::{Transaction, TxnType};

fn load(name: &str, csv: &str) -> (Vec<Transaction>, String) {
    let c = Config::from_texts(&Default::default()).expect("defaults");
    let (txns, report) = load_files(&c, &[(name.to_string(), csv.as_bytes().to_vec())]);
    assert!(report.errors.is_empty(), "{name}: {:?}", report.errors);
    let profile = report
        .loaded
        .first()
        .map(|l| l.1.clone())
        .unwrap_or_default();
    (txns, profile)
}

fn amt(t: &Transaction) -> Decimal {
    t.amount.amount()
}
fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

#[test]
fn accounting_import_with_payee_and_day_first_dates() {
    let (t, p) = load(
        "export.csv",
        "Date,Amount,Payee,Description,Reference\n\
         01/09/2999,1800.00,Planet Express,Client payment - Planet Express,INV1042\n\
         03/09/2999,-950.00,Robot Arms Apts,Office rent September,SO-RENT\n\
         05/09/2999,-64.35,Paper & Co,Card payment - Office supplies,\n\
         25/09/2999,-6.00,Bank,Monthly account fee,FEE\n",
    );
    assert!(p.starts_with("auto-bank-"), "{p}");
    assert_eq!(t.len(), 4);
    // 25/09 can only be day-first, so 03/09 is 3 September.
    assert_eq!(t[1].date.to_string(), "2999-09-03");
    assert_eq!(t[1].raw_description, "Robot Arms Apts");
    assert_eq!(t[1].memo.as_deref(), Some("Office rent September"));
    assert_eq!(amt(&t[1]), d("-950.00"));
    assert_eq!(t[0].txn_type, TxnType::Income);
    assert_eq!(t[1].txn_type, TxnType::Spend);
}

#[test]
fn three_and_four_column_imports() {
    let (t, _) = load(
        "three.csv",
        "Date,Description,Amount\n01/09/2999,Client payment,1800.00\n03/09/2999,Office rent,-950.00\n25/09/2999,Fee,-6.00\n",
    );
    assert_eq!(amt(&t[1]), d("-950.00"));
    assert_eq!(t[2].date.to_string(), "2999-09-25");
    let (t, _) = load(
        "four.csv",
        "Date,Description,Credit,Debit\n01/09/2999,Client payment,1800.00,\n03/09/2999,Office rent,,950.00\n25/09/2999,Fee,,6.00\n",
    );
    assert_eq!(amt(&t[0]), d("1800.00"));
    assert_eq!(amt(&t[1]), d("-950.00"));
}

#[test]
fn money_out_money_in_with_balance() {
    let (t, _) = load(
        "statement.csv",
        "Date,Description,Money out,Money in,Balance\n\
         01/09/2999,Client payment,,1800.00,3050.00\n\
         03/09/2999,Office rent,950.00,,2100.00\n\
         25/09/2999,Fee,6.00,,2094.00\n",
    );
    assert_eq!(amt(&t[0]), d("1800.00"));
    assert_eq!(amt(&t[1]), d("-950.00"));
    assert_eq!(t[2].date.to_string(), "2999-09-25");
}

#[test]
fn card_with_transaction_and_post_dates_payment_is_a_transfer() {
    let (t, p) = load(
        "card_1234.csv",
        "Transaction Date,Post Date,Description,Category,Type,Amount,Memo\n\
         09/02/2999,09/03/2999,SLURM VENDING,Food & Drink,Sale,-4.25,\n\
         09/05/2999,09/05/2999,PAYMENT THANK YOU,,Payment,250.00,\n\
         09/07/2999,09/08/2999,MOMCORP,Shopping,Return,12.00,\n",
    );
    assert!(p.starts_with("auto-card-"), "{p}");
    assert_eq!(t[0].date.to_string(), "2999-09-02");
    assert_eq!(t[0].txn_type, TxnType::Spend);
    assert_eq!(t[1].txn_type, TxnType::Transfer);
}

#[test]
fn card_that_writes_charges_positive() {
    let (t, p) = load(
        "card.csv",
        "Date,Description,Card Member,Amount\n\
         09/01/2999,SLURM VENDING,P J FRY,4.25\n\
         09/02/2999,FISHY JOES,P J FRY,18.40\n\
         09/03/2999,MOMCORP,P J FRY,99.99\n\
         09/04/2999,ROBOT OIL,P J FRY,12.00\n\
         09/05/2999,ONLINE PAYMENT - THANK YOU,P J FRY,-134.64\n\
         09/06/2999,ELZARS,P J FRY,35.00\n",
    );
    assert!(
        p.starts_with("auto-card-") && p.ends_with("charges_positive"),
        "{p}"
    );
    assert_eq!(amt(&t[0]), d("-4.25"));
    assert_eq!(amt(&t[4]), d("134.64"));
    assert_eq!(t[4].txn_type, TxnType::Transfer);
    assert_eq!(t[0].cardholder.as_deref(), Some("P J FRY"));
}

#[test]
fn card_with_card_number_and_debit_credit() {
    let (t, p) = load(
        "card.csv",
        "Transaction Date,Posted Date,Card No.,Description,Category,Debit,Credit\n\
         2999-09-01,2999-09-02,1234,SLURM VENDING,Dining,4.25,\n\
         2999-09-05,2999-09-05,1234,PAYMENT RECEIVED,Payment/Credit,,250.00\n",
    );
    assert!(p.starts_with("auto-card-"), "{p}");
    assert_eq!(amt(&t[0]), d("-4.25"));
    assert_eq!(amt(&t[1]), d("250.00"));
    assert_eq!(t[1].txn_type, TxnType::Transfer);
}

#[test]
fn summary_lines_above_the_header_and_a_balance_row() {
    let (t, _) = load(
        "stmt.csv",
        "Description,,Summary Amt.\n\
         Beginning balance as of 09/01/2999,,\"1,000.00\"\n\
         Total credits,,\"1,800.00\"\n\
         Ending balance as of 09/30/2999,,\"1,845.75\"\n\
         \n\
         Date,Description,Amount,Running Bal.\n\
         09/01/2999,Beginning balance as of 09/01/2999,,\"1,000.00\"\n\
         09/02/2999,\"PLANET EXPRESS PAYROLL\",\"1,800.00\",\"2,800.00\"\n\
         09/03/2999,\"SLURM VENDING\",\"-4.25\",\"2,795.75\"\n",
    );
    assert_eq!(t.len(), 2, "the balance row is skipped");
    assert_eq!(amt(&t[0]), d("1800.00"));
    assert_eq!(t[1].raw_description, "SLURM VENDING");
}

#[test]
fn headerless_debit_credit_balance() {
    let (t, p) = load(
        "accountactivity.csv",
        "09/01/2999,PLANET EXPRESS PAYROLL,,1800.00,2800.00\n\
         09/02/2999,SLURM VENDING,4.25,,2795.75\n\
         09/03/2999,FISHY JOES,18.40,,2777.35\n",
    );
    assert!(p.contains("noheader"), "{p}");
    assert_eq!(amt(&t[0]), d("1800.00"));
    assert_eq!(amt(&t[1]), d("-4.25"));
    assert_eq!(t[1].raw_description, "SLURM VENDING");
}

#[test]
fn headerless_signed_amount_with_filler_columns() {
    let (t, _) = load(
        "checking.csv",
        "\"09/01/2999\",\"1800.00\",\"*\",\"\",\"PLANET EXPRESS PAYROLL\"\n\
         \"09/02/2999\",\"-4.25\",\"*\",\"\",\"SLURM VENDING 0042\"\n\
         \"09/03/2999\",\"-18.40\",\"*\",\"1234\",\"CHECK # 1234\"\n",
    );
    assert_eq!(amt(&t[1]), d("-4.25"));
    assert_eq!(t[1].raw_description, "SLURM VENDING 0042");
}

#[test]
fn pending_and_reverted_rows_are_left_out() {
    let (t, _) = load(
        "account-statement.csv",
        "Type,Product,Started Date,Completed Date,Description,Amount,Fee,Currency,State,Balance\n\
         CARD_PAYMENT,Current,2999-09-01 10:00:00,2999-09-02 09:00:00,Slurm,-4.25,0.00,GBP,COMPLETED,95.75\n\
         CARD_PAYMENT,Current,2999-09-03 11:00:00,,Fishy Joes,-18.40,0.00,GBP,PENDING,\n\
         CARD_PAYMENT,Current,2999-09-04 12:00:00,2999-09-04 12:30:00,Momcorp,-9.99,0.00,GBP,REVERTED,\n",
    );
    assert_eq!(t.len(), 1);
    assert_eq!(t[0].date.to_string(), "2999-09-01");
    assert_eq!(t[0].currency.to_string(), "GBP");
}

#[test]
fn semicolons_and_decimal_commas() {
    let (t, _) = load(
        "umsaetze.csv",
        "Buchungstag;Verwendungszweck;Betrag\n01.09.2999;SLURM GMBH;-1.234,50\n15.09.2999;GEHALT PLANET EXPRESS;2.000,00\n",
    );
    assert_eq!(amt(&t[0]), d("-1234.50"));
    assert_eq!(t[1].date.to_string(), "2999-09-15");
}

#[test]
fn unsigned_amounts_with_a_debit_credit_indicator() {
    let (t, _) = load(
        "transactions.csv",
        "Posting Date,Transaction Date,Amount,Credit Debit Indicator,Description\n\
         09/02/2999,09/01/2999,4.25,Debit,SLURM VENDING\n\
         09/03/2999,09/03/2999,1800.00,Credit,PLANET EXPRESS PAYROLL\n",
    );
    assert_eq!(amt(&t[0]), d("-4.25"));
    assert_eq!(amt(&t[1]), d("1800.00"));
    assert_eq!(t[0].date.to_string(), "2999-09-01");
}

#[test]
fn multi_account_export_with_currency_columns() {
    let (t, _) = load(
        "csv12345.csv",
        "Account Type,Account Number,Transaction Date,Cheque Number,Description 1,Description 2,CAD$,USD$\n\
         Chequing,00000-1234567,9/1/2999,,PAYROLL,PLANET EXPRESS,1800.00,\n\
         Chequing,00000-1234567,9/2/2999,,Interac purchase,SLURM VENDING,-4.25,\n",
    );
    assert_eq!(amt(&t[1]), d("-4.25"));
    assert_eq!(t[1].raw_description, "Interac purchase");
    assert_eq!(t[1].memo.as_deref(), Some("SLURM VENDING"));
    assert_eq!(t[1].cardholder.as_deref(), Some("00000-1234567"));
}

#[test]
fn a_built_in_format_still_wins() {
    let (_, p) = load(
        "chequing.csv",
        "Date,Transaction,Name,Memo,Amount\n01/02/2999,DEBIT,SLURM,,-4.25\n",
    );
    assert_eq!(p, "bank_ledger");
}

#[test]
fn a_csv_that_is_not_a_statement_is_still_rejected() {
    let c = Config::from_texts(&Default::default()).unwrap();
    let csv = "name,email,age\nFry,fry@example.com,1025\nLeela,leela@example.com,30\n";
    let (t, report) = load_files(&c, &[("people.csv".into(), csv.as_bytes().to_vec())]);
    assert!(t.is_empty());
    assert_eq!(report.errors.len(), 1);
}
