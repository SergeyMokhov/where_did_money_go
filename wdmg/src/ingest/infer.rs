//! Layout inference: read a statement no configured profile matches.
//!
//! Banks export the same few shapes under many column names — one signed
//! amount, separate money-out/money-in columns, charges written positive on a
//! card — sometimes below a few summary lines, sometimes with no header at all.
//! [`infer`] recognises the columns by their usual names (English, plus common
//! French and German ones), or by their contents when there is no header, and
//! builds an ordinary [`Profile`] for the reader, so everything downstream treats
//! the file like any configured format. Its id starts with [`AUTO_PREFIX`] and
//! says whether it is a card ("auto-card-…") or a bank account ("auto-bank-…").
//!
//! Explicit profiles always win: inference only runs when none matches.

use std::collections::{BTreeMap, BTreeSet};

use crate::config::{
    AccountSpec, AmountSpec, BySign, Config, DateSpec, DirectionSpec, Encoding, Layout, MatchRules,
    MemoSpec, MerchantSpec, PersonSpec, Profile, Sign, StatusSpec, TypeSpec,
};
use crate::model::Money;

use super::reader::{clean_amount, parse_date_with, pick_date_format, strip_time_suffix};

/// Id prefix of every inferred profile.
pub const AUTO_PREFIX: &str = "auto-";

/// The account kind an inferred profile's id encodes ("auto-card-…" → card).
#[must_use]
pub fn kind_of(profile_id: &str) -> Option<&'static str> {
    let rest = profile_id.strip_prefix(AUTO_PREFIX)?;
    if rest.starts_with("card-") {
        Some("credit_card")
    } else {
        Some("debit")
    }
}

/// Date formats tried, in order of preference for an ambiguous file
/// (month-first before day-first; see [`pick_date_format`]).
const DATE_FORMATS: &[&str] = &[
    "%Y-%m-%d",
    "%m/%d/%Y",
    "%d/%m/%Y",
    "%m/%d/%y",
    "%d/%m/%y",
    "%Y/%m/%d",
    "%d.%m.%Y",
    "%d.%m.%y",
    "%m-%d-%Y",
    "%d-%m-%Y",
    "%Y%m%d",
    "%d %b %Y",
    "%d-%b-%Y",
    "%d-%b-%y",
    "%d %b %y",
    "%b %d, %Y",
    "%b %d %Y",
    "%d %B %Y",
    "%B %d, %Y",
    "%B %d %Y",
];

// Column names, compared after `key()` (lowercase, accents folded, "(GBP)",
// "$", ".", "#" and ":" removed, spaces collapsed). Earlier = preferred.
const DATE_NAMES: &[&str] = &[
    "transaction date",
    "trans date",
    "date",
    "started date",
    "booking date",
    "date posted",
    "posted date",
    "posting date",
    "post date",
    "transaction posted date",
    "completed date",
    "value date",
    "effective date",
    "created",
    "date de transaction",
    "date de l'operation",
    "date operation",
    "buchungstag",
    "buchungsdatum",
    "datum",
];
/// A second date column that marks a card statement (transaction + posting date).
const POSTING_NAMES: &[&str] = &[
    "post date",
    "posted date",
    "posting date",
    "clearing date",
    "date posted",
];
const AMOUNT_NAMES: &[&str] = &[
    "amount",
    "transaction amount",
    "amount cad",
    "amount usd",
    "cad",
    "usd",
    "net amount",
    "value",
    "montant",
    "betrag",
];
/// (money out, money in) column pairs.
const PAIRS: &[(&str, &str)] = &[
    ("debit", "credit"),
    ("debits", "credits"),
    ("debit amount", "credit amount"),
    ("money out", "money in"),
    ("paid out", "paid in"),
    ("funds out", "funds in"),
    ("withdrawals", "deposits"),
    ("withdrawal", "deposit"),
    ("withdrawal amount", "deposit amount"),
    ("outflow", "inflow"),
    ("spent", "received"),
    ("out", "in"),
    ("charges", "payments"),
    ("retrait", "depot"),
    ("retraits", "depots"),
    ("soll", "haben"),
];
const DESC_NAMES: &[&str] = &[
    "payee",
    "payee name",
    "merchant",
    "merchant name",
    "name",
    "counter party",
    "counterparty",
    "partner name",
    "description",
    "transaction description",
    "description 1",
    "transaction details",
    "details",
    "narrative",
    "particulars",
    "libelle",
    "beschreibung",
    "verwendungszweck",
    "empfanger",
    "memo",
    "reference",
];
const MEMO_NAMES: &[&str] = &[
    "description",
    "description 2",
    "memo",
    "notes",
    "notes and #tags",
    "payment reference",
    "reference",
    "transaction details",
    "verwendungszweck",
];
const TYPE_NAMES: &[&str] = &[
    "type",
    "transaction type",
    "type of transaction",
    "credit debit indicator",
];
const STATUS_NAMES: &[&str] = &["status", "state", "transaction status"];
const PERSON_NAMES: &[&str] = &[
    "card member",
    "card holder name",
    "cardholder name",
    "cardholder",
    "card holder",
    "purchased by",
    "card no",
    "card number",
    "card",
    "account number",
];
/// Status values whose rows are left out.
const SKIP_STATUS: &[&str] = &[
    "pending",
    "reverted",
    "declined",
    "failed",
    "cancelled",
    "canceled",
    "rejected",
];
/// Type values meaning a payment onto a card (a transfer, not income).
const CARD_PAYMENT_TYPES: &[&str] = &["payment", "payments", "payment/credit", "autopay", "pymt"];
/// Direction-column values meaning money out.
const OUTFLOW_WORDS: &[&str] = &["debit", "dr", "withdrawal", "d", "out", "debit card"];

