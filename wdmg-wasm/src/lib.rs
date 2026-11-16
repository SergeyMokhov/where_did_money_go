//! WebAssembly bindings for the `wdmg` pipeline: the browser runs the **same**
//! Rust analysis as the CLI, on files the user picked (never uploaded anywhere).
//!
//! Every export takes and returns JSON strings (snake_case, exactly as `serde`
//! emits the CLI's `analysis.json`). The `wasm_bindgen` wrappers are thin: each
//! delegates to a plain `*_json` function returning `Result<String, String>`, so
//! the logic is unit-tested natively and errors reach JS as readable messages.
//!
//! Panics: none of these paths touch `std::fs`, the clock, or randomness; all
//! untrusted input (JS values, TOML, CSV bytes, change-point requests) is
//! validated at this boundary and surfaced as `JsError`.

use std::collections::BTreeMap;

use serde::Serialize;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use wdmg::analytics::changepoint;
use wdmg::config::{self, Config};
use wdmg::emit::{Analysis, ReviewQueue};

/// Upper bound on the total bytes of one `analyze` call (defensive: a browser
/// tab should fail with a message, not by exhausting memory).
pub const MAX_TOTAL_BYTES: usize = 256 * 1024 * 1024;

/// Embedded default config files as a JSON object `{file name: TOML text}`.
#[must_use]
pub fn default_config_json() -> String {
    let map: BTreeMap<&str, &str> = [
        ("profiles.toml", config::defaults::profiles()),
        ("categories.toml", config::defaults::categories()),
        ("people.toml", config::defaults::people()),
        ("events.toml", config::defaults::events()),
        ("settings.toml", config::defaults::settings()),
    ]
    .into_iter()
    .collect();
    // Serializing a map of strings cannot fail.
    serde_json::to_string(&map).unwrap_or_else(|_| "{}".to_string())
}

#[derive(Serialize)]
struct LoadedFile<'a> {
    file: &'a str,
    profile: &'a str,
    rows: usize,
}

#[derive(Serialize)]
struct FileError<'a> {
    file: &'a str,
    error: &'a str,
}

#[derive(Serialize)]
struct LoadReportOut<'a> {
    loaded: Vec<LoadedFile<'a>>,
    skipped: &'a [String],
    errors: Vec<FileError<'a>>,
}

#[derive(Serialize)]
struct AnalyzeOut<'a> {
    analysis: &'a Analysis,
    review_queue: &'a ReviewQueue,
    load_report: LoadReportOut<'a>,
}

/// Parse the `{file name: TOML text}` config object into a [`Config`]. An empty
/// string means "no config files" (embedded defaults). Unknown file names are
/// rejected so a typo cannot silently fall back to defaults.
fn parse_config(config_json: &str) -> Result<Config, String> {
    let texts: BTreeMap<String, String> = if config_json.trim().is_empty() {
        BTreeMap::new()
    } else {
        serde_json::from_str(config_json).map_err(|e| {
            format!("config must be a JSON object of {{\"<name>.toml\": \"<text>\"}}: {e}")
        })?
    };
    if let Some(bad) = texts
        .keys()
        .find(|k| !Config::FILE_NAMES.contains(&k.as_str()))
    {
        return Err(format!(
            "unknown config file {bad:?}; expected one of {:?}",
            Config::FILE_NAMES
        ));
    }
    Config::from_texts(&texts).map_err(|e| format!("invalid config: {}", error_chain(&e)))
}

/// Render an error with its `source()` chain, skipping sources whose text the
/// parent message already includes (thiserror messages often embed them).
fn error_chain(e: &dyn std::error::Error) -> String {
    let mut msg = e.to_string();
    let mut cur = e.source();
    while let Some(s) = cur {
        let t = s.to_string();
        if !msg.contains(&t) {
            msg.push_str(": ");
            msg.push_str(&t);
        }
        cur = s.source();
    }
    msg
}

