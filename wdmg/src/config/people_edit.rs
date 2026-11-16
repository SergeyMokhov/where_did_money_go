//! Structured edits to `people.toml` that keep the user's comments and layout.
//!
//! The browser app's "People & accounts" editor (and anything else that wants to
//! change identity config) sends a list of [`PeopleOp`]s; they are applied with
//! `toml_edit`, so unrelated text — comments, ordering, formatting — survives.
//! Renames also retarget every `[[transfer_rule]] to = "…"` that named the old
//! id, because person/account ids are referenced only from this file.
//!
//! The result is re-parsed into a [`PeopleDoc`] and checked for duplicate ids
//! before it is returned, so a caller never receives a document the engine
//! would reject for those reasons.

use std::collections::BTreeSet;

use serde::Deserialize;
use toml_edit::{value, ArrayOfTables, DocumentMut, Item, Table};

use super::people::PeopleDoc;

/// One edit. JSON: `{"op": "rename_account", "from": "...", "to": "..."}`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PeopleOp {
    /// Add a person with no accounts.
    AddPerson {
        /// New person id (also used as the display name when none is given).
        id: String,
        /// Display name; defaults to `id`.
        #[serde(default)]
        display_name: Option<String>,
    },
    /// Rename a person: id and display name, plus transfer rules naming them.
    RenamePerson {
        /// Current id.
        from: String,
        /// New id / display name.
        to: String,
    },
    /// Rename an account id, plus transfer rules naming it.
    RenameAccount {
        /// Current id.
        from: String,
        /// New id.
        to: String,
    },
    /// Move an account (with its matching criteria) to another person.
    MoveAccount {
        /// Account id.
        account: String,
        /// Destination person id.
        to_person: String,
    },
    /// Add an account to a person, optionally matched by one statement file.
    AddAccount {
        /// Owner person id.
        person: String,
        /// New account id.
        id: String,
        /// Account kind (`debit`, `savings`, `credit_card`, …).
        kind: String,
        /// Statement file basename this account owns (matched exactly).
        #[serde(default)]
        file: Option<String>,
        /// An external account (no statements; a transfer destination).
        #[serde(default)]
        external: bool,
    },
    /// Make an existing account also own a statement file (exact basename).
    /// An external account that gets a file becomes a regular one.
    LinkFile {
        /// Account id.
        account: String,
        /// Statement file basename.
        file: String,
    },
    /// Change an account's kind.
    SetKind {
        /// Account id.
        account: String,
        /// New kind (`debit`, `savings`, `credit_card`, …).
        kind: String,
    },
    /// Replace how an account picks up statement rows. Every given criterion
    /// must hold; at least one is required. An external account becomes a
    /// regular one.
    SetMatch {
        /// Account id.
        account: String,
        /// Regex on the statement file's basename.
        #[serde(default)]
        filename: Option<String>,
        /// Statement format (profile id).
        #[serde(default)]
        profile: Option<String>,
        /// Exact value of the format's cardholder column.
        #[serde(default)]
        cardholder: Option<String>,
        /// Id the format captures from the file name (e.g. a card's last 4).
        #[serde(default)]
        card_last4: Option<String>,
    },
    /// Turn an account into an external one: no statements, a transfer
    /// destination only. Its matching criteria are removed.
    MakeExternal {
        /// Account id.
        account: String,
    },
    /// Delete an account, and the transfer rules that send money to it.
    DeleteAccount {
        /// Account id.
        account: String,
    },
    /// Delete a person with no accounts, and the transfer rules naming them.
    DeletePerson {
        /// Person id.
        person: String,
    },
    /// Add a transfer rule: an unpaired transfer whose description matches
    /// `pattern` goes to `to` (a person or account). It's tried after the
    /// existing rules unless `priority` says otherwise.
    AddRule {
        /// Case-insensitive regex on the description.
        pattern: String,
        /// Person or account id.
        to: String,
        /// Explicit priority (higher is tried first).
        #[serde(default)]
        priority: Option<i32>,
    },
    /// Change a transfer rule (by its position in the file).
    UpdateRule {
        /// Index among the `[[transfer_rule]]` tables, in file order.
        index: usize,
        /// New pattern.
        pattern: String,
        /// New target.
        to: String,
    },
    /// Delete a transfer rule (by its position in the file).
    DeleteRule {
        /// Index among the `[[transfer_rule]]` tables, in file order.
        index: usize,
    },
    /// Put the rules in the order they should be tried: `order` lists every
    /// file index once, first-tried first. The tables are reordered and their
    /// priorities rewritten to descend in that order.
    OrderRules {
        /// A permutation of the file indices.
        order: Vec<usize>,
    },
}

/// The keys that decide which statement rows an account picks up.
const MATCH_KEYS: [&str; 4] = ["filename", "profile", "cardholder", "card_last4"];

