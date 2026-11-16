//! Structured edits to the user's `categories.toml` that keep comments and layout.
//!
//! The user file is an *overlay*: the engine layers it over the built-in
//! categories (and `learned.toml`), and a user subcategory replaces the built-in
//! one with the same `(category, subcategory)` name. So an edit to a built-in
//! subcategory first copies it — patterns, amount limits, priority — into the
//! user file ("copy on write") and changes the copy. Built-ins can't be removed
//! from the layered result, only emptied: deleting one writes it with no
//! patterns, which turns it off.
//!
//! The browser app's categories editor sends a list of [`CategoryOp`]s; the
//! result is re-parsed and every pattern is checked to compile before it is
//! returned.

use rust_decimal::Decimal;
use serde::Deserialize;
use toml_edit::{value, Array, ArrayOfTables, DocumentMut, Item, Table};

use super::categories::{
    pin_key, CategoriesDoc, Category, CategoryKind, PinKey, RangedPattern, Subcategory,
};

/// One edit. JSON: `{"op": "add_pattern", "category": "...", ...}`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum CategoryOp {
    /// Add a pattern to a subcategory, creating the category/subcategory when
    /// needed (a new category gets `kind`, default spend).
    AddPattern {
        /// Category name.
        category: String,
        /// Subcategory name.
        subcategory: String,
        /// Case-insensitive, unanchored regex.
        pattern: String,
        /// Kind for a new category.
        #[serde(default)]
        kind: Option<CategoryKind>,
        /// Give the pattern its own amount range (replacing the subcategory's);
        /// absent = a plain pattern under the subcategory's range.
        #[serde(default)]
        range: Option<AmountRange>,
    },
    /// Change a pattern in place (keeps its position): new text and/or range.
    EditPattern {
        /// Category name.
        category: String,
        /// Subcategory name.
        subcategory: String,
        /// The pattern as written now.
        from: String,
        /// The new pattern.
        pattern: String,
        /// Its own amount range, or absent for the subcategory's.
        #[serde(default)]
        range: Option<AmountRange>,
    },
    /// Remove one pattern (plain or ranged) from a subcategory.
    RemovePattern {
        /// Category name.
        category: String,
        /// Subcategory name.
        subcategory: String,
        /// The pattern, exactly as written.
        pattern: String,
    },
    /// Create an (empty) subcategory, and its category when new.
    AddSubcategory {
        /// Category name.
        category: String,
        /// Subcategory name.
        subcategory: String,
        /// Kind for a new category.
        #[serde(default)]
        kind: Option<CategoryKind>,
    },
    /// Only apply a subcategory's patterns within an amount range (absolute
    /// value, inclusive). `None` removes that limit.
    SetAmounts {
        /// Category name.
        category: String,
        /// Subcategory name.
        subcategory: String,
        /// Lower limit, as a decimal string.
        #[serde(default)]
        amount_min: Option<String>,
        /// Upper limit, as a decimal string.
        #[serde(default)]
        amount_max: Option<String>,
    },
    /// Tie-break priority for equally specific patterns (higher wins).
    SetPriority {
        /// Category name.
        category: String,
        /// Subcategory name.
        subcategory: String,
        /// Priority.
        priority: i32,
    },
    /// Change how a category counts: spend, income or transfer.
    SetKind {
        /// Category name.
        category: String,
        /// New kind.
        kind: CategoryKind,
    },
    /// Rename a category (built-in: copied under the new name, old one emptied).
    RenameCategory {
        /// Current name.
        from: String,
        /// New name.
        to: String,
    },
    /// Rename a subcategory within its category.
    RenameSubcategory {
        /// Category name.
        category: String,
        /// Current name.
        from: String,
        /// New name.
        to: String,
    },
    /// Move a subcategory (with its patterns) to another category.
    MoveSubcategory {
        /// Current category.
        category: String,
        /// Subcategory name.
        subcategory: String,
        /// Destination category (created as spend when new).
        to_category: String,
    },
    /// Delete a subcategory (built-in: emptied, which turns it off).
    DeleteSubcategory {
        /// Category name.
        category: String,
        /// Subcategory name.
        subcategory: String,
    },
    /// Delete a category and all its subcategories.
    DeleteCategory {
        /// Category name.
        category: String,
    },
    /// Put one transaction (date + signed amount + bank wording) into a
    /// subcategory, whatever the patterns say; creates the category/subcategory
    /// when needed. Replaces an earlier pin of the same transaction.
    Pin {
        /// `YYYY-MM-DD`.
        date: String,
        /// Signed amount, as a decimal string.
        amount: String,
        /// The bank's description of the transaction.
        description: String,
        /// Category name.
        category: String,
        /// Subcategory name.
        subcategory: String,
        /// Kind for a new category.
        #[serde(default)]
        kind: Option<CategoryKind>,
    },
    /// Remove the pin of one transaction.
    Unpin {
        /// `YYYY-MM-DD`.
        date: String,
        /// Signed amount, as a decimal string.
        amount: String,
        /// The bank's description of the transaction.
        description: String,
    },
}

/// A pattern's own amount range (absolute amounts, inclusive; empty = no limit).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
pub struct AmountRange {
    /// Lower limit, as a decimal string.
    #[serde(default)]
    pub amount_min: Option<String>,
    /// Upper limit, as a decimal string.
    #[serde(default)]
    pub amount_max: Option<String>,
}

/// Parse an optional amount limit ("" = none); the absolute value is kept.
fn parse_limit(v: Option<&str>, what: &str) -> R<Option<Decimal>> {
    match v.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(s) => s
            .parse::<Decimal>()
            .map(|d| Some(d.abs().normalize()))
            .map_err(|_| CategoriesEditError::Invalid(format!("{what} {s:?} isn't a number"))),
    }
}

/// Parse both limits and check their order.
fn parse_range(min: Option<&str>, max: Option<&str>) -> R<(Option<Decimal>, Option<Decimal>)> {
    let (lo, hi) = (
        parse_limit(min, "the lower limit")?,
        parse_limit(max, "the upper limit")?,
    );
    if let (Some(a), Some(b)) = (lo, hi) {
        if a > b {
            return Err(CategoriesEditError::Invalid(format!(
                "the lower limit {a} is above the upper limit {b}"
            )));
        }
    }
    Ok((lo, hi))
}

