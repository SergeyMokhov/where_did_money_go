//! Identity resolution: assign each transaction an `owner_person` and `account_id`.
//!
//! An account in `people.toml` matches a transaction by any combination of:
//! - `cardholder` — case-insensitive exact value of the profile's person column,
//!   which ingest retains on [`Transaction::cardholder`] when the profile declares
//!   a `person.column` (e.g. PC's "Card Holder Name"),
//! - `card_last4` — the 4-digit group captured by the profile's
//!   `account.id_from_filename` regex applied to the source basename,
//! - `filename` — a regex on the source basename (disambiguates shared-header
//!   ledgers like debit vs. savings),
//! - `profile` — restrict the match to a specific profile id.
//!
//! All declared criteria on an account must hold (logical AND). The first
//! matching account in declaration order wins; accounts marked `external = true`
//! never source transactions, so they are excluded from matching here (they are
//! valid transfer destinations only, used by [`crate::reconcile`]).
//!
//! Per defensive-coding: filename regexes were compiled at config load, so this
//! phase only applies them. We mutate the transactions in place (setting the two
//! `Option` fields) because identity is intrinsic provenance, not analysis.

use std::collections::BTreeSet;

use crate::config::{CompiledAccountMatcher, Config};
use crate::model::Transaction;

/// The household's declared people and accounts, including `external` ones, so
/// reconcile and the breakdowns can enumerate every valid node up front.
#[derive(Debug, Clone, Default)]
pub struct Roster {
    /// All declared person ids (sorted, unique).
    pub people: Vec<String>,
    /// All declared account ids, including external ones (sorted, unique).
    pub accounts: Vec<String>,
    /// Account ids declared `external = true`.
    pub external_accounts: BTreeSet<String>,
}

impl Roster {
    /// Build the roster from config.
    #[must_use]
    pub fn from_config(config: &Config) -> Self {
        let mut people = BTreeSet::new();
        let mut accounts = BTreeSet::new();
        let mut external = BTreeSet::new();
        for person in &config.people.persons {
            people.insert(person.id.clone());
            for account in &person.accounts {
                accounts.insert(account.id.clone());
                if account.external {
                    external.insert(account.id.clone());
                }
            }
        }
        Self {
            people: people.into_iter().collect(),
            accounts: accounts.into_iter().collect(),
            external_accounts: external,
        }
    }
}

/// Apply `id_from_filename` of the transaction's profile to recover the captured
/// id (typically a 4-digit suffix).
fn captured_last4(config: &Config, txn: &Transaction) -> Option<String> {
    let profile = config
        .profiles
        .iter()
        .find(|p| p.profile.id == txn.profile_id)?;
    let re = profile.id_from_filename.as_ref()?;
    re.captures(&txn.source_file)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
}

/// Does this account matcher apply to `txn`? All declared criteria must hold.
fn matcher_applies(
    matcher: &CompiledAccountMatcher,
    txn: &Transaction,
    last4: Option<&str>,
) -> bool {
    let account = &matcher.account;

    // External accounts hold no transactions of their own.
    if account.external {
        return false;
    }

    // profile restriction
    if let Some(profile) = &account.profile {
        if profile != &txn.profile_id {
            return false;
        }
    }

    // filename regex
    if let Some(re) = &matcher.filename {
        if !re.is_match(&txn.source_file) {
            return false;
        }
    }

    // card_last4 must equal the captured filename group
    if let Some(want) = &account.card_last4 {
        match last4 {
            Some(got) if got == want => {}
            _ => return false,
        }
    }

    // cardholder: compared (case-insensitive, exact) against the transaction's
    // retained raw cardholder cell (the profile's `person.column`). Ingest now
    // preserves this in `Transaction::cardholder`, so a `cardholder = "..."`
    // criterion is a real, discriminating gate rather than a no-op.
    if let Some(want) = &account.cardholder {
        match &txn.cardholder {
            Some(got) if got.trim().eq_ignore_ascii_case(want.trim()) => {}
            _ => return false,
        }
    }

    // Require at least one structural/identifying criterion so a malformed account
    // with no criteria at all cannot blanket-match every transaction. A cardholder
    // now counts, since it is matched against a concrete retained cell.
    let has_structural = account.profile.is_some()
        || matcher.filename.is_some()
        || account.card_last4.is_some()
        || account.cardholder.is_some();
    if !has_structural {
        return false;
    }

    true
}

/// Every account whose criteria hold for `txn`, in declaration order. The first
/// one is what [`resolve`] assigns; any others are shadowed by it (diagnostics
/// for config editors: "these rows also fit account X").
#[must_use]
pub fn matching_accounts<'c>(config: &'c Config, txn: &Transaction) -> Vec<&'c str> {
    let last4 = captured_last4(config, txn);
    config
        .account_matchers
        .iter()
        .filter(|m| matcher_applies(m, txn, last4.as_deref()))
        .map(|m| m.account.id.as_str())
        .collect()
}

/// The id the transaction's profile captures from its file name
/// (`account.id_from_filename`, typically a card's last 4 digits).
#[must_use]
pub fn filename_id(config: &Config, txn: &Transaction) -> Option<String> {
    captured_last4(config, txn)
}

