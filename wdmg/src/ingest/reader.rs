//! CSV reading: parse a file with a chosen profile into [`Transaction`]s.
//!
//! Handles real-world quirks: a UTF-8 BOM, blank rows after the header, a header
//! name with a leading space (`" Transaction Amount"`), quoted fields, and
//! leading-apostrophe card numbers (`'5510...'`). Dates are parsed by trying each
//! configured format in order; amounts are parsed exactly via [`Money`].

use std::collections::HashMap;
use std::path::Path;

use jiff::civil::{Date, Time};
use rust_decimal::Decimal;

use crate::config::{CompiledProfile, Sign, TypeSpec};
use crate::error::{IngestError, ParseError};
use crate::model::{Currency, Money, Transaction, TxnType};
use crate::normalize::normalize_merchant;

use super::detect::{basename, strip_bom};

/// Parse the file at `path` into transactions using `profile`. Thin fs wrapper
/// over [`read_text`].
pub fn read_file(profile: &CompiledProfile, path: &Path) -> Result<Vec<Transaction>, IngestError> {
    let raw = std::fs::read_to_string(path).map_err(|source| IngestError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    read_text(profile, path, &raw)
}

/// Parse decoded file `content` (from a file named `label`) into transactions
/// using `profile`. Merchant normalization is applied here;
/// `dedup_key`/owner/account are left for later phases.
pub fn read_text(
    profile: &CompiledProfile,
    label: &Path,
    content: &str,
) -> Result<Vec<Transaction>, IngestError> {
    let text = strip_bom(content);
    let base = basename(label);
    let delimiter = profile.profile.layout.delimiter.unwrap_or(',') as u8;

    // Split header from body so we can drop the configured count of blank rows
    // BEFORE handing the body to the CSV reader (csv treats blank lines as records
    // when flexible, which would otherwise desync our column lookups).
    let lay = &profile.profile.layout;
    let mut lines = text.lines().skip(lay.header_row);
    // A headerless file's first line is data: name its columns "1", "2", … instead.
    let header_line = if lay.headerless {
        ""
    } else {
        lines.next().unwrap_or("")
    };
    let skip = profile.profile.layout.skip_blank_rows_after_header;
    let mut skipped = 0usize;
    let mut body_lines: Vec<&str> = Vec::new();
    for line in lines {
        if skipped < skip && line.trim().is_empty() {
            skipped += 1;
            continue;
        }
        body_lines.push(line);
    }
    // Drop any further fully-blank lines defensively.
    let body: String = {
        let mut s = String::with_capacity(text.len());
        if !lay.headerless {
            s.push_str(header_line);
            s.push('\n');
        }
        for l in body_lines {
            s.push_str(l);
            s.push('\n');
        }
        s
    };

    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .flexible(true)
        .has_headers(!lay.headerless)
        .from_reader(body.as_bytes());

    // Map header name -> column index. Header names are used verbatim (including
    // any leading space, e.g. " Transaction Amount"); headerless files use "1"…
    let index: HashMap<String, usize> = if lay.headerless {
        (1..=MAX_POSITIONAL_COLUMNS)
            .map(|i| (i.to_string(), i - 1))
            .collect()
    } else {
        let headers = rdr.headers().map_err(|source| IngestError::Csv {
            path: label.to_path_buf(),
            source,
        })?;
        headers
            .iter()
            .enumerate()
            .map(|(i, h)| (h.to_string(), i))
            .collect()
    };

    let p = &profile.profile;
    // Validate every declared column exists up front ("parse, don't validate":
    // fail at the boundary with a clear typed error, never silently).
    let mut required = vec![p.date.column.as_str(), p.amount.column.as_str()];
    required.extend(p.merchant.columns());
    if let Some(t) = &p.type_spec.column {
        required.push(t.as_str());
    }
    if let Some(m) = &p.memo {
        required.push(m.column.as_str());
    }
    if let Some(pe) = &p.person {
        required.push(pe.column.as_str());
    }
    if let Some(st) = &p.status {
        required.push(st.column.as_str());
    }
    if let Some(d) = &p.amount.direction {
        required.push(d.column.as_str());
    }
    if p.amount.sign == Sign::SeparateDebitCredit {
        required.extend(p.amount.debit_column.as_deref());
        required.extend(p.amount.credit_column.as_deref());
    }
    for col in required {
        if !index.contains_key(col) {
            return Err(ParseError::MissingColumn {
                profile: p.id.clone(),
                column: col.to_string(),
            }
            .into());
        }
    }

    let currency = Currency::new(p.account.currency.clone());

    let mut records = Vec::new();
    for record in rdr.records() {
        let record = record.map_err(|source| IngestError::Csv {
            path: label.to_path_buf(),
            source,
        })?;
        records.push(record);
    }
    // Several date formats (e.g. month-first and day-first): settle on one for
    // the whole file when one reads every row, so "03/09" means the same as
    // "25/09" does. Otherwise (mixed padding, …) each row tries them in order.
    let mut date_formats = p.date.all_formats();
    if date_formats.len() > 1 {
        let values: Vec<&str> = records
            .iter()
            .map(|r| cell(&index, r, &p.date.column).trim())
            .filter(|v| !v.is_empty())
            .collect();
        if let Some(f) = pick_date_format(&values, &date_formats, false) {
            date_formats = vec![f];
        }
    }
    let skip_status: Vec<String> = p
        .status
        .as_ref()
        .map(|st| st.skip.iter().map(|v| v.trim().to_lowercase()).collect())
        .unwrap_or_default();

    let mut out = Vec::new();
    for (row_idx, record) in records.iter().enumerate() {
        // Skip stray fully-empty records.
        if record.iter().all(|c| c.trim().is_empty()) {
            continue;
        }
        if let Some(st) = &p.status {
            let v = cell(&index, record, &st.column).trim().to_lowercase();
            if skip_status.contains(&v) {
                continue;
            }
        }
        let row = row_idx + 1;
        match parse_record(
            profile,
            &index,
            record,
            &date_formats,
            &currency,
            &base,
            row,
        ) {
            Ok(txn) => out.push(txn),
            // Balance lines, totals and footers: skipped when the layout allows it.
            Err(_) if lay.lenient => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(out)
}

/// Columns a headerless file may have ("1" … this).
const MAX_POSITIONAL_COLUMNS: usize = 64;

/// Plausible statement years: guards against `%Y` reading "24" as the year 24.
fn plausible(d: Date) -> bool {
    (1900..=3100).contains(&d.year())
}

/// Parse a date cell with one format, accepting a trailing clock time.
pub(crate) fn parse_date_with(fmt: &str, v: &str) -> Option<Date> {
    let one = |v: &str| {
        Date::strptime(fmt.as_bytes(), v.as_bytes())
            .ok()
            .filter(|d| plausible(*d))
    };
    one(v).or_else(|| strip_time_suffix(v).and_then(one))
}

/// The one format (of `formats`, in order) that reads every value, preferring one
/// under which the dates run in order (statements are sorted, a misread day/month
/// order scrambles them). `day_first` breaks a remaining tie toward a format whose
/// day comes before its month. `None` when no format reads them all.
pub(crate) fn pick_date_format(
    values: &[&str],
    formats: &[String],
    day_first: bool,
) -> Option<String> {
    let mut readers: Vec<(&String, Vec<Date>)> = Vec::new();
    for f in formats {
        let dates: Option<Vec<Date>> = values.iter().map(|v| parse_date_with(f, v)).collect();
        if let Some(d) = dates {
            readers.push((f, d));
        }
    }
    if readers.is_empty() {
        return None;
    }
    let sorted =
        |d: &[Date]| d.windows(2).all(|w| w[0] <= w[1]) || d.windows(2).all(|w| w[0] >= w[1]);
    let ordered: Vec<&(&String, Vec<Date>)> = readers.iter().filter(|(_, d)| sorted(d)).collect();
    let pool: Vec<&(&String, Vec<Date>)> = if ordered.is_empty() {
        readers.iter().collect()
    } else {
        ordered
    };
    // All candidates read the same dates (e.g. padded vs unpadded): the first wins.
    let first = pool[0];
    if pool.iter().all(|(_, d)| *d == first.1) {
        return Some(first.0.clone());
    }
    let dayish = |f: &str| {
        let (d, m) = (
            f.find("%d").or_else(|| f.find("%-d")),
            f.find("%m")
                .or_else(|| f.find("%-m"))
                .or_else(|| f.find("%b"))
                .or_else(|| f.find("%B")),
        );
        matches!((d, m), (Some(d), Some(m)) if d < m)
    };
    pool.iter()
        .find(|(f, _)| dayish(f) == day_first)
        .or(pool.first())
        .map(|(f, _)| (*f).clone())
}

/// Fetch a cell by configured column name. The column's presence was validated
/// already; a short record simply yields an empty string.
fn cell<'r>(
    index: &HashMap<String, usize>,
    record: &'r csv::StringRecord,
    column: &str,
) -> &'r str {
    index.get(column).and_then(|&i| record.get(i)).unwrap_or("")
}

fn parse_record(
    profile: &CompiledProfile,
    index: &HashMap<String, usize>,
    record: &csv::StringRecord,
    date_formats: &[String],
    currency: &Currency,
    source_file: &str,
    row: usize,
) -> Result<Transaction, ParseError> {
    let p = &profile.profile;

    // --- date (try each format in order) ---
    let date_raw = cell(index, record, &p.date.column).trim();
    if date_raw.is_empty() {
        return Err(ParseError::EmptyField {
            profile: p.id.clone(),
            row,
            column: p.date.column.clone(),
        });
    }
    let (date, time) = parse_date_time(p, index, record, date_raw, date_formats, row)?;

    // --- amount (exact decimal) ---
    let amount = parse_amount(profile, index, record, row)?;

    // --- merchant ---
    let raw_description = merchant_text(&p.merchant, index, record);
    let normalized_merchant = normalize_merchant(&raw_description, profile);

    // --- type map ---
    let txn_type = classify(&p.type_spec, index, record, &amount);

    // --- cardholder (raw person column, retained for identity matching) ---
    let cardholder = p.person.as_ref().and_then(|pe| {
        let v = cell(index, record, &pe.column).trim();
        if v.is_empty() {
            None
        } else {
            Some(v.to_string())
        }
    });

    // --- memo + harvested category ---
    let (memo, harvested_category) = match &p.memo {
        Some(memo_spec) => {
            let memo_val = cell(index, record, &memo_spec.column).trim();
            let memo = if memo_val.is_empty() {
                None
            } else {
                Some(memo_val.to_string())
            };
            let cat = profile.category_harvest.as_ref().and_then(|re| {
                re.captures(memo_val)
                    .and_then(|c| c.name("cat"))
                    .map(|m| m.as_str().trim().to_string())
                    .filter(|s| !s.is_empty())
            });
            (memo, cat)
        }
        None => (None, None),
    };

    Ok(Transaction {
        raw_description,
        normalized_merchant,
        date,
        time,
        amount,
        currency: currency.clone(),
        txn_type,
        memo,
        cardholder,
        profile_id: p.id.clone(),
        source_file: source_file.to_string(),
        owner_person: None,
        account_id: None,
        harvested_category,
        dedup_key: String::new(),
    })
}

/// The description: the merchant column, plus any `extra_columns` (non-empty
/// cells only), joined by single spaces.
fn merchant_text(
    spec: &crate::config::MerchantSpec,
    index: &HashMap<String, usize>,
    record: &csv::StringRecord,
) -> String {
    if spec.extra_columns.is_empty() {
        return strip_card_quote(cell(index, record, &spec.column)).to_string();
    }
    spec.columns()
        .map(|c| strip_card_quote(cell(index, record, c)).trim())
        .filter(|v| !v.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// `"2999-03-31 12:00:00 AM"` → `"2999-03-31"`: the value with a trailing clock
/// time (12/24h, optional seconds/fraction, AM/PM, `T` separator, `Z`/offset)
/// removed, or `None` when there is none. Lets every date format also accept
/// exports that stamp a (usually meaningless) time onto the date.
pub(crate) fn strip_time_suffix(s: &str) -> Option<&str> {
    // INVARIANT: a constant pattern (covered by tests); it always compiles.
    #[allow(clippy::expect_used)]
    static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r"(?i)^(.*?\d)[ T]+\d{1,2}:\d{2}(?::\d{2}(?:\.\d+)?)?\s*(?:[ap]\.?m\.?)?\s*(?:z|[+-]\d{2}:?\d{2})?$",
        )
        .expect("time-suffix regex")
    });
    RE.captures(s).and_then(|c| c.get(1)).map(|m| m.as_str())
}

