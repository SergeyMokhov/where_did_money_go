//! The `analyze` pipeline, wired end-to-end and reusable by the CLI and tests.
//!
//! Order: ingest -> dedup -> identity -> categorize -> reconcile -> analytics ->
//! emit. The currency guard runs before any sum. Because [`Categorized`] borrows
//! both a [`Transaction`] and its [`Assignment`], this function owns both vectors
//! for the duration and returns the assembled [`Analysis`]/[`ReviewQueue`] (which
//! own their data) so the borrows do not escape.
//!
//! Determinism: ingest sorts files, dedup keys are stable, and every analytics
//! and reconcile collection is sorted by a stable key, so running twice on the
//! same inputs produces byte-identical output (verified by an integration test).

use std::path::Path;

use crate::analytics::{self, Categorized};
use crate::categorize::Categorizer;
use crate::config::Config;
use crate::dedup;
use crate::emit::{self, Analysis, ReviewQueue};
use crate::error::WdmgError;
use crate::ingest;
use crate::{identity, reconcile};

/// What the [`analyze`] pipeline produced: the two output documents plus the
/// per-file load report so the CLI can show what was ingested.
pub struct AnalyzeOutcome {
    /// The `analysis.json` document.
    pub analysis: Analysis,
    /// The `review_queue.json` document.
    pub review_queue: ReviewQueue,
    /// Per-file ingest report.
    pub load_report: ingest::LoadReport,
}

/// Run the full analyze pipeline over `input_dir` using `config`.
///
/// Does not write any files; the caller (or [`run_to_dir`]) decides where output
/// goes. Returns a typed [`WdmgError`] on ingest/parse/mixed-currency failures.
pub fn analyze(config: &Config, input_dir: &Path) -> Result<AnalyzeOutcome, WdmgError> {
    // 1. Ingest.
    let (txns, load_report) = ingest::load_dir(config, input_dir)?;

    // 2. Dedup (sets stable dedup_key, returns kept rows + report).
    let (mut txns, dedup_report) = dedup::dedup(txns);

    // 3. Currency guard — fail closed before any sum.
    let currency = analytics::single_currency(&txns)?;

    // 4. Identity resolution (mutates owner_person/account_id in place).
    let roster = identity::resolve(config, &mut txns);

    // 5. Categorize into a parallel Vec aligned by index.
    let categorizer = Categorizer::new(&config.categories);
    let assignments = categorizer.assign_all(&txns);

    // Pair each txn with its assignment for the analysis phases.
    let items: Vec<Categorized<'_>> = txns
        .iter()
        .zip(assignments.iter())
        .map(|(txn, assignment)| Categorized { txn, assignment })
        .collect();

    // 6. Reconcile transfers.
    let reconciliation = reconcile::reconcile(&items, config, &roster);

    // 7. Analytics (transfers excluded inside).
    let analytics = analytics::compute(&items, &currency, &config.settings);

    // 8. Emit documents (owns its data; borrows end here).
    let review_queue = emit::build_review_queue(&items, &reconciliation);
    let analysis = emit::build_analysis(
        analytics,
        reconciliation,
        dedup_report,
        config.events.clone(),
    );

    Ok(AnalyzeOutcome {
        analysis,
        review_queue,
        load_report,
    })
}

/// Run [`analyze`] and write `analysis.json` + `review_queue.json` into `out_dir`.
pub fn run_to_dir(
    config: &Config,
    input_dir: &Path,
    out_dir: &Path,
) -> Result<AnalyzeOutcome, WdmgError> {
    let outcome = analyze(config, input_dir)?;
    emit::write_outputs(out_dir, &outcome.analysis, &outcome.review_queue)?;
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Write a minimal Mastercard CSV to a temp dir and analyze it.
    fn pc_csv() -> &'static str {
        "\"Description\",\"Type\",\"Card Holder Name\",\"Date\",\"Time\",\"Amount\"\n\
         \"SHELL\",\"PURCHASE\",\"FRY\",\"01/05/2026\",\"10:00 AM\",\"-60.00\"\n\
         \"NETFLIX\",\"PURCHASE\",\"FRY\",\"01/06/2026\",\"10:00 AM\",\"-16.99\"\n\
         \"PAYMENT - THANK YOU\",\"PAYMENT\",\"FRY\",\"01/10/2026\",\"10:00 AM\",\"500.00\"\n"
    }

    fn write_temp(dir: &Path, name: &str, contents: &str) {
        let mut f = std::fs::File::create(dir.join(name)).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
    }

    #[test]
    fn analyze_is_deterministic_and_idempotent() {
        let cfg = Config::from_defaults().unwrap();
        let tmp = std::env::temp_dir().join(format!("wdmg-pipe-{}", std::process::id()));
        let input = tmp.join("in");
        std::fs::create_dir_all(&input).unwrap();
        write_temp(&input, "7001 xxxx xxxx 7003.CSV", pc_csv());

        let out1 = analyze(&cfg, &input).unwrap();
        let out2 = analyze(&cfg, &input).unwrap();

        let j1 = serde_json::to_string_pretty(&out1.analysis).unwrap();
        let j2 = serde_json::to_string_pretty(&out2.analysis).unwrap();
        assert_eq!(j1, j2, "analysis.json must be byte-identical across runs");

        // SHELL ($60) is Gas, NETFLIX is a digital subscription; payment is a transfer.
        assert_eq!(
            out1.analysis.analytics.flows.total_outflow.amount(),
            rust_decimal_macros::dec!(76.99)
        );

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn run_to_dir_writes_both_files() {
        let cfg = Config::from_defaults().unwrap();
        let tmp = std::env::temp_dir().join(format!("wdmg-pipe-w-{}", std::process::id()));
        let input = tmp.join("in");
        let out = tmp.join("out");
        std::fs::create_dir_all(&input).unwrap();
        write_temp(&input, "7001 xxxx xxxx 7003.CSV", pc_csv());

        run_to_dir(&cfg, &input, &out).unwrap();
        assert!(out.join("analysis.json").exists());
        assert!(out.join("review_queue.json").exists());

        std::fs::remove_dir_all(&tmp).ok();
    }
}
