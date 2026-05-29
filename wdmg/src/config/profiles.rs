//! Source/bank profile config — mirrors `default_profiles.toml`.
//!
//! Detection rules and parsing instructions for one CSV export layout. Regexes
//! are stored as raw strings here; [`crate::config`] compiles them once at load
//! and validates them, returning a [`ConfigError`](crate::error::ConfigError)
//! rather than panicking on a bad pattern.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The full `profiles.toml` document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfilesDoc {
    /// Config schema version.
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    /// Ordered profiles; matched top-to-bottom, first match wins.
    #[serde(default, rename = "profile")]
    pub profiles: Vec<Profile>,
}

fn default_schema_version() -> u32 {
    1
}

/// One source layout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    /// Stable profile id (e.g. `mastercard_credit`).
    pub id: String,
    /// Detection rules.
    #[serde(rename = "match")]
    pub match_rules: MatchRules,
    /// Encoding hints.
    #[serde(default)]
    pub encoding: Encoding,
    /// Row layout hints.
    #[serde(default)]
    pub layout: Layout,
    /// Date column + format(s).
    pub date: DateSpec,
    /// Amount column + sign handling.
    pub amount: AmountSpec,
    /// Merchant column + cleanup.
    pub merchant: MerchantSpec,
    /// Optional memo column + category harvest.
    #[serde(default)]
    pub memo: Option<MemoSpec>,
    /// Transaction-type column + value map.
    #[serde(rename = "type")]
    pub type_spec: TypeSpec,
    /// Optional person/cardholder column.
    #[serde(default)]
    pub person: Option<PersonSpec>,
    /// Account metadata.
    pub account: AccountSpec,
}

/// Detection rules (`[profile.match]`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MatchRules {
    /// First header cell after BOM/quote stripping. May be a regex alternation.
    #[serde(default)]
    pub header_first_cell: Option<String>,
    /// Substrings that must all appear in the header line.
    #[serde(default)]
    pub header_contains: Vec<String>,
    /// Regex that must match somewhere in the first few data rows.
    #[serde(default)]
    pub content_probe: Option<String>,
    /// Regex on the file basename.
    #[serde(default)]
    pub filename: Option<String>,
}

/// Encoding hints (`[profile.encoding]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Encoding {
    /// Encoding name (only `utf8` is supported in phase 1).
    #[serde(default = "default_encoding")]
    pub encoding: String,
    /// Whether a UTF-8 BOM precedes the header.
    #[serde(default)]
    pub has_bom: bool,
}

fn default_encoding() -> String {
    "utf8".to_string()
}

impl Default for Encoding {
    fn default() -> Self {
        Self {
            encoding: default_encoding(),
            has_bom: false,
        }
    }
}

/// Row layout hints (`[profile.layout]`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Layout {
    /// Number of blank rows to skip after the header.
    #[serde(default)]
    pub skip_blank_rows_after_header: usize,
    /// Field delimiter; defaults to comma.
    #[serde(default)]
    pub delimiter: Option<char>,
}

/// Date column + strptime format(s) (`[profile.date]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DateSpec {
    /// Column carrying the date.
    pub column: String,
    /// Single strptime format.
    #[serde(default)]
    pub format: Option<String>,
    /// List of formats; first that parses wins.
    #[serde(default)]
    pub formats: Vec<String>,
}

impl DateSpec {
    /// All formats to try, in order (single `format` first, then `formats`).
    #[must_use]
    pub fn all_formats(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(f) = &self.format {
            out.push(f.clone());
        }
        out.extend(self.formats.iter().cloned());
        out
    }
}

/// How amounts are signed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sign {
    /// Negative = outflow, positive = inflow (phase-1 supported).
    Signed,
    /// All amounts positive, treated as outflow (hook for later phases).
    OutflowPositive,
    /// Separate debit/credit columns (hook for later phases).
    SeparateDebitCredit,
}

/// Amount column + sign handling (`[profile.amount]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AmountSpec {
    /// Column carrying the amount (may contain a leading space, e.g. `" Transaction Amount"`).
    pub column: String,
    /// Sign convention.
    pub sign: Sign,
    /// Debit column when `sign = separate_debit_credit`.
    #[serde(default)]
    pub debit_column: Option<String>,
    /// Credit column when `sign = separate_debit_credit`.
    #[serde(default)]
    pub credit_column: Option<String>,
}

/// Merchant column + cleanup (`[profile.merchant]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MerchantSpec {
    /// Column carrying the merchant/description.
    pub column: String,
    /// Optional regex matching a code prefix to strip (e.g. `[CW]`).
    #[serde(default)]
    pub strip_codes: Option<String>,
    /// Trim mode (`trailing_whitespace`).
    #[serde(default)]
    pub trim: Option<String>,
}

/// Memo column + category harvest (`[profile.memo]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoSpec {
    /// Column carrying the memo.
    pub column: String,
    /// Regex with a named group `cat` to harvest a category from the memo.
    #[serde(default)]
    pub category_harvest: Option<String>,
}

/// Transaction-type column + mapping (`[profile.type]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeSpec {
    /// Column carrying the raw type value.
    pub column: String,
    /// Raw value -> `spend|income|transfer`. Unmapped values become `Unknown`.
    #[serde(default)]
    pub map: BTreeMap<String, String>,
}

/// Person/cardholder column (`[profile.person]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersonSpec {
    /// Column carrying the cardholder/owner name.
    pub column: String,
}

/// Account metadata (`[profile.account]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountSpec {
    /// Account kind (e.g. `credit_card`, `debit`).
    pub kind: String,
    /// ISO currency code.
    pub currency: String,
    /// Regex capturing an account id (e.g. last-4) from the filename.
    #[serde(default)]
    pub id_from_filename: Option<String>,
}