/// Header-row search depth (lines of account summary some banks put first).
const HEADER_SEARCH: usize = 20;
/// Data rows sampled to choose date format, sign and decimal separator.
const SAMPLE_ROWS: usize = 500;

/// A column name normalised for matching.
fn key(name: &str) -> String {
    let mut s = String::with_capacity(name.len());
    let mut depth = 0u32;
    for c in name.trim().trim_matches('"').chars() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ if depth > 0 => {}
            '$' | '.' | '#' | ':' | '*' | '_' => s.push(' '),
            c => s.extend(fold(c).to_lowercase()),
        }
    }
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Fold common accented Latin letters to ASCII ("Débit" → "Debit").
fn fold(c: char) -> char {
    match c {
        'à' | 'á' | 'â' | 'ä' | 'À' | 'Â' | 'Ä' => 'a',
        'é' | 'è' | 'ê' | 'ë' | 'É' | 'È' | 'Ê' => 'e',
        'î' | 'ï' | 'Î' => 'i',
        'ô' | 'ö' | 'Ô' | 'Ö' => 'o',
        'ù' | 'û' | 'ü' | 'Û' | 'Ü' => 'u',
        'ç' | 'Ç' => 'c',
        c => c,
    }
}

/// The file's records (blank lines dropped), each with the 0-based line it starts on.
struct Grid {
    delimiter: u8,
    rows: Vec<(usize, Vec<String>)>,
}

fn grid(text: &str, delimiter: u8, limit: usize) -> Grid {
    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .flexible(true)
        .has_headers(false)
        .from_reader(text.as_bytes());
    let mut rows = Vec::new();
    for rec in rdr.records().take(limit) {
        let Ok(rec) = rec else { break };
        if rec.iter().all(|c| c.trim().is_empty()) {
            continue;
        }
        let line = rec
            .position()
            .map_or(0, |p| p.line().saturating_sub(1) as usize);
        rows.push((line, rec.iter().map(str::to_string).collect()));
    }
    Grid { delimiter, rows }
}

/// The delimiter that splits the file into the most consistent ≥3-column rows.
fn sniff(text: &str) -> Option<Grid> {
    let mut best: Option<(usize, Grid)> = None;
    for d in [b',', b';', b'\t', b'|'] {
        let g = grid(text, d, HEADER_SEARCH + SAMPLE_ROWS);
        let mut counts: BTreeMap<usize, usize> = BTreeMap::new();
        for (_, r) in &g.rows {
            *counts.entry(r.len()).or_default() += 1;
        }
        let Some((&width, &n)) = counts
            .iter()
            .filter(|(w, _)| **w >= 3)
            .max_by_key(|(_, n)| **n)
        else {
            continue;
        };
        let score = n * 100 + width;
        if best.as_ref().is_none_or(|(s, _)| score > *s) {
            best = Some((score, g));
        }
    }
    best.map(|(_, g)| g)
}

