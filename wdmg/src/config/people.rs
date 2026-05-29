//! Household identity config — mirrors `default_people.toml`.
//!
//! Models who owns each account and where transferred money goes. Transfer-rule
//! and account-matcher regexes are compiled once at load (validated, never
//! panicking).

use serde::{Deserialize, Serialize};

/// The full `people.toml` document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeopleDoc {
    /// Config schema version.
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    /// People in the household.
    #[serde(default, rename = "person")]
    pub persons: Vec<Person>,
    /// Transfer counterparty assignment rules.
    #[serde(default, rename = "transfer_rule")]
    pub transfer_rules: Vec<TransferRule>,
}

fn default_schema_version() -> u32 {
    1
}

/// A household member and their accounts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Person {
    /// Stable person id (e.g. `me`).
    pub id: String,
    /// Display name.
    pub display_name: String,
    /// Accounts owned by this person.
    #[serde(default, rename = "account")]
    pub accounts: Vec<Account>,
}

/// An account owned by a person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    /// Stable account id.
    pub id: String,
    /// Exact value of the profile's person column that maps here.
    #[serde(default)]
    pub cardholder: Option<String>,
    /// Last-4 captured by the profile's `account.id_from_filename`.
    #[serde(default)]
    pub card_last4: Option<String>,
    /// Regex on the source basename (disambiguates shared-header ledgers).
    #[serde(default)]
    pub filename: Option<String>,
    /// Restrict the match to a specific profile id.
    #[serde(default)]
    pub profile: Option<String>,
    /// Account kind (e.g. `debit`, `savings`, `credit_card`).
    pub kind: String,
    /// When true, holds no transactions but is a valid transfer destination.
    #[serde(default)]
    pub external: bool,
}

/// A transfer counterparty rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferRule {
    /// Regex matched against the transfer description.
    pub pattern: String,
    /// Person or account id to attribute the other leg to.
    pub to: String,
    /// Match priority (higher first); defaults to 0.
    #[serde(default)]
    pub priority: i32,
}