/// `{ pattern = "...", amount_min = "...", amount_max = "..." }`.
fn ranged_value(r: &RangedPattern) -> toml_edit::Value {
    let mut t = toml_edit::InlineTable::new();
    t.insert("pattern", r.pattern.as_str().into());
    if let Some(m) = r.amount_min {
        t.insert("amount_min", m.to_string().into());
    }
    if let Some(m) = r.amount_max {
        t.insert("amount_max", m.to_string().into());
    }
    toml_edit::Value::InlineTable(t)
}

/// Write a subcategory's patterns, limits, priority and ranged patterns into `t`.
fn write_sub(t: &mut Table, s: &Subcategory) {
    t["patterns"] = value(s.patterns.iter().map(String::as_str).collect::<Array>());
    for k in ["amount_min", "amount_max", "priority", "ranged"] {
        t.remove(k);
    }
    if let Some(m) = s.amount_min {
        t["amount_min"] = value(m.to_string());
    }
    if let Some(m) = s.amount_max {
        t["amount_max"] = value(m.to_string());
    }
    if s.priority != 0 {
        t["priority"] = value(i64::from(s.priority));
    }
    if !s.ranged.is_empty() {
        t["ranged"] = value(s.ranged.iter().map(ranged_value).collect::<Array>());
    }
}

/// Why an edit could not be applied.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CategoriesEditError {
    /// The input text is not valid TOML / not a categories document.
    #[error("categories.toml is not valid: {0}")]
    Parse(String),
    /// The ops JSON is malformed.
    #[error("invalid edit request: {0}")]
    Request(String),
    /// A referenced category/subcategory does not exist.
    #[error("no {kind} named {id:?}")]
    NotFound {
        /// `category` or `subcategory`.
        kind: &'static str,
        /// The missing name.
        id: String,
    },
    /// A new/renamed name collides with an existing one.
    #[error("there is already a {kind} named {id:?}")]
    Duplicate {
        /// `category` or `subcategory`.
        kind: &'static str,
        /// The colliding name.
        id: String,
    },
    /// Empty names, bad patterns or amounts.
    #[error("{0}")]
    Invalid(String),
}

type R<T> = Result<T, CategoriesEditError>;

const HEADER: &str = "# Your categories — layered over the built-in ones. A subcategory here\n\
# replaces the built-in subcategory with the same name (an empty one turns it\n\
# off). Patterns are case-insensitive and match anywhere in the description;\n\
# when several match, the longest pattern wins, then the higher priority.\n\
schema_version = 1\n";

fn clean(name: &str, what: &str) -> R<String> {
    let t = name.trim();
    if t.is_empty() {
        return Err(CategoriesEditError::Invalid(format!(
            "{what} can't be empty"
        )));
    }
    Ok(t.to_string())
}

fn kind_str(k: CategoryKind) -> &'static str {
    match k {
        CategoryKind::Spend => "spend",
        CategoryKind::Income => "income",
        CategoryKind::Transfer => "transfer",
    }
}

/// The edit state: the user's document plus the layered view (base + user) that
/// copy-on-write reads from.
struct Ed<'b> {
    doc: DocumentMut,
    base: &'b CategoriesDoc,
}

