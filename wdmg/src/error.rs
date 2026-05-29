//! Typed error hierarchy for the `wdmg` library.
//!
//! Library code returns these typed errors; the binary layer adds human context
//! via `anyhow`. Each error carries enough information to be actionable without
//! re-reading the source data.

use std::path::PathBuf;

use thiserror::Error;

/// Errors raised while loading or validating configuration TOML files.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// A required config file could not be read from disk.
    #[error("failed to read config file {path}: {source}")]
    Read {
        /// Path that failed to read.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// A config file failed to deserialize from TOML.
    #[error("failed to parse TOML config {path}: {source}")]
    Toml {
        /// Path that failed to parse.
        path: PathBuf,
        /// Underlying TOML error.
        #[source]
        source: toml::de::Error,
    },

    /// A regex declared in config did not compile.
    #[error("invalid regex in {context} (pattern {pattern:?}): {source}")]
    Regex {
        /// Where the pattern came from (profile id, category, etc.).
        context: String,
        /// The offending pattern.
        pattern: String,
        /// Underlying regex compile error.
        #[source]
        source: regex::Error,
    },

    /// A config invariant was violated (e.g. an unknown sign mode).
    #[error("invalid configuration in {context}: {message}")]
    Invalid {
        /// Where the problem is (profile id, etc.).
        context: String,
        /// Human-readable explanation.
        message: String,
    },
}

/// Errors raised while detecting which profile applies to a file, or while
/// reading rows out of a file.
#[derive(Debug, Error)]
pub enum IngestError {
    /// The file could not be opened/read.
    #[error("failed to read input file {path}: {source}")]
    Read {
        /// Offending path.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// The file's header was empty or unreadable.
    #[error("input file {path} has no header row")]
    EmptyHeader {
        /// Offending path.
        path: PathBuf,
    },

    /// No configured profile matched the file. Lists, per profile, the first
    /// reason it was rejected so the user can tell which rule failed.
    #[error("no profile matched {path}; tried: {reasons}")]
    NoProfileMatch {
        /// Offending path.
        path: PathBuf,
        /// Semicolon-joined per-profile rejection reasons.
        reasons: String,
    },

    /// A row could not be turned into a [`crate::model::Transaction`].
    #[error(transparent)]
    Parse(#[from] ParseError),

    /// The CSV reader itself failed (malformed quoting, etc.).
    #[error("CSV error in {path}: {source}")]
    Csv {
        /// Offending path.
        path: PathBuf,
        /// Underlying csv error.
        #[source]
        source: csv::Error,
    },
}

/// Errors raised converting a single raw CSV record into a typed value.
#[derive(Debug, Error)]
pub enum ParseError {
    /// A column declared by the profile was absent from the header.
    #[error("profile {profile} expects column {column:?} but it is missing from the header")]
    MissingColumn {
        /// Profile id.
        profile: String,
        /// Column name that was expected.
        column: String,
    },

    /// A required cell was empty.
    #[error("row {row} (profile {profile}): column {column:?} is empty")]
    EmptyField {
        /// Profile id.
        profile: String,
        /// 1-based data-row index.
        row: usize,
        /// Column name.
        column: String,
    },

    /// A date cell did not parse under any configured format.
    #[error(
        "row {row} (profile {profile}): could not parse date {value:?} with formats {formats:?}"
    )]
    Date {
        /// Profile id.
        profile: String,
        /// 1-based data-row index.
        row: usize,
        /// Raw value.
        value: String,
        /// Formats that were attempted.
        formats: Vec<String>,
    },

    /// An amount cell could not be parsed into an exact decimal.
    #[error("row {row} (profile {profile}): could not parse amount {value:?}: {source}")]
    Amount {
        /// Profile id.
        profile: String,
        /// 1-based data-row index.
        row: usize,
        /// Raw value.
        value: String,
        /// Underlying decimal error.
        #[source]
        source: rust_decimal::Error,
    },
}

/// Errors raised during deduplication.
#[derive(Debug, Error)]
pub enum DedupError {
    /// A transaction lacked the fields required to build a stable key.
    #[error("cannot build dedup key for transaction from {source_file}: {message}")]
    KeyConstruction {
        /// Source file the transaction came from.
        source_file: String,
        /// What was missing.
        message: String,
    },
}

/// Errors raised during analytics and emission.
#[derive(Debug, Error)]
pub enum AnalyticsError {
    /// Inputs mixed currencies; the binary refuses to sum across them.
    #[error("inputs mix currencies ({first} and {other}); refusing to sum across currencies")]
    MixedCurrency {
        /// The first currency observed.
        first: String,
        /// A conflicting currency observed later.
        other: String,
    },

    /// Serializing an output document to JSON failed.
    #[error("failed to serialize {what} to JSON: {source}")]
    Serialize {
        /// Which document failed (e.g. `analysis.json`).
        what: String,
        /// Underlying serde error.
        #[source]
        source: serde_json::Error,
    },

    /// Writing an output document to disk failed.
    #[error("failed to write {path}: {source}")]
    Write {
        /// Path that failed to write.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// Rendering the HTML report template failed.
    #[error("failed to render {what}: {source}")]
    Template {
        /// Which template failed (e.g. `report.html`).
        what: String,
        /// Underlying askama error.
        #[source]
        source: askama::Error,
    },
}

/// Top-level library error wrapping every phase-specific error.
#[derive(Debug, Error)]
pub enum WdmgError {
    /// Configuration failure.
    #[error(transparent)]
    Config(#[from] ConfigError),

    /// Ingest/detection/parse failure.
    #[error(transparent)]
    Ingest(#[from] IngestError),

    /// Parse failure surfaced directly.
    #[error(transparent)]
    Parse(#[from] ParseError),

    /// Dedup failure.
    #[error(transparent)]
    Dedup(#[from] DedupError),

    /// Analytics/emit failure.
    #[error(transparent)]
    Analytics(#[from] AnalyticsError),
}

/// Convenience alias for library results.
pub type Result<T, E = WdmgError> = std::result::Result<T, E>;