/// Column roles found in a header (or by contents, for a headerless file).
#[derive(Debug, Default)]
struct Roles {
    date: Option<usize>,
    amount: Option<usize>,
    pair: Option<(usize, usize)>,
    desc: Option<usize>,
    memo: Option<usize>,
    type_col: Option<usize>,
    status: Option<usize>,
    person: Option<usize>,
    balance: bool,
    card_hint: bool,
}

fn cell(row: &[String], i: usize) -> &str {
    row.get(i).map_or("", |s| s.trim())
}

/// Share of `rows` where column `i` is non-empty.
fn filled(rows: &[&Vec<String>], i: usize) -> f64 {
    if rows.is_empty() {
        return 0.0;
    }
    rows.iter().filter(|r| !cell(r, i).is_empty()).count() as f64 / rows.len() as f64
}

/// Does a cell look like a money amount ("-1,234.56", "$12.00", "(3.50)",
/// "1.234,56 €", "12.00 CAD")? Text with digits in it ("STORE #123") doesn't.
fn is_amount(v: &str) -> bool {
    let letters: String = v.chars().filter(|c| c.is_alphabetic()).collect();
    let symbols_ok = v.chars().all(|c| {
        c.is_alphanumeric()
            || matches!(
                c,
                '.' | ',' | '-' | '+' | '(' | ')' | '$' | '£' | '€' | ' ' | '\u{a0}'
            )
    });
    let letters_ok = letters.is_empty()
        || (letters.len() == 3 && letters.chars().all(|c| c.is_ascii_uppercase()))
        || letters == "CR"
        || letters == "DR";
    symbols_ok
        && letters_ok
        && v.chars().any(|c| c.is_ascii_digit())
        && (clean_amount(v, false).parse::<Money>().is_ok()
            || clean_amount(v, true).parse::<Money>().is_ok())
}

fn is_date(v: &str) -> bool {
    DATE_FORMATS.iter().any(|f| parse_date_with(f, v).is_some())
}

/// Roles from a header row, checked against the data under it.
fn roles_from_header(header: &[String], data: &[&Vec<String>]) -> Option<Roles> {
    let keys: Vec<String> = header.iter().map(|h| key(h)).collect();
    let find = |names: &[&str], used: &[usize]| -> Option<usize> {
        names.iter().find_map(|n| {
            keys.iter()
                .enumerate()
                .find(|(i, k)| k == n && !used.contains(i))
                .map(|(i, _)| i)
        })
    };
    // Prefer a candidate that actually has values (an RBC-style CAD$/USD$ pair,
    // a mostly empty "Payee Name").
    let find_filled = |names: &[&str], used: &[usize], min: f64| -> Option<usize> {
        names
            .iter()
            .filter_map(|n| keys.iter().position(|k| k == n))
            .filter(|i| !used.contains(i))
            .find(|i| filled(data, *i) >= min)
    };
    let mut r = Roles {
        date: find(DATE_NAMES, &[]),
        ..Roles::default()
    };
    // The date column must hold dates.
    let date = r.date?;
    if !data.iter().any(|row| is_date(cell(row, date))) {
        return None;
    }
    r.amount = find_filled(AMOUNT_NAMES, &[date], 0.5);
    if r.amount.is_none() {
        r.pair = PAIRS.iter().find_map(|(o, i)| {
            let (o, i) = (
                keys.iter().position(|k| k == o)?,
                keys.iter().position(|k| k == i)?,
            );
            Some((o, i))
        });
    }
    if r.amount.is_none() && r.pair.is_none() {
        return None;
    }
    let mut used = vec![date];
    used.extend(r.amount);
    if let Some((o, i)) = r.pair {
        used.extend([o, i]);
    }
    r.desc = find_filled(DESC_NAMES, &used, 0.5).or_else(|| find(DESC_NAMES, &used));
    let desc = r.desc?;
    used.push(desc);
    r.memo = find_filled(MEMO_NAMES, &used, 0.01);
    r.type_col = find(TYPE_NAMES, &used);
    r.status = find(STATUS_NAMES, &used);
    r.person = find_filled(PERSON_NAMES, &used, 0.5);
    r.balance = keys.iter().any(|k| {
        k.starts_with("balance") || k.contains("running bal") || k == "solde" || k == "saldo"
    });
    r.card_hint = keys.iter().any(|k| k.contains("card"))
        || keys
            .iter()
            .enumerate()
            .any(|(i, k)| i != date && POSTING_NAMES.contains(&k.as_str()));
    Some(r)
}

