//! Ingest: detect a file's profile and parse it into [`Transaction`]s.
//!
//! Public entry points:
//! - [`load_file`] — detect + parse one file.
//! - [`load_dir`] — load every ingestible file in a directory, skipping files
//!   that match no profile and are not CSVs.

pub mod detect;
pub mod reader;

use std::path::Path;

use crate::config::Config;
use crate::error::IngestError;
use crate::model::Transaction;

pub use detect::detect;
pub use reader::read_file;

/// Detect the profile for `path` and parse it into transactions.
pub fn load_file(config: &Config, path: &Path) -> Result<Vec<Transaction>, IngestError> {
    let profile = detect(config, path)?;
    read_file(profile, path)
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
        let name = detect::basename(&path);
        match detect(config, &path) {
            Ok(profile) => match read_file(profile, &path) {
                Ok(mut rows) => {
                    let n = rows.len();
                    report.loaded.push((name, profile.profile.id.clone(), n));
                    txns.append(&mut rows);
                }
                Err(e) => report.errors.push((name, e.to_string())),
            },
            Err(e) => {
                if is_csv(&path) {
                    report.errors.push((name, e.to_string()));
                } else {
                    report.skipped.push(name);
                }
            }
        }
    }

    Ok((txns, report))
}
