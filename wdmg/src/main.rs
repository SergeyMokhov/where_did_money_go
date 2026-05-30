//! `wdmg` command-line interface.
//!
//! Subcommands:
//! - `init` — write default config files into a directory.
//! - `analyze` — parse → dedup → categorize → reconcile → analytics; writes
//!   `analysis.json` and `review_queue.json`.
//! - `report` — render a self-contained `report.html` from `analysis.json`
//!   (+ optional `insights.json`).
//! - `add-category` / `add-rule` / `add-person` / `assign-account` /
//!   `assign-transfer` — lossless TOML edits of the working-dir config files.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use wdmg::config::{
    defaults, Account, CategoriesDoc, Category, CategoryKind, PeopleDoc, Person, Subcategory,
    TransferRule,
};
use wdmg::emit::Analysis;
use wdmg::report::{write_report, Insights};

/// where-did-money-go: analyze personal finance statements.
#[derive(Debug, Parser)]
#[command(name = "wdmg", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Write the default config files (profiles/categories/people) into a directory.
    Init {
        /// Target directory (created if missing). Existing files are NOT overwritten.
        dir: PathBuf,
    },
    /// Analyze statements end-to-end, writing analysis.json + review_queue.json.
    Analyze {
        /// Directory of statement exports to ingest.
        #[arg(long)]
        input: PathBuf,
        /// Directory holding config (profiles/categories/people). Embedded
        /// defaults are used for any file that is absent.
        #[arg(long)]
        config: PathBuf,
        /// Output directory for analysis.json and review_queue.json (created).
        #[arg(long)]
        out: PathBuf,
    },
    /// Render a self-contained interactive report.html.
    Report {
        /// Path to analysis.json (from `analyze`).
        #[arg(long)]
        analysis: PathBuf,
        /// Optional path to insights.json (LLM-authored narrative).
        #[arg(long)]
        insights: Option<PathBuf>,
        /// Output path for report.html.
        #[arg(long)]
        out: PathBuf,
    },
    /// Add a top-level category (creating categories.toml from defaults if absent).
    AddCategory {
        /// Config directory.
        #[arg(long)]
        config: PathBuf,
        /// Category name.
        #[arg(long)]
        name: String,
        /// Category kind: spend | income | transfer.
        #[arg(long)]
        kind: String,
    },
    /// Add a categorization rule (pattern) to a category's subcategory.
    AddRule {
        /// Config directory.
        #[arg(long)]
        config: PathBuf,
        /// Existing or new category name.
        #[arg(long)]
        category: String,
        /// Subcategory name (created if absent).
        #[arg(long)]
        sub: String,
        /// Regex pattern to append.
        #[arg(long)]
        pattern: String,
        /// Optional minimum absolute amount (decimal).
        #[arg(long)]
        min: Option<String>,
        /// Optional maximum absolute amount (decimal).
        #[arg(long)]
        max: Option<String>,
        /// Optional match priority (default 0).
        #[arg(long)]
        priority: Option<i32>,
    },
    /// Add a household person.
    AddPerson {
        /// Config directory.
        #[arg(long)]
        config: PathBuf,
        /// Person id.
        #[arg(long)]
        id: String,
        /// Display name.
        #[arg(long)]
        name: String,
    },
    /// Assign an account/card to a person.
    AssignAccount {
        /// Config directory.
        #[arg(long)]
        config: PathBuf,
        /// Person id the account belongs to.
        #[arg(long)]
        person: String,
        /// Account id.
        #[arg(long)]
        id: String,
        /// Match by cardholder name.
        #[arg(long)]
        cardholder: Option<String>,
        /// Match by card last-4.
        #[arg(long)]
        card_last4: Option<String>,
        /// Match by source filename regex.
        #[arg(long)]
        filename: Option<String>,
        /// Restrict to a profile id.
        #[arg(long)]
        profile: Option<String>,
        /// Account kind (debit/credit_card/savings/...).
        #[arg(long, default_value = "debit")]
        kind: String,
        /// External account (transfer destination, no transactions of its own).
        #[arg(long)]
        external: bool,
    },
    /// Assign a transfer counterparty pattern to a person/account.
    AssignTransfer {
        /// Config directory.
        #[arg(long)]
        config: PathBuf,
        /// Regex on the transfer description.
        #[arg(long)]
        pattern: String,
        /// Target person or account id.
        #[arg(long)]
        to: String,
        /// Optional priority (default 0).
        #[arg(long)]
        priority: Option<i32>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Init { dir } => cmd_init(&dir),
        Command::Analyze { input, config, out } => cmd_analyze(&input, &config, &out),
        Command::Report {
            analysis,
            insights,
            out,
        } => cmd_report(&analysis, insights.as_deref(), &out),
        Command::AddCategory { config, name, kind } => cmd_add_category(&config, &name, &kind),
        Command::AddRule {
            config,
            category,
            sub,
            pattern,
            min,
            max,
            priority,
        } => cmd_add_rule(&config, &category, &sub, &pattern, min, max, priority),
        Command::AddPerson { config, id, name } => cmd_add_person(&config, &id, &name),
        Command::AssignAccount {
            config,
            person,
            id,
            cardholder,
            card_last4,
            filename,
            profile,
            kind,
            external,
        } => cmd_assign_account(
            &config, &person, &id, cardholder, card_last4, filename, profile, &kind, external,
        ),
        Command::AssignTransfer {
            config,
            pattern,
            to,
            priority,
        } => cmd_assign_transfer(&config, &pattern, &to, priority),
    }
}

