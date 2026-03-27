/// Integration tests for Config: defaults, TOML parsing, Retention, save/load.
use nixclip_core::config::{
    Config, GeneralConfig, IgnoreConfig, KeybindConfig, Retention, UiConfig,
};

// ===========================================================================
// Retention::to_duration
// ===========================================================================

#[test]
fn retention_7days_is_7_days() {
    let d = Retention::Days7.to_duration().expect("should not be None");
    assert_eq!(d.num_days(), 7);
}

#[test]
fn retention_30days_is_30_days() {
    let d = Retention::Days30.to_duration().expect("should not be None");
    assert_eq!(d.num_days(), 30);
}

#[test]
fn retention_3months_is_90_days() {
    let d = Retention::Months3
        .to_duration()
        .expect("should not be None");
    assert_eq!(d.num_days(), 90);
}

#[test]
fn retention_6months_is_180_days() {
    let d = Retention::Months6
        .to_duration()
        .expect("should not be None");
    assert_eq!(d.num_days(), 180);
}

#[test]
fn retention_1year_is_365_days() {
    let d = Retention::Year1.to_duration().expect("should not be None");
    assert_eq!(d.num_days(), 365);
}

#[test]
fn retention_unlimited_is_none() {
    assert!(
        Retention::Unlimited.to_duration().is_none(),
        "Unlimited should return None duration"
    );
}

// ===========================================================================
// Config::default()
// ===========================================================================

#[test]
fn default_general_max_entries() {
    let cfg = Config::default();
    assert_eq!(cfg.general.max_entries, 1000);
}

#[test]
fn default_general_max_blob_size_mb() {
    let cfg = Config::default();
    assert_eq!(cfg.general.max_blob_size_mb, 500);
}

#[test]
fn default_general_retention_is_3_months() {
    let cfg = Config::default();
    assert_eq!(cfg.general.retention, Retention::Months3);
}

#[test]
fn default_ui_theme_is_auto() {
    let cfg = Config::default();
    assert_eq!(cfg.ui.theme, "auto");
}

#[test]
fn default_ui_width() {
    let cfg = Config::default();
    assert_eq!(cfg.ui.width, 680);
}

#[test]
fn default_ui_max_visible_entries() {
    let cfg = Config::default();
    assert_eq!(cfg.ui.max_visible_entries, 8);
}

#[test]
fn default_ui_show_source_app_is_true() {
    let cfg = Config::default();
    assert!(cfg.ui.show_source_app);
}

#[test]
fn default_ui_show_content_badges_is_true() {
    let cfg = Config::default();
    assert!(cfg.ui.show_content_badges);
}

#[test]
fn default_ui_position_is_top_center() {
    let cfg = Config::default();
    assert_eq!(cfg.ui.position, "top-center");
}

#[test]
fn default_keybind_open_formatted() {
    let cfg = Config::default();
    assert_eq!(cfg.keybind.open_formatted, "Super+V");
}

#[test]
fn default_keybind_open_plain() {
    let cfg = Config::default();
    assert_eq!(cfg.keybind.open_plain, "Super+Shift+V");
}

#[test]
fn default_keybind_restore_original() {
    let cfg = Config::default();
    assert_eq!(cfg.keybind.restore_original, "Return");
}

#[test]
fn default_keybind_restore_plain() {
    let cfg = Config::default();
    assert_eq!(cfg.keybind.restore_plain, "Shift+Return");
}

#[test]
fn default_keybind_delete() {
    let cfg = Config::default();
    assert_eq!(cfg.keybind.delete, "Ctrl+BackSpace");
}

#[test]
fn default_keybind_pin() {
    let cfg = Config::default();
    assert_eq!(cfg.keybind.pin, "Ctrl+P");
}

#[test]
fn default_keybind_clear_all() {
    let cfg = Config::default();
    assert_eq!(cfg.keybind.clear_all, "Ctrl+Shift+Delete");
}

#[test]
fn default_ignore_apps_include_keepassxc_and_1password() {
    let cfg = Config::default();
    let apps = &cfg.ignore.apps;
    assert!(
        apps.iter()
            .any(|a| a.contains("keepassxc") || a.contains("KeePassXC")),
        "default apps should include KeePassXC: {apps:?}"
    );
    assert!(
        apps.iter()
            .any(|a| a.contains("1password") || a.contains("1Password")),
        "default apps should include 1Password: {apps:?}"
    );
}

#[test]
fn default_ignore_patterns_include_sk_and_ghp() {
    let cfg = Config::default();
    let patterns = &cfg.ignore.patterns;
    assert!(
        patterns.iter().any(|p| p.starts_with(r"^sk-")),
        "default patterns should include sk- (OpenAI key): {patterns:?}"
    );
    assert!(
        patterns.iter().any(|p| p.starts_with(r"^ghp_")),
        "default patterns should include ghp_ (GitHub token): {patterns:?}"
    );
}