/// Why an edit could not be applied.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PeopleEditError {
    /// The input text is not valid TOML.
    #[error("people.toml is not valid TOML: {0}")]
    Parse(String),
    /// The ops JSON is malformed.
    #[error("invalid edit request: {0}")]
    Request(String),
    /// A referenced person/account does not exist.
    #[error("no {kind} named {id:?}")]
    NotFound {
        /// `person` or `account`.
        kind: &'static str,
        /// The missing id.
        id: String,
    },
    /// A new/renamed id collides with an existing one.
    #[error("there is already a {kind} named {id:?}")]
    Duplicate {
        /// `person` or `account`.
        kind: &'static str,
        /// The colliding id.
        id: String,
    },
    /// Empty / whitespace-only names and similar.
    #[error("{0}")]
    Invalid(String),
}

/// Anchored, case-insensitive regex matching exactly one file basename.
#[must_use]
pub fn exact_file_regex(file: &str) -> String {
    format!("(?i)^{}$", regex::escape(file))
}

fn clean(name: &str, what: &str) -> Result<String, PeopleEditError> {
    let t = name.trim();
    if t.is_empty() {
        return Err(PeopleEditError::Invalid(format!("{what} can't be empty")));
    }
    Ok(t.to_string())
}

fn persons_mut(doc: &mut DocumentMut) -> &mut ArrayOfTables {
    if !doc.contains_key("person") {
        doc.insert("person", Item::ArrayOfTables(ArrayOfTables::new()));
    }
    // INVARIANT: just ensured the key exists; a non-array `person` is rejected
    // earlier by `check_shape`.
    #[allow(clippy::expect_used)]
    doc["person"]
        .as_array_of_tables_mut()
        .expect("person is an array of tables")
}

/// The person table at `i` (indices come from `find_person` / `find_account`).
fn person_at(doc: &mut DocumentMut, i: usize) -> Result<&mut Table, PeopleEditError> {
    persons_mut(doc)
        .get_mut(i)
        .ok_or_else(|| PeopleEditError::Invalid("person index out of range".into()))
}

/// A person's `[[person.account]]` array.
fn accounts_mut(person: &mut Table) -> Result<&mut ArrayOfTables, PeopleEditError> {
    person
        .get_mut("account")
        .and_then(Item::as_array_of_tables_mut)
        .ok_or_else(|| PeopleEditError::Invalid("malformed [[person.account]]".into()))
}

fn str_of<'t>(t: &'t Table, key: &str) -> Option<&'t str> {
    t.get(key).and_then(Item::as_str)
}

fn accounts_of(person: &Table) -> impl Iterator<Item = &Table> {
    person
        .get("account")
        .and_then(Item::as_array_of_tables)
        .into_iter()
        .flat_map(ArrayOfTables::iter)
}

fn all_person_ids(doc: &DocumentMut) -> BTreeSet<String> {
    doc.get("person")
        .and_then(Item::as_array_of_tables)
        .into_iter()
        .flat_map(ArrayOfTables::iter)
        .filter_map(|p| str_of(p, "id").map(str::to_string))
        .collect()
}

fn all_account_ids(doc: &DocumentMut) -> BTreeSet<String> {
    doc.get("person")
        .and_then(Item::as_array_of_tables)
        .into_iter()
        .flat_map(ArrayOfTables::iter)
        .flat_map(accounts_of)
        .filter_map(|a| str_of(a, "id").map(str::to_string))
        .collect()
}

/// Locate an account: (person index, account index).
fn find_account(doc: &DocumentMut, id: &str) -> Option<(usize, usize)> {
    let persons = doc.get("person").and_then(Item::as_array_of_tables)?;
    persons.iter().enumerate().find_map(|(pi, p)| {
        accounts_of(p)
            .position(|a| str_of(a, "id") == Some(id))
            .map(|ai| (pi, ai))
    })
}

fn find_person(doc: &DocumentMut, id: &str) -> Option<usize> {
    doc.get("person")
        .and_then(Item::as_array_of_tables)?
        .iter()
        .position(|p| str_of(p, "id") == Some(id))
}

fn retarget_rules(doc: &mut DocumentMut, from: &str, to: &str) {
    if let Some(rules) = doc
        .get_mut("transfer_rule")
        .and_then(Item::as_array_of_tables_mut)
    {
        for r in rules.iter_mut() {
            if r.get("to").and_then(Item::as_str) == Some(from) {
                r["to"] = value(to);
            }
        }
    }
}

/// `person` / `transfer_rule` must be arrays of tables if present.
fn check_shape(doc: &DocumentMut) -> Result<(), PeopleEditError> {
    for key in ["person", "transfer_rule"] {
        if let Some(item) = doc.get(key) {
            if item.as_array_of_tables().is_none() {
                return Err(PeopleEditError::Invalid(format!(
                    "`{key}` must be written as [[{key}]] tables"
                )));
            }
        }
    }
    Ok(())
}

