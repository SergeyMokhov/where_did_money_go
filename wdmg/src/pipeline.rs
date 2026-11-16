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
    analyze_txns(config, txns, load_report)
}

/// Run the full analyze pipeline over in-memory `(file name, contents)` pairs —
/// the browser/WebAssembly entry point. Identical to [`analyze`] except for where
/// the bytes come from (see [`ingest::load_files`]); touches no filesystem.
pub fn analyze_files(
    config: &Config,
    files: &[(String, Vec<u8>)],
) -> Result<AnalyzeOutcome, WdmgError> {
    let (txns, load_report) = ingest::load_files(config, files);
    analyze_txns(config, txns, load_report)
}

/// Stages 2..8, shared by [`analyze`] and [`analyze_files`] so they cannot drift.
fn analyze_txns(
    config: &Config,
    txns: Vec<crate::model::Transaction>,
    load_report: ingest::LoadReport,
) -> Result<AnalyzeOutcome, WdmgError> {
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
    fn analyze_files_matches_analyze_dir() {
        let cfg = Config::from_defaults().unwrap();
        let tmp = std::env::temp_dir().join(format!("wdmg-pipe-mem-{}", std::process::id()));
        let input = tmp.join("in");
        std::fs::create_dir_all(&input).unwrap();
        write_temp(&input, "7001 xxxx xxxx 7003.CSV", pc_csv());
        write_temp(&input, "notes.txt", "not a statement");

        let from_dir = analyze(&cfg, &input).unwrap();
        // Supply the files in reverse order: load_files must sort by name.
        let files = vec![
            ("notes.txt".to_string(), b"not a statement".to_vec()),
            (
                "7001 xxxx xxxx 7003.CSV".to_string(),
                pc_csv().as_bytes().to_vec(),
            ),
        ];
        let from_mem = analyze_files(&cfg, &files).unwrap();

        assert_eq!(
            serde_json::to_string(&from_dir.analysis).unwrap(),
            serde_json::to_string(&from_mem.analysis).unwrap()
        );
        assert_eq!(from_dir.load_report.loaded, from_mem.load_report.loaded);
        assert_eq!(from_dir.load_report.skipped, from_mem.load_report.skipped);
        assert_eq!(from_mem.load_report.skipped, vec!["notes.txt".to_string()]);

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

/// How a transfer row's other side was found (see [`rule_lab`]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct LabTransfer {
    /// `paired` (its twin is in another statement), `rule` (a transfer rule
    /// named the other side) or `unmatched`.
    pub how: &'static str,
    /// The other side's account/person, when known.
    pub other: Option<String>,
    /// For `rule`: the rule's index among `[[transfer_rule]]` in file order.
    pub rule: Option<usize>,
}

/// One transaction as the rule editors see it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct LabRow {
    /// ISO date.
    pub date: String,
    /// Signed amount.
    pub amount: String,
    /// Account node (account, else owner, else `unassigned`).
    pub account: String,
    /// Normalized merchant / description.
    pub merchant: String,
    /// Raw description as exported.
    pub raw: String,
    /// The statement file it came from.
    pub file: String,
    /// `spend` / `income` / `transfer` / `unknown`, after categorization.
    pub kind: &'static str,
    /// Category label, if any.
    pub category: Option<String>,
    /// Subcategory label, if any.
    pub subcategory: Option<String>,
    /// The category rule that decided it, if any.
    pub hit: Option<crate::categorize::RuleHit>,
    /// For transfers: how the other side was found.
    pub transfer: Option<LabTransfer>,
}

fn kind_name(k: crate::categorize::EffectiveKind) -> &'static str {
    use crate::categorize::EffectiveKind as K;
    match k {
        K::Spend => "spend",
        K::Income => "income",
        K::Transfer => "transfer",
        K::Unknown => "unknown",
    }
}