/// Run the pipeline over in-memory files with a JSON config; return the
/// `{analysis, review_queue, load_report}` JSON document.
pub fn analyze_json(files: &[(String, Vec<u8>)], config_json: &str) -> Result<String, String> {
    let total = files
        .iter()
        .try_fold(0usize, |acc, (_, b)| acc.checked_add(b.len()))
        .unwrap_or(usize::MAX);
    if total > MAX_TOTAL_BYTES {
        return Err(format!(
            "input too large: {total} bytes across {} files (limit {MAX_TOTAL_BYTES})",
            files.len()
        ));
    }
    let config = parse_config(config_json)?;
    let outcome = wdmg::pipeline::analyze_files(&config, files)
        .map_err(|e| format!("analysis failed: {}", error_chain(&e)))?;
    let r = &outcome.load_report;
    let out = AnalyzeOut {
        analysis: &outcome.analysis,
        review_queue: &outcome.review_queue,
        load_report: LoadReportOut {
            loaded: r
                .loaded
                .iter()
                .map(|(file, profile, rows)| LoadedFile {
                    file,
                    profile,
                    rows: *rows,
                })
                .collect(),
            skipped: &r.skipped,
            errors: r
                .errors
                .iter()
                .map(|(file, error)| FileError { file, error })
                .collect(),
        },
    };
    serde_json::to_string(&out).map_err(|e| format!("failed to serialize analysis: {e}"))
}

#[derive(Serialize)]
struct SampleRow {
    date: String,
    amount: String,
    merchant: String,
    kind: String,
}

#[derive(Serialize)]
struct FileCheck {
    file: String,
    /// Matched profile id, or `null` when none matched / parsing failed.
    profile: Option<String>,
    rows: usize,
    /// Why the file could not be read (detection reasons or parse error).
    error: Option<String>,
    /// `true` when no profile matched (as opposed to a parse failure).
    unrecognized: bool,
    /// First few parsed rows, so the UI can show how the file was read.
    sample: Vec<SampleRow>,
    /// Rows per account after identity resolution (`unassigned` = no account
    /// in people.toml claims them), so the UI can offer to assign the file.
    accounts: BTreeMap<String, usize>,
    /// Rows that also fit an account other than the one they went to (shadowed
    /// by an earlier account), per shadowed account.
    also: BTreeMap<String, usize>,
    /// Rows per distinct cardholder value (empty when the format has no
    /// cardholder column).
    cardholders: BTreeMap<String, usize>,
    /// The id the format captures from the file name (e.g. a card's last 4).
    file_id: Option<String>,
}