/// Roles of a headerless file, from what its columns hold.
fn roles_from_contents(data: &[&Vec<String>]) -> Option<Roles> {
    let width = data.iter().map(|r| r.len()).max()?;
    let all = |i: usize, f: &dyn Fn(&str) -> bool| {
        data.iter()
            .map(|r| cell(r, i))
            .filter(|v| !v.is_empty())
            .all(f)
            && filled(data, i) > 0.0
    };
    let date = (0..width).find(|&i| filled(data, i) >= 0.9 && all(i, &is_date))?;
    let numeric: Vec<usize> = (0..width)
        .filter(|&i| i != date && all(i, &is_amount))
        .collect();
    let full: Vec<usize> = numeric
        .iter()
        .copied()
        .filter(|&i| filled(data, i) >= 0.99)
        .collect();
    let sparse: Vec<usize> = numeric
        .iter()
        .copied()
        .filter(|&i| filled(data, i) < 0.99)
        .collect();
    let mut r = Roles {
        date: Some(date),
        ..Roles::default()
    };
    if sparse.len() >= 2 {
        // date, description, money out, money in[, balance]
        r.pair = Some((sparse[0], sparse[1]));
        r.balance = !full.is_empty();
    } else if let Some(&a) = full.first() {
        r.amount = Some(a);
        r.balance = full.len() > 1;
    } else {
        return None;
    }
    // The description: the text column with the longest values.
    let avg_len =
        |i: usize| data.iter().map(|r| cell(r, i).len()).sum::<usize>() as f64 / data.len() as f64;
    r.desc = (0..width)
        .filter(|i| *i != date && !numeric.contains(i))
        .max_by(|a, b| avg_len(*a).total_cmp(&avg_len(*b)))
        .filter(|&i| avg_len(i) >= 3.0);
    r.desc?;
    Some(r)
}

/// A currency the header or a currency column names, if any.
fn currency_hint(header: &[String], data: &[&Vec<String>]) -> Option<String> {
    const CODES: &[&str] = &[
        "CAD", "USD", "GBP", "EUR", "AUD", "NZD", "CHF", "SEK", "NOK", "DKK", "PLN", "INR",
    ];
    for h in header {
        let u = h.to_uppercase();
        if let Some(c) = CODES.iter().find(|c| u.contains(*c)) {
            return Some((*c).to_string());
        }
        if u.contains('£') {
            return Some("GBP".into());
        }
        if u.contains('€') {
            return Some("EUR".into());
        }
    }
    let col = header.iter().position(|h| key(h) == "currency")?;
    let v = data.iter().map(|r| cell(r, col)).find(|v| !v.is_empty())?;
    CODES
        .iter()
        .find(|c| v.eq_ignore_ascii_case(c))
        .map(|c| (*c).to_string())
}