/// Load config, run the analyze pipeline, write outputs, and print a summary.
fn cmd_analyze(input: &Path, config_dir: &Path, out: &Path) -> Result<()> {
    let config = wdmg::Config::load(config_dir)
        .with_context(|| format!("failed to load config from {}", config_dir.display()))?;

    let outcome = wdmg::pipeline::run_to_dir(&config, input, out)
        .with_context(|| format!("failed to analyze {}", input.display()))?;

    let report = &outcome.load_report;
    for (file, profile, n) in &report.loaded {
        println!("loaded {file} ({profile}): {n} rows");
    }
    for file in &report.skipped {
        println!("skip   {file} (no matching profile, not a CSV)");
    }
    for (file, err) in &report.errors {
        eprintln!("error  {file}: {err}");
    }

    let a = &outcome.analysis;
    println!(
        "\nwrote {}\nwrote {}",
        out.join("analysis.json").display(),
        out.join("review_queue.json").display()
    );
    println!(
        "flows: inflow {} / outflow {} / net {} ({})",
        a.analytics.flows.total_inflow,
        a.analytics.flows.total_outflow,
        a.analytics.flows.net,
        a.analytics.currency,
    );
    println!(
        "spend categories: {}, recurring: {}, anomalies: {}, transfer pairs: {}, unresolved: {}, review items: {}",
        a.analytics.spend_by_category.len(),
        a.analytics.recurring.len(),
        a.analytics.anomalies.len(),
        a.transfers.matched_pairs.len(),
        a.transfers.unresolved.len(),
        outcome.review_queue.unknown_merchants.len()
            + outcome.review_queue.unassigned_transfers.len(),
    );
    Ok(())
}

/// Render report.html from analysis.json (+ optional insights.json).
fn cmd_report(analysis_path: &Path, insights_path: Option<&Path>, out: &Path) -> Result<()> {
    let analysis_text = std::fs::read_to_string(analysis_path)
        .with_context(|| format!("failed to read {}", analysis_path.display()))?;
    let analysis: Analysis = serde_json::from_str(&analysis_text)
        .with_context(|| format!("failed to parse {}", analysis_path.display()))?;

    let insights = match insights_path {
        Some(p) => {
            let text = std::fs::read_to_string(p)
                .with_context(|| format!("failed to read {}", p.display()))?;
            Insights::from_json(&text)
                .with_context(|| format!("failed to parse {}", p.display()))?
        }
        None => Insights::default(),
    };

    write_report(out, &analysis, &insights)
        .with_context(|| format!("failed to write {}", out.display()))?;
    println!("wrote {}", out.display());
    Ok(())
}

