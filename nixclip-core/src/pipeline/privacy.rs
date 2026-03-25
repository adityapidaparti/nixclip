//! Privacy filters — determine whether clipboard content should be stored.

use tracing::debug;

use crate::config::IgnoreConfig;
use crate::error::{NixClipError, Result};

// ---------------------------------------------------------------------------
// FilterResult
// ---------------------------------------------------------------------------

/// The outcome of a [`PrivacyFilter::check`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterResult {
    /// Content is safe to persist normally.
    Allow,
    /// Content must not be stored at all.
    Reject,
    /// Content may be surfaced in the current session but must not be
    /// persisted beyond it.
    Ephemeral,
}

// ---------------------------------------------------------------------------
// Sensitive MIME constants
// ---------------------------------------------------------------------------

/// MIME types (exact match) that indicate sensitive clipboard content.
const SENSITIVE_MIMES_EXACT: &[&str] = &[
    "x-kde-passwordManagerHint",
    "org.nspasteboard.ConcealedType",
    "org.nspasteboard.TransientType",
];

// ---------------------------------------------------------------------------
// PrivacyFilter
// ---------------------------------------------------------------------------

/// Decides whether a clipboard event should be stored, stored ephemerally,
/// or rejected entirely.
pub struct PrivacyFilter {
    /// Application identifiers whose clipboard content should always be
    /// ignored. Compared case-insensitively via substring match.
    ignore_apps: Vec<String>,
    /// Pre-compiled regex patterns. If the preview text matches any of these,
    /// the entry is stored as [`FilterResult::Ephemeral`].
    compiled_patterns: Vec<regex::Regex>,
    /// When `true`, MIME-type hints that indicate sensitive data cause a
    /// [`FilterResult::Reject`].
    respect_sensitive_hints: bool,
}

