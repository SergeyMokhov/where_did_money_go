//! CSV reading: parse a file with a chosen profile into [`Transaction`]s.
//!
//! Handles real-world quirks: a UTF-8 BOM, blank rows after the header, a header
//! name with a leading space (`" Transaction Amount"`), quoted fields, and
//! leading-apostrophe card numbers (`'5510...'`). Dates are parsed by trying each
//! configured format in order; amounts are parsed exactly via [`Money`].

use std::collections::HashMap;
use std::path::Path;

use jiff::civil::{Date, Time};

use crate::config::{CompiledProfile, Sign};
use crate::error::{IngestError, ParseError};
use crate::model::{Currency, Money, Transaction, TxnType};
use crate::normalize::normalize_merchant;

use super::detect::{basename, strip_bom};

/// Parse `path` into transactions using `profile`. Merchant normalization is
/// applied here; `dedup_key`/owner/account are left for later phases.
pub fn read_file(profile: &CompiledProfile, path: &Path) -> Result<Vec<Transaction>, IngestError> {
    let raw = std::fs::read_to_string(path).map_err(|source| IngestError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let text = strip_bom(&raw);
    let base = basename(path);
    let delimiter = profile.profile.layout.delimiter.unwrap_or(',') as u8;

    // Split header from body so we can drop the configured count of blank rows
    // BEFORE handing the body to the CSV reader (csv treats blank lines as records
    // when flexible, which would otherwise desync our column lookups).
    let mut lines = text.lines();
    let header_line = lines.next().unwrap_or("");
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
        s.push_str(header_line);
        s.push('\n');
        for l in body_lines {
            s.push_str(l);
            s.push('\n');
        }
        s
    };

    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .flexible(true)
        .has_headers(true)
        .from_reader(body.as_bytes());

    // Map header name -> column index. Header names are used verbatim (including
    // any leading space, e.g. " Transaction Amount").
    let headers = rdr.headers().map_err(|source| IngestError::Csv {
        path: path.to_path_buf(),
        source,
    })?;
    let index: HashMap<String, usize> = headers
        .iter()
        .enumerate()
        .map(|(i, h)| (h.to_string(), i))
        .collect();

    let p = &profile.profile;
    // Validate every declared column exists up front ("parse, don't validate":
    // fail at the boundary with a clear typed error, never silently).
    let mut required = vec![
        p.date.column.as_str(),
        p.amount.column.as_str(),
        p.merchant.column.as_str(),
        p.type_spec.column.as_str(),
    ];
    if let Some(m) = &p.memo {
        required.push(m.column.as_str());
    }
    if let Some(pe) = &p.person {
        required.push(pe.column.as_str());
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

    let date_formats = p.date.all_formats();
    let currency = Currency::new(p.account.currency.clone());

    let mut out = Vec::new();
    for (row_idx, record) in rdr.records().enumerate() {
        let record = record.map_err(|source| IngestError::Csv {
            path: path.to_path_buf(),
            source,
        })?;
        // Skip stray fully-empty records.
        if record.iter().all(|c| c.trim().is_empty()) {
            continue;
        }
        let row = row_idx + 1;
        let txn = parse_record(
            profile,
            &index,
            &record,
            &date_formats,
            &currency,
            &base,
            row,
        )?;
        out.push(txn);
    }
    Ok(out)
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
    let raw_description = strip_card_quote(cell(index, record, &p.merchant.column)).to_string();
    let normalized_merchant = normalize_merchant(&raw_description, profile);

    // --- type map ---
    let raw_type = cell(index, record, &p.type_spec.column).trim();
    let txn_type = p
        .type_spec
        .map
        .get(raw_type)
        .map(|m| TxnType::from_mapped(m))
        .unwrap_or(TxnType::Unknown);

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

/// Parse the date and any optional companion `Time` column.
fn parse_date_time(
    p: &crate::config::Profile,
    index: &HashMap<String, usize>,
    record: &csv::StringRecord,
    date_raw: &str,
    formats: &[String],
    row: usize,
) -> Result<(Date, Option<Time>), ParseError> {
    let mut date = None;
    for fmt in formats {
        if let Ok(d) = Date::strptime(fmt.as_bytes(), date_raw.as_bytes()) {
            date = Some(d);
            break;
        }
    }
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
        v.parse::<Money>().map_err(|source| ParseError::Amount {
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
            parse(raw)
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
            if !debit.is_empty() {
                Ok(Money::new(-parse(debit)?.amount()))
            } else if !credit.is_empty() {
                parse(credit)
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
    fn strips_card_quotes() {
        assert_eq!(strip_card_quote("'5550000000001234'"), "5550000000001234");
        assert_eq!(strip_card_quote("Amazon"), "Amazon");
    }
}