fn apply_one(doc: &mut DocumentMut, op: &PeopleOp) -> Result<(), PeopleEditError> {
    match op {
        PeopleOp::AddPerson { id, display_name } => {
            let id = clean(id, "a person's name")?;
            if all_person_ids(doc).contains(&id) {
                return Err(PeopleEditError::Duplicate { kind: "person", id });
            }
            let mut t = Table::new();
            t["id"] = value(id.as_str());
            let name = display_name
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(&id);
            t["display_name"] = value(name);
            persons_mut(doc).push(t);
        }
        PeopleOp::RenamePerson { from, to } => {
            let to = clean(to, "a person's name")?;
            if *from == to {
                return Ok(());
            }
            let pi = find_person(doc, from).ok_or_else(|| PeopleEditError::NotFound {
                kind: "person",
                id: from.clone(),
            })?;
            if all_person_ids(doc).contains(&to) {
                return Err(PeopleEditError::Duplicate {
                    kind: "person",
                    id: to,
                });
            }
            let p = person_at(doc, pi)?;
            p["id"] = value(to.as_str());
            p["display_name"] = value(to.as_str());
            retarget_rules(doc, from, &to);
        }
        PeopleOp::RenameAccount { from, to } => {
            let to = clean(to, "an account's name")?;
            if *from == to {
                return Ok(());
            }
            let (pi, ai) = find_account(doc, from).ok_or_else(|| PeopleEditError::NotFound {
                kind: "account",
                id: from.clone(),
            })?;
            if all_account_ids(doc).contains(&to) {
                return Err(PeopleEditError::Duplicate {
                    kind: "account",
                    id: to,
                });
            }
            let accts = accounts_mut(person_at(doc, pi)?)?;
            if let Some(a) = accts.get_mut(ai) {
                a["id"] = value(to.as_str());
            }
            retarget_rules(doc, from, &to);
        }
        PeopleOp::MoveAccount { account, to_person } => {
            let (pi, ai) = find_account(doc, account).ok_or_else(|| PeopleEditError::NotFound {
                kind: "account",
                id: account.clone(),
            })?;
            let dest = find_person(doc, to_person).ok_or_else(|| PeopleEditError::NotFound {
                kind: "person",
                id: to_person.clone(),
            })?;
            if dest == pi {
                return Ok(());
            }
            let src_person = person_at(doc, pi)?;
            let src = accounts_mut(src_person)?;
            let moved = src
                .get(ai)
                .cloned()
                .ok_or_else(|| PeopleEditError::NotFound {
                    kind: "account",
                    id: account.clone(),
                })?;
            src.remove(ai);
            if src.is_empty() {
                src_person.remove("account");
            }
            // Its file position is fixed up by `renumber` once all ops ran.
            push_account(person_at(doc, dest)?, moved);
        }
        PeopleOp::AddAccount {
            person,
            id,
            kind,
            file,
            external,
        } => {
            let id = clean(id, "an account's name")?;
            let kind = clean(kind, "an account's kind")?;
            let pi = find_person(doc, person).ok_or_else(|| PeopleEditError::NotFound {
                kind: "person",
                id: person.clone(),
            })?;
            if all_account_ids(doc).contains(&id) {
                return Err(PeopleEditError::Duplicate {
                    kind: "account",
                    id,
                });
            }
            let mut t = Table::new();
            t["id"] = value(id.as_str());
            if let Some(f) = file.as_deref().map(str::trim).filter(|f| !f.is_empty()) {
                t["filename"] = value(exact_file_regex(f));
            }
            t["kind"] = value(kind.as_str());
            if *external {
                t["external"] = value(true);
            }
            push_account(person_at(doc, pi)?, t);
        }
        PeopleOp::LinkFile { account, file } => {
            let file = clean(file, "a file name")?;
            let (pi, ai) = find_account(doc, account).ok_or_else(|| PeopleEditError::NotFound {
                kind: "account",
                id: account.clone(),
            })?;
            let accts = accounts_mut(person_at(doc, pi)?)?;
            let Some(a) = accts.get_mut(ai) else {
                return Err(PeopleEditError::NotFound {
                    kind: "account",
                    id: account.clone(),
                });
            };
            let exact = exact_file_regex(&file);
            let pattern = match a.get("filename").and_then(Item::as_str) {
                // Keep the existing pattern as one alternative, add the file as another.
                Some(old) if !old.is_empty() => format!("(?:{old})|{exact}"),
                _ => exact,
            };
            a["filename"] = value(pattern);
            a.remove("external");
            indent_like_id(a);
        }
        PeopleOp::SetKind { account, kind } => {
            let kind = clean(kind, "an account's kind")?;
            account_table(doc, account)?["kind"] = value(kind.as_str());
        }
        PeopleOp::SetMatch {
            account,
            filename,
            profile,
            cardholder,
            card_last4,
        } => {
            let given = |v: &Option<String>| {
                v.as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            };
            let (filename, profile, cardholder, card_last4) = (
                given(filename),
                given(profile),
                given(cardholder),
                given(card_last4),
            );
            if let Some(f) = &filename {
                regex::Regex::new(f).map_err(|e| {
                    PeopleEditError::Invalid(format!("the file-name pattern isn't valid: {e}"))
                })?;
            }
            if filename.is_none()
                && profile.is_none()
                && cardholder.is_none()
                && card_last4.is_none()
            {
                return Err(PeopleEditError::Invalid(format!(
                    "{account:?} needs at least one way to recognise its statements"
                )));
            }
            let a = account_table(doc, account)?;
            for k in MATCH_KEYS {
                a.remove(k);
            }
            a.remove("external");
            // Written before `kind` so the file reads "what it matches, then what it is".
            let kind = a.remove("kind");
            for (k, v) in [
                ("cardholder", cardholder),
                ("card_last4", card_last4),
                ("filename", filename),
                ("profile", profile),
            ] {
                if let Some(v) = v {
                    a[k] = value(v);
                }
            }
            if let Some(kind) = kind {
                a.insert("kind", kind);
            }
            indent_like_id(a);
        }
        PeopleOp::MakeExternal { account } => {
            let a = account_table(doc, account)?;
            for k in MATCH_KEYS {
                a.remove(k);
            }
            a["external"] = value(true);
            indent_like_id(a);
        }
        PeopleOp::DeleteAccount { account } => {
            let (pi, ai) = find_account(doc, account).ok_or_else(|| PeopleEditError::NotFound {
                kind: "account",
                id: account.clone(),
            })?;
            let person = person_at(doc, pi)?;
            let accts = accounts_mut(person)?;
            accts.remove(ai);
            if accts.is_empty() {
                person.remove("account");
            }
            drop_rules_to(doc, account);
        }
        PeopleOp::DeletePerson { person } => {
            let pi = find_person(doc, person).ok_or_else(|| PeopleEditError::NotFound {
                kind: "person",
                id: person.clone(),
            })?;
            let has_accounts = person_at(doc, pi)?
                .get("account")
                .and_then(Item::as_array_of_tables)
                .is_some_and(|a| !a.is_empty());
            if has_accounts {
                return Err(PeopleEditError::Invalid(format!(
                    "{person:?} still has accounts — move or delete them first"
                )));
            }
            persons_mut(doc).remove(pi);
            if persons_mut(doc).is_empty() {
                doc.remove("person");
            }
            drop_rules_to(doc, person);
        }
        PeopleOp::AddRule {
            pattern,
            to,
            priority,
        } => {
            let (pattern, to) = check_rule(doc, pattern, to)?;
            // Default: tried after every existing rule.
            let prio = priority.unwrap_or_else(|| {
                doc.get("transfer_rule")
                    .and_then(Item::as_array_of_tables)
                    .into_iter()
                    .flat_map(ArrayOfTables::iter)
                    .filter_map(|r| r.get("priority").and_then(Item::as_integer))
                    .min()
                    .map_or(0, |m| i32::try_from(m).unwrap_or(0).min(0))
            });
            let mut t = Table::new();
            t["pattern"] = value(pattern);
            t["to"] = value(to);
            t["priority"] = value(i64::from(prio));
            rules_mut(doc).push(t);
        }
        PeopleOp::UpdateRule { index, pattern, to } => {
            let (pattern, to) = check_rule(doc, pattern, to)?;
            let r = rule_at(doc, *index)?;
            r["pattern"] = value(pattern);
            r["to"] = value(to);
        }
        PeopleOp::DeleteRule { index } => {
            rule_at(doc, *index)?;
            let rules = rules_mut(doc);
            rules.remove(*index);
            if rules.is_empty() {
                doc.remove("transfer_rule");
            }
        }
        PeopleOp::OrderRules { order } => {
            let rules = rules_mut(doc);
            let n = rules.len();
            let mut seen = vec![false; n];
            if order.len() != n
                || order
                    .iter()
                    .any(|&i| i >= n || std::mem::replace(&mut seen[i], true))
            {
                return Err(PeopleEditError::Invalid(
                    "the new order must list every rule exactly once".into(),
                ));
            }
            let mut tables: Vec<Option<Table>> = rules.iter().cloned().map(Some).collect();
            let mut next = ArrayOfTables::new();
            for (rank, &i) in order.iter().enumerate() {
                if let Some(mut t) = tables[i].take() {
                    let prio = i64::try_from((n - rank) * 10).unwrap_or(i64::MAX);
                    t["priority"] = value(prio);
                    next.push(t);
                }
            }
            *rules = next;
        }
    }
    Ok(())
}

