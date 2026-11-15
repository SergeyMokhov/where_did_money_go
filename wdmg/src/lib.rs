//! `wdmg` — where-did-money-go: a personal finance statement analyzer.
//!
//! Phase 1 (this crate) provides the foundation: the domain [`model`], a typed
//! [`error`] hierarchy, [`config`] loading/validation, CSV [`ingest`], merchant
//! [`normalize`]ation, and cross-file [`dedup`]. Later phases (categorize,
//! identity, reconcile, analytics, emit, report) build on these public APIs.
//!
//! Design notes for later phases:
//! - Money is [`model::Money`] (a `Decimal` newtype), never `f64`; it serializes
//!   as a JSON/TOML string.
//! - [`config::Config`] holds compiled, validated profiles, categories, people,
//!   transfer rules, and account matchers — no raw regex re-compilation needed
//!   downstream.
//! - A [`model::Transaction`]'s `dedup_key` is set by [`dedup::dedup`]; its
//!   `owner_person`/`account_id`/category fields are `None` until later phases
//!   populate them.

#![warn(missing_docs)]

pub mod analytics;
pub mod categorize;
pub mod config;
pub mod dedup;
pub mod emit;
pub mod error;
pub mod identity;
pub mod ingest;
pub mod model;
pub mod normalize;
pub mod pipeline;
pub mod reconcile;
pub mod report;

pub use categorize::{Assignment, Categorizer, EffectiveKind};
pub use config::Config;
pub use dedup::{dedup, DedupReport};
pub use error::{Result, WdmgError};
pub use identity::Roster;
pub use ingest::{load_dir, load_file, load_files, LoadReport};
pub use model::{Currency, Money, Transaction, TxnType};
pub use normalize::normalize_merchant;
pub use pipeline::{analyze, analyze_files, AnalyzeOutcome};
pub use report::{write_report, Insights};