/// Write the three default config files into `dir`, never overwriting existing ones.
fn cmd_init(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("failed to create directory {}", dir.display()))?;

    let files = [
        ("profiles.toml", defaults::profiles()),
        ("categories.toml", defaults::categories()),
        ("people.toml", defaults::people()),
        ("events.toml", defaults::events()),
    ];

    for (name, contents) in files {
        let path = dir.join(name);
        if path.exists() {
            println!("skip   {} (already exists)", path.display());
            continue;
        }
        std::fs::write(&path, contents)
            .with_context(|| format!("failed to write {}", path.display()))?;
        println!("create {}", path.display());
    }
    Ok(())
}

// ---- config-editing helpers -------------------------------------------------

/// Read a config doc from `dir/<name>`, falling back to the embedded default.
fn load_doc<T: serde::de::DeserializeOwned>(
    dir: &Path,
    name: &str,
    default_text: &str,
) -> Result<T> {
    let path = dir.join(name);
    let text = if path.exists() {
        std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?
    } else {
        default_text.to_string()
    };
    toml::from_str(&text).with_context(|| format!("failed to parse {name}"))
}

/// Serialize a config doc back to `dir/<name>` losslessly.
fn save_doc<T: serde::Serialize>(dir: &Path, name: &str, doc: &T) -> Result<()> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("failed to create directory {}", dir.display()))?;
    let text =
        toml::to_string_pretty(doc).with_context(|| format!("failed to serialize {name}"))?;
    let path = dir.join(name);
    std::fs::write(&path, text).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

fn parse_kind(s: &str) -> Result<CategoryKind> {
    match s.to_ascii_lowercase().as_str() {
        "spend" => Ok(CategoryKind::Spend),
        "income" => Ok(CategoryKind::Income),
        "transfer" => Ok(CategoryKind::Transfer),
        other => anyhow::bail!("invalid kind '{other}' (expected spend|income|transfer)"),
    }
}