/// Detect and parse each file on its own (no analysis): per-file profile, row
/// count, error and a few parsed rows. Cheap enough to run on every render of
/// the configuration screen.
pub fn check_files_json(files: &[(String, Vec<u8>)], config_json: &str) -> Result<String, String> {
    let config = parse_config(config_json)?;
    let out: Vec<FileCheck> = files
        .iter()
        .map(|(name, bytes)| {
            let label = std::path::Path::new(name);
            match wdmg::ingest::load_bytes(&config, label, bytes) {
                Ok(mut txns) => FileCheck {
                    accounts: {
                        wdmg::identity::resolve(&config, &mut txns);
                        let mut m = BTreeMap::new();
                        for t in &txns {
                            let k = t.account_id.clone().unwrap_or_else(|| "unassigned".into());
                            *m.entry(k).or_insert(0) += 1;
                        }
                        m
                    },
                    also: {
                        let mut m = BTreeMap::new();
                        for t in &txns {
                            for a in wdmg::identity::matching_accounts(&config, t)
                                .into_iter()
                                .skip(1)
                            {
                                *m.entry(a.to_string()).or_insert(0) += 1;
                            }
                        }
                        m
                    },
                    cardholders: {
                        let mut m = BTreeMap::new();
                        for t in &txns {
                            if let Some(c) = t
                                .cardholder
                                .as_deref()
                                .map(str::trim)
                                .filter(|c| !c.is_empty())
                            {
                                *m.entry(c.to_string()).or_insert(0) += 1;
                            }
                        }
                        m
                    },
                    file_id: txns
                        .first()
                        .and_then(|t| wdmg::identity::filename_id(&config, t)),
                    file: name.clone(),
                    profile: txns.first().map(|t| t.profile_id.clone()).or_else(|| {
                        wdmg::ingest::detect::decode(label, bytes)
                            .ok()
                            .and_then(|t| wdmg::ingest::detect_text(&config, label, t).ok())
                            .map(|p| p.profile.id.clone())
                    }),
                    rows: txns.len(),
                    error: None,
                    unrecognized: false,
                    sample: txns
                        .iter()
                        .take(5)
                        .map(|t| SampleRow {
                            date: t.date.to_string(),
                            amount: t.amount.amount().to_string(),
                            merchant: t.normalized_merchant.clone(),
                            kind: t.txn_type.to_string(),
                        })
                        .collect(),
                },
                Err(e) => FileCheck {
                    file: name.clone(),
                    profile: None,
                    rows: 0,
                    unrecognized: matches!(e, wdmg::error::IngestError::NoProfileMatch { .. }),
                    error: Some(error_chain(&e)),
                    sample: Vec::new(),
                    accounts: BTreeMap::new(),
                    also: BTreeMap::new(),
                    cardholders: BTreeMap::new(),
                    file_id: None,
                },
            }
        })
        .collect();
    serde_json::to_string(&out).map_err(|e| format!("failed to serialize file checks: {e}"))
}

/// Apply structured edits to `people.toml` text (comments preserved).
pub fn edit_people_json(text: &str, ops_json: &str) -> Result<String, String> {
    wdmg::config::people_edit::apply_json(text, ops_json).map_err(|e| e.to_string())
}

/// `people.toml` text → its parsed form as JSON (persons, accounts, rules).
pub fn people_json(text: &str) -> Result<String, String> {
    let doc = wdmg::config::people_edit::parse(text).map_err(|e| e.to_string())?;
    serde_json::to_string(&doc).map_err(|e| format!("failed to serialize people: {e}"))
}

/// The categories the user's file is layered over: built-in + `learned.toml`.
fn category_base(texts: &BTreeMap<String, String>) -> Result<wdmg::config::CategoriesDoc, String> {
    let defaults: wdmg::config::CategoriesDoc =
        toml::from_str(wdmg::config::defaults::categories())
            .map_err(|e| format!("built-in categories: {e}"))?;
    let learned = match texts.get("learned.toml") {
        Some(t) => Some(toml::from_str(t).map_err(|e| format!("learned.toml: {e}"))?),
        None => None,
    };
    Ok(defaults.merge(learned, None))
}

fn config_texts(config_json: &str) -> Result<BTreeMap<String, String>, String> {
    if config_json.trim().is_empty() {
        return Ok(BTreeMap::new());
    }
    serde_json::from_str(config_json).map_err(|e| format!("config must be a JSON object: {e}"))
}

#[derive(Serialize)]
struct LabSub {
    name: String,
    patterns: Vec<String>,
    /// Patterns with their own amount range.
    ranged: Vec<LabRanged>,
    amount_min: Option<String>,
    amount_max: Option<String>,
    priority: i32,
    /// `builtin` (as shipped), `changed` (a built-in your file overrides),
    /// `yours` (only in your file) or `off` (a built-in your file emptied).
    source: &'static str,
}
#[derive(Serialize)]
struct LabRanged {
    pattern: String,
    amount_min: Option<String>,
    amount_max: Option<String>,
}
#[derive(Serialize)]
struct LabCategory {
    name: String,
    kind: wdmg::config::CategoryKind,
    subs: Vec<LabSub>,
}
#[derive(Serialize)]
struct LabRule {
    pattern: String,
    to: String,
    priority: i32,
}
#[derive(Serialize)]
struct LabOut {
    rows: Vec<wdmg::pipeline::LabRow>,
    categories: Vec<LabCategory>,
    /// Transfer rules in file order (indices match `rows[].transfer.rule`).
    rules: Vec<LabRule>,
    /// Valid transfer-rule targets: people, then accounts.
    targets: Vec<String>,
}