fn rules_mut(doc: &mut DocumentMut) -> &mut ArrayOfTables {
    if !doc.contains_key("transfer_rule") {
        doc.insert("transfer_rule", Item::ArrayOfTables(ArrayOfTables::new()));
    }
    // INVARIANT: ensured above; a non-array `transfer_rule` fails `check_shape`.
    #[allow(clippy::expect_used)]
    doc["transfer_rule"]
        .as_array_of_tables_mut()
        .expect("transfer_rule is an array of tables")
}

/// A rule's pattern must compile; its target must be a person or account.
fn check_rule(
    doc: &DocumentMut,
    pattern: &str,
    to: &str,
) -> Result<(String, String), PeopleEditError> {
    let pattern = clean(pattern, "a rule's pattern")?;
    regex::Regex::new(&pattern).map_err(|e| {
        PeopleEditError::Invalid(format!("the pattern {pattern:?} isn't valid: {e}"))
    })?;
    let to = clean(to, "a rule's target")?;
    if !all_person_ids(doc).contains(&to) && !all_account_ids(doc).contains(&to) {
        return Err(PeopleEditError::NotFound {
            kind: "person or account",
            id: to,
        });
    }
    Ok((pattern, to))
}

fn rule_at(doc: &mut DocumentMut, index: usize) -> Result<&mut Table, PeopleEditError> {
    doc.get_mut("transfer_rule")
        .and_then(Item::as_array_of_tables_mut)
        .and_then(|r| r.get_mut(index))
        .ok_or_else(|| PeopleEditError::NotFound {
            kind: "transfer rule",
            id: format!("#{}", index + 1),
        })
}