impl PrivacyFilter {
    /// Build a [`PrivacyFilter`] from the `[ignore]` section of the config.
    ///
    /// Returns an error if any regex pattern is invalid.
    pub fn new(config: &IgnoreConfig) -> Result<Self> {
        let compiled_patterns = config
            .patterns
            .iter()
            .enumerate()
            .map(|(idx, pat)| {
                regex::Regex::new(pat).map_err(|e| {
                    NixClipError::Config(format!(
                        "invalid regex pattern at index {idx} ({pat:?}): {e}"
                    ))
                })
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(Self {
            ignore_apps: config.apps.clone(),
            compiled_patterns,
            respect_sensitive_hints: config.respect_sensitive_hints,
        })
    }

    /// Phase-1 check: evaluates source-app and MIME-type rules only.
    ///
    /// This is intended to run **before** content processing so that obviously
    /// rejected events avoid the cost of classification / hashing / thumbnailing.
    /// Only app-name and MIME-type checks are performed here; content-pattern
    /// checks require the preview text produced by the content processor and are
    /// deferred to [`check_content_patterns`].
    pub fn check_pre_content(
        &self,
        source_app: Option<&str>,
        mimes: &[String],
    ) -> FilterResult {
        // 1. Check source application.
        if let Some(app) = source_app {
            if self.should_ignore_app(app) {
                debug!(app, "clipboard rejected: ignored application");
                return FilterResult::Reject;
            }
        }

        // 2. Check for sensitive MIME hints.
        if self.respect_sensitive_hints && self.has_sensitive_mimes(mimes) {
            debug!("clipboard rejected: sensitive MIME hint detected");
            return FilterResult::Reject;
        }

        FilterResult::Allow
    }

    /// Phase-2 check: evaluates content-based regex patterns against the
    /// preview text produced by the content processor.
    ///
    /// Returns [`FilterResult::Ephemeral`] if any pattern matches, otherwise
    /// [`FilterResult::Allow`].
    pub fn check_content_patterns(&self, preview_text: Option<&str>) -> FilterResult {
        if let Some(text) = preview_text {
            if self.matches_sensitive_pattern(text) {
                debug!("clipboard marked ephemeral: matched sensitive pattern");
                return FilterResult::Ephemeral;
            }
        }
        FilterResult::Allow
    }

    /// Evaluate a clipboard event and return the appropriate [`FilterResult`].
    ///
    /// Checks are performed in this order:
    /// 1. Source application → [`FilterResult::Reject`]
    /// 2. Sensitive MIME hints → [`FilterResult::Reject`]
    /// 3. Preview text matches a pattern → [`FilterResult::Ephemeral`]
    /// 4. Otherwise → [`FilterResult::Allow`]
    pub fn check(
        &self,
        source_app: Option<&str>,
        mimes: &[String],
        preview_text: Option<&str>,
    ) -> FilterResult {
        // 1. Check source application.
        if let Some(app) = source_app {
            if self.should_ignore_app(app) {
                debug!(app, "clipboard rejected: ignored application");
                return FilterResult::Reject;
            }
        }

        // 2. Check for sensitive MIME hints.
        if self.respect_sensitive_hints && self.has_sensitive_mimes(mimes) {
            debug!("clipboard rejected: sensitive MIME hint detected");
            return FilterResult::Reject;
        }

        // 3. Check preview text against compiled patterns.
        if let Some(text) = preview_text {
            if self.matches_sensitive_pattern(text) {
                debug!("clipboard marked ephemeral: matched sensitive pattern");
                return FilterResult::Ephemeral;
            }
        }

        FilterResult::Allow
    }

    // -----------------------------------------------------------------------
    // Public helpers (also used in tests and by callers that need granular
    // checks without going through `check`).
    // -----------------------------------------------------------------------

    /// Return `true` if the application identifier should be ignored.
    ///
    /// The match is case-insensitive substring containment — e.g., both
    /// `"com.1password.1Password"` and `"1password"` match the stored entry
    /// `"com.1password.1Password"`.
    pub fn should_ignore_app(&self, app_id: &str) -> bool {
        let app_lower = app_id.to_lowercase();
        self.ignore_apps
            .iter()
            .any(|ignored| app_lower.contains(&ignored.to_lowercase()))
    }

    /// Return `true` when any offered MIME type indicates sensitive content.
    ///
    /// Sensitivity checks:
    /// - Exact match against [`SENSITIVE_MIMES_EXACT`].
    /// - Case-insensitive substring match for the word `"password"`.
    pub fn has_sensitive_mimes(&self, mimes: &[String]) -> bool {
        mimes.iter().any(|m| {
            // Exact match
            SENSITIVE_MIMES_EXACT.contains(&m.as_str())
                // Substring match for "password" (case-insensitive)
                || m.to_lowercase().contains("password")
        })
    }

    /// Return `true` if `text` matches any of the compiled regex patterns.
    pub fn matches_sensitive_pattern(&self, text: &str) -> bool {
        self.compiled_patterns.iter().any(|re| re.is_match(text))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn default_filter() -> PrivacyFilter {
        PrivacyFilter::new(&IgnoreConfig::default()).unwrap()
    }

    fn filter_with_apps(apps: Vec<&str>) -> PrivacyFilter {
        let config = IgnoreConfig {
            apps: apps.into_iter().map(String::from).collect(),
            patterns: vec![],
            respect_sensitive_hints: true,
        };
        PrivacyFilter::new(&config).unwrap()
    }

    fn filter_with_patterns(patterns: Vec<&str>) -> PrivacyFilter {
        let config = IgnoreConfig {
            apps: vec![],
            patterns: patterns.into_iter().map(String::from).collect(),
            respect_sensitive_hints: true,
        };
        PrivacyFilter::new(&config).unwrap()
    }

    fn mimes(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    // --- new() ---

    #[test]
    fn invalid_regex_returns_error() {
        let config = IgnoreConfig {
            apps: vec![],
            patterns: vec!["[invalid".to_string()],
            respect_sensitive_hints: true,
        };
        assert!(PrivacyFilter::new(&config).is_err());
    }

    #[test]
    fn valid_config_constructs_ok() {
        assert!(PrivacyFilter::new(&IgnoreConfig::default()).is_ok());
    }

    // --- should_ignore_app ---

    #[test]
    fn ignore_app_exact() {
        let f = filter_with_apps(vec!["com.1password.1Password"]);
        assert!(f.should_ignore_app("com.1password.1Password"));
    }

    #[test]
    fn ignore_app_case_insensitive() {
        let f = filter_with_apps(vec!["KeePassXC"]);
        assert!(f.should_ignore_app("org.keepassxc.keepassxc"));
    }

    #[test]
    fn ignore_app_substring() {
        let f = filter_with_apps(vec!["1password"]);
        assert!(f.should_ignore_app("com.1password.1Password"));
    }

    #[test]
    fn ignore_app_no_match() {
        let f = filter_with_apps(vec!["bitwarden"]);
        assert!(!f.should_ignore_app("org.mozilla.firefox"));
    }

    // --- has_sensitive_mimes ---

    #[test]
    fn sensitive_kde_hint() {
        let f = default_filter();
        assert!(f.has_sensitive_mimes(&mimes(&["x-kde-passwordManagerHint"])));
    }

    #[test]
    fn sensitive_nspasteboard_concealed() {
        let f = default_filter();
        assert!(f.has_sensitive_mimes(&mimes(&["org.nspasteboard.ConcealedType"])));
    }

    #[test]
    fn sensitive_nspasteboard_transient() {
        let f = default_filter();
        assert!(f.has_sensitive_mimes(&mimes(&["org.nspasteboard.TransientType"])));
    }

    #[test]
    fn sensitive_password_substring() {
        let f = default_filter();
        assert!(f.has_sensitive_mimes(&mimes(&["application/x-password-data"])));
    }

    #[test]
    fn sensitive_password_case_insensitive() {
        let f = default_filter();
        assert!(f.has_sensitive_mimes(&mimes(&["application/Password"])));
    }

    #[test]
    fn not_sensitive_plain_text() {
        let f = default_filter();
        assert!(!f.has_sensitive_mimes(&mimes(&["text/plain", "text/html"])));
    }

    // --- matches_sensitive_pattern ---

    #[test]
    fn pattern_matches_openai_key() {
        let f = filter_with_patterns(vec![r"^sk-[a-zA-Z0-9]{48}"]);
        let key = format!("sk-{}", "A".repeat(48));
        assert!(f.matches_sensitive_pattern(&key));
    }

    #[test]
    fn pattern_no_match() {
        let f = filter_with_patterns(vec![r"^sk-[a-zA-Z0-9]{48}"]);
        assert!(!f.matches_sensitive_pattern("just some text"));
    }

    // --- check ordering ---

    #[test]
    fn check_allows_normal_content() {
        let f = default_filter();
        let result = f.check(
            Some("org.mozilla.firefox"),
            &mimes(&["text/plain"]),
            Some("hello world"),
        );
        assert_eq!(result, FilterResult::Allow);
    }

    #[test]
    fn check_rejects_ignored_app_before_mimes() {
        let f = filter_with_apps(vec!["keepassxc"]);
        // Even with no sensitive MIMEs, ignored app triggers Reject first.
        let result = f.check(
            Some("org.keepassxc.KeePassXC"),
            &mimes(&["text/plain"]),
            Some("super secret"),
        );
        assert_eq!(result, FilterResult::Reject);
    }

    #[test]
    fn check_rejects_on_sensitive_mime() {
        let f = default_filter();
        let result = f.check(
            Some("org.mozilla.firefox"),
            &mimes(&["text/plain", "x-kde-passwordManagerHint"]),
            Some("some text"),
        );
        assert_eq!(result, FilterResult::Reject);
    }

    #[test]
    fn check_ephemeral_on_pattern_match() {
        let f = filter_with_patterns(vec![r"^ghp_[a-zA-Z0-9]{36}"]);
        let token = format!("ghp_{}", "B".repeat(36));
        let result = f.check(
            Some("org.mozilla.firefox"),
            &mimes(&["text/plain"]),
            Some(&token),
        );
        assert_eq!(result, FilterResult::Ephemeral);
    }

    #[test]
    fn check_no_source_app_still_works() {
        let f = default_filter();
        let result = f.check(None, &mimes(&["text/plain"]), Some("normal text"));
        assert_eq!(result, FilterResult::Allow);
    }

    #[test]
    fn check_sensitive_hints_skipped_when_disabled() {
        let config = IgnoreConfig {
            apps: vec![],
            patterns: vec![],
            respect_sensitive_hints: false,
        };
        let f = PrivacyFilter::new(&config).unwrap();
        // Would normally be Reject, but respect_sensitive_hints is off.
        let result = f.check(
            None,
            &mimes(&["x-kde-passwordManagerHint"]),
            None,
        );
        assert_eq!(result, FilterResult::Allow);
    }

    #[test]
    fn check_app_rejection_takes_priority_over_pattern() {
        let config = IgnoreConfig {
            apps: vec!["keepassxc".to_string()],
            patterns: vec![r"super secret".to_string()],
            respect_sensitive_hints: true,
        };
        let f = PrivacyFilter::new(&config).unwrap();
        // App match should give Reject, not Ephemeral.
        let result = f.check(
            Some("org.keepassxc.KeePassXC"),
            &mimes(&["text/plain"]),
            Some("super secret"),
        );
        assert_eq!(result, FilterResult::Reject);
    }

    // --- check_pre_content (phase 1) ---

    #[test]
    fn pre_content_allows_normal_event() {
        let f = default_filter();
        let result = f.check_pre_content(
            Some("org.mozilla.firefox"),
            &mimes(&["text/plain"]),
        );
        assert_eq!(result, FilterResult::Allow);
    }

    #[test]
    fn pre_content_rejects_ignored_app() {
        let f = filter_with_apps(vec!["keepassxc"]);
        let result = f.check_pre_content(
            Some("org.keepassxc.KeePassXC"),
            &mimes(&["text/plain"]),
        );
        assert_eq!(result, FilterResult::Reject);
    }

    #[test]
    fn pre_content_rejects_sensitive_mime() {
        let f = default_filter();
        let result = f.check_pre_content(
            None,
            &mimes(&["text/plain", "x-kde-passwordManagerHint"]),
        );
        assert_eq!(result, FilterResult::Reject);
    }

    #[test]
    fn pre_content_does_not_check_patterns() {
        // Even with patterns configured, pre_content should not evaluate them.
        let f = filter_with_patterns(vec![r"^sk-[a-zA-Z0-9]{48}"]);
        let result = f.check_pre_content(
            Some("org.mozilla.firefox"),
            &mimes(&["text/plain"]),
        );
        // Should allow — pattern matching is deferred to phase 2.
        assert_eq!(result, FilterResult::Allow);
    }

    // --- check_content_patterns (phase 2) ---

    #[test]
    fn content_patterns_allows_normal_text() {
        let f = filter_with_patterns(vec![r"^sk-[a-zA-Z0-9]{48}"]);
        let result = f.check_content_patterns(Some("hello world"));
        assert_eq!(result, FilterResult::Allow);
    }

    #[test]
    fn content_patterns_marks_ephemeral_on_match() {
        let f = filter_with_patterns(vec![r"^ghp_[a-zA-Z0-9]{36}"]);
        let token = format!("ghp_{}", "B".repeat(36));
        let result = f.check_content_patterns(Some(&token));
        assert_eq!(result, FilterResult::Ephemeral);
    }

    #[test]
    fn content_patterns_allows_when_no_preview() {
        let f = filter_with_patterns(vec![r"^sk-[a-zA-Z0-9]{48}"]);
        let result = f.check_content_patterns(None);
        assert_eq!(result, FilterResult::Allow);
    }

    #[test]
    fn content_patterns_allows_when_no_patterns_configured() {
        let config = IgnoreConfig {
            apps: vec![],
            patterns: vec![],
            respect_sensitive_hints: true,
        };
        let _f = PrivacyFilter::new(&config).unwrap();
        let key = format!("sk-{}", "A".repeat(48));
        let result = f.check_content_patterns(Some(&key));
        assert_eq!(result, FilterResult::Allow);
    }
}