/// Ingest → dedup → identity → categorize → reconcile, then report for every
/// row which rule categorized it and how its transfer was resolved. Cheap
/// enough to re-run on every edit, so a rule editor can preview exactly what a
/// change does (the analytics stage is skipped).
///
/// # Errors
/// As [`analyze_files`] for the currency guard.
pub fn rule_lab(config: &Config, files: &[(String, Vec<u8>)]) -> Result<Vec<LabRow>, WdmgError> {
    use std::collections::HashMap;
    let (txns, _) = ingest::load_files(config, files);
    let (mut txns, _) = dedup::dedup(txns);
    analytics::single_currency(&txns)?;
    let roster = identity::resolve(config, &mut txns);
    let categorizer = Categorizer::new(&config.categories);
    let (assignments, hits): (Vec<_>, Vec<_>) =
        txns.iter().map(|t| categorizer.assign_explained(t)).unzip();
    let items: Vec<Categorized<'_>> = txns
        .iter()
        .zip(assignments.iter())
        .map(|(txn, assignment)| Categorized { txn, assignment })
        .collect();
    let recon = reconcile::reconcile(&items, config, &roster);

    // Outcomes keyed by (node, date, signed amount); several identical legs
    // simply take one outcome each.
    let key = |node: &str, date: &str, amt: rust_decimal::Decimal| {
        format!("{node}|{date}|{}", amt.normalize())
    };
    let mut outcomes: HashMap<String, Vec<LabTransfer>> = HashMap::new();
    let mut put = |k: String, t: LabTransfer| outcomes.entry(k).or_default().push(t);
    for p in &recon.matched_pairs {
        let a = p.amount.amount().abs();
        put(
            key(&p.from_account, &p.out_date, -a),
            LabTransfer {
                how: "paired",
                other: Some(p.to_account.clone()),
                rule: None,
            },
        );
        put(
            key(&p.to_account, &p.in_date, a),
            LabTransfer {
                how: "paired",
                other: Some(p.from_account.clone()),
                rule: None,
            },
        );
    }
    for e in &recon.external {
        let a = e.amount.amount().abs();
        let signed = if e.incoming { a } else { -a };
        put(
            key(&e.from_account, &e.date, signed),
            LabTransfer {
                how: "rule",
                other: Some(e.to.clone()),
                rule: None,
            },
        );
    }
    for u in &recon.unresolved {
        put(
            key(&u.account, &u.date, u.amount.amount()),
            LabTransfer {
                how: "unmatched",
                other: None,
                rule: None,
            },
        );
    }
    // Which rule (file index) a description falls to — the same first-match
    // reconcile uses, over the priority-sorted compiled rules.
    let rule_index = |raw: &str, merchant: &str| -> Option<usize> {
        let hit = config
            .transfer_rules
            .iter()
            .find(|r| r.pattern.is_match(raw) || r.pattern.is_match(merchant))?;
        config
            .people
            .transfer_rules
            .iter()
            .position(|r| *r == hit.rule)
    };

    Ok(items
        .iter()
        .zip(hits)
        .map(|(c, hit)| {
            let t = c.txn;
            let node = t
                .account_id
                .clone()
                .or_else(|| t.owner_person.clone())
                .unwrap_or_else(|| "unassigned".into());
            let date = t.date.to_string();
            let kind = kind_name(c.assignment.kind);
            let transfer = (kind == "transfer").then(|| {
                let mut o = outcomes
                    .get_mut(&key(&node, &date, t.amount.amount()))
                    .and_then(Vec::pop)
                    .unwrap_or(LabTransfer {
                        how: "unmatched",
                        other: None,
                        rule: None,
                    });
                if o.how == "rule" {
                    o.rule = rule_index(&t.raw_description, &t.normalized_merchant);
                }
                o
            });
            LabRow {
                date,
                amount: t.amount.amount().to_string(),
                account: node,
                merchant: t.normalized_merchant.clone(),
                raw: t.raw_description.clone(),
                file: t.source_file.clone(),
                kind,
                category: c.assignment.category.clone(),
                subcategory: c.assignment.subcategory.clone(),
                hit,
                transfer,
            }
        })
        .collect())
}
