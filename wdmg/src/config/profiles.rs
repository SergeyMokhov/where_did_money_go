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

impl ProfilesDoc {
    /// `self`'s profiles first (in order), followed by each profile of `base`
    /// whose id `self` does not define.
    #[must_use]
    pub fn layered_over(mut self, base: ProfilesDoc) -> ProfilesDoc {
        let mine: std::collections::HashSet<String> =
            self.profiles.iter().map(|p| p.id.clone()).collect();
        self.profiles
            .extend(base.profiles.into_iter().filter(|p| !mine.contains(&p.id)));
        self
    }
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
    /// Transaction-type column + value map. Optional: a source with no type
    /// column can classify rows by the amount's sign instead (`by_sign`).
    #[serde(rename = "type", default)]
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

impl MerchantSpec {
    /// Every description column, primary first.
    pub fn columns(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.column.as_str()).chain(self.extra_columns.iter().map(String::as_str))
    }
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
    /// More description columns (e.g. `Description2`, `Description3`). Their
    /// non-empty cells are appended to `column`'s, space-separated, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra_columns: Vec<String>,
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
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TypeSpec {
    /// Column carrying the raw type value, if the source has one.
    #[serde(default)]
    pub column: Option<String>,
    /// Raw value -> `spend|income|transfer`. Unmapped values fall back to
    /// `by_sign` when given, else become `Unknown`.
    #[serde(default)]
    pub map: BTreeMap<String, String>,
    /// Kind for rows the column/map does not classify, chosen by the amount's
    /// sign (after sign normalisation: negative = money out).
    #[serde(default)]
    pub by_sign: Option<BySign>,
}

/// Sign-based fallback kinds (`[profile.type.by_sign]`), each
/// `spend|income|transfer`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BySign {
    /// Kind for money out (negative amounts).
    pub negative: String,
    /// Kind for money in (positive amounts).
    pub positive: String,
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
