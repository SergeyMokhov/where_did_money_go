//! Core domain types shared across all phases.
//!
//! Money is *never* an `f64`. [`Money`] wraps a [`rust_decimal::Decimal`] parsed
//! directly from the raw cell string to preserve exact cents, and serializes to
//! JSON/TOML as a string so no precision is lost on round-trip.

use std::fmt;
use std::str::FromStr;

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// An exact monetary amount. Internally a [`Decimal`]; serialized as a string.
///
/// Negative values denote outflows for "signed" sources. The sign convention is
/// resolved at ingest time per the source profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Money(#[serde(with = "rust_decimal::serde::str")] pub Decimal);

impl Money {
    /// Construct from a raw [`Decimal`].
    #[must_use]
    pub const fn new(value: Decimal) -> Self {
        Self(value)
    }

    /// The zero amount.
    #[must_use]
    pub const fn zero() -> Self {
        Self(Decimal::ZERO)
    }

    /// The underlying decimal.
    #[must_use]
    pub const fn amount(&self) -> Decimal {
        self.0
    }

    /// Canonical string form used in dedup keys: the exact decimal, normalized so
    /// that `-21.72` and `-21.720` hash identically.
    #[must_use]
    pub fn dedup_repr(&self) -> String {
        self.0.normalize().to_string()
    }
}

impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for Money {
    type Err = rust_decimal::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Tolerate thousands separators and surrounding whitespace; reject empties.
        let cleaned = s.trim().replace(',', "");
        Decimal::from_str(&cleaned).map(Money)
    }
}

impl From<Decimal> for Money {
    fn from(value: Decimal) -> Self {
        Self(value)
    }
}

/// How a transaction affects the household's money flow.
///
/// `Transfer` moves the owner's own money (credit-card payments, inter-account
/// moves) and is netted out of both spend and income. `Unknown` is the
/// fail-safe default for rows whose type column is unmapped — later phases
/// (categorize/reconcile) decide their fate rather than mis-counting them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TxnType {
    /// Outflow / spending.
    Spend,
    /// Inflow / income.
    Income,
    /// Movement of the owner's own money; netted from both flows.
    Transfer,
    /// Type not determined at ingest; resolved by later phases.
    Unknown,
}

impl TxnType {
    /// Parse the lowercase mapping value found in profile `type.map`.
    #[must_use]
    pub fn from_mapped(value: &str) -> Self {
        match value {
            "spend" => Self::Spend,
            "income" => Self::Income,
            "transfer" => Self::Transfer,
            _ => Self::Unknown,
        }
    }
}

impl fmt::Display for TxnType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Spend => "spend",
            Self::Income => "income",
            Self::Transfer => "transfer",
            Self::Unknown => "unknown",
        };
        f.write_str(s)
    }
}

/// A 3-letter ISO currency code (e.g. `CAD`). A newtype prevents mixing it up
/// with arbitrary strings; validated lightly at construction.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Currency(pub String);

impl Currency {
    /// Construct, uppercasing the code.
    #[must_use]
    pub fn new(code: impl Into<String>) -> Self {
        Self(code.into().to_uppercase())
    }
}

impl fmt::Display for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One normalized financial transaction.
///
/// Produced by [`crate::ingest`] + [`crate::normalize`], its `dedup_key` is
/// populated by [`crate::dedup`]. Fields that later phases fill (`owner_person`,
/// `account_id`, category outputs) are `Option` and start `None`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transaction {
    /// Description exactly as it appeared in the source (post-CSV-unquoting).
    pub raw_description: String,
    /// Cleaned, uppercased merchant string used for categorization & dedup.
    pub normalized_merchant: String,
    /// Calendar date of the transaction (no time zone).
    pub date: jiff::civil::Date,
    /// Wall-clock time if the source carried one (e.g. Mastercard). ISO `HH:MM:SS`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<jiff::civil::Time>,
    /// Signed amount; negative = outflow for signed sources.
    pub amount: Money,
    /// ISO currency code.
    pub currency: Currency,
    /// Effect on money flow.
    pub txn_type: TxnType,
    /// Free-text memo, if the source had one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
    /// Raw cardholder/owner cell from the source, when the profile declared a
    /// `person.column`. Used by the identity phase to match an account's
    /// `cardholder = "..."` criterion. `None` when the source carried no such column.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cardholder: Option<String>,
    /// The profile that parsed this row.
    pub profile_id: String,
    /// Basename of the file this row came from.
    pub source_file: String,
    /// Owning person id; set by the identity phase. `None` in phase 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_person: Option<String>,
    /// Owning account id; set by the identity phase. `None` in phase 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    /// Category harvested directly from the memo (e.g. Rewards rewards line).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harvested_category: Option<String>,
    /// Stable composite key set by [`crate::dedup`]; empty until then.
    #[serde(default)]
    pub dedup_key: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn money_roundtrips_through_string() {
        let m = Money::new(dec!(-21.72));
        let s = m.to_string();
        assert_eq!(Money::from_str(&s).unwrap(), m);
    }

    #[test]
    fn money_serializes_as_json_string() {
        let m = Money::new(dec!(-2425.00));
        let json = serde_json::to_string(&m).unwrap();
        assert_eq!(json, "\"-2425.00\"");
        let back: Money = serde_json::from_str(&json).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn money_dedup_repr_normalizes_trailing_zeros() {
        assert_eq!(Money::from_str("-40.0").unwrap().dedup_repr(), "-40");
        assert_eq!(Money::from_str("-21.720").unwrap().dedup_repr(), "-21.72");
    }

    #[test]
    fn txn_type_maps_unknown_to_unknown() {
        assert_eq!(TxnType::from_mapped("spend"), TxnType::Spend);
        assert_eq!(TxnType::from_mapped("OTHER"), TxnType::Unknown);
    }
}