/// Parse the date and any optional companion `Time` column.
fn parse_date_time(
    p: &crate::config::Profile,
    index: &HashMap<String, usize>,
    record: &csv::StringRecord,
    date_raw: &str,
    formats: &[String],
    row: usize,
) -> Result<(Date, Option<Time>), ParseError> {
    let date = formats
        .iter()
        .find_map(|fmt| parse_date_with(fmt, date_raw));
    let date = date.ok_or_else(|| ParseError::Date {
        profile: p.id.clone(),
        row,
        value: date_raw.to_string(),
        formats: formats.to_vec(),
    })?;

    // Optional Time column ("Time"), e.g. Mastercard "10:22 PM". Parse leniently;
    // a missing/unparseable time is not fatal — it's metadata.
    let time = index
        .get("Time")
        .and_then(|&i| record.get(i))
        .and_then(|t| {
            let t = t.trim();
            if t.is_empty() {
                None
            } else {
                Time::strptime("%I:%M %p", t.as_bytes())
                    .or_else(|_| Time::strptime("%H:%M:%S", t.as_bytes()))
                    .or_else(|_| Time::strptime("%H:%M", t.as_bytes()))
                    .ok()
            }
        });

    Ok((date, time))
}

/// Parse the amount according to the profile's sign mode.
///
/// Phase 1 fully supports `signed`. `outflow_positive` negates, and
/// `separate_debit_credit` combines two columns; both are implemented minimally
/// so later phases can rely on the same entry point.
fn parse_amount(
    profile: &CompiledProfile,
    index: &HashMap<String, usize>,
    record: &csv::StringRecord,
    row: usize,
) -> Result<Money, ParseError> {
    let p = &profile.profile;
    let parse = |val: &str| -> Result<Money, ParseError> {
        let v = val.trim();
        clean_amount(v, p.amount.decimal_comma)
            .parse::<Money>()
            .map_err(|source| ParseError::Amount {
                profile: p.id.clone(),
                row,
                value: v.to_string(),
                source,
            })
    };

    match p.amount.sign {
        Sign::Signed => {
            let raw = cell(index, record, &p.amount.column).trim();
            if raw.is_empty() {
                return Err(ParseError::EmptyField {
                    profile: p.id.clone(),
                    row,
                    column: p.amount.column.clone(),
                });
            }
            let m = parse(raw)?;
            // Unsigned amount + a direction column ("Debit"/"Credit").
            match &p.amount.direction {
                Some(d) => {
                    let v = cell(index, record, &d.column).trim();
                    let out = d.outflow.iter().any(|o| o.eq_ignore_ascii_case(v));
                    Ok(Money::new(if out {
                        -m.amount().abs()
                    } else {
                        m.amount().abs()
                    }))
                }
                None => Ok(m),
            }
        }
        Sign::OutflowPositive => {
            let raw = cell(index, record, &p.amount.column).trim();
            let m = parse(raw)?;
            // Outflow stored positive in source -> negate to our signed convention.
            Ok(Money::new(-m.amount()))
        }
        Sign::SeparateDebitCredit => {
            let debit_col = p.amount.debit_column.as_deref().unwrap_or("");
            let credit_col = p.amount.credit_column.as_deref().unwrap_or("");
            let debit = cell(index, record, debit_col).trim();
            let credit = cell(index, record, credit_col).trim();
            // Money out is the debit, money in the credit, whatever sign a bank
            // writes them with; a row with both (rare) nets them.
            if !debit.is_empty() || !credit.is_empty() {
                let d = if debit.is_empty() {
                    Decimal::ZERO
                } else {
                    parse(debit)?.amount().abs()
                };
                let c = if credit.is_empty() {
                    Decimal::ZERO
                } else {
                    parse(credit)?.amount().abs()
                };
                Ok(Money::new(c - d))
            } else {
                Err(ParseError::EmptyField {
                    profile: p.id.clone(),
                    row,
                    column: format!("{debit_col}/{credit_col}"),
                })
            }
        }
    }
}

