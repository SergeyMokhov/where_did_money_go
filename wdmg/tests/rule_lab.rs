//! `rule_lab` agrees with the full pipeline on the playground household.
use std::path::Path;

use wdmg::config::Config;

fn playground() -> (Config, Vec<(String, Vec<u8>)>) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../playground");
    let config = Config::load(&root.join("config")).unwrap();
    let mut files: Vec<(String, Vec<u8>)> = std::fs::read_dir(root.join("data"))
        .unwrap()
        .map(|e| {
            let p = e.unwrap().path();
            (
                p.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read(&p).unwrap(),
            )
        })
        .collect();
    files.sort();
    (config, files)
}

#[test]
fn explains_categories_and_transfers() {
    let (config, files) = playground();
    let rows = wdmg::pipeline::rule_lab(&config, &files).unwrap();
    let full = wdmg::pipeline::analyze_files(&config, &files).unwrap();
    let recon = &full.analysis.transfers;

    let count = |how: &str| {
        rows.iter()
            .filter(|r| r.transfer.as_ref().is_some_and(|t| t.how == how))
            .count()
    };
    assert_eq!(count("paired"), recon.matched_pairs.len() * 2);
    assert_eq!(count("rule"), recon.external.len());
    assert_eq!(count("unmatched"), recon.unresolved.len());

    // Every rule-resolved row names the rule (by file index) whose target it went to.
    for r in rows
        .iter()
        .filter(|r| r.transfer.as_ref().is_some_and(|t| t.how == "rule"))
    {
        let t = r.transfer.as_ref().unwrap();
        let i = t.rule.expect("rule index");
        assert_eq!(Some(&config.people.transfer_rules[i].to), t.other.as_ref());
    }
    // A categorized row says which pattern decided it, and it agrees.
    let hit = rows.iter().find(|r| r.hit.is_some()).unwrap();
    let h = hit.hit.as_ref().unwrap();
    assert_eq!(hit.category.as_ref(), Some(&h.category));
    assert_eq!(hit.subcategory.as_ref(), Some(&h.subcategory));
    assert!(regex::RegexBuilder::new(&h.pattern)
        .case_insensitive(true)
        .build()
        .unwrap()
        .is_match(&format!("{} {}", hit.merchant, hit.raw)));
}

#[test]
fn regex_set_prefilter_matches_the_plain_scan() {
    let (config, files) = playground();
    let (txns, _) = wdmg::ingest::load_files(&config, &files);
    let fast = wdmg::categorize::Categorizer::new(&config.categories);
    let slow = wdmg::categorize::Categorizer::without_prefilter(&config.categories);
    let mut moved = 0;
    for t in &txns {
        assert_eq!(
            fast.assign_explained(t),
            slow.assign_explained(t),
            "{} {}",
            t.normalized_merchant,
            t.amount.amount()
        );
        moved += usize::from(fast.explain(t).is_some());
    }
    assert!(
        moved > 1000,
        "the playground is mostly categorized ({moved})"
    );
}