#[test]
fn default_ignore_respect_sensitive_hints_is_true() {
    let cfg = Config::default();
    assert!(cfg.ignore.respect_sensitive_hints);
}

// ===========================================================================
// TOML parsing
// ===========================================================================

fn parse(toml: &str) -> Config {
    toml::from_str(toml).expect("parse TOML")
}

#[test]
fn empty_toml_gives_all_defaults() {
    let cfg = parse("");
    assert_eq!(cfg.general.max_entries, 1000);
    assert_eq!(cfg.ui.theme, "auto");
    assert_eq!(cfg.keybind.open_formatted, "Super+V");
    assert_eq!(cfg.keybind.open_plain, "Super+Shift+V");
    assert!(cfg.ignore.respect_sensitive_hints);
}

#[test]
fn partial_general_section_fills_missing_with_defaults() {
    let cfg = parse(
        r#"
[general]
max_entries = 500
"#,
    );
    assert_eq!(cfg.general.max_entries, 500);
    // Defaults for other fields.
    assert_eq!(cfg.general.max_blob_size_mb, 500);
    assert_eq!(cfg.general.retention, Retention::Months3);
}

#[test]
fn all_retention_values_parse_from_toml() {
    for (toml_val, expected) in [
        ("\"7days\"", Retention::Days7),
        ("\"30days\"", Retention::Days30),
        ("\"3months\"", Retention::Months3),
        ("\"6months\"", Retention::Months6),
        ("\"1year\"", Retention::Year1),
        ("\"unlimited\"", Retention::Unlimited),
    ] {
        let toml = format!("[general]\nretention = {toml_val}");
        let cfg: Config = toml::from_str(&toml).expect("parse retention");
        assert_eq!(cfg.general.retention, expected, "failed for {toml_val}");
    }
}

#[test]
fn ui_section_parses_custom_values() {
    let cfg = parse(
        r#"
[ui]
theme = "dark"
width = 800
max_visible_entries = 12
show_source_app = false
show_content_badges = false
position = "bottom-left"
"#,
    );
    assert_eq!(cfg.ui.theme, "dark");
    assert_eq!(cfg.ui.width, 800);
    assert_eq!(cfg.ui.max_visible_entries, 12);
    assert!(!cfg.ui.show_source_app);
    assert!(!cfg.ui.show_content_badges);
    assert_eq!(cfg.ui.position, "bottom-left");
}

#[test]
fn keybind_section_parses_custom_values() {
    let cfg = parse(
        r#"
[keybind]
open_formatted = "Ctrl+Alt+V"
pin = "Ctrl+G"
"#,
    );
    assert_eq!(cfg.keybind.open_formatted, "Ctrl+Alt+V");
    assert_eq!(cfg.keybind.pin, "Ctrl+G");
    // Unprovided keys stay as default.
    assert_eq!(cfg.keybind.restore_original, "Return");
}

#[test]
fn ignore_section_parses_custom_apps_and_patterns() {
    let cfg = parse(
        r#"
[ignore]
apps = ["com.custom.PasswordManager"]
patterns = ["^TOKEN_[a-z]{20}"]
respect_sensitive_hints = false
"#,
    );
    assert_eq!(cfg.ignore.apps, vec!["com.custom.PasswordManager"]);
    assert_eq!(cfg.ignore.patterns, vec!["^TOKEN_[a-z]{20}"]);
    assert!(!cfg.ignore.respect_sensitive_hints);
}

#[test]
fn ignore_empty_arrays_are_valid() {
    let cfg = parse(
        r#"
[ignore]
apps = []
patterns = []
"#,
    );
    assert!(cfg.ignore.apps.is_empty());
    assert!(cfg.ignore.patterns.is_empty());
}