/// An amount cell as a plain decimal string: currency symbols and codes, spaces
/// and thousands separators dropped; `(12.50)` and `12.50-` are negative; with
/// `decimal_comma`, `1.234,56` → `1234.56`.
pub(crate) fn clean_amount(raw: &str, decimal_comma: bool) -> String {
    let mut t = raw.trim();
    let mut neg = false;
    if let Some(inner) = t.strip_prefix('(').and_then(|x| x.strip_suffix(')')) {
        neg = true;
        t = inner.trim();
    }
    if let Some(inner) = t.strip_suffix('-') {
        neg = !neg;
        t = inner.trim();
    }
    let kept: String = t
        .chars()
        .filter(|c| c.is_ascii_digit() || matches!(c, '.' | ',' | '-' | '+'))
        .collect();
    let mut num = if decimal_comma {
        kept.replace('.', "").replace(',', ".")
    } else {
        kept.replace(',', "")
    };
    if neg {
        num = match num.strip_prefix('-') {
            Some(pos) => pos.to_string(),
            None => format!("-{}", num.trim_start_matches('+')),
        };
    }
    num
}

/// Kind from the type column's map, else from the amount's sign (`by_sign`),
/// else `Unknown` (left to categorization rules).
fn classify(
    spec: &TypeSpec,
    index: &HashMap<String, usize>,
    record: &csv::StringRecord,
    amount: &Money,
) -> TxnType {
    let mapped = spec
        .column
        .as_deref()
        .map(|c| cell(index, record, c).trim())
        .and_then(|raw| spec.map.get(raw))
        .map(|m| TxnType::from_mapped(m))
        .filter(|t| *t != TxnType::Unknown);
    if let Some(t) = mapped {
        return t;
    }
    match &spec.by_sign {
        Some(bs) if amount.amount().is_sign_negative() => TxnType::from_mapped(&bs.negative),
        Some(bs) => TxnType::from_mapped(&bs.positive),
        None => TxnType::Unknown,
    }
}

