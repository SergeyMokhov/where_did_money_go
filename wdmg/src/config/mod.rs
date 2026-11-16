//! Configuration loading and validation.
//!
//! Reads `profiles.toml`, `categories.toml`, `people.toml` (and optional
//! `learned.toml`) from a working directory, validates and compiles every regex
//! once, and returns a typed [`Config`]. The shipped defaults are embedded via
//! `include_str!` for `wdmg init` and for tests.
//!
//! Per defensive-coding ("parse, don't validate"): all untrusted TOML is turned
//! into validated, regex-compiled values here at the boundary, so later phases
//! never re-check raw config.

pub mod categories;
pub mod categories_edit;
pub mod events;
pub mod people;
pub mod people_edit;
pub mod profiles;
pub mod settings;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use regex::{Regex, RegexBuilder};

use crate::error::ConfigError;

pub use categories::{
    pin_key, CategoriesDoc, Category, CategoryKind, Pin, PinKey, RangedPattern, Subcategory,
};
pub use events::{EventsDoc, LifeEvent};
pub use people::{Account, PeopleDoc, Person, TransferRule};
pub use profiles::{
    AccountSpec, AmountSpec, BySign, DateSpec, MemoSpec, MerchantSpec, Profile, ProfilesDoc, Sign,
    TypeSpec,
};
pub use settings::{AnalysisProfile, AnalysisSettings, SettingsDoc};

/// Embedded default `profiles.toml` (shipped asset).
pub const DEFAULT_PROFILES: &str = include_str!("../../../assets/default_profiles.toml");
/// Embedded default `categories.toml` (shipped asset).
pub const DEFAULT_CATEGORIES: &str = include_str!("../../../assets/default_categories.toml");
/// Embedded default `people.toml` (shipped asset).
pub const DEFAULT_PEOPLE: &str = include_str!("../../../assets/default_people.toml");
/// Embedded default `events.toml` (shipped asset).
pub const DEFAULT_EVENTS: &str = include_str!("../../../assets/default_events.toml");
/// Embedded default `settings.toml` (shipped asset).
pub const DEFAULT_SETTINGS: &str = include_str!("../../../assets/default_settings.toml");

/// Accessor for the embedded default config files, used by `wdmg init`.
pub mod defaults {
    /// The default `profiles.toml` contents.
    #[must_use]
    pub fn profiles() -> &'static str {
        super::DEFAULT_PROFILES
    }
    /// The default `categories.toml` contents.
    #[must_use]
    pub fn categories() -> &'static str {
        super::DEFAULT_CATEGORIES
    }
    /// The default `people.toml` contents.
    #[must_use]
    pub fn people() -> &'static str {
        super::DEFAULT_PEOPLE
    }
    /// The default `events.toml` contents.
    #[must_use]
    pub fn events() -> &'static str {
        super::DEFAULT_EVENTS
    }
    /// The default `settings.toml` contents.
    #[must_use]
    pub fn settings() -> &'static str {
        super::DEFAULT_SETTINGS
    }
}

/// A profile whose detection/parse regexes have been compiled and validated.
#[derive(Debug, Clone)]
pub struct CompiledProfile {
    /// The raw profile definition.
    pub profile: Profile,
    /// Compiled `header_first_cell` (anchored alternation), if any.
    pub header_first_cell: Option<Regex>,
    /// Compiled `content_probe`, if any.
    pub content_probe: Option<Regex>,
    /// Compiled `filename` matcher, if any.
    pub filename: Option<Regex>,
    /// Compiled merchant `strip_codes` prefix, if any.
    pub strip_codes: Option<Regex>,
    /// Compiled memo `category_harvest` (named group `cat`), if any.
    pub category_harvest: Option<Regex>,
    /// Compiled `account.id_from_filename`, if any.
    pub id_from_filename: Option<Regex>,
}

/// A transfer rule whose pattern has been compiled.
#[derive(Debug, Clone)]
pub struct CompiledTransferRule {
    /// The raw rule.
    pub rule: TransferRule,
    /// Compiled pattern.
    pub pattern: Regex,
}