/// Remove the transfer rules whose `to` is `id` (they would point at nothing).
fn drop_rules_to(doc: &mut DocumentMut, id: &str) {
    if let Some(rules) = doc
        .get_mut("transfer_rule")
        .and_then(Item::as_array_of_tables_mut)
    {
        rules.retain(|r| r.get("to").and_then(Item::as_str) != Some(id));
        if rules.is_empty() {
            doc.remove("transfer_rule");
        }
    }
}

/// Give keys added to an account table the same indentation as its `id`, so
/// an indented file stays indented.
fn indent_like_id(t: &mut Table) {
    let Some(indent) = t
        .key("id")
        .and_then(|k| k.leaf_decor().prefix())
        .and_then(|p| p.as_str())
        .map(str::to_string)
    else {
        return;
    };
    let keys: Vec<String> = t.iter().map(|(k, _)| k.to_string()).collect();
    for k in keys {
        if let Some(mut key) = t.key_mut(&k) {
            if key.leaf_decor().prefix().is_none() {
                key.leaf_decor_mut().set_prefix(indent.clone());
            }
        }
    }
}

fn account_table<'d>(
    doc: &'d mut DocumentMut,
    account: &str,
) -> Result<&'d mut Table, PeopleEditError> {
    let not_found = || PeopleEditError::NotFound {
        kind: "account",
        id: account.to_string(),
    };
    let (pi, ai) = find_account(doc, account).ok_or_else(not_found)?;
    accounts_mut(person_at(doc, pi)?)?
        .get_mut(ai)
        .ok_or_else(not_found)
}

/// Tables print in their remembered file positions, and a `[[person.account]]`
/// belongs to whichever `[[person]]` precedes it in the text — so after moving
/// tables around, renumber them in logical order: each person followed by its
/// accounts, then the transfer rules. Comments travel with their table.
fn renumber(doc: &mut DocumentMut) {
    let mut pos = 1usize;
    if let Some(persons) = doc.get_mut("person").and_then(Item::as_array_of_tables_mut) {
        for p in persons.iter_mut() {
            p.set_position(pos);
            pos += 1;
            if let Some(accts) = p.get_mut("account").and_then(Item::as_array_of_tables_mut) {
                for a in accts.iter_mut() {
                    a.set_position(pos);
                    pos += 1;
                }
            }
        }
    }
    if let Some(rules) = doc
        .get_mut("transfer_rule")
        .and_then(Item::as_array_of_tables_mut)
    {
        for r in rules.iter_mut() {
            r.set_position(pos);
            pos += 1;
        }
    }
}

fn push_account(person: &mut Table, account: Table) {
    match person
        .get_mut("account")
        .and_then(Item::as_array_of_tables_mut)
    {
        Some(aot) => aot.push(account),
        None => {
            let mut aot = ArrayOfTables::new();
            aot.push(account);
            person.insert("account", Item::ArrayOfTables(aot));
        }
    }
}

/// Apply `ops` to `people.toml` text, keeping everything else intact.
///
/// # Errors
/// [`PeopleEditError`] when the text isn't TOML, an op names something that
/// doesn't exist, or the result would have duplicate ids / fail to parse.
pub fn apply(text: &str, ops: &[PeopleOp]) -> Result<String, PeopleEditError> {
    let mut doc: DocumentMut = text
        .parse()
        .map_err(|e: toml_edit::TomlError| PeopleEditError::Parse(e.to_string()))?;
    check_shape(&doc)?;
    for op in ops {
        apply_one(&mut doc, op)?;
    }
    renumber(&mut doc);
    let out = doc.to_string();
    // The engine must still accept it.
    let parsed: PeopleDoc =
        toml::from_str(&out).map_err(|e| PeopleEditError::Parse(e.to_string()))?;
    let mut seen = BTreeSet::new();
    for p in &parsed.persons {
        if !seen.insert(("person", p.id.clone())) {
            return Err(PeopleEditError::Duplicate {
                kind: "person",
                id: p.id.clone(),
            });
        }
        for a in &p.accounts {
            if !seen.insert(("account", a.id.clone())) {
                return Err(PeopleEditError::Duplicate {
                    kind: "account",
                    id: a.id.clone(),
                });
            }
        }
    }
    Ok(out)
}

