//! Content classification from MIME types.

use crate::ContentClass;

/// Classify clipboard content based solely on the offered MIME types.
///
/// Priority (first match wins):
/// 1. `image/png` or `image/jpeg`                          → [`ContentClass::Image`]
/// 2. `text/uri-list` or `x-special/gnome-copied-files`   → [`ContentClass::Files`]
/// 3. `text/html` AND `text/plain` both present            → [`ContentClass::RichText`]
/// 4. `text/plain` only                                    → [`ContentClass::Text`]
/// 5. Otherwise                                            → `None`
pub fn classify(offered_mimes: &[String]) -> Option<ContentClass> {
    // Priority 1: Image
    if offered_mimes
        .iter()
        .any(|m| m == "image/png" || m == "image/jpeg")
    {
        return Some(ContentClass::Image);
    }

    // Priority 2: Files
    if offered_mimes
        .iter()
        .any(|m| m == "text/uri-list" || m == "x-special/gnome-copied-files")
    {
        return Some(ContentClass::Files);
    }

    // Priority 3: RichText (both html and plain must be present)
    let has_html = offered_mimes.iter().any(|m| m == "text/html");
    let has_plain = offered_mimes.iter().any(|m| m == "text/plain");

    if has_html && has_plain {
        return Some(ContentClass::RichText);
    }

    // Priority 4: Plain text
    if has_plain {
        return Some(ContentClass::Text);
    }

    None
}

/// Classify clipboard content using both MIME types and text content.
///
/// Identical to [`classify`] except that step 4 promotes `Text` to
/// [`ContentClass::Url`] when the text content looks like a URL.
///
/// URL heuristic:
/// - starts with `http://` or `https://`, **or**
/// - matches a simple `word.word` domain pattern (with an optional path).
pub fn classify_with_content(
    offered_mimes: &[String],
    text_content: Option<&str>,
) -> Option<ContentClass> {
    // Priority 1: Image
    if offered_mimes
        .iter()
        .any(|m| m == "image/png" || m == "image/jpeg")
    {
        return Some(ContentClass::Image);
    }

    // Priority 2: Files
    if offered_mimes
        .iter()
        .any(|m| m == "text/uri-list" || m == "x-special/gnome-copied-files")
    {
        return Some(ContentClass::Files);
    }

    // Priority 3: RichText
    let has_html = offered_mimes.iter().any(|m| m == "text/html");
    let has_plain = offered_mimes.iter().any(|m| m == "text/plain");

    if has_html && has_plain {
        return Some(ContentClass::RichText);
    }

    // Priority 4: Text — with optional URL promotion
    if has_plain {
        if let Some(text) = text_content {
            if looks_like_url(text.trim()) {
                return Some(ContentClass::Url);
            }
        }
        return Some(ContentClass::Text);
    }

    None
}

/// Heuristic URL check used by [`classify_with_content`].
///
/// Returns `true` when the trimmed text:
/// - starts with `http://` or `https://`, or
/// - matches a simple `<word>.<word>` domain pattern (optionally followed by
///   a `/` and more path characters), with no internal whitespace.
fn looks_like_url(text: &str) -> bool {
    // Must be a single token — no whitespace allowed.
    if text.contains(char::is_whitespace) {
        return false;
    }

    if text.starts_with("http://") || text.starts_with("https://") {
        return true;
    }

    // Simple domain heuristic: at least one dot, each segment is non-empty
    // and contains only URL-safe characters.
    // e.g. "example.com", "example.com/path", "sub.example.co.uk/foo"
    let host_part = text.split('/').next().unwrap_or(text);
    let segments: Vec<&str> = host_part.split('.').collect();
    if segments.len() >= 2 {
        let all_valid = segments.iter().all(|seg| {
            !seg.is_empty()
                && seg
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
        });
        if all_valid {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mimes(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    // --- classify -----------------------------------------------------------

    #[test]
    fn image_png_wins() {
        assert_eq!(
            classify(&mimes(&["image/png", "text/plain"])),
            Some(ContentClass::Image)
        );
    }

    #[test]
    fn image_jpeg_wins() {
        assert_eq!(classify(&mimes(&["image/jpeg"])), Some(ContentClass::Image));
    }

    #[test]
    fn files_uri_list() {
        assert_eq!(
            classify(&mimes(&["text/uri-list", "text/plain"])),
            Some(ContentClass::Files)
        );
    }

    #[test]
    fn files_gnome() {
        assert_eq!(
            classify(&mimes(&["x-special/gnome-copied-files"])),
            Some(ContentClass::Files)
        );
    }

    #[test]
    fn richtext_both_present() {
        assert_eq!(
            classify(&mimes(&["text/html", "text/plain"])),
            Some(ContentClass::RichText)
        );
    }

    #[test]
    fn html_only_is_none() {
        assert_eq!(classify(&mimes(&["text/html"])), None);
    }

    #[test]
    fn plain_only_is_text() {
        assert_eq!(classify(&mimes(&["text/plain"])), Some(ContentClass::Text));
    }

    #[test]
    fn empty_is_none() {
        assert_eq!(classify(&[]), None);
    }

    // --- classify_with_content ----------------------------------------------

    #[test]
    fn url_promotion_http() {
        assert_eq!(
            classify_with_content(&mimes(&["text/plain"]), Some("https://example.com/path")),
            Some(ContentClass::Url)
        );
    }

    #[test]
    fn url_promotion_domain() {
        assert_eq!(
            classify_with_content(&mimes(&["text/plain"]), Some("example.com/path")),
            Some(ContentClass::Url)
        );
    }

    #[test]
    fn no_url_promotion_whitespace() {
        assert_eq!(
            classify_with_content(&mimes(&["text/plain"]), Some("hello world")),
            Some(ContentClass::Text)
        );
    }

    #[test]
    fn no_url_promotion_no_content() {
        assert_eq!(
            classify_with_content(&mimes(&["text/plain"]), None),
            Some(ContentClass::Text)
        );
    }

    #[test]
    fn image_beats_url_content() {
        assert_eq!(
            classify_with_content(
                &mimes(&["image/png", "text/plain"]),
                Some("https://example.com")
            ),
            Some(ContentClass::Image)
        );
    }
}
