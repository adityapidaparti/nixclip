use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::{NixClipError, Result};

// ---------------------------------------------------------------------------
// Retention
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Retention {
    #[serde(rename = "7days")]
    Days7,
    #[serde(rename = "30days")]
    Days30,
    #[default]
    #[serde(rename = "3months")]
    Months3,
    #[serde(rename = "6months")]
    Months6,
    #[serde(rename = "1year")]
    Year1,
    #[serde(rename = "unlimited")]
    Unlimited,
}

impl Retention {
    pub fn to_duration(&self) -> Option<chrono::Duration> {
        match self {
            Retention::Days7 => Some(chrono::Duration::days(7)),
            Retention::Days30 => Some(chrono::Duration::days(30)),
            Retention::Months3 => Some(chrono::Duration::days(90)),
            Retention::Months6 => Some(chrono::Duration::days(180)),
            Retention::Year1 => Some(chrono::Duration::days(365)),
            Retention::Unlimited => None,
        }
    }
}

impl fmt::Display for Retention {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Retention::Days7 => "7days",
            Retention::Days30 => "30days",
            Retention::Months3 => "3months",
            Retention::Months6 => "6months",
            Retention::Year1 => "1year",
            Retention::Unlimited => "unlimited",
        };
        f.write_str(s)
    }
}

// ---------------------------------------------------------------------------
// GeneralConfig
// ---------------------------------------------------------------------------

fn default_max_entries() -> u32 {
    1000
}

fn default_max_blob_size_mb() -> u32 {
    500
}

fn default_ephemeral_ttl_hours() -> u32 {
    24
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralConfig {
    #[serde(default = "default_max_entries")]
    pub max_entries: u32,

    #[serde(default = "default_max_blob_size_mb")]
    pub max_blob_size_mb: u32,

    #[serde(default)]
    pub retention: Retention,

    /// Time-to-live for ephemeral entries, in hours.  Ephemeral entries older
    /// than this are deleted during the periodic prune pass.  Defaults to 24.
    #[serde(default = "default_ephemeral_ttl_hours")]
    pub ephemeral_ttl_hours: u32,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            max_entries: default_max_entries(),
            max_blob_size_mb: default_max_blob_size_mb(),
            retention: Retention::default(),
            ephemeral_ttl_hours: default_ephemeral_ttl_hours(),
        }
    }
}

// ---------------------------------------------------------------------------
// UiConfig
// ---------------------------------------------------------------------------

fn default_theme() -> String {
    "auto".to_string()
}

fn default_width() -> u32 {
    680
}

fn default_max_visible_entries() -> u32 {
    8
}

fn default_show_source_app() -> bool {
    true
}

fn default_show_content_badges() -> bool {
    true
}

fn default_position() -> String {
    "top-center".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiConfig {
    #[serde(default = "default_theme")]
    pub theme: String,

    #[serde(default = "default_width")]
    pub width: u32,

    #[serde(default = "default_max_visible_entries")]
    pub max_visible_entries: u32,

    #[serde(default = "default_show_source_app")]
    pub show_source_app: bool,

    #[serde(default = "default_show_content_badges")]
    pub show_content_badges: bool,

    #[serde(default = "default_position")]
    pub position: String,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            theme: default_theme(),
            width: default_width(),
            max_visible_entries: default_max_visible_entries(),
            show_source_app: default_show_source_app(),
            show_content_badges: default_show_content_badges(),
            position: default_position(),
        }
    }
}

// ---------------------------------------------------------------------------
// KeybindConfig
// ---------------------------------------------------------------------------

fn default_toggle() -> String {
    "Super+Shift+V".to_string()
}

fn default_restore_original() -> String {
    "Return".to_string()
}

fn default_restore_plain() -> String {
    "Shift+Return".to_string()
}

fn default_delete() -> String {
    "Ctrl+BackSpace".to_string()
}

fn default_pin() -> String {
    "Ctrl+P".to_string()
}

