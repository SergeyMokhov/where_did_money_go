//! Verifies config written by `wdmg init` re-loads as a valid Config.
use std::path::Path;
use wdmg::config::Config;

#[test]
fn written_defaults_reload() {
    let dir = std::env::temp_dir().join("wdmg-init-load-test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    use wdmg::config::defaults;
    std::fs::write(dir.join("profiles.toml"), defaults::profiles()).unwrap();
    std::fs::write(dir.join("categories.toml"), defaults::categories()).unwrap();
    std::fs::write(dir.join("people.toml"), defaults::people()).unwrap();

    let cfg = Config::load(Path::new(&dir)).expect("written config must reload");
    assert_eq!(cfg.profiles.len(), 4);
    assert!(!cfg.transfer_rules.is_empty());
}