impl Ed<'_> {
    fn cats_mut(&mut self) -> &mut ArrayOfTables {
        if !self.doc.contains_key("category") {
            self.doc
                .insert("category", Item::ArrayOfTables(ArrayOfTables::new()));
        }
        // INVARIANT: ensured above; a non-array `category` is rejected in `apply`.
        #[allow(clippy::expect_used)]
        self.doc["category"]
            .as_array_of_tables_mut()
            .expect("category is an array of tables")
    }

    fn user_cat_index(&self, name: &str) -> Option<usize> {
        self.doc
            .get("category")
            .and_then(Item::as_array_of_tables)?
            .iter()
            .position(|t| t.get("name").and_then(Item::as_str) == Some(name))
    }

    /// The layered (base + user) document, as the engine would see it.
    fn layered(&self) -> R<CategoriesDoc> {
        let user: CategoriesDoc = toml::from_str(&self.doc.to_string())
            .map_err(|e| CategoriesEditError::Parse(e.to_string()))?;
        Ok(self.base.clone().merge(None, Some(user)))
    }

    fn base_cat(&self, name: &str) -> Option<&Category> {
        self.base.categories.iter().find(|c| c.name == name)
    }

    fn base_sub(&self, cat: &str, sub: &str) -> Option<&Subcategory> {
        self.base_cat(cat)?
            .subcategories
            .iter()
            .find(|s| s.name == sub)
    }

    /// The user's category table, created (with the layered kind, else `kind`,
    /// else spend) when missing.
    fn ensure_cat(&mut self, name: &str, kind: Option<CategoryKind>) -> R<usize> {
        if let Some(i) = self.user_cat_index(name) {
            return Ok(i);
        }
        let k = self
            .layered()?
            .categories
            .iter()
            .find(|c| c.name == name)
            .map(|c| c.kind)
            .or(kind)
            .unwrap_or(CategoryKind::Spend);
        let mut t = Table::new();
        t["name"] = value(name);
        t["kind"] = value(kind_str(k));
        self.cats_mut().push(t);
        Ok(self.cats_mut().len() - 1)
    }

    fn subs_of(&mut self, ci: usize) -> R<&mut ArrayOfTables> {
        let cat = self
            .cats_mut()
            .get_mut(ci)
            .ok_or_else(|| CategoriesEditError::Invalid("category index out of range".into()))?;
        if !cat.contains_key("subcategory") {
            cat.insert("subcategory", Item::ArrayOfTables(ArrayOfTables::new()));
        }
        cat["subcategory"].as_array_of_tables_mut().ok_or_else(|| {
            CategoriesEditError::Invalid("malformed [[category.subcategory]]".into())
        })
    }

    fn user_sub_index(&mut self, ci: usize, sub: &str) -> R<Option<usize>> {
        Ok(self
            .subs_of(ci)?
            .iter()
            .position(|t| t.get("name").and_then(Item::as_str) == Some(sub)))
    }

    /// The user's subcategory table (category index, sub index), copying the
    /// layered one in when it only exists in the base. `create` makes a new
    /// empty one when it exists nowhere.
    fn ensure_sub(
        &mut self,
        cat: &str,
        sub: &str,
        create: bool,
        kind: Option<CategoryKind>,
    ) -> R<(usize, usize)> {
        let layered = self.layered()?;
        let found = layered
            .categories
            .iter()
            .find(|c| c.name == cat)
            .and_then(|c| c.subcategories.iter().find(|s| s.name == sub))
            .cloned();
        if found.is_none() && !create {
            return Err(CategoriesEditError::NotFound {
                kind: "subcategory",
                id: format!("{cat} › {sub}"),
            });
        }
        let ci = self.ensure_cat(cat, kind)?;
        if let Some(si) = self.user_sub_index(ci, sub)? {
            return Ok((ci, si));
        }
        let mut t = Table::new();
        t["name"] = value(sub);
        let s = found.unwrap_or(Subcategory {
            name: sub.to_string(),
            patterns: Vec::new(),
            amount_min: None,
            amount_max: None,
            priority: 0,
            ranged: Vec::new(),
        });
        write_sub(&mut t, &s);
        let subs = self.subs_of(ci)?;
        subs.push(t);
        Ok((ci, subs.len() - 1))
    }

    fn sub_table(&mut self, ci: usize, si: usize) -> R<&mut Table> {
        self.subs_of(ci)?
            .get_mut(si)
            .ok_or_else(|| CategoriesEditError::Invalid("subcategory index out of range".into()))
    }

    fn patterns_mut(&mut self, ci: usize, si: usize) -> R<&mut Array> {
        let t = self.sub_table(ci, si)?;
        if t.get("patterns").and_then(Item::as_array).is_none() {
            t["patterns"] = value(Array::new());
        }
        t["patterns"]
            .as_array_mut()
            .ok_or_else(|| CategoriesEditError::Invalid("`patterns` must be a list".into()))
    }

    fn ranged_mut(&mut self, ci: usize, si: usize) -> R<&mut Array> {
        let t = self.sub_table(ci, si)?;
        if t.get("ranged").and_then(Item::as_array).is_none() {
            t["ranged"] = value(Array::new());
        }
        t["ranged"]
            .as_array_mut()
            .ok_or_else(|| CategoriesEditError::Invalid("`ranged` must be a list".into()))
    }

    /// Drop an empty `ranged = []` so files stay as they were.
    fn tidy_ranged(&mut self, ci: usize, si: usize) -> R<()> {
        let t = self.sub_table(ci, si)?;
        if t.get("ranged")
            .and_then(Item::as_array)
            .is_some_and(Array::is_empty)
        {
            t.remove("ranged");
        }
        Ok(())
    }

    /// Remove a subcategory from the layered result: dropped from the user file,
    /// or — when the base has it — written there empty, which turns it off.
    fn delete_sub(&mut self, cat: &str, sub: &str) -> R<()> {
        if self.base_sub(cat, sub).is_some() {
            let (ci, si) = self.ensure_sub(cat, sub, false, None)?;
            let t = self.sub_table(ci, si)?;
            t["patterns"] = value(Array::new());
            t.remove("amount_min");
            t.remove("amount_max");
            t.remove("priority");
            t.remove("ranged");
            return Ok(());
        }
        let Some(ci) = self.user_cat_index(cat) else {
            return Err(CategoriesEditError::NotFound {
                kind: "subcategory",
                id: format!("{cat} › {sub}"),
            });
        };
        let Some(si) = self.user_sub_index(ci, sub)? else {
            return Err(CategoriesEditError::NotFound {
                kind: "subcategory",
                id: format!("{cat} › {sub}"),
            });
        };
        self.subs_of(ci)?.remove(si);
        // Tidy: a user-only category left with nothing goes too.
        let empty = self.subs_of(ci)?.is_empty();
        if empty {
            if self.base_cat(cat).is_none() {
                self.cats_mut().remove(ci);
            } else if let Some(t) = self.cats_mut().get_mut(ci) {
                t.remove("subcategory");
            }
        }
        Ok(())
    }

    fn layered_sub_exists(&self, cat: &str, sub: &str) -> R<bool> {
        Ok(self.layered()?.categories.iter().any(|c| {
            c.name == cat
                && c.subcategories
                    .iter()
                    .any(|s| s.name == sub && !s.is_empty())
        }))
    }

    /// Live (non-emptied) subcategory names of a layered category.
    fn live_subs(&self, cat: &str) -> R<Vec<String>> {
        Ok(self
            .layered()?
            .categories
            .iter()
            .find(|c| c.name == cat)
            .map(|c| {
                c.subcategories
                    .iter()
                    .filter(|s| !s.is_empty() || self.base_sub(cat, &s.name).is_none())
                    .map(|s| s.name.clone())
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Copy subcategory `sub` of `cat` to `to_cat`/`to_sub` (layered values).
    fn copy_sub(
        &mut self,
        cat: &str,
        sub: &str,
        to_cat: &str,
        to_sub: &str,
        kind: Option<CategoryKind>,
    ) -> R<()> {
        let layered = self.layered()?;
        let s = layered
            .categories
            .iter()
            .find(|c| c.name == cat)
            .and_then(|c| c.subcategories.iter().find(|s| s.name == sub))
            .cloned()
            .ok_or_else(|| CategoriesEditError::NotFound {
                kind: "subcategory",
                id: format!("{cat} › {sub}"),
            })?;
        let (ci, si) = self.ensure_sub(to_cat, to_sub, true, kind)?;
        let t = self.sub_table(ci, si)?;
        write_sub(t, &s);
        Ok(())
    }

    /// The user's `[[pin]]` tables, if any.
    fn pins_mut(&mut self) -> Option<&mut ArrayOfTables> {
        self.doc
            .get_mut("pin")
            .and_then(Item::as_array_of_tables_mut)
    }

    /// Retarget pins of `cat` (and of `sub` only, when given) to `to_cat`/`to_sub`.
    fn retarget_pins(&mut self, cat: &str, sub: Option<&str>, to_cat: &str, to_sub: Option<&str>) {
        let Some(pins) = self.pins_mut() else { return };
        for t in pins.iter_mut() {
            let c = t.get("category").and_then(Item::as_str) == Some(cat);
            let s = sub.is_none_or(|sub| t.get("subcategory").and_then(Item::as_str) == Some(sub));
            if c && s {
                t["category"] = value(to_cat);
                if let Some(ts) = to_sub {
                    t["subcategory"] = value(ts);
                }
            }
        }
    }

    /// Drop pins of `cat` (and of `sub` only, when given).
    fn drop_pins(&mut self, cat: &str, sub: Option<&str>) {
        let Some(pins) = self.pins_mut() else { return };
        pins.retain(|t| {
            !(t.get("category").and_then(Item::as_str) == Some(cat)
                && sub.is_none_or(|sub| t.get("subcategory").and_then(Item::as_str) == Some(sub)))
        });
        if pins.is_empty() {
            self.doc.remove("pin");
        }
    }

    /// Remove the pin with `key`; whether there was one.
    fn unpin(&mut self, key: &PinKey) -> bool {
        let Some(pins) = self.pins_mut() else {
            return false;
        };
        let before = pins.len();
        pins.retain(|t| table_pin_key(t).as_ref() != Some(key));
        let removed = pins.len() != before;
        if pins.is_empty() {
            self.doc.remove("pin");
        }
        removed
    }

    fn apply_one(&mut self, op: &CategoryOp) -> R<()> {
        match op {
            CategoryOp::AddPattern {
                category,
                subcategory,
                pattern,
                kind,
                range,
            } => {
                let (cat, sub) = (
                    clean(category, "a category name")?,
                    clean(subcategory, "a subcategory name")?,
                );
                let pat = check_pattern(pattern)?;
                let range = match range {
                    Some(r) => Some(parse_range(
                        r.amount_min.as_deref(),
                        r.amount_max.as_deref(),
                    )?),
                    None => None,
                };
                let (ci, si) = self.ensure_sub(&cat, &sub, true, *kind)?;
                match range {
                    None => {
                        let arr = self.patterns_mut(ci, si)?;
                        if !arr.iter().any(|v| v.as_str() == Some(pat.as_str())) {
                            arr.push(pat);
                        }
                    }
                    Some((lo, hi)) => {
                        let r = RangedPattern {
                            pattern: pat,
                            amount_min: lo,
                            amount_max: hi,
                        };
                        let arr = self.ranged_mut(ci, si)?;
                        // One entry per pattern: a new range replaces the old one.
                        let at = arr
                            .iter()
                            .position(|v| ranged_pattern(v) == Some(r.pattern.as_str()));
                        match at {
                            Some(i) => {
                                arr.replace(i, ranged_value(&r));
                            }
                            None => arr.push(ranged_value(&r)),
                        }
                    }
                }
            }
            CategoryOp::EditPattern {
                category,
                subcategory,
                from,
                pattern,
                range,
            } => {
                let pat = check_pattern(pattern)?;
                let range = match range {
                    Some(r) => Some(parse_range(
                        r.amount_min.as_deref(),
                        r.amount_max.as_deref(),
                    )?),
                    None => None,
                };
                let (ci, si) = self.ensure_sub(category, subcategory, false, None)?;
                let plain_at = self
                    .patterns_mut(ci, si)?
                    .iter()
                    .position(|v| v.as_str() == Some(from.as_str()));
                let ranged_at = self
                    .ranged_mut(ci, si)?
                    .iter()
                    .position(|v| ranged_pattern(v) == Some(from.as_str()));
                if plain_at.is_none() && ranged_at.is_none() {
                    return Err(CategoriesEditError::NotFound {
                        kind: "pattern",
                        id: from.clone(),
                    });
                }
                match (range, plain_at, ranged_at) {
                    // Stays plain: replace in place.
                    (None, Some(i), _) => {
                        self.patterns_mut(ci, si)?.replace(i, pat.as_str());
                        if let Some(j) = ranged_at {
                            self.ranged_mut(ci, si)?.remove(j);
                        }
                    }
                    // Ranged → plain.
                    (None, None, Some(j)) => {
                        self.ranged_mut(ci, si)?.remove(j);
                        let arr = self.patterns_mut(ci, si)?;
                        if !arr.iter().any(|v| v.as_str() == Some(pat.as_str())) {
                            arr.push(pat.as_str());
                        }
                    }
                    (Some((lo, hi)), _, _) => {
                        let r = RangedPattern {
                            pattern: pat,
                            amount_min: lo,
                            amount_max: hi,
                        };
                        if let Some(i) = plain_at {
                            self.patterns_mut(ci, si)?.remove(i);
                        }
                        let arr = self.ranged_mut(ci, si)?;
                        match ranged_at {
                            Some(j) => {
                                arr.replace(j, ranged_value(&r));
                            }
                            None => arr.push(ranged_value(&r)),
                        }
                    }
                    (None, None, None) => {}
                }
                self.tidy_ranged(ci, si)?;
            }
            CategoryOp::RemovePattern {
                category,
                subcategory,
                pattern,
            } => {
                let (ci, si) = self.ensure_sub(category, subcategory, false, None)?;
                let arr = self.patterns_mut(ci, si)?;
                let mut before = arr.len();
                arr.retain(|v| v.as_str() != Some(pattern.as_str()));
                let mut after = arr.len();
                let r = self.ranged_mut(ci, si)?;
                before += r.len();
                r.retain(|v| ranged_pattern(v) != Some(pattern.as_str()));
                after += r.len();
                self.tidy_ranged(ci, si)?;
                if after == before {
                    return Err(CategoriesEditError::NotFound {
                        kind: "pattern",
                        id: pattern.clone(),
                    });
                }
            }
            CategoryOp::AddSubcategory {
                category,
                subcategory,
                kind,
            } => {
                let (cat, sub) = (
                    clean(category, "a category name")?,
                    clean(subcategory, "a subcategory name")?,
                );
                if self.layered_sub_exists(&cat, &sub)? {
                    return Err(CategoriesEditError::Duplicate {
                        kind: "subcategory",
                        id: format!("{cat} › {sub}"),
                    });
                }
                self.ensure_sub(&cat, &sub, true, *kind)?;
            }
            CategoryOp::SetAmounts {
                category,
                subcategory,
                amount_min,
                amount_max,
            } => {
                let parse = |v: &Option<String>, what: &str| -> R<Option<Decimal>> {
                    match v.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                        None => Ok(None),
                        Some(s) => s
                            .parse::<Decimal>()
                            .map(|d| Some(d.abs().normalize()))
                            .map_err(|_| {
                                CategoriesEditError::Invalid(format!("{what} {s:?} isn't a number"))
                            }),
                    }
                };
                let (lo, hi) = (
                    parse(amount_min, "the lower limit")?,
                    parse(amount_max, "the upper limit")?,
                );
                if let (Some(a), Some(b)) = (lo, hi) {
                    if a > b {
                        return Err(CategoriesEditError::Invalid(format!(
                            "the lower limit {a} is above the upper limit {b}"
                        )));
                    }
                }
                let (ci, si) = self.ensure_sub(category, subcategory, false, None)?;
                let t = self.sub_table(ci, si)?;
                for (k, v) in [("amount_min", lo), ("amount_max", hi)] {
                    match v {
                        Some(d) => t[k] = value(d.to_string()),
                        None => {
                            t.remove(k);
                        }
                    }
                }
            }
            CategoryOp::SetPriority {
                category,
                subcategory,
                priority,
            } => {
                let (ci, si) = self.ensure_sub(category, subcategory, false, None)?;
                let t = self.sub_table(ci, si)?;
                if *priority == 0 {
                    t.remove("priority");
                } else {
                    t["priority"] = value(i64::from(*priority));
                }
            }
            CategoryOp::SetKind { category, kind } => {
                if !self
                    .layered()?
                    .categories
                    .iter()
                    .any(|c| &c.name == category)
                {
                    return Err(CategoriesEditError::NotFound {
                        kind: "category",
                        id: category.clone(),
                    });
                }
                let ci = self.ensure_cat(category, Some(*kind))?;
                if let Some(t) = self.cats_mut().get_mut(ci) {
                    t["kind"] = value(kind_str(*kind));
                }
            }
            CategoryOp::RenameCategory { from, to } => {
                let to = clean(to, "a category name")?;
                if *from == to {
                    return Ok(());
                }
                let layered = self.layered()?;
                let src = layered
                    .categories
                    .iter()
                    .find(|c| &c.name == from)
                    .ok_or_else(|| CategoriesEditError::NotFound {
                        kind: "category",
                        id: from.clone(),
                    })?;
                if layered.categories.iter().any(|c| c.name == to)
                    && !self.live_subs(&to)?.is_empty()
                {
                    return Err(CategoriesEditError::Duplicate {
                        kind: "category",
                        id: to,
                    });
                }
                let kind = src.kind;
                let subs = self.live_subs(from)?;
                self.ensure_cat(&to, Some(kind))?;
                for s in &subs {
                    self.copy_sub(from, s, &to, s, Some(kind))?;
                }
                for s in &subs {
                    self.delete_sub(from, s)?;
                }
                // A user-only category that is now empty is gone; a base one stays emptied.
                if self.base_cat(from).is_none() {
                    if let Some(ci) = self.user_cat_index(from) {
                        self.cats_mut().remove(ci);
                    }
                }
                self.retarget_pins(from, None, &to, None);
            }
            CategoryOp::RenameSubcategory { category, from, to } => {
                let to = clean(to, "a subcategory name")?;
                if *from == to {
                    return Ok(());
                }
                if self.layered_sub_exists(category, &to)? {
                    return Err(CategoriesEditError::Duplicate {
                        kind: "subcategory",
                        id: format!("{category} › {to}"),
                    });
                }
                self.copy_sub(category, from, category, &to, None)?;
                self.delete_sub(category, from)?;
                self.retarget_pins(category, Some(from), category, Some(&to));
            }
            CategoryOp::MoveSubcategory {
                category,
                subcategory,
                to_category,
            } => {
                let to_cat = clean(to_category, "a category name")?;
                if *category == to_cat {
                    return Ok(());
                }
                if self.layered_sub_exists(&to_cat, subcategory)? {
                    return Err(CategoriesEditError::Duplicate {
                        kind: "subcategory",
                        id: format!("{to_cat} › {subcategory}"),
                    });
                }
                self.copy_sub(category, subcategory, &to_cat, subcategory, None)?;
                self.delete_sub(category, subcategory)?;
                self.retarget_pins(category, Some(subcategory), &to_cat, None);
            }
            CategoryOp::DeleteSubcategory {
                category,
                subcategory,
            } => {
                self.delete_sub(category, subcategory)?;
                self.drop_pins(category, Some(subcategory));
            }
            CategoryOp::DeleteCategory { category } => {
                let subs = self.live_subs(category)?;
                if subs.is_empty()
                    && !self
                        .layered()?
                        .categories
                        .iter()
                        .any(|c| &c.name == category)
                {
                    return Err(CategoriesEditError::NotFound {
                        kind: "category",
                        id: category.clone(),
                    });
                }
                for s in &subs {
                    self.delete_sub(category, s)?;
                }
                if self.base_cat(category).is_none() {
                    if let Some(ci) = self.user_cat_index(category) {
                        self.cats_mut().remove(ci);
                    }
                }
                self.drop_pins(category, None);
            }
            CategoryOp::Pin {
                date,
                amount,
                description,
                category,
                subcategory,
                kind,
            } => {
                let (cat, sub) = (
                    clean(category, "a category name")?,
                    clean(subcategory, "a subcategory name")?,
                );
                let (d, a) = parse_pin(date, amount)?;
                if description.trim().is_empty() {
                    return Err(CategoriesEditError::Invalid(
                        "a pinned transaction needs its description".into(),
                    ));
                }
                let exists = self
                    .layered()?
                    .categories
                    .iter()
                    .any(|c| c.name == cat && c.subcategories.iter().any(|s| s.name == sub));
                if !exists {
                    self.ensure_sub(&cat, &sub, true, *kind)?;
                }
                self.unpin(&pin_key(d, a, description));
                if !self.doc.contains_key("pin") {
                    self.doc
                        .insert("pin", Item::ArrayOfTables(ArrayOfTables::new()));
                }
                let mut t = Table::new();
                t["date"] = value(d.to_string());
                t["amount"] = value(a.normalize().to_string());
                t["description"] = value(description.trim());
                t["category"] = value(cat);
                t["subcategory"] = value(sub);
                if let Some(pins) = self.pins_mut() {
                    pins.push(t);
                }
            }
            CategoryOp::Unpin {
                date,
                amount,
                description,
            } => {
                let (d, a) = parse_pin(date, amount)?;
                if !self.unpin(&pin_key(d, a, description)) {
                    return Err(CategoriesEditError::NotFound {
                        kind: "pinned transaction",
                        id: format!("{date} {amount} {description}"),
                    });
                }
            }
        }
        Ok(())
    }
}

/// The `pattern` of a ranged entry.
fn ranged_pattern(v: &toml_edit::Value) -> Option<&str> {
    v.as_inline_table()?.get("pattern")?.as_str()
}

/// Parse a pin's date and amount.
fn parse_pin(date: &str, amount: &str) -> R<(jiff::civil::Date, Decimal)> {
    let d = date
        .trim()
        .parse::<jiff::civil::Date>()
        .map_err(|_| CategoriesEditError::Invalid(format!("{date:?} isn't a date (YYYY-MM-DD)")))?;
    let a = amount
        .trim()
        .parse::<Decimal>()
        .map_err(|_| CategoriesEditError::Invalid(format!("{amount:?} isn't an amount")))?;
    Ok((d, a))
}

/// The key of a `[[pin]]` table, when it is well-formed.
fn table_pin_key(t: &Table) -> Option<PinKey> {
    let (d, a) = parse_pin(t.get("date")?.as_str()?, t.get("amount")?.as_str()?).ok()?;
    Some(pin_key(d, a, t.get("description")?.as_str()?))
}

/// A pattern must be non-empty and compile (case-insensitive) with the `regex`
/// crate; look-around is rejected here with a hint rather than silently
/// stripped.
fn check_pattern(p: &str) -> R<String> {
    let p = p.trim();
    if p.is_empty() {
        return Err(CategoriesEditError::Invalid(
            "a pattern can't be empty".into(),
        ));
    }
    regex::RegexBuilder::new(p)
        .case_insensitive(true)
        .build()
        .map_err(|e| CategoriesEditError::Invalid(format!("the pattern {p:?} isn't valid: {e}")))?;
    Ok(p.to_string())
}

/// Apply `ops` to the user's `categories.toml` text (empty = no file yet),
/// layered over `base` (built-in + learned categories).
///
/// # Errors
/// [`CategoriesEditError`] when the text isn't a categories document, an op
/// names something missing, a name collides, or a pattern/amount is invalid.
pub fn apply(user_text: &str, base: &CategoriesDoc, ops: &[CategoryOp]) -> R<String> {
    let text = if user_text.trim().is_empty() {
        HEADER
    } else {
        user_text
    };
    let doc: DocumentMut = text
        .parse()
        .map_err(|e: toml_edit::TomlError| CategoriesEditError::Parse(e.to_string()))?;
    if doc
        .get("category")
        .is_some_and(|i| i.as_array_of_tables().is_none())
    {
        return Err(CategoriesEditError::Invalid(
            "`category` must be written as [[category]] tables".into(),
        ));
    }
    let mut ed = Ed { doc, base };
    for op in ops {
        ed.apply_one(op)?;
    }
    let out = ed.doc.to_string();
    let parsed: CategoriesDoc =
        toml::from_str(&out).map_err(|e| CategoriesEditError::Parse(e.to_string()))?;
    for c in &parsed.categories {
        for s in &c.subcategories {
            for p in s.patterns.iter().chain(s.ranged.iter().map(|r| &r.pattern)) {
                // Built-in patterns may use look-around the engine strips; only
                // reject what can't compile even after that.
                if regex::Regex::new(p).is_err() && !p.contains("(?") {
                    return Err(CategoriesEditError::Invalid(format!(
                        "{} › {}: the pattern {p:?} isn't valid",
                        c.name, s.name
                    )));
                }
            }
        }
    }
    Ok(out)
}

/// [`apply`] with the ops as a JSON array (the browser app's entry point).
///
/// # Errors
/// [`CategoriesEditError::Request`] for malformed JSON, else as [`apply`].
pub fn apply_json(user_text: &str, base: &CategoriesDoc, ops_json: &str) -> R<String> {
    let ops: Vec<CategoryOp> =
        serde_json::from_str(ops_json).map_err(|e| CategoriesEditError::Request(e.to_string()))?;
    apply(user_text, base, &ops)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = r#"
[[category]]
name = "Food"
kind = "spend"
  [[category.subcategory]]
  name = "Coffee"
  patterns = ["CAFE", "COFFEE"]
  amount_max = "20"
  [[category.subcategory]]
  name = "Groceries"
  patterns = ["MART"]
"#;
    const USER: &str = r#"# My tweaks — keep me.
schema_version = 1

[[category]]
name = "Hobbies"
kind = "spend"
  [[category.subcategory]]
  name = "Comics"
  # comic shops
  patterns = ["COMIX"]
"#;

    fn base() -> CategoriesDoc {
        toml::from_str(BASE).unwrap()
    }
    fn layered(user: &str) -> CategoriesDoc {
        base().merge(None, Some(toml::from_str(user).unwrap()))
    }
    fn sub<'d>(d: &'d CategoriesDoc, c: &str, s: &str) -> Option<&'d Subcategory> {
        d.categories
            .iter()
            .find(|x| x.name == c)?
            .subcategories
            .iter()
            .find(|x| x.name == s)
    }
    fn run(user: &str, ops: &[CategoryOp]) -> String {
        apply(user, &base(), ops).expect("edit applies")
    }

    #[test]
    fn add_pattern_to_builtin_copies_it_first() {
        let out = run(
            USER,
            &[CategoryOp::AddPattern {
                category: "Food".into(),
                subcategory: "Coffee".into(),
                pattern: "BEAN BAR".into(),
                kind: None,
                range: None,
            }],
        );
        assert!(out.contains("# My tweaks — keep me.") && out.contains("# comic shops"));
        let d = layered(&out);
        let s = sub(&d, "Food", "Coffee").unwrap();
        assert_eq!(s.patterns, ["CAFE", "COFFEE", "BEAN BAR"]);
        assert_eq!(s.amount_max, Some(Decimal::from(20)), "limits copied too");
        assert_eq!(
            sub(&d, "Food", "Groceries").unwrap().patterns,
            ["MART"],
            "untouched built-in stays"
        );
    }

    #[test]
    fn new_category_from_nothing() {
        let out = run(
            "",
            &[CategoryOp::AddPattern {
                category: "Pay".into(),
                subcategory: "Salary".into(),
                pattern: "PAYROLL".into(),
                kind: Some(CategoryKind::Income),
                range: None,
            }],
        );
        assert!(out.starts_with("# Your categories"));
        let d = layered(&out);
        assert_eq!(
            d.categories.iter().find(|c| c.name == "Pay").unwrap().kind,
            CategoryKind::Income
        );
        assert_eq!(sub(&d, "Pay", "Salary").unwrap().patterns, ["PAYROLL"]);
    }

    #[test]
    fn delete_builtin_empties_it_and_user_one_goes() {
        let out = run(
            USER,
            &[
                CategoryOp::DeleteSubcategory {
                    category: "Food".into(),
                    subcategory: "Coffee".into(),
                },
                CategoryOp::DeleteSubcategory {
                    category: "Hobbies".into(),
                    subcategory: "Comics".into(),
                },
            ],
        );
        let d = layered(&out);
        assert!(sub(&d, "Food", "Coffee").unwrap().patterns.is_empty());
        assert!(d.categories.iter().all(|c| c.name != "Hobbies"));
    }

    #[test]
    fn rename_and_move() {
        let out = run(
            USER,
            &[
                CategoryOp::RenameSubcategory {
                    category: "Food".into(),
                    from: "Coffee".into(),
                    to: "Cafés".into(),
                },
                CategoryOp::RenameCategory {
                    from: "Hobbies".into(),
                    to: "Fun".into(),
                },
                CategoryOp::MoveSubcategory {
                    category: "Food".into(),
                    subcategory: "Groceries".into(),
                    to_category: "Home".into(),
                },
            ],
        );
        let d = layered(&out);
        assert_eq!(
            sub(&d, "Food", "Cafés").unwrap().patterns,
            ["CAFE", "COFFEE"]
        );
        assert!(sub(&d, "Food", "Coffee").unwrap().patterns.is_empty());
        assert_eq!(sub(&d, "Fun", "Comics").unwrap().patterns, ["COMIX"]);
        assert!(d.categories.iter().all(|c| c.name != "Hobbies"));
        assert_eq!(sub(&d, "Home", "Groceries").unwrap().patterns, ["MART"]);
    }

    #[test]
    fn amounts_kind_priority_and_errors() {
        let out = run(
            USER,
            &[
                CategoryOp::SetAmounts {
                    category: "Hobbies".into(),
                    subcategory: "Comics".into(),
                    amount_min: Some("-5".into()),
                    amount_max: Some("100.50".into()),
                },
                CategoryOp::SetKind {
                    category: "Hobbies".into(),
                    kind: CategoryKind::Transfer,
                },
                CategoryOp::SetPriority {
                    category: "Food".into(),
                    subcategory: "Groceries".into(),
                    priority: 3,
                },
            ],
        );
        let d = layered(&out);
        let c = sub(&d, "Hobbies", "Comics").unwrap();
        assert_eq!(
            (c.amount_min, c.amount_max),
            (Some(Decimal::from(5)), Some("100.5".parse().unwrap()))
        );
        assert_eq!(
            d.categories
                .iter()
                .find(|c| c.name == "Hobbies")
                .unwrap()
                .kind,
            CategoryKind::Transfer
        );
        assert_eq!(sub(&d, "Food", "Groceries").unwrap().priority, 3);

        let bad = |op: CategoryOp| apply(USER, &base(), &[op]).unwrap_err();
        assert!(matches!(
            bad(CategoryOp::AddPattern {
                category: "Food".into(),
                subcategory: "Coffee".into(),
                pattern: "(unclosed".into(),
                kind: None,
                range: None,
            }),
            CategoriesEditError::Invalid(_)
        ));
        assert!(matches!(
            bad(CategoryOp::SetAmounts {
                category: "Food".into(),
                subcategory: "Coffee".into(),
                amount_min: Some("9".into()),
                amount_max: Some("1".into())
            }),
            CategoriesEditError::Invalid(_)
        ));
        assert!(matches!(
            bad(CategoryOp::RenameSubcategory {
                category: "Food".into(),
                from: "Coffee".into(),
                to: "Groceries".into()
            }),
            CategoriesEditError::Duplicate { .. }
        ));
        assert!(matches!(
            bad(CategoryOp::RemovePattern {
                category: "Food".into(),
                subcategory: "Nope".into(),
                pattern: "X".into()
            }),
            CategoriesEditError::NotFound { .. }
        ));
    }

    #[test]
    fn pins_follow_renames_and_go_with_deletes() {
        let pin = |c: &str, s: &str| CategoryOp::Pin {
            date: "2026-02-19".into(),
            amount: "-32893.250".into(),
            description: "Official Cheque  Ref 1".into(),
            category: c.into(),
            subcategory: s.into(),
            kind: None,
        };
        // Into a new subcategory (created), then re-pinned elsewhere (replaced).
        let out = run(USER, &[pin("Car", "Purchase"), pin("Hobbies", "Comics")]);
        let d = layered(&out);
        assert_eq!(d.pins.len(), 1, "{out}");
        assert_eq!(d.pins[0].subcategory, "Comics");
        assert_eq!(d.pins[0].amount.to_string(), "-32893.25");
        assert!(
            sub(&d, "Car", "Purchase").is_some(),
            "created by the first pin"
        );
        assert!(out.contains("# comic shops"), "comments kept");
        // A pin to a built-in doesn't copy it into the user file.
        let b = run("", &[pin("Food", "Coffee")]);
        assert!(!b.contains("[[category]]"), "{b}");
        // Rename and move carry the pin.
        let r = run(
            &out,
            &[CategoryOp::RenameSubcategory {
                category: "Hobbies".into(),
                from: "Comics".into(),
                to: "Books".into(),
            }],
        );
        assert_eq!(layered(&r).pins[0].subcategory, "Books");
        let m = run(
            &r,
            &[CategoryOp::RenameCategory {
                from: "Hobbies".into(),
                to: "Fun".into(),
            }],
        );
        assert_eq!(layered(&m).pins[0].category, "Fun");
        // Unpin by the same transaction (spacing/case/zeros don't matter).
        let u = run(
            &m,
            &[CategoryOp::Unpin {
                date: "2026-02-19".into(),
                amount: "-32893.25".into(),
                description: "OFFICIAL CHEQUE REF 1".into(),
            }],
        );
        assert!(layered(&u).pins.is_empty() && !u.contains("[[pin]]"), "{u}");
        // Deleting the subcategory drops its pins.
        let del = run(
            &m,
            &[CategoryOp::DeleteCategory {
                category: "Fun".into(),
            }],
        );
        assert!(layered(&del).pins.is_empty());
        // Bad input is refused.
        let bad = apply(
            USER,
            &base(),
            &[CategoryOp::Pin {
                date: "19/02/2026".into(),
                amount: "1".into(),
                description: "x".into(),
                category: "A".into(),
                subcategory: "B".into(),
                kind: None,
            }],
        );
        assert!(matches!(bad, Err(CategoriesEditError::Invalid(_))));
    }

    #[test]
    fn ranged_patterns_add_edit_and_remove() {
        let range = |lo: Option<&str>, hi: Option<&str>| {
            Some(AmountRange {
                amount_min: lo.map(Into::into),
                amount_max: hi.map(Into::into),
            })
        };
        // A ranged copy of a built-in pattern beats the subcategory's cap.
        let out = run(
            USER,
            &[CategoryOp::EditPattern {
                category: "Food".into(),
                subcategory: "Coffee".into(),
                from: "CAFE".into(),
                pattern: "CAFE".into(),
                range: range(None, None),
            }],
        );
        let s = sub(&layered(&out), "Food", "Coffee").unwrap().clone();
        assert_eq!(s.patterns, ["COFFEE"]);
        assert_eq!(s.ranged.len(), 1);
        assert_eq!(s.ranged[0].amount_max, None);
        assert!(out.contains(r#"ranged = [{ pattern = "CAFE" }]"#), "{out}");
        // Edit its range in place, then back to plain (it moves to `patterns`).
        let out = run(
            &out,
            &[CategoryOp::EditPattern {
                category: "Food".into(),
                subcategory: "Coffee".into(),
                from: "CAFE".into(),
                pattern: "CAFES?".into(),
                range: range(Some("-35"), Some("")),
            }],
        );
        let s = sub(&layered(&out), "Food", "Coffee").unwrap().clone();
        assert_eq!(
            (
                s.ranged[0].pattern.as_str(),
                s.ranged[0].amount_min.map(|d| d.to_string())
            ),
            ("CAFES?", Some("35".into()))
        );
        let out = run(
            &out,
            &[CategoryOp::EditPattern {
                category: "Food".into(),
                subcategory: "Coffee".into(),
                from: "CAFES?".into(),
                pattern: "CAFES?".into(),
                range: None,
            }],
        );
        let s = sub(&layered(&out), "Food", "Coffee").unwrap().clone();
        assert!(s.ranged.is_empty() && !out.contains("ranged"), "{out}");
        assert_eq!(s.patterns, ["COFFEE", "CAFES?"]);
        // Plain edit keeps the position.
        let out = run(
            &out,
            &[CategoryOp::EditPattern {
                category: "Food".into(),
                subcategory: "Coffee".into(),
                from: "COFFEE".into(),
                pattern: "COFFEES".into(),
                range: None,
            }],
        );
        assert_eq!(
            sub(&layered(&out), "Food", "Coffee").unwrap().patterns,
            ["COFFEES", "CAFES?"]
        );
        // Add ranged, remove it; a bad range is refused.
        let out = run(
            &out,
            &[CategoryOp::AddPattern {
                category: "Food".into(),
                subcategory: "Snacks".into(),
                pattern: "SHELL".into(),
                kind: None,
                range: range(None, Some("20")),
            }],
        );
        let d = layered(&out);
        assert_eq!(
            sub(&d, "Food", "Snacks").unwrap().ranged[0]
                .amount_max
                .unwrap()
                .to_string(),
            "20"
        );
        let out = run(
            &out,
            &[CategoryOp::RemovePattern {
                category: "Food".into(),
                subcategory: "Snacks".into(),
                pattern: "SHELL".into(),
            }],
        );
        assert!(sub(&layered(&out), "Food", "Snacks").unwrap().is_empty());
        assert!(matches!(
            apply(
                USER,
                &base(),
                &[CategoryOp::AddPattern {
                    category: "A".into(),
                    subcategory: "B".into(),
                    pattern: "X".into(),
                    kind: None,
                    range: range(Some("30"), Some("20"))
                }]
            ),
            Err(CategoriesEditError::Invalid(_))
        ));
    }

    #[test]
    fn remove_pattern_and_json() {
        let out = apply_json(
            USER,
            &base(),
            r#"[{"op":"remove_pattern","category":"Food","subcategory":"Coffee","pattern":"CAFE"}]"#,
        )
        .unwrap();
        assert_eq!(
            sub(&layered(&out), "Food", "Coffee").unwrap().patterns,
            ["COFFEE"]
        );
    }

    #[test]
    fn edits_on_the_shipped_defaults() {
        let defaults: CategoriesDoc =
            toml::from_str(crate::config::defaults::categories()).unwrap();
        let first = &defaults.categories[0];
        let s = &first.subcategories[0];
        let out = apply(
            "",
            &defaults,
            &[CategoryOp::AddPattern {
                category: first.name.clone(),
                subcategory: s.name.clone(),
                pattern: "ZZ TEST SHOP".into(),
                kind: None,
                range: None,
            }],
        )
        .unwrap();
        let d = defaults
            .clone()
            .merge(None, Some(toml::from_str(&out).unwrap()));
        let got = d.categories[0]
            .subcategories
            .iter()
            .find(|x| x.name == s.name)
            .unwrap();
        assert!(got.patterns.contains(&"ZZ TEST SHOP".to_string()));
        assert_eq!(got.patterns.len(), s.patterns.len() + 1);
    }
}