#[test]
fn full_config_round_trip_via_toml_string() {
    let cfg = Config {
        general: GeneralConfig {
            max_entries: 250,
            max_blob_size_mb: 100,
            retention: Retention::Days7,
            ephemeral_ttl_hours: 24,
        },
        ui: UiConfig {
            theme: "dark".to_string(),
            width: 720,
            max_visible_entries: 6,
            show_source_app: false,
            show_content_badges: true,
            position: "bottom-right".to_string(),
        },
        keybind: KeybindConfig {
            open_formatted: "Super+V".to_string(),
            open_plain: "Super+Shift+V".to_string(),
            restore_original: "Return".to_string(),
            restore_plain: "Shift+Return".to_string(),
            delete: "Delete".to_string(),
            pin: "Ctrl+P".to_string(),
            clear_all: "Ctrl+Shift+Delete".to_string(),
        },
        ignore: IgnoreConfig {
            apps: vec!["com.example.App".to_string()],
            patterns: vec![r"^secret_".to_string()],
            respect_sensitive_hints: true,
        },
    };

    let toml_str = toml::to_string_pretty(&cfg).expect("serialize");
    let loaded: Config = toml::from_str(&toml_str).expect("deserialize");

    assert_eq!(loaded.general.max_entries, 250);
    assert_eq!(loaded.general.retention, Retention::Days7);
    assert_eq!(loaded.ui.theme, "dark");
    assert_eq!(loaded.ui.width, 720);
    assert_eq!(loaded.keybind.open_formatted, "Super+V");
    assert_eq!(loaded.keybind.open_plain, "Super+Shift+V");
    assert_eq!(loaded.ignore.apps, vec!["com.example.App"]);
    assert_eq!(loaded.ignore.patterns, vec![r"^secret_"]);
}

// ===========================================================================
// Config::save / Config::load (file round-trip)
// ===========================================================================

#[test]
fn save_and_load_round_trip() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");

    let cfg = Config {
        general: GeneralConfig {
            max_entries: 42,
            max_blob_size_mb: 10,
            retention: Retention::Year1,
            ephemeral_ttl_hours: 24,
        },
        ui: UiConfig {
            theme: "light".to_string(),
            width: 500,
            max_visible_entries: 4,
            show_source_app: false,
            show_content_badges: false,
            position: "bottom-center".to_string(),
        },
        keybind: KeybindConfig::default(),
        ignore: IgnoreConfig::default(),
    };

    cfg.save(&path).expect("save");
    assert!(path.exists(), "config file should exist after save");

    let loaded = Config::load(&path).expect("load");
    assert_eq!(loaded.general.max_entries, 42);
    assert_eq!(loaded.general.max_blob_size_mb, 10);
    assert_eq!(loaded.general.retention, Retention::Year1);
    assert_eq!(loaded.ui.theme, "light");
    assert_eq!(loaded.ui.width, 500);
    assert!(!loaded.ui.show_source_app);
}

#[test]
fn save_creates_parent_directory() {
    let dir = tempfile::tempdir().expect("tempdir");
    // Nested path — the subdirectory does not exist yet.
    let path = dir.path().join("subdir").join("nested").join("config.toml");
    let cfg = Config::default();
    cfg.save(&path).expect("save with nested path");
    assert!(path.exists());
}

#[test]
fn load_nonexistent_file_returns_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let missing = dir.path().join("missing.toml");
    assert!(Config::load(&missing).is_err());
}

#[test]
fn load_invalid_toml_returns_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("bad.toml");
    std::fs::write(&path, b"this is not valid toml ][[[").expect("write bad file");
    assert!(Config::load(&path).is_err());
}

#[test]
fn load_empty_file_gives_all_defaults() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("empty.toml");
    std::fs::write(&path, b"").expect("write empty file");

    let cfg = Config::load(&path).expect("load empty TOML");
    assert_eq!(cfg.general.max_entries, 1000);
    assert_eq!(cfg.ui.theme, "auto");
}

#[test]
fn load_or_default_returns_default_on_missing_file() {
    // Point the function at a nonexistent path by using a temp dir that we
    // don't write anything into.  load_or_default() reads Config::config_path()
    // internally, but we can verify the fallback by calling save+load manually
    // with a nonexistent path and checking that load_or_default() at least
    // doesn't panic and returns a valid Config.
    //
    // We can't easily redirect the default path in tests, so we test the
    // fallback indirectly: calling load() on a missing path returns Err, while
    // the public API load_or_default() should never panic.
    let cfg = Config::load_or_default();
    // Must produce a usable Config regardless.
    assert!(cfg.general.max_entries > 0);
}

// ===========================================================================
// Directory helpers
// ===========================================================================

#[test]
fn config_path_ends_with_config_toml() {
    let p = Config::config_path();
    assert_eq!(p.file_name().and_then(|n| n.to_str()), Some("config.toml"));
}

#[test]
fn db_path_ends_with_nixclip_db() {
    let p = Config::db_path();
    assert_eq!(p.file_name().and_then(|n| n.to_str()), Some("nixclip.db"));
}

#[test]
fn blob_dir_ends_with_blobs() {
    let p = Config::blob_dir();
    assert_eq!(p.file_name().and_then(|n| n.to_str()), Some("blobs"));
}

#[test]
fn socket_path_ends_with_nixclip_sock() {
    let p = Config::socket_path();
    assert_eq!(p.file_name().and_then(|n| n.to_str()), Some("nixclip.sock"));
}