/// Resolve `owner_person` and `account_id` on every transaction in place.
///
/// Returns the [`Roster`] of declared people/accounts for downstream phases.
pub fn resolve(config: &Config, txns: &mut [Transaction]) -> Roster {
    for txn in txns.iter_mut() {
        let last4 = captured_last4(config, txn);
        let hit = config
            .account_matchers
            .iter()
            .find(|m| matcher_applies(m, txn, last4.as_deref()));
        if let Some(m) = hit {
            txn.owner_person = Some(m.person_id.clone());
            txn.account_id = Some(m.account.id.clone());
        }
    }
    Roster::from_config(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::model::{Currency, Money, TxnType};
    use jiff::civil::date;
    use std::str::FromStr;

    fn txn(profile: &str, file: &str) -> Transaction {
        Transaction {
            raw_description: "X".into(),
            normalized_merchant: "X".into(),
            date: date(2026, 1, 2),
            time: None,
            amount: Money::from_str("-10.00").unwrap(),
            currency: Currency::new("CAD"),
            txn_type: TxnType::Spend,
            memo: None,
            cardholder: None,
            profile_id: profile.into(),
            source_file: file.into(),
            owner_person: None,
            account_id: None,
            harvested_category: None,
            dedup_key: String::new(),
        }
    }

    #[test]
    fn roster_includes_external_accounts() {
        let cfg = Config::from_defaults().unwrap();
        let roster = Roster::from_config(&cfg);
        assert!(roster.people.contains(&"me".to_string()));
        assert!(roster.people.contains(&"spouse".to_string()));
        assert!(roster.external_accounts.contains("spouse_savings"));
        assert!(roster.accounts.contains(&"spouse_savings".to_string()));
    }

    #[test]
    fn resolves_pc_last4_to_account() {
        // card_last4 matching, tested with a custom config (the shipped defaults key
        // PC by cardholder, not last-4). mastercard_credit's id_from_filename = (\d{4})
        // captures 7001 from the filename, which the account requires.
        use crate::config::{CategoriesDoc, PeopleDoc, ProfilesDoc};
        let profiles: ProfilesDoc = toml::from_str(crate::config::defaults::profiles()).unwrap();
        let categories: CategoriesDoc = toml::from_str("schema_version = 1").unwrap();
        let people: PeopleDoc = toml::from_str(
            r#"
schema_version = 1
[[person]]
id = "me"
display_name = "Me"
[[person.account]]
id = "pc_last4"
card_last4 = "7001"
profile = "mastercard_credit"
kind = "credit_card"
"#,
        )
        .unwrap();
        let cfg = Config::build(profiles, categories, people).unwrap();
        let mut t = vec![txn("mastercard_credit", "7001 xxxx xxxx 7003.CSV")];
        resolve(&cfg, &mut t);
        assert_eq!(t[0].owner_person.as_deref(), Some("me"));
        assert_eq!(t[0].account_id.as_deref(), Some("pc_last4"));
    }

    #[test]
    fn resolves_debit_by_filename_regex() {
        let cfg = Config::from_defaults().unwrap();
        let mut t = vec![txn("bank_ledger", "Debit_Recent.csv")];
        resolve(&cfg, &mut t);
        assert_eq!(t[0].account_id.as_deref(), Some("ledger_debit"));
    }

    #[test]
    fn resolves_rewards_savings_by_filename() {
        let cfg = Config::from_defaults().unwrap();
        let mut t = vec![txn("bank_ledger", "rewards_savings.csv")];
        resolve(&cfg, &mut t);
        assert_eq!(t[0].account_id.as_deref(), Some("rewards_savings"));
    }

    #[test]
    fn first_bank_card_profile_only() {
        let cfg = Config::from_defaults().unwrap();
        let mut t = vec![txn("first_bank_card", "statement.csv")];
        resolve(&cfg, &mut t);
        assert_eq!(t[0].owner_person.as_deref(), Some("spouse"));
        assert_eq!(t[0].account_id.as_deref(), Some("first_bank"));
    }

    #[test]
    fn unmatched_stays_none() {
        let cfg = Config::from_defaults().unwrap();
        let mut t = vec![txn("unknown_profile", "mystery.csv")];
        resolve(&cfg, &mut t);
        assert!(t[0].owner_person.is_none());
        assert!(t[0].account_id.is_none());
    }

    #[test]
    fn pc_cardholder_resolves_to_person_me() {
        // A PC row whose retained "Card Holder Name" is FRY resolves to person
        // "me" via the default people.toml mastercard_credit account (cardholder="FRY").
        let cfg = Config::from_defaults().unwrap();
        let mut row = txn("mastercard_credit", "PC_Financial_2025.csv");
        row.cardholder = Some("FRY".into());
        let mut t = vec![row];
        resolve(&cfg, &mut t);
        assert_eq!(t[0].owner_person.as_deref(), Some("me"));
        assert_eq!(t[0].account_id.as_deref(), Some("mastercard_credit"));
    }

    #[test]
    fn cardholder_mismatch_blocks_pc() {
        // A different cardholder must not match the FRY-bound mastercard_credit account.
        let cfg = Config::from_defaults().unwrap();
        let mut row = txn("mastercard_credit", "PC_Financial_2025.csv");
        row.cardholder = Some("LEELA".into());
        let mut t = vec![row];
        resolve(&cfg, &mut t);
        assert_ne!(t[0].account_id.as_deref(), Some("mastercard_credit"));
    }
}
