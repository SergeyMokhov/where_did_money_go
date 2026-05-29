//! Category config — mirrors `default_categories.toml`.
//!
//! Patterns are case-insensitive, unanchored regexes matched against the
//! normalized merchant. They are compiled once at load (validated, never
//! panicking). The overlay merge order is documented on [`merge`]:
//! `user categories.toml > learned.toml > defaults`.

use serde::{Deserialize, Serialize};

/// The full `categories.toml` (or `learned.toml`) document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CategoriesDoc {
    /// Config schema version.
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    /// Top-level categories.
    #[serde(default, rename = "category")]
    pub categories: Vec<Category>,
}

fn default_schema_version() -> u32 {
    1
}

/// A top-level category (e.g. `Car`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Category {
    /// Display name.
    pub name: String,
    /// Spend / income / transfer.
    pub kind: CategoryKind,
    /// Subcategories carrying the actual match patterns.
    #[serde(default, rename = "subcategory")]
    pub subcategories: Vec<Subcategory>,
}

/// How a category counts toward flows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CategoryKind {
    /// Counted as outflow.
    Spend,
    /// Counted as inflow.
    Income,
    /// Netted out of both flows.
    Transfer,
}

/// A subcategory with match patterns and optional amount bounds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subcategory {
    /// Display name.
    pub name: String,
    /// Unanchored, case-insensitive regex patterns against the normalized merchant.
    #[serde(default)]
    pub patterns: Vec<String>,
    /// Inclusive lower bound on the (absolute) amount, written as a decimal string.
    #[serde(default, with = "opt_decimal_str")]
    pub amount_min: Option<rust_decimal::Decimal>,
    /// Inclusive upper bound on the (absolute) amount, written as a decimal string.
    #[serde(default, with = "opt_decimal_str")]
    pub amount_max: Option<rust_decimal::Decimal>,
    /// Tie-break priority (higher wins); defaults to 0.
    #[serde(default)]
    pub priority: i32,
}

/// Serialize `Option<Decimal>` as an optional string so amount bounds round-trip
/// exactly (never via float).
mod opt_decimal_str {
    use rust_decimal::Decimal;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(
        value: &Option<Decimal>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(d) => serializer.serialize_some(&d.to_string()),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Decimal>, D::Error> {
        let opt = Option::<String>::deserialize(deserializer)?;
        match opt {
            Some(s) => s
                .parse::<Decimal>()
                .map(Some)
                .map_err(serde::de::Error::custom),
            None => Ok(None),
        }
    }
}

impl CategoriesDoc {
    /// Layer overlay documents on top of `self` (the defaults).
    ///
    /// Merge order, lowest to highest precedence: `self` (defaults) < `learned`
    /// < `user`. A later document fully replaces an earlier subcategory that has
    /// the same `(category name, subcategory name)`; categories/subcategories not
    /// present in an overlay are retained from the base. This keeps user edits and
    /// learned rules additive without losing the shipped defaults.
    ///
    /// Phase-1 implementation is intentionally minimal but type-complete: later
    /// phases can refine the merge (e.g. pattern-level union) without changing the
    /// call sites.
    #[must_use]
    pub fn merge(
        self,
        learned: Option<CategoriesDoc>,
        user: Option<CategoriesDoc>,
    ) -> CategoriesDoc {
        let mut acc = self;
        if let Some(l) = learned {
            acc = acc.overlay(l);
        }
        if let Some(u) = user {
            acc = acc.overlay(u);
        }
        acc
    }

    /// Overlay `other` on top of `self`: replace matching subcategories by name,
    /// append new categories/subcategories.
    fn overlay(mut self, other: CategoriesDoc) -> CategoriesDoc {
        self.schema_version = other.schema_version.max(self.schema_version);
        for other_cat in other.categories {
            match self
                .categories
                .iter_mut()
                .find(|c| c.name == other_cat.name)
            {
                Some(existing) => {
                    existing.kind = other_cat.kind;
                    for other_sub in other_cat.subcategories {
                        match existing
                            .subcategories
                            .iter_mut()
                            .find(|s| s.name == other_sub.name)
                        {
                            Some(s) => *s = other_sub,
                            None => existing.subcategories.push(other_sub),
                        }
                    }
                }
                None => self.categories.push(other_cat),
            }
        }
        self
    }
}