/// Parse `people.toml` text (for showing it in an editor).
///
/// # Errors
/// [`PeopleEditError::Parse`] when the text isn't a valid people document.
pub fn parse(text: &str) -> Result<PeopleDoc, PeopleEditError> {
    toml::from_str(text).map_err(|e| PeopleEditError::Parse(e.to_string()))
}

/// [`apply`] with the ops given as a JSON array (the browser app's entry point).
///
/// # Errors
/// [`PeopleEditError::Request`] for malformed JSON, else as [`apply`].
pub fn apply_json(text: &str, ops_json: &str) -> Result<String, PeopleEditError> {
    let ops: Vec<PeopleOp> =
        serde_json::from_str(ops_json).map_err(|e| PeopleEditError::Request(e.to_string()))?;
    apply(text, &ops)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = r#"# Household identity — my notes stay put.
schema_version = 1

[[person]]
id = "me"
display_name = "Me"
  # My chequing account.
  [[person.account]]
  id = "chequing"
  filename = "(?i)debit"
  kind = "debit"
  [[person.account]]
  id = "card"
  cardholder = "ME"
  kind = "credit_card"

[[person]]
id = "partner"
display_name = "Partner"

# Spouse transfers.
[[transfer_rule]]
pattern  = "(?i)to partner"
to       = "partner"

[[transfer_rule]]
pattern  = "(?i)to savings"
to       = "chequing"
"#;

    fn doc(text: &str) -> PeopleDoc {
        toml::from_str(text).expect("valid people doc")
    }
    fn owner_of(d: &PeopleDoc, account: &str) -> Option<String> {
        d.persons
            .iter()
            .find(|p| p.accounts.iter().any(|a| a.id == account))
            .map(|p| p.id.clone())
    }
    fn run(ops: &[PeopleOp]) -> String {
        apply(DOC, ops).expect("edit applies")
    }

    #[test]
    fn rename_person_keeps_comments_and_retargets_rules() {
        let out = run(&[PeopleOp::RenamePerson {
            from: "partner".into(),
            to: "Sam".into(),
        }]);
        assert!(out.contains("# Household identity — my notes stay put."));
        assert!(out.contains("# Spouse transfers."));
        let d = doc(&out);
        assert!(d
            .persons
            .iter()
            .any(|p| p.id == "Sam" && p.display_name == "Sam"));
        assert_eq!(d.transfer_rules[0].to, "Sam");
        assert_eq!(
            d.transfer_rules[1].to, "chequing",
            "unrelated rule untouched"
        );
    }

    #[test]
    fn rename_account_retargets_rules() {
        let d = doc(&run(&[PeopleOp::RenameAccount {
            from: "chequing".into(),
            to: "Everyday".into(),
        }]));
        assert_eq!(owner_of(&d, "Everyday").as_deref(), Some("me"));
        assert_eq!(d.transfer_rules[1].to, "Everyday");
    }

    #[test]
    fn move_account_both_directions() {
        // Earlier person → later person (the case a stale table position would break).
        let out = run(&[PeopleOp::MoveAccount {
            account: "chequing".into(),
            to_person: "partner".into(),
        }]);
        assert!(
            out.contains("# My chequing account."),
            "the account's comment travels with it"
        );
        let d = doc(&out);
        assert_eq!(owner_of(&d, "chequing").as_deref(), Some("partner"));
        assert_eq!(owner_of(&d, "card").as_deref(), Some("me"));
        // …and back again.
        let d = doc(&apply(
            &out,
            &[PeopleOp::MoveAccount {
                account: "chequing".into(),
                to_person: "me".into(),
            }],
        )
        .unwrap());
        assert_eq!(owner_of(&d, "chequing").as_deref(), Some("me"));
        assert_eq!(
            d.persons
                .iter()
                .find(|p| p.id == "partner")
                .unwrap()
                .accounts
                .len(),
            0
        );
    }

    #[test]
    fn add_person_then_account_for_a_file() {
        let out = run(&[
            PeopleOp::AddPerson {
                id: "Kid".into(),
                display_name: None,
            },
            PeopleOp::AddAccount {
                person: "Kid".into(),
                id: "Kid savings".into(),
                kind: "savings".into(),
                file: Some("download (2).csv".into()),
                external: false,
            },
        ]);
        let d = doc(&out);
        let a = &d.persons.iter().find(|p| p.id == "Kid").unwrap().accounts[0];
        assert_eq!(a.id, "Kid savings");
        let re = regex::Regex::new(a.filename.as_deref().unwrap()).unwrap();
        assert!(re.is_match("download (2).csv") && re.is_match("DOWNLOAD (2).CSV"));
        assert!(!re.is_match("download (2).csv.bak") && !re.is_match("xdownload (2).csv"));
    }

    fn acct<'d>(d: &'d PeopleDoc, id: &str) -> &'d super::super::people::Account {
        d.persons
            .iter()
            .flat_map(|p| &p.accounts)
            .find(|a| a.id == id)
            .expect("account exists")
    }

    #[test]
    fn set_match_replaces_criteria_and_keeps_comments() {
        let out = run(&[PeopleOp::SetMatch {
            account: "chequing".into(),
            filename: Some("(?i)^Chq_".into()),
            profile: Some("bank_ledger".into()),
            cardholder: Some("  ".into()),
            card_last4: None,
        }]);
        assert!(out.contains("# My chequing account."));
        assert!(
            out.contains("\n  profile = \"bank_ledger\""),
            "new keys follow the file's indentation:\n{out}"
        );
        assert!(out.contains("\n  filename = "), "{out}");
        let d = doc(&out);
        let a = acct(&d, "chequing");
        assert_eq!(a.filename.as_deref(), Some("(?i)^Chq_"));
        assert_eq!(a.profile.as_deref(), Some("bank_ledger"));
        assert_eq!(a.cardholder, None, "blank criteria are dropped");
        assert_eq!(a.kind, "debit", "kind survives");
        // Card's cardholder criterion is replaced, not merged.
        let d = doc(&run(&[PeopleOp::SetMatch {
            account: "card".into(),
            filename: None,
            profile: Some("mastercard_credit".into()),
            cardholder: None,
            card_last4: Some("1234".into()),
        }]));
        let a = acct(&d, "card");
        assert_eq!(a.cardholder, None);
        assert_eq!(a.card_last4.as_deref(), Some("1234"));
    }

    #[test]
    fn set_match_rejects_nothing_and_bad_regex() {
        let none = PeopleOp::SetMatch {
            account: "chequing".into(),
            filename: None,
            profile: Some(" ".into()),
            cardholder: None,
            card_last4: None,
        };
        assert!(matches!(
            apply(DOC, &[none]),
            Err(PeopleEditError::Invalid(_))
        ));
        let bad = PeopleOp::SetMatch {
            account: "chequing".into(),
            filename: Some("(unclosed".into()),
            profile: None,
            cardholder: None,
            card_last4: None,
        };
        assert!(matches!(
            apply(DOC, &[bad]),
            Err(PeopleEditError::Invalid(_))
        ));
        let missing = PeopleOp::SetMatch {
            account: "nope".into(),
            filename: None,
            profile: Some("x".into()),
            cardholder: None,
            card_last4: None,
        };
        assert!(matches!(
            apply(DOC, &[missing]),
            Err(PeopleEditError::NotFound { .. })
        ));
    }

    #[test]
    fn external_round_trip_and_kind() {
        let d = doc(&run(&[
            PeopleOp::MakeExternal {
                account: "chequing".into(),
            },
            PeopleOp::SetKind {
                account: "chequing".into(),
                kind: "savings".into(),
            },
        ]));
        let a = acct(&d, "chequing");
        assert!(a.external);
        assert_eq!(a.filename, None);
        assert_eq!(a.kind, "savings");
        // Giving an external account a statement makes it a regular one.
        let text = apply(
            DOC,
            &[PeopleOp::MakeExternal {
                account: "chequing".into(),
            }],
        )
        .unwrap();
        for op in [
            PeopleOp::LinkFile {
                account: "chequing".into(),
                file: "download.csv".into(),
            },
            PeopleOp::SetMatch {
                account: "chequing".into(),
                filename: None,
                profile: Some("split_description_ledger".into()),
                cardholder: None,
                card_last4: None,
            },
        ] {
            let d = doc(&apply(&text, &[op]).unwrap());
            assert!(!acct(&d, "chequing").external);
        }
    }

    #[test]
    fn delete_account_and_person_drop_their_rules() {
        let out = run(&[PeopleOp::DeleteAccount {
            account: "chequing".into(),
        }]);
        let d = doc(&out);
        assert!(d.persons[0].accounts.iter().all(|a| a.id != "chequing"));
        assert!(d.transfer_rules.iter().all(|r| r.to != "chequing"));
        assert_eq!(d.transfer_rules.len(), 1, "other rules stay");
        assert!(out.contains("# Household identity"));
        // A person with accounts can't go; one without can.
        assert!(matches!(
            apply(
                DOC,
                &[PeopleOp::DeletePerson {
                    person: "me".into()
                }]
            ),
            Err(PeopleEditError::Invalid(_))
        ));
        let d = doc(&run(&[PeopleOp::DeletePerson {
            person: "partner".into(),
        }]));
        assert_eq!(d.persons.len(), 1);
        assert!(d.transfer_rules.iter().all(|r| r.to != "partner"));
        // Removing the last account of a person leaves a valid, account-less person.
        let d = doc(&run(&[
            PeopleOp::DeleteAccount {
                account: "chequing".into(),
            },
            PeopleOp::DeleteAccount {
                account: "card".into(),
            },
        ]));
        assert!(d.persons[0].accounts.is_empty());
        assert!(matches!(
            apply(
                DOC,
                &[PeopleOp::DeleteAccount {
                    account: "nope".into()
                }]
            ),
            Err(PeopleEditError::NotFound { .. })
        ));
    }

    #[test]
    fn transfer_rules_add_update_order_delete() {
        let out = run(&[PeopleOp::AddRule {
            pattern: "(?i)to card".into(),
            to: "card".into(),
            priority: None,
        }]);
        let d = doc(&out);
        assert_eq!(d.transfer_rules.len(), 3);
        assert_eq!(d.transfer_rules[2].to, "card");
        assert!(
            d.transfer_rules[2].priority
                <= d.transfer_rules.iter().map(|r| r.priority).min().unwrap()
        );
        // Unknown target / bad regex are refused.
        for (pattern, to) in [("x", "nobody"), ("(bad", "card")] {
            assert!(apply(
                &out,
                &[PeopleOp::AddRule {
                    pattern: pattern.into(),
                    to: to.into(),
                    priority: None
                }]
            )
            .is_err());
        }
        // Order: last first. Priorities descend in the new order; comments travel.
        let out = apply(
            &out,
            &[PeopleOp::OrderRules {
                order: vec![2, 0, 1],
            }],
        )
        .unwrap();
        let d = doc(&out);
        assert_eq!(
            d.transfer_rules
                .iter()
                .map(|r| r.to.as_str())
                .collect::<Vec<_>>(),
            ["card", "partner", "chequing"]
        );
        assert!(d
            .transfer_rules
            .windows(2)
            .all(|w| w[0].priority > w[1].priority));
        assert!(out.contains("# Spouse transfers."));
        assert!(apply(
            &out,
            &[PeopleOp::OrderRules {
                order: vec![0, 0, 1]
            }]
        )
        .is_err());
        let out = apply(
            &out,
            &[PeopleOp::UpdateRule {
                index: 0,
                pattern: "CARD PMT".into(),
                to: "me".into(),
            }],
        )
        .unwrap();
        assert_eq!(doc(&out).transfer_rules[0].pattern, "CARD PMT");
        let out = apply(&out, &[PeopleOp::DeleteRule { index: 0 }]).unwrap();
        assert_eq!(doc(&out).transfer_rules.len(), 2);
        assert!(apply(&out, &[PeopleOp::DeleteRule { index: 5 }]).is_err());
    }

    #[test]
    fn link_file_keeps_the_old_pattern() {
        let d = doc(&run(&[PeopleOp::LinkFile {
            account: "chequing".into(),
            file: "Chq_all.CSV".into(),
        }]));
        let re = regex::Regex::new(d.persons[0].accounts[0].filename.as_deref().unwrap()).unwrap();
        assert!(re.is_match("my_debit.csv"), "old match still works");
        assert!(re.is_match("Chq_all.CSV"));
        assert!(!re.is_match("other.csv"));
    }

    #[test]
    fn rejects_duplicates_missing_and_empty() {
        assert_eq!(
            apply(
                DOC,
                &[PeopleOp::RenameAccount {
                    from: "card".into(),
                    to: "chequing".into()
                }]
            ),
            Err(PeopleEditError::Duplicate {
                kind: "account",
                id: "chequing".into()
            })
        );
        assert_eq!(
            apply(
                DOC,
                &[PeopleOp::MoveAccount {
                    account: "nope".into(),
                    to_person: "me".into()
                }]
            ),
            Err(PeopleEditError::NotFound {
                kind: "account",
                id: "nope".into()
            })
        );
        assert!(matches!(
            apply(
                DOC,
                &[PeopleOp::RenamePerson {
                    from: "me".into(),
                    to: "  ".into()
                }]
            ),
            Err(PeopleEditError::Invalid(_))
        ));
        assert_eq!(
            apply(
                DOC,
                &[PeopleOp::AddPerson {
                    id: "me".into(),
                    display_name: None
                }]
            ),
            Err(PeopleEditError::Duplicate {
                kind: "person",
                id: "me".into()
            })
        );
    }

    #[test]
    fn json_entry_point() {
        let out = apply_json(DOC, r#"[{"op":"rename_person","from":"me","to":"Alex"}]"#).unwrap();
        assert!(doc(&out).persons.iter().any(|p| p.id == "Alex"));
        assert!(matches!(
            apply_json(DOC, "[{\"op\":\"explode\"}]"),
            Err(PeopleEditError::Request(_))
        ));
    }

    #[test]
    fn edits_the_shipped_defaults() {
        let text = crate::config::defaults::people();
        let d0 = doc(text);
        let first = d0.persons[0]
            .accounts
            .first()
            .map(|a| a.id.clone())
            .expect("defaults have an account");
        let out = apply(
            text,
            &[PeopleOp::RenameAccount {
                from: first.clone(),
                to: "Renamed".into(),
            }],
        )
        .unwrap();
        let mut texts = std::collections::BTreeMap::new();
        texts.insert("people.toml".to_string(), out);
        let cfg =
            crate::config::Config::from_texts(&texts).expect("engine accepts the edited file");
        assert!(cfg
            .people
            .persons
            .iter()
            .flat_map(|p| &p.accounts)
            .any(|a| a.id == "Renamed"));
    }
}