/// A people-account filename matcher compiled for reuse.
#[derive(Debug, Clone)]
pub struct CompiledAccountMatcher {
    /// Owning person id.
    pub person_id: String,
    /// The account definition.
    pub account: Account,
    /// Compiled filename matcher, if the account declares one.
    pub filename: Option<Regex>,
}

/// The fully-validated configuration consumed by later phases.
#[derive(Debug, Clone)]
pub struct Config {
    /// Compiled source profiles, in match order.
    pub profiles: Vec<CompiledProfile>,
    /// Merged categories (user > learned > defaults).
    pub categories: CategoriesDoc,
    /// Identity document.
    pub people: PeopleDoc,
    /// Compiled transfer rules, sorted by descending priority.
    pub transfer_rules: Vec<CompiledTransferRule>,
    /// Compiled per-account filename matchers (flattened across people).
    pub account_matchers: Vec<CompiledAccountMatcher>,
    /// Validated, date-sorted life events (annotation only; may be empty).
    pub events: Vec<LifeEvent>,
    /// Analysis settings (persona profile + jurisdiction params).
    pub settings: AnalysisSettings,
}

/// Compile a case-insensitive regex, mapping failures to a typed error.
fn compile(pattern: &str, context: &str) -> Result<Regex, ConfigError> {
    Regex::new(pattern).map_err(|source| ConfigError::Regex {
        context: context.to_string(),
        pattern: pattern.to_string(),
        source,
    })
}

/// Remove look-around groups (`(?=...)`, `(?!...)`, `(?<=...)`, `(?<!...)`) from a
/// category pattern.
///
/// The `regex` crate intentionally does not support look-around (it guarantees
/// linear-time matching). The shipped category patterns occasionally use it for
/// readability (e.g. `UBER(?! EATS)`). For phase-1 *validation* we strip such
/// groups so the remainder can be compiled and checked; the categorize phase
/// (later) owns the real disambiguation semantics. This is deliberately simple
/// and only removes well-formed, non-nested look-around groups.
fn sanitize_category_pattern(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len());
    let bytes = pattern.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Detect the start of a look-around group: "(?=", "(?!", "(?<=", "(?<!".
        let is_lookaround = bytes[i] == b'('
            && i + 2 < bytes.len()
            && bytes[i + 1] == b'?'
            && (bytes[i + 2] == b'='
                || bytes[i + 2] == b'!'
                || (bytes[i + 2] == b'<'
                    && i + 3 < bytes.len()
                    && (bytes[i + 3] == b'=' || bytes[i + 3] == b'!')));
        if is_lookaround {
            // Skip to the matching close paren, accounting for nesting and escapes.
            let mut depth = 0usize;
            let mut j = i;
            while j < bytes.len() {
                match bytes[j] {
                    b'\\' => j += 1, // skip escaped char
                    b'(' => depth += 1,
                    b')' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            i = j + 1; // resume after the close paren
        } else {
            // Copy this char (handle multi-byte UTF-8 by copying the whole char).
            let ch_len = utf8_len(bytes[i]);
            out.push_str(&pattern[i..i + ch_len]);
            i += ch_len;
        }
    }
    out
}

/// Byte length of a UTF-8 char given its lead byte.
fn utf8_len(lead: u8) -> usize {
    if lead < 0x80 {
        1
    } else if lead >> 5 == 0b110 {
        2
    } else if lead >> 4 == 0b1110 {
        3
    } else {
        4
    }
}

/// Compile a case-insensitive regex (used for category patterns).
fn compile_ci(pattern: &str, context: &str) -> Result<Regex, ConfigError> {
    RegexBuilder::new(pattern)
        .case_insensitive(true)
        .build()
        .map_err(|source| ConfigError::Regex {
            context: context.to_string(),
            pattern: pattern.to_string(),
            source,
        })
}