fn default_clear_all() -> String {
    "Ctrl+Shift+Delete".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeybindConfig {
    #[serde(default = "default_toggle")]
    pub toggle: String,

    #[serde(default = "default_restore_original")]
    pub restore_original: String,

    #[serde(default = "default_restore_plain")]
    pub restore_plain: String,

    #[serde(default = "default_delete")]
    pub delete: String,

    #[serde(default = "default_pin")]
    pub pin: String,

    #[serde(default = "default_clear_all")]
    pub clear_all: String,
}

impl Default for KeybindConfig {
    fn default() -> Self {
        Self {
            toggle: default_toggle(),
            restore_original: default_restore_original(),
            restore_plain: default_restore_plain(),
            delete: default_delete(),
            pin: default_pin(),
            clear_all: default_clear_all(),
        }
    }
}

// ---------------------------------------------------------------------------
// IgnoreConfig
// ---------------------------------------------------------------------------

fn default_ignore_apps() -> Vec<String> {
    vec![
        "org.keepassxc.KeePassXC".to_string(),
        "com.1password.1Password".to_string(),
        "com.bitwarden.desktop".to_string(),
    ]
}

fn default_ignore_patterns() -> Vec<String> {
    vec![
        r"^sk-[a-zA-Z0-9]{48}".to_string(),
        r"^ghp_[a-zA-Z0-9]{36}".to_string(),
    ]
}

fn default_respect_sensitive_hints() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IgnoreConfig {
    #[serde(default = "default_ignore_apps")]
    pub apps: Vec<String>,

    #[serde(default = "default_ignore_patterns")]
    pub patterns: Vec<String>,

    #[serde(default = "default_respect_sensitive_hints")]
    pub respect_sensitive_hints: bool,
}

impl Default for IgnoreConfig {
    fn default() -> Self {
        Self {
            apps: default_ignore_apps(),
            patterns: default_ignore_patterns(),
            respect_sensitive_hints: default_respect_sensitive_hints(),
        }
    }
}

// ---------------------------------------------------------------------------
// Config (top level)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub general: GeneralConfig,

    #[serde(default)]
    pub ui: UiConfig,

    #[serde(default)]
    pub keybind: KeybindConfig,

    #[serde(default)]
    pub ignore: IgnoreConfig,
}

impl Config {
    /// Read TOML from `path` and deserialize, filling missing fields from defaults.
    pub fn load(path: impl AsRef<std::path::Path>) -> Result<Config> {
        let raw = std::fs::read_to_string(path.as_ref())
            .map_err(|e| NixClipError::Config(format!("cannot read config file: {e}")))?;
        let cfg: Config =
            toml::from_str(&raw).map_err(|e| NixClipError::Config(format!("invalid TOML: {e}")))?;
        Ok(cfg)
    }

    /// Try to load from the default config path; fall back to `Config::default()` on any error.
    pub fn load_or_default() -> Config {
        Self::load(Self::config_path()).unwrap_or_default()
    }

    /// Serialize to TOML and write atomically (write temp file, then rename).
    pub fn save(&self, path: impl AsRef<std::path::Path>) -> Result<()> {
        let path = path.as_ref();

        // Ensure the parent directory exists.
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| NixClipError::Config(format!("cannot create config dir: {e}")))?;
        }

        let content = toml::to_string_pretty(self)
            .map_err(|e| NixClipError::Config(format!("cannot serialize config: {e}")))?;

        // Write to a temp file next to the target, then rename for atomicity.
        let tmp_path = path.with_extension("toml.tmp");
        std::fs::write(&tmp_path, &content)
            .map_err(|e| NixClipError::Config(format!("cannot write temp config: {e}")))?;
        std::fs::rename(&tmp_path, path)
            .map_err(|e| NixClipError::Config(format!("cannot rename config file: {e}")))?;

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Directory helpers
    // -----------------------------------------------------------------------

    /// `$XDG_CONFIG_HOME/nixclip/` (falls back to `~/.config/nixclip/`).
    pub fn config_dir() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("~/.config"))
            .join("nixclip")
    }

    /// `config_dir()/config.toml`
    pub fn config_path() -> PathBuf {
        Self::config_dir().join("config.toml")
    }

    /// `$XDG_DATA_HOME/nixclip/` (falls back to `~/.local/share/nixclip/`).
    pub fn data_dir() -> PathBuf {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("~/.local/share"))
            .join("nixclip")
    }

    /// `data_dir()/nixclip.db`
    pub fn db_path() -> PathBuf {
        Self::data_dir().join("nixclip.db")
    }

    /// `data_dir()/blobs/`
    pub fn blob_dir() -> PathBuf {
        Self::data_dir().join("blobs")
    }

    /// `$XDG_RUNTIME_DIR` or `/tmp` as fallback.
    pub fn runtime_dir() -> PathBuf {
        dirs::runtime_dir().unwrap_or_else(|| PathBuf::from("/tmp"))
    }

    /// `runtime_dir()/nixclip.sock`
    pub fn socket_path() -> PathBuf {
        Self::runtime_dir().join("nixclip.sock")
    }
}