/// Rows with the rule that categorized each and how its transfer resolved,
/// plus the layered categories (with where each subcategory comes from) and
/// the transfer rules — everything the rule editors show and preview.
pub fn rule_lab_json(files: &[(String, Vec<u8>)], config_json: &str) -> Result<String, String> {
    let texts = config_texts(config_json)?;
    let config = parse_config(config_json)?;
    let rows = wdmg::pipeline::rule_lab(&config, files)
        .map_err(|e| format!("analysis failed: {}", error_chain(&e)))?;
    let base = category_base(&texts)?;
    let user: Option<wdmg::config::CategoriesDoc> = match texts.get("categories.toml") {
        Some(t) => Some(toml::from_str(t).map_err(|e| format!("categories.toml: {e}"))?),
        None => None,
    };
    let find = |doc: &wdmg::config::CategoriesDoc, c: &str, s: &str| {
        doc.categories
            .iter()
            .find(|x| x.name == c)
            .and_then(|x| x.subcategories.iter().find(|y| y.name == s))
            .cloned()
    };
    let categories = config
        .categories
        .categories
        .iter()
        .map(|c| LabCategory {
            name: c.name.clone(),
            kind: c.kind,
            subs: c
                .subcategories
                .iter()
                .map(|s| {
                    let in_base = find(&base, &c.name, &s.name);
                    let in_user = user.as_ref().and_then(|u| find(u, &c.name, &s.name));
                    let source = match (in_base, in_user) {
                        (Some(b), Some(u)) if u.is_empty() && !b.is_empty() => {
                            "off"
                        }
                        (Some(b), Some(u)) if b != u => "changed",
                        (Some(_), _) => "builtin",
                        _ => "yours",
                    };
                    LabSub {
                        name: s.name.clone(),
                        patterns: s.patterns.clone(),
                        ranged: s
                            .ranged
                            .iter()
                            .map(|r| LabRanged {
                                pattern: r.pattern.clone(),
                                amount_min: r.amount_min.map(|d| d.to_string()),
                                amount_max: r.amount_max.map(|d| d.to_string()),
                            })
                            .collect(),
                        amount_min: s.amount_min.map(|d| d.to_string()),
                        amount_max: s.amount_max.map(|d| d.to_string()),
                        priority: s.priority,
                        source,
                    }
                })
                .collect(),
        })
        .collect();
    let rules = config
        .people
        .transfer_rules
        .iter()
        .map(|r| LabRule {
            pattern: r.pattern.clone(),
            to: r.to.clone(),
            priority: r.priority,
        })
        .collect();
    let targets = config
        .people
        .persons
        .iter()
        .map(|p| p.id.clone())
        .chain(
            config
                .people
                .persons
                .iter()
                .flat_map(|p| p.accounts.iter().map(|a| a.id.clone())),
        )
        .collect();
    serde_json::to_string(&LabOut {
        rows,
        categories,
        rules,
        targets,
    })
    .map_err(|e| format!("failed to serialize: {e}"))
}

/// Edit the user's `categories.toml` (empty = none yet), layered over the
/// built-in + learned categories from `config_json`.
pub fn edit_categories_json(
    user_text: &str,
    config_json: &str,
    ops_json: &str,
) -> Result<String, String> {
    let base = category_base(&config_texts(config_json)?)?;
    wdmg::config::categories_edit::apply_json(user_text, &base, ops_json).map_err(|e| e.to_string())
}

