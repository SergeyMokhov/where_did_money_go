//! Ingest: detect a file's profile and parse it into [`Transaction`]s.
//!
//! Public entry points:
//! - [`load_file`] — detect + parse one file.
//! - [`load_dir`] — load every ingestible file in a directory, skipping files
//!   that match no profile and are not CSVs.
//! - [`load_files`] — the same over in-memory `(name, bytes)` pairs (the browser
//!   / WebAssembly path). `load_dir` is a thin fs wrapper over the same per-file
//!   logic, so both classify and report files identically.

pub mod detect;
pub mod infer;
pub mod reader;

use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::error::IngestError;
use crate::model::Transaction;

pub use detect::{detect, detect_text};
pub use reader::{read_file, read_text};

/// Detect the profile for `path` and parse it into transactions.
pub fn load_file(config: &Config, path: &Path) -> Result<Vec<Transaction>, IngestError> {
    let bytes = std::fs::read(path).map_err(|source| IngestError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    load_bytes(config, path, &bytes)
}

/// Detect the profile for in-memory file contents and parse them. `label` is the
/// file's name/path, used for filename-based detection and error messages.
pub fn load_bytes(
    config: &Config,
    label: &Path,
    bytes: &[u8],
) -> Result<Vec<Transaction>, IngestError> {
    let text = detect::decode(label, bytes)?;
    let profile = detect_text(config, label, text)?;
    read_text(&profile, label, text)
}

/// Outcome of loading a directory: parsed transactions plus a per-file report so
/// callers can surface what was loaded vs. skipped.
#[derive(Debug, Default)]
pub struct LoadReport {
    /// Files successfully parsed, with the profile id and row count.
    pub loaded: Vec<(String, String, usize)>,
    /// Files skipped because no profile matched and they were not CSVs.
    pub skipped: Vec<String>,
    /// Files that errored during parsing, with the error rendered as a string.
    pub errors: Vec<(String, String)>,
}

/// Whether a path looks like a CSV we should attempt to ingest.
fn is_csv(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("csv")
    )
}

/// Classify and parse one file into `txns`/`report` — the single per-file rule
/// shared by [`load_dir`] and [`load_files`].
///
/// A file that cannot be read/decoded or matches no profile is skipped if it is
/// not a `.csv` and recorded as an error if it is; a detected file that fails to
/// parse is always an error.
fn ingest_one(
    config: &Config,
    label: &Path,
    bytes: Result<&[u8], IngestError>,
    txns: &mut Vec<Transaction>,
    report: &mut LoadReport,
) {
    let name = detect::basename(label);
    let detected = bytes.and_then(|b| {
        let text = detect::decode(label, b)?;
        let profile = detect_text(config, label, text)?;
        let rows = read_text(&profile, label, text);
        Ok((profile, rows))
    });
    match detected {
        Ok((profile, Ok(mut rows))) => {
            report
                .loaded
                .push((name, profile.profile.id.clone(), rows.len()));
            txns.append(&mut rows);
        }
        Ok((_, Err(e))) => report.errors.push((name, e.to_string())),
        Err(e) => {
            if is_csv(label) {
                report.errors.push((name, e.to_string()));
            } else {
                report.skipped.push(name);
            }
        }
    }
}

/// Load every ingestible file directly under `dir`.
///
/// Files matching no profile that are not `.csv` are skipped silently (reported
/// in [`LoadReport::skipped`]). A `.csv` that matches no profile, or that fails
/// to parse, is recorded in [`LoadReport::errors`] but does not abort the run —
/// callers decide whether partial ingest is acceptable.
pub fn load_dir(
    config: &Config,
    dir: &Path,
) -> Result<(Vec<Transaction>, LoadReport), IngestError> {
    let mut txns = Vec::new();
    let mut report = LoadReport::default();

    let entries = std::fs::read_dir(dir).map_err(|source| IngestError::Read {
        path: dir.to_path_buf(),
        source,
    })?;

    // Sort for deterministic ordering across runs (dedup intra-file ordinals and
    // tests both rely on stable iteration).
    let mut paths: Vec<_> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file())
        .collect();
    paths.sort();

    for path in paths {
        match std::fs::read(&path) {
            Ok(bytes) => ingest_one(config, &path, Ok(&bytes), &mut txns, &mut report),
            Err(source) => {
                let err = IngestError::Read {
                    path: path.clone(),
                    source,
                };
                ingest_one(config, &path, Err(err), &mut txns, &mut report);
            }
        }
    }

    Ok((txns, report))
}

/// Load in-memory files given as `(file name, contents)` pairs, with exactly the
/// [`load_dir`] semantics (detection by name + contents, same [`LoadReport`]
/// classification). Files are processed sorted by name (stable for duplicate
/// names) so the result does not depend on the caller's order.
#[must_use]
pub fn load_files(config: &Config, files: &[(String, Vec<u8>)]) -> (Vec<Transaction>, LoadReport) {
    let mut txns = Vec::new();
    let mut report = LoadReport::default();

    let mut order: Vec<&(String, Vec<u8>)> = files.iter().collect();
    order.sort_by(|a, b| a.0.cmp(&b.0));

    for (name, bytes) in order {
        let label = PathBuf::from(name);
        ingest_one(config, &label, Ok(bytes), &mut txns, &mut report);
    }

    (txns, report)
}