impl CompiledProfile {
    /// Compile every regex in a profile, validating each at load time.
    fn compile(profile: Profile) -> Result<Self, ConfigError> {
        let ctx = &profile.id;
        if let Some(bs) = &profile.type_spec.by_sign {
            for (side, v) in [("negative", &bs.negative), ("positive", &bs.positive)] {
                if !matches!(v.as_str(), "spend" | "income" | "transfer") {
                    return Err(ConfigError::Invalid {
                        context: format!("profile {ctx} [type.by_sign]"),
                        message: format!("{side} = {v:?}; expected spend, income or transfer"),
                    });
                }
            }
        }
        // Anchor header_first_cell so an alternation "A|B" matches the WHOLE cell.
        let header_first_cell = match &profile.match_rules.header_first_cell {
            Some(p) => Some(compile(&format!("^(?:{p})$"), ctx)?),
            None => None,
        };
        let content_probe = match &profile.match_rules.content_probe {
            Some(p) => Some(compile(p, ctx)?),
            None => None,
        };
        let filename = match &profile.match_rules.filename {
            Some(p) => Some(compile(p, ctx)?),
            None => None,
        };
        let strip_codes = match &profile.merchant.strip_codes {
            Some(p) => Some(compile(p, ctx)?),
            None => None,
        };
        let category_harvest = match profile
            .memo
            .as_ref()
            .and_then(|m| m.category_harvest.as_ref())
        {
            Some(p) => {
                let re = compile(p, ctx)?;
                if !re.capture_names().flatten().any(|n| n == "cat") {
                    return Err(ConfigError::Invalid {
                        context: ctx.clone(),
                        message: "memo.category_harvest must contain a named group `cat`".into(),
                    });
                }
                Some(re)
            }
            None => None,
        };
        let id_from_filename = match &profile.account.id_from_filename {
            Some(p) => Some(compile(p, ctx)?),
            None => None,
        };

        // Validate the sign mode is one phase-1 can honor at parse time.
        if !matches!(profile.amount.sign, Sign::Signed)
            && matches!(profile.amount.sign, Sign::SeparateDebitCredit)
            && (profile.amount.debit_column.is_none() || profile.amount.credit_column.is_none())
        {
            return Err(ConfigError::Invalid {
                context: ctx.clone(),
                message: "separate_debit_credit requires debit_column and credit_column".into(),
            });
        }

        Ok(Self {
            profile,
            header_first_cell,
            content_probe,
            filename,
            strip_codes,
            category_harvest,
            id_from_filename,
        })
    }
}