/// Validate a change-point request JSON and run the engine.
pub fn changepoints_json(input_json: &str) -> Result<String, String> {
    let req: changepoint::Request =
        serde_json::from_str(input_json).map_err(|e| format!("invalid changepoints input: {e}"))?;
    let result = req
        .run()
        .map_err(|e| format!("invalid changepoints input: {e}"))?;
    serde_json::to_string(&result).map_err(|e| format!("failed to serialize change points: {e}"))
}

/// Convert the JS `files` array (`[{name: string, bytes: Uint8Array}]`).
fn files_from_js(files: &js_sys::Array) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut out = Vec::with_capacity(files.length() as usize);
    for (i, item) in files.iter().enumerate() {
        let name = js_sys::Reflect::get(&item, &JsValue::from_str("name"))
            .ok()
            .and_then(|v| v.as_string())
            .ok_or_else(|| format!("files[{i}].name must be a string"))?;
        let bytes = js_sys::Reflect::get(&item, &JsValue::from_str("bytes"))
            .ok()
            .and_then(|v| v.dyn_into::<js_sys::Uint8Array>().ok())
            .ok_or_else(|| format!("files[{i}].bytes must be a Uint8Array ({name})"))?;
        out.push((name, bytes.to_vec()));
    }
    Ok(out)
}

/// The embedded default config: JSON `{"profiles.toml": "...", "categories.toml":
/// "...", "people.toml": "...", "events.toml": "...", "settings.toml": "..."}`.
#[wasm_bindgen]
#[must_use]
pub fn default_config() -> String {
    default_config_json()
}

/// Analyze statement files. `files`: `[{name: string, bytes: Uint8Array}]`;
/// `config_json`: `{"<name>.toml": "<toml text>"}` (missing files ⇒ defaults,
/// exactly like the CLI's `--config` dir). Returns JSON
/// `{"analysis", "review_queue", "load_report": {"loaded", "skipped", "errors"}}`.
#[wasm_bindgen]
pub fn analyze(files: js_sys::Array, config_json: &str) -> Result<String, JsError> {
    let files = files_from_js(&files).map_err(|e| JsError::new(&e))?;
    analyze_json(&files, config_json).map_err(|e| JsError::new(&e))
}

/// Per-file detection/parse check. `files` as for [`analyze`]; returns JSON
/// `[{"file","profile","rows","error","unrecognized","sample":[{"date","amount","merchant","kind"}]}]`.
#[wasm_bindgen]
pub fn check_files(files: js_sys::Array, config_json: &str) -> Result<String, JsError> {
    let files = files_from_js(&files).map_err(|e| JsError::new(&e))?;
    check_files_json(&files, config_json).map_err(|e| JsError::new(&e))
}

/// Change-point detection on a daily series. Input JSON:
/// `{"start":"YYYY-MM-DD","daily":[cents...],"metric":"net|in|out",
/// "sensitivity":"standard|sensitive","min_days":14,"bucket":"day|week|biweek|month"}`.
#[wasm_bindgen]
pub fn changepoints(input_json: &str) -> Result<String, JsError> {
    changepoints_json(input_json).map_err(|e| JsError::new(&e))
}

/// Edit `people.toml`; `ops_json` is a JSON array of ops (see wdmg's
/// `config::people_edit::PeopleOp`). Returns the new text.
#[wasm_bindgen]
pub fn edit_people(text: &str, ops_json: &str) -> Result<String, JsError> {
    edit_people_json(text, ops_json).map_err(|e| JsError::new(&e))
}

/// Rows + rules for the rule editors (see [`rule_lab_json`]).
#[wasm_bindgen]
pub fn rule_lab(files: js_sys::Array, config_json: &str) -> Result<String, JsError> {
    let files = files_from_js(&files).map_err(|e| JsError::new(&e))?;
    rule_lab_json(&files, config_json).map_err(|e| JsError::new(&e))
}