/// Strip the leading-apostrophe-and-trailing-apostrophe wrapper some exports put
/// around card numbers and codes (e.g. `'5550000000001234'`). Leaves ordinary
/// descriptions untouched.
fn strip_card_quote(s: &str) -> &str {
    let t = s.trim();
    if t.len() >= 2 && t.starts_with('\'') && t.ends_with('\'') {
        &t[1..t.len() - 1]
    } else {
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_amounts() {
        assert_eq!(clean_amount("$1,234.56", false), "1234.56");
        assert_eq!(clean_amount("(12.50)", false), "-12.50");
        assert_eq!(clean_amount("12.50-", false), "-12.50");
        assert_eq!(clean_amount("-£3.00", false), "-3.00");
        assert_eq!(clean_amount("1.234,56 €", true), "1234.56");
        assert_eq!(clean_amount("-7,5", true), "-7.5");
        assert_eq!(clean_amount("CAD 9.99", false), "9.99");
    }

    #[test]
    fn picks_one_date_order_for_the_whole_file() {
        let f = |v: &[&str], day_first| {
            pick_date_format(v, &["%m/%d/%Y".into(), "%d/%m/%Y".into()], day_first)
        };
        // 25/09 only reads day-first, so 03/09 is 3 September too.
        assert_eq!(
            f(&["01/09/2999", "03/09/2999", "25/09/2999"], false).as_deref(),
            Some("%d/%m/%Y")
        );
        assert_eq!(
            f(&["09/01/2999", "09/03/2999", "09/25/2999"], false).as_deref(),
            Some("%m/%d/%Y")
        );
        // Ambiguous, but only day-first keeps them in order.
        assert_eq!(
            f(&["11/01/2999", "02/02/2999", "05/03/2999"], false).as_deref(),
            Some("%d/%m/%Y")
        );
        // Truly ambiguous: the preference decides.
        assert_eq!(f(&["01/02/2999"], true).as_deref(), Some("%d/%m/%Y"));
        assert_eq!(f(&["01/02/2999"], false).as_deref(), Some("%m/%d/%Y"));
        // Two-digit years aren't read as the year 24.
        assert_eq!(
            pick_date_format(
                &["09/05/24"],
                &["%m/%d/%Y".into(), "%m/%d/%y".into()],
                false
            )
            .as_deref(),
            Some("%m/%d/%y")
        );
    }

    #[test]
    fn strips_card_quotes() {
        assert_eq!(strip_card_quote("'5550000000001234'"), "5550000000001234");
        assert_eq!(strip_card_quote("Amazon"), "Amazon");
    }
}