fn cmd_add_category(config_dir: &Path, name: &str, kind: &str) -> Result<()> {
    let kind = parse_kind(kind)?;
    let mut doc: CategoriesDoc = load_doc(config_dir, "categories.toml", defaults::categories())?;
    if doc.categories.iter().any(|c| c.name == name) {
        println!("category '{name}' already exists; nothing to do");
        return Ok(());
    }
    doc.categories.push(Category {
        name: name.to_string(),
        kind,
        subcategories: Vec::new(),
    });
    save_doc(config_dir, "categories.toml", &doc)?;
    println!("added category '{name}' ({kind:?})");
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn cmd_add_rule(
    config_dir: &Path,
    category: &str,
    sub: &str,
    pattern: &str,
    min: Option<String>,
    max: Option<String>,
    priority: Option<i32>,
) -> Result<()> {
    use std::str::FromStr;
    let amount_min = min
        .as_deref()
        .map(rust_decimal::Decimal::from_str)
        .transpose()
        .context("invalid --min decimal")?;
    let amount_max = max
        .as_deref()
        .map(rust_decimal::Decimal::from_str)
        .transpose()
        .context("invalid --max decimal")?;

    let mut doc: CategoriesDoc = load_doc(config_dir, "categories.toml", defaults::categories())?;
    let cat = match doc.categories.iter_mut().find(|c| c.name == category) {
        Some(c) => c,
        None => {
            doc.categories.push(Category {
                name: category.to_string(),
                kind: CategoryKind::Spend,
                subcategories: Vec::new(),
            });
            // INVARIANT: just pushed, so last() is Some.
            doc.categories
                .last_mut()
                .expect("category just pushed exists")
        }
    };
    let subcat = match cat.subcategories.iter_mut().find(|s| s.name == sub) {
        Some(s) => s,
        None => {
            cat.subcategories.push(Subcategory {
                name: sub.to_string(),
                patterns: Vec::new(),
                amount_min: None,
                amount_max: None,
                priority: 0,
            });
            cat.subcategories
                .last_mut()
                .expect("subcategory just pushed exists")
        }
    };
    if subcat.patterns.iter().any(|p| p == pattern) {
        println!("pattern already present on {category}/{sub}; nothing to do");
        return Ok(());
    }
    subcat.patterns.push(pattern.to_string());
    if amount_min.is_some() {
        subcat.amount_min = amount_min;
    }
    if amount_max.is_some() {
        subcat.amount_max = amount_max;
    }
    if let Some(p) = priority {
        subcat.priority = p;
    }
    save_doc(config_dir, "categories.toml", &doc)?;
    println!("added rule to {category}/{sub}: /{pattern}/");
    Ok(())
}

fn cmd_add_person(config_dir: &Path, id: &str, name: &str) -> Result<()> {
    let mut doc: PeopleDoc = load_doc(config_dir, "people.toml", defaults::people())?;
    if doc.persons.iter().any(|p| p.id == id) {
        println!("person '{id}' already exists; nothing to do");
        return Ok(());
    }
    doc.persons.push(Person {
        id: id.to_string(),
        display_name: name.to_string(),
        accounts: Vec::new(),
    });
    save_doc(config_dir, "people.toml", &doc)?;
    println!("added person '{id}' ({name})");
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn cmd_assign_account(
    config_dir: &Path,
    person: &str,
    id: &str,
    cardholder: Option<String>,
    card_last4: Option<String>,
    filename: Option<String>,
    profile: Option<String>,
    kind: &str,
    external: bool,
) -> Result<()> {
    let mut doc: PeopleDoc = load_doc(config_dir, "people.toml", defaults::people())?;
    let p = doc
        .persons
        .iter_mut()
        .find(|p| p.id == person)
        .with_context(|| format!("no person '{person}' (add it first with add-person)"))?;
    if let Some(acct) = p.accounts.iter_mut().find(|a| a.id == id) {
        // Update existing account in place.
        acct.cardholder = cardholder.or(acct.cardholder.take());
        acct.card_last4 = card_last4.or(acct.card_last4.take());
        acct.filename = filename.or(acct.filename.take());
        acct.profile = profile.or(acct.profile.take());
        acct.kind = kind.to_string();
        acct.external = external;
    } else {
        p.accounts.push(Account {
            id: id.to_string(),
            cardholder,
            card_last4,
            filename,
            profile,
            kind: kind.to_string(),
            external,
        });
    }
    save_doc(config_dir, "people.toml", &doc)?;
    println!("assigned account '{id}' to person '{person}'");
    Ok(())
}

fn cmd_assign_transfer(
    config_dir: &Path,
    pattern: &str,
    to: &str,
    priority: Option<i32>,
) -> Result<()> {
    let mut doc: PeopleDoc = load_doc(config_dir, "people.toml", defaults::people())?;
    if doc
        .transfer_rules
        .iter()
        .any(|r| r.pattern == pattern && r.to == to)
    {
        println!("transfer rule already present; nothing to do");
        return Ok(());
    }
    doc.transfer_rules.push(TransferRule {
        pattern: pattern.to_string(),
        to: to.to_string(),
        priority: priority.unwrap_or(0),
    });
    // Keep deterministic, highest-priority-first ordering on disk.
    doc.transfer_rules.sort_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then_with(|| a.pattern.cmp(&b.pattern))
    });
    save_doc(config_dir, "people.toml", &doc)?;
    println!("assigned transfers matching /{pattern}/ → '{to}'");
    Ok(())
}