/// Read a config file's text from disk; `Ok(None)` when it does not exist.
fn read_text(path: &Path) -> Result<Option<String>, ConfigError> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(ConfigError::Read {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// Parse an optional TOML text, attributing parse errors to `label` (a path on
/// disk, or the bare file name for in-memory sources).
fn parse_opt<T: serde::de::DeserializeOwned>(
    text: Option<(String, PathBuf)>,
) -> Result<Option<T>, ConfigError> {
    match text {
        Some((s, label)) => toml::from_str(&s)
            .map(Some)
            .map_err(|source| ConfigError::Toml {
                path: label,
                source,
            }),
        None => Ok(None),
    }
}

fn parse_embedded<T: serde::de::DeserializeOwned>(
    text: &str,
    name: &str,
) -> Result<T, ConfigError> {
    toml::from_str(text).map_err(|source| ConfigError::Toml {
        path: PathBuf::from(format!("<embedded {name}>")),
        source,
    })
}

impl Config {
    /// Build a [`Config`] from already-parsed documents. Compiles & validates all
    /// regexes. Shared by [`Config::load`] and tests.
    pub fn build(
        profiles_doc: ProfilesDoc,
        categories: CategoriesDoc,
        people: PeopleDoc,
    ) -> Result<Self, ConfigError> {
        let profiles = profiles_doc
            .profiles
            .into_iter()
            .map(CompiledProfile::compile)
            .collect::<Result<Vec<_>, _>>()?;

        // Validate every category pattern compiles (case-insensitive). Category
        // patterns are authored for human readability and may use look-around
        // (e.g. `UBER(?! EATS)`), which the `regex` crate does not support; we
        // sanitize those constructs before validating so the categorize phase can
        // decide how to honor them, but still reject patterns that are malformed
        // for any other reason. See `sanitize_category_pattern`.
        for cat in &categories.categories {
            for sub in &cat.subcategories {
                for pat in sub
                    .patterns
                    .iter()
                    .chain(sub.ranged.iter().map(|r| &r.pattern))
                {
                    let sanitized = sanitize_category_pattern(pat);
                    let _ = compile_ci(&sanitized, &format!("category {}/{}", cat.name, sub.name))?;
                }
            }
        }

        let mut transfer_rules = people
            .transfer_rules
            .iter()
            .map(|rule| {
                let pattern = compile(&rule.pattern, &format!("transfer_rule -> {}", rule.to))?;
                Ok(CompiledTransferRule {
                    rule: rule.clone(),
                    pattern,
                })
            })
            .collect::<Result<Vec<_>, ConfigError>>()?;
        transfer_rules.sort_by_key(|r| std::cmp::Reverse(r.rule.priority));

        let mut account_matchers = Vec::new();
        for person in &people.persons {
            for account in &person.accounts {
                let filename = match &account.filename {
                    Some(p) => Some(compile(
                        p,
                        &format!("person {} account {}", person.id, account.id),
                    )?),
                    None => None,
                };
                account_matchers.push(CompiledAccountMatcher {
                    person_id: person.id.clone(),
                    account: account.clone(),
                    filename,
                });
            }
        }

        Ok(Self {
            profiles,
            categories,
            people,
            transfer_rules,
            account_matchers,
            events: Vec::new(),
            settings: AnalysisSettings::default(),
        })
    }

    /// The config file names [`Config::load`] / [`Config::from_texts`] consult.
    pub const FILE_NAMES: [&'static str; 6] = [
        "profiles.toml",
        "categories.toml",
        "people.toml",
        "events.toml",
        "settings.toml",
        "learned.toml",
    ];

    /// The shared layering logic behind [`Config::load`] and
    /// [`Config::from_texts`], so the two can never drift.
    ///
    /// `source(name)` returns the text of config file `name` (one of
    /// [`Config::FILE_NAMES`]) plus the label to attribute parse errors to, or
    /// `None` when the file is absent. Profiles are layered user > built-in (by
    /// id); absent people fall back to the embedded defaults; categories are layered user > learned > defaults; absent
    /// events/settings yield empty/consumer defaults.
    fn from_source<F>(mut source: F) -> Result<Self, ConfigError>
    where
        F: FnMut(&str) -> Result<Option<(String, PathBuf)>, ConfigError>,
    {
        // Profiles: the supplied file's formats first (so they win detection and
        // can override a built-in by reusing its id), then every built-in format
        // it does not override — known layouts are always recognised, and a
        // user's profiles.toml only needs their own additions.
        let builtin: ProfilesDoc = parse_embedded(DEFAULT_PROFILES, "profiles")?;
        let profiles_doc = match parse_opt::<ProfilesDoc>(source("profiles.toml")?)? {
            Some(user) => user.layered_over(builtin),
            None => builtin,
        };
        // People: prefer the supplied file, else embedded defaults.
        let people: PeopleDoc = match parse_opt(source("people.toml")?)? {
            Some(d) => d,
            None => parse_embedded(DEFAULT_PEOPLE, "people")?,
        };

        // Categories: defaults < learned < user.
        let defaults: CategoriesDoc = parse_embedded(DEFAULT_CATEGORIES, "categories")?;
        let learned: Option<CategoriesDoc> = parse_opt(source("learned.toml")?)?;
        let user: Option<CategoriesDoc> = parse_opt(source("categories.toml")?)?;
        let categories = defaults.merge(learned, user);

        // Optional life-event annotations (validated + date-sorted; empty if absent).
        let events = parse_opt::<EventsDoc>(source("events.toml")?)?
            .unwrap_or_default()
            .validated()?;

        // Optional analysis settings (persona profile + jurisdiction params).
        let settings = parse_opt::<SettingsDoc>(source("settings.toml")?)?
            .unwrap_or_default()
            .validated();

        let mut config = Self::build(profiles_doc, categories, people)?;
        config.events = events;
        config.settings = settings;
        Ok(config)
    }

    /// Load configuration from `dir`. `categories.toml` and `learned.toml` are
    /// layered onto the embedded defaults (user > learned > defaults). If a config
    /// file is absent in `dir`, the embedded default is used.
    pub fn load(dir: &Path) -> Result<Self, ConfigError> {
        Self::from_source(|name| {
            let path = dir.join(name);
            Ok(read_text(&path)?.map(|text| (text, path)))
        })
    }

    /// Build configuration from in-memory TOML texts keyed by file name (see
    /// [`Config::FILE_NAMES`]; other keys are ignored). Semantics are identical to
    /// [`Config::load`] with a directory holding exactly these files: a missing
    /// key behaves like a missing file. Parse errors name the offending key.
    pub fn from_texts(texts: &BTreeMap<String, String>) -> Result<Self, ConfigError> {
        Self::from_source(|name| {
            Ok(texts
                .get(name)
                .map(|text| (text.clone(), PathBuf::from(name))))
        })
    }

    /// Load a [`Config`] entirely from the embedded defaults (no filesystem).
    pub fn from_defaults() -> Result<Self, ConfigError> {
        let profiles_doc: ProfilesDoc = parse_embedded(DEFAULT_PROFILES, "profiles")?;
        let categories: CategoriesDoc = parse_embedded(DEFAULT_CATEGORIES, "categories")?;
        let people: PeopleDoc = parse_embedded(DEFAULT_PEOPLE, "people")?;
        Self::build(profiles_doc, categories, people)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_defaults_parse_and_compile() {
        let cfg = Config::from_defaults().expect("defaults must build");
        assert_eq!(cfg.profiles.len(), 5);
        assert!(cfg
            .profiles
            .iter()
            .any(|p| p.profile.id == "mastercard_credit"));
        assert!(cfg
            .profiles
            .iter()
            .any(|p| p.profile.id == "first_bank_card"));
        // Transfer rules are sorted by descending priority.
        assert!(cfg
            .transfer_rules
            .windows(2)
            .all(|w| w[0].rule.priority >= w[1].rule.priority));
        // People accounts flattened.
        assert!(cfg.account_matchers.iter().any(|m| m.account.external));
    }

    #[test]
    fn each_default_doc_roundtrips() {
        let p: ProfilesDoc = parse_embedded(DEFAULT_PROFILES, "profiles").unwrap();
        let back = toml::to_string(&p).unwrap();
        let reparsed: ProfilesDoc = toml::from_str(&back).unwrap();
        assert_eq!(p, reparsed);
    }

    /// `from_texts` and `load` share one layering path: the playground config
    /// read from disk and the same files supplied in memory build identical configs.
    #[test]
    fn from_texts_matches_load() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../playground/config");
        let mut texts = BTreeMap::new();
        for name in Config::FILE_NAMES {
            if let Ok(t) = std::fs::read_to_string(dir.join(name)) {
                texts.insert(name.to_string(), t);
            }
        }
        assert!(texts.contains_key("profiles.toml"));
        let a = Config::load(&dir).unwrap();
        let b = Config::from_texts(&texts).unwrap();
        assert_eq!(format!("{a:?}"), format!("{b:?}"));
    }

    #[test]
    fn from_texts_empty_equals_missing_dir() {
        let missing = std::env::temp_dir().join("wdmg-no-such-config-dir-xyz");
        let a = Config::load(&missing).unwrap();
        let b = Config::from_texts(&BTreeMap::new()).unwrap();
        assert_eq!(format!("{a:?}"), format!("{b:?}"));
    }

    #[test]
    fn from_texts_names_the_bad_file() {
        let mut texts = BTreeMap::new();
        texts.insert("people.toml".to_string(), "persons = [[[".to_string());
        let err = Config::from_texts(&texts).unwrap_err().to_string();
        assert!(err.contains("people.toml"), "{err}");
    }
}