/// Edit the user's `categories.toml`; `ops_json` is a JSON array of
/// `config::categories_edit::CategoryOp`. Returns the new text.
#[wasm_bindgen]
pub fn edit_categories(
    user_text: &str,
    config_json: &str,
    ops_json: &str,
) -> Result<String, JsError> {
    edit_categories_json(user_text, config_json, ops_json).map_err(|e| JsError::new(&e))
}

/// Parse `people.toml` text into JSON for the UI.
#[wasm_bindgen]
pub fn parse_people(text: &str) -> Result<String, JsError> {
    people_json(text).map_err(|e| JsError::new(&e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_all_files_and_builds() {
        let map: BTreeMap<String, String> = serde_json::from_str(&default_config_json()).unwrap();
        assert_eq!(map.len(), 5);
        assert!(Config::from_texts(&map).is_ok());
    }

    #[test]
    fn config_errors_are_readable() {
        let err = parse_config(r#"{"people.toml": "persons = [[["}"#).unwrap_err();
        assert!(err.contains("people.toml"), "{err}");
        let err = parse_config(r#"{"catagories.toml": ""}"#).unwrap_err();
        assert!(err.contains("unknown config file"), "{err}");
        assert!(parse_config("[1,2]").is_err());
        assert!(parse_config("").is_ok());
    }

    #[test]
    fn analyze_reports_files() {
        let csv = "\"Description\",\"Type\",\"Card Holder Name\",\"Date\",\"Time\",\"Amount\"\n\
                   \"SHELL\",\"PURCHASE\",\"FRY\",\"01/05/2026\",\"10:00 AM\",\"-60.00\"\n";
        let files = vec![
            (
                "7001 xxxx xxxx 7003.CSV".to_string(),
                csv.as_bytes().to_vec(),
            ),
            ("notes.txt".to_string(), b"hello".to_vec()),
            ("bad.csv".to_string(), b"nope,nothing\n1,2\n".to_vec()),
        ];
        let out: serde_json::Value =
            serde_json::from_str(&analyze_json(&files, "").unwrap()).unwrap();
        let lr = &out["load_report"];
        assert_eq!(lr["loaded"][0]["rows"], 1);
        assert_eq!(lr["skipped"][0], "notes.txt");
        assert_eq!(lr["errors"][0]["file"], "bad.csv");
        assert!(out["analysis"]["transactions"].is_array());
        assert!(out["review_queue"]["unknown_merchants"].is_array());
    }

    #[test]
    fn rule_lab_and_category_edits_round_trip() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../playground");
        let mut cfg = BTreeMap::new();
        for n in ["profiles.toml", "categories.toml", "people.toml"] {
            cfg.insert(
                n.to_string(),
                std::fs::read_to_string(root.join("config").join(n)).unwrap(),
            );
        }
        let files: Vec<(String, Vec<u8>)> = ["joint_chequing.csv", "fry_mastercard_7011.csv"]
            .iter()
            .map(|f| {
                (
                    f.to_string(),
                    std::fs::read(root.join("data").join(f)).unwrap(),
                )
            })
            .collect();
        let cj = serde_json::to_string(&cfg).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&rule_lab_json(&files, &cj).unwrap()).unwrap();
        assert!(v["rows"].as_array().unwrap().len() > 100);
        assert!(!v["rules"].as_array().unwrap().is_empty());
        let cats = v["categories"].as_array().unwrap();
        let src = |c: &str, s: &str| {
            cats.iter()
                .find(|x| x["name"] == c)
                .and_then(|x| {
                    x["subs"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|y| y["name"] == s)
                })
                .map(|y| y["source"].clone())
        };
        // The playground overrides the built-in "Coffee" (adds patterns): changed.
        assert_eq!(
            src("Takeout & Restaurants", "Coffee"),
            Some(serde_json::json!("changed"))
        );
        // Adding a pattern from the browser: the next lab run categorizes with it.
        let user = cfg["categories.toml"].clone();
        let row = v["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["kind"] == "spend" && r["merchant"].as_str().is_some_and(|m| m.len() > 12))
            .cloned();
        assert!(row.is_some(), "the playground has a spend row");
        if let Some(row) = row {
            let m = row["merchant"].as_str().unwrap().to_string();
            let ops = serde_json::json!([{"op":"add_pattern","category":"Test","subcategory":"Probe","pattern": regex_escape(&m)}]).to_string();
            let edited = edit_categories_json(&user, &cj, &ops).unwrap();
            cfg.insert("categories.toml".into(), edited);
            let v2: serde_json::Value = serde_json::from_str(
                &rule_lab_json(&files, &serde_json::to_string(&cfg).unwrap()).unwrap(),
            )
            .unwrap();
            assert!(v2["rows"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["merchant"] == m.as_str() && r["category"] == "Test"));
        }
    }

    fn regex_escape(s: &str) -> String {
        s.chars()
            .flat_map(|c| {
                if "\\.+*?()|[]{}^$#&-~".contains(c) {
                    vec!['\\', c]
                } else {
                    vec![c]
                }
            })
            .collect()
    }

    #[test]
    fn check_files_reports_cardholders_and_file_id() {
        let csv = "\"Description\",\"Type\",\"Card Holder Name\",\"Date\",\"Time\",\"Amount\"\n\
                   \"CAFE 3000\",\"PURCHASE\",\"FRY\",\"01/02/2997\",\"03:36 PM\",\"-6.05\"\n\
                   \"EL CHEAPO\",\"PURCHASE\",\"BENDER\",\"01/03/2997\",\"10:14 AM\",\"-35.01\"\n";
        let files = vec![("card_7011.csv".to_string(), csv.as_bytes().to_vec())];
        let v: serde_json::Value =
            serde_json::from_str(&check_files_json(&files, "").unwrap()).unwrap();
        assert_eq!(v[0]["profile"], "mastercard_credit");
        assert_eq!(v[0]["cardholders"]["FRY"], 1);
        assert_eq!(v[0]["cardholders"]["BENDER"], 1);
        assert_eq!(v[0]["file_id"], "7011");
        assert!(v[0]["also"].is_object());
    }

    #[test]
    fn check_files_reports_per_file_status() {
        let ok = "Transaction date,Transaction,Name,Memo,Amount\n\
                  03/02/2026,CREDIT,PAYMENT - THANK YOU,,54.10\n";
        let files = vec![
            ("card.csv".to_string(), ok.as_bytes().to_vec()),
            (
                "odd.csv".to_string(),
                b"Posted,Details,Value\n2026-11-01,X,1\n".to_vec(),
            ),
        ];
        let v: serde_json::Value =
            serde_json::from_str(&check_files_json(&files, "").unwrap()).unwrap();
        assert_eq!(v[0]["profile"], "rewards_credit");
        assert_eq!(v[0]["rows"], 1);
        assert_eq!(v[0]["sample"][0]["kind"], "transfer");
        assert_eq!(v[1]["unrecognized"], true);
        assert!(v[1]["profile"].is_null());
        assert!(v[1]["error"]
            .as_str()
            .unwrap()
            .contains("no profile matched"));
    }

    #[test]
    fn changepoints_validates_and_runs() {
        let ok = changepoints_json(
            r#"{"start":"2024-01-01","daily":[1,2,3,4,5,6,7,8],"metric":"out","bucket":"week"}"#,
        )
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&ok).unwrap();
        assert_eq!(v["block_days"], 7);
        assert_eq!(v["segments"][0]["start"], "2024-01-01");
        let err =
            changepoints_json(r#"{"start":"01/02/2024","daily":[1],"metric":"out"}"#).unwrap_err();
        assert!(err.contains("YYYY-MM-DD"), "{err}");
        let err = changepoints_json(r#"{"start":"2024-01-01","daily":[1],"metric":"sideways"}"#)
            .unwrap_err();
        assert!(err.contains("metric") || err.contains("variant"), "{err}");
    }
}