/// Infer a profile for `text` (already decoded, BOM stripped), or `None` when it
/// doesn't look like a statement.
#[must_use]
pub fn infer(config: &Config, text: &str) -> Option<Profile> {
    let g = sniff(text)?;
    // A header row within the first lines, else a headerless file.
    let mut found: Option<(usize, Roles, bool)> = None;
    for (idx, (_, row)) in g.rows.iter().enumerate().take(HEADER_SEARCH) {
        let data: Vec<&Vec<String>> = g.rows[idx + 1..]
            .iter()
            .map(|(_, r)| r)
            .take(SAMPLE_ROWS)
            .collect();
        if data.is_empty() {
            break;
        }
        if let Some(r) = roles_from_header(row, &data) {
            found = Some((idx, r, false));
            break;
        }
    }
    if found.is_none() {
        let data: Vec<&Vec<String>> = g.rows.iter().map(|(_, r)| r).take(SAMPLE_ROWS).collect();
        // Every sampled row must start like data (a date somewhere) for a headerless read.
        if data.len() >= 2 {
            if let Some(r) = roles_from_contents(&data) {
                found = Some((0, r, true));
            }
        }
    }
    let (idx, roles, headerless) = found?;
    let header: Vec<String> = if headerless {
        Vec::new()
    } else {
        g.rows[idx].1.clone()
    };
    let data: Vec<&Vec<String>> = g.rows[if headerless { idx } else { idx + 1 }..]
        .iter()
        .map(|(_, r)| r)
        .take(SAMPLE_ROWS)
        .collect();
    let name = |i: usize| -> String {
        if headerless {
            (i + 1).to_string()
        } else {
            header[i].clone()
        }
    };
    let date = roles.date?;
    let currency = currency_hint(&header, &data).unwrap_or_else(|| default_currency(config));
    let day_first = matches!(
        currency.as_str(),
        "GBP" | "EUR" | "AUD" | "NZD" | "CHF" | "SEK" | "NOK" | "DKK" | "PLN" | "INR"
    );

    // Date format: one that reads (nearly) every row's date, in statement order.
    let dates: Vec<&str> = data
        .iter()
        .map(|r| cell(r, date))
        .filter(|v| !v.is_empty())
        .collect();
    let formats: Vec<String> = DATE_FORMATS.iter().map(|f| (*f).to_string()).collect();
    let readable: Vec<&str> = dates
        .iter()
        .copied()
        .filter(|v| is_date(v) || strip_time_suffix(v).is_some_and(is_date))
        .collect();
    if readable.is_empty() || (readable.len() as f64) < dates.len() as f64 * 0.8 {
        return None;
    }
    let date_format = pick_date_format(&readable, &formats, day_first)?;

    // Amount values (for the separator and sign checks).
    let amount_cells: Vec<&str> = match (roles.amount, roles.pair) {
        (Some(a), _) => data
            .iter()
            .map(|r| cell(r, a))
            .filter(|v| !v.is_empty())
            .collect(),
        (None, Some((o, i))) => data
            .iter()
            .flat_map(|r| [cell(r, o), cell(r, i)])
            .filter(|v| !v.is_empty())
            .collect(),
        _ => return None,
    };
    let decimal_comma = {
        let ends = |v: &str, sep: char| {
            let digits: String = v
                .chars()
                .filter(|c| c.is_ascii_digit() || *c == '.' || *c == ',')
                .collect();
            digits
                .rfind(sep)
                .is_some_and(|p| (1..=2).contains(&(digits.len() - p - 1)))
                && !digits[digits.rfind(sep).unwrap_or(0) + 1..].contains(['.', ','])
        };
        let comma = amount_cells.iter().filter(|v| ends(v, ',')).count();
        let dot = amount_cells.iter().filter(|v| ends(v, '.')).count();
        comma > dot
    };

    let type_values: BTreeSet<String> = roles
        .type_col
        .map(|t| {
            data.iter()
                .map(|r| cell(r, t).to_string())
                .filter(|v| !v.is_empty())
                .collect()
        })
        .unwrap_or_default();

    // Sign.
    let mut sign = Sign::Signed;
    let mut direction = None;
    let mut outflow_positive = false;
    if let Some(a) = roles.amount {
        let vals: Vec<rust_decimal::Decimal> = data
            .iter()
            .filter_map(|r| {
                clean_amount(cell(r, a), decimal_comma)
                    .parse::<Money>()
                    .ok()
            })
            .map(|m| m.amount())
            .collect();
        let pos = vals
            .iter()
            .filter(|v| v.is_sign_positive() && !v.is_zero())
            .count();
        let neg = vals.iter().filter(|v| v.is_sign_negative()).count();
        let words: Vec<String> = type_values
            .iter()
            .filter(|v| OUTFLOW_WORDS.contains(&v.to_lowercase().as_str()))
            .cloned()
            .collect();
        if neg == 0 && !words.is_empty() {
            // Unsigned amounts + a Debit/Credit indicator column.
            direction = roles.type_col.map(|t| DirectionSpec {
                column: name(t),
                outflow: words,
            });
        } else if vals.len() >= 5 && pos * 4 >= vals.len() * 3 && !roles.balance {
            // Mostly positive and no running balance to say otherwise: a card that
            // writes charges positive (payments and refunds negative).
            sign = Sign::OutflowPositive;
            outflow_positive = true;
        }
    }
    let (amount_column, debit_column, credit_column) = match (roles.amount, roles.pair) {
        (Some(a), _) => (name(a), None, None),
        (None, Some((o, i))) => {
            sign = Sign::SeparateDebitCredit;
            (name(o), Some(name(o)), Some(name(i)))
        }
        _ => return None,
    };
    let card = roles.card_hint
        || outflow_positive
        || type_values.iter().any(|v| {
            CARD_PAYMENT_TYPES.contains(&v.to_lowercase().as_str())
                || v.eq_ignore_ascii_case("sale")
        });

    // Card payments are transfers (as the built-in card formats treat them).
    let mut map = BTreeMap::new();
    if card {
        for v in &type_values {
            if CARD_PAYMENT_TYPES.contains(&v.to_lowercase().as_str()) {
                map.insert(v.clone(), "transfer".to_string());
            }
        }
    }
    // Pending, reverted, declined… rows aren't (yet) real money movements.
    let status = roles.status.map(|s| StatusSpec {
        column: name(s),
        skip: SKIP_STATUS.iter().map(|v| v.to_uppercase()).collect(),
    });

    let desc = roles.desc?;
    let slug = |s: &str| {
        let k = key(s).replace(' ', "_");
        if k.is_empty() {
            "x".to_string()
        } else {
            k
        }
    };
    let mut parts = vec![slug(&name(date)), slug(&name(desc))];
    parts.push(slug(&amount_column));
    if let Some(c) = &credit_column {
        parts.push(slug(c));
    }
    let id = format!(
        "{AUTO_PREFIX}{}-{}{}",
        if card { "card" } else { "bank" },
        if headerless { "noheader-" } else { "" },
        parts.join("-")
    );
    let mut id: String = id.chars().take(80).collect();
    if outflow_positive {
        id.push_str("-charges_positive");
    }

    Some(Profile {
        id,
        match_rules: MatchRules::default(),
        encoding: Encoding::default(),
        layout: Layout {
            skip_blank_rows_after_header: 0,
            delimiter: Some(g.delimiter as char),
            header_row: g.rows[idx].0,
            headerless,
            lenient: true,
        },
        date: DateSpec {
            column: name(date),
            format: Some(date_format),
            formats: Vec::new(),
        },
        amount: AmountSpec {
            column: amount_column,
            sign,
            debit_column,
            credit_column,
            decimal_comma,
            direction,
        },
        merchant: MerchantSpec {
            column: name(desc),
            extra_columns: Vec::new(),
            strip_codes: None,
            trim: None,
        },
        memo: roles.memo.map(|m| MemoSpec {
            column: name(m),
            category_harvest: None,
        }),
        type_spec: TypeSpec {
            column: roles.type_col.filter(|_| !map.is_empty()).map(name),
            map,
            by_sign: Some(BySign {
                negative: "spend".into(),
                positive: if card { "transfer" } else { "income" }.into(),
            }),
        },
        person: roles.person.map(|p| PersonSpec { column: name(p) }),
        status,
        account: AccountSpec {
            kind: if card { "credit_card" } else { "debit" }.into(),
            currency,
            id_from_filename: Some(r"(\d{4})".into()),
        },
    })
}

/// The currency most configured profiles use (the household's usual one).
fn default_currency(config: &Config) -> String {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for p in &config.profiles {
        *counts
            .entry(p.profile.account.currency.as_str())
            .or_default() += 1;
    }
    counts
        .into_iter()
        .max_by_key(|(_, n)| *n)
        .map_or_else(|| "CAD".to_string(), |(c, _)| c.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_normalise_names() {
        assert_eq!(key("Trans. Date"), "trans date");
        assert_eq!(key("Amount (GBP)"), "amount");
        assert_eq!(key(" Withdrawals ($)"), "withdrawals");
        assert_eq!(key("CAD$"), "cad");
        assert_eq!(key("Débit"), "debit");
        assert_eq!(key("\"Running Bal.\""), "running bal");
    }
}
