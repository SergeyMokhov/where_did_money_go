//! Profile detection: choose the first profile whose match rules all succeed.

use std::path::Path;

use crate::config::{CompiledProfile, Config};
use crate::error::IngestError;

/// UTF-8 BOM bytes.
const BOM: &str = "\u{feff}";

/// Strip a leading UTF-8 BOM from a string slice.
#[must_use]
pub fn strip_bom(s: &str) -> &str {
    s.strip_prefix(BOM).unwrap_or(s)
}

/// Read the header line and first `probe_rows` data lines from a file as text.
///
/// Returns `(header_line, data_lines)`. The header has any BOM stripped. Blank
/// lines are skipped when collecting probe rows so the detector sees real data
/// even for sources with blank rows after the header.
fn read_head(path: &Path, probe_rows: usize) -> Result<(String, Vec<String>), IngestError> {
    let content = std::fs::read_to_string(path).map_err(|source| IngestError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let content = strip_bom(&content);
    let mut lines = content.lines();
    let header = lines
        .next()
        .ok_or_else(|| IngestError::EmptyHeader {
            path: path.to_path_buf(),
        })?
        .to_string();
    let data: Vec<String> = lines
        .filter(|l| !l.trim().is_empty())
        .take(probe_rows)
        .map(str::to_string)
        .collect();
    Ok((header, data))
}

/// Unquote the first header cell: take everything before the first delimiter,
/// then strip surrounding double quotes.
fn first_cell(header: &str, delimiter: char) -> &str {
    let cell = header.split(delimiter).next().unwrap_or(header);
    let cell = cell.trim();
    cell.strip_prefix('"')
        .and_then(|c| c.strip_suffix('"'))
        .unwrap_or(cell)
}

/// Why a single profile was rejected (for actionable `NoProfileMatch` errors).
fn rejection_reason(
    profile: &CompiledProfile,
    header: &str,
    data: &[String],
    basename: &str,
) -> Option<String> {
    let delimiter = profile.profile.layout.delimiter.unwrap_or(',');
    let cell = first_cell(strip_bom(header), delimiter);

    if let Some(re) = &profile.header_first_cell {
        if !re.is_match(cell) {
            return Some(format!(
                "{}: header_first_cell {:?} != {:?}",
                profile.profile.id,
                profile
                    .profile
                    .match_rules
                    .header_first_cell
                    .as_deref()
                    .unwrap_or(""),
                cell
            ));
        }
    }
    for needle in &profile.profile.match_rules.header_contains {
        if !header.contains(needle.as_str()) {
            return Some(format!(
                "{}: header missing substring {needle:?}",
                profile.profile.id
            ));
        }
    }
    if let Some(re) = &profile.content_probe {
        if !data.iter().any(|row| re.is_match(row)) {
            return Some(format!(
                "{}: content_probe did not match first data rows",
                profile.profile.id
            ));
        }
    }
    if let Some(re) = &profile.filename {
        if !re.is_match(basename) {
            return Some(format!(
                "{}: filename regex did not match {basename:?}",
                profile.profile.id
            ));
        }
    }
    None
}

/// The basename of a path as a lossy string.
#[must_use]
pub fn basename(path: &Path) -> String {
    path.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Select the first profile that matches `path`, or a typed error listing why
/// each profile was rejected.
pub fn detect<'c>(config: &'c Config, path: &Path) -> Result<&'c CompiledProfile, IngestError> {
    let (header, data) = read_head(path, 5)?;
    let base = basename(path);

    let mut reasons = Vec::new();
    for profile in &config.profiles {
        match rejection_reason(profile, &header, &data, &base) {
            None => return Ok(profile),
            Some(r) => reasons.push(r),
        }
    }
    Err(IngestError::NoProfileMatch {
        path: path.to_path_buf(),
        reasons: reasons.join("; "),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_cell_unquotes() {
        assert_eq!(first_cell(r#""Description","Type""#, ','), "Description");
        assert_eq!(first_cell("Date,Transaction", ','), "Date");
    }

    #[test]
    fn strip_bom_works() {
        assert_eq!(strip_bom("\u{feff}First Bank Card"), "First Bank Card");
        assert_eq!(strip_bom("Date"), "Date");
    }
}
