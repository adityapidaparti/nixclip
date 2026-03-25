//! Wayland clipboard watcher.
//!
//! Connects to the Wayland compositor and monitors clipboard selection changes.
//! Each change is processed through the privacy filter and content processor,
//! then inserted into the clip store and broadcast to subscribers.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
#[cfg(target_os = "linux")]
use wayland_client::globals::{registry_queue_init, GlobalListContents};
#[cfg(target_os = "linux")]
use wayland_client::protocol::wl_registry;
#[cfg(target_os = "linux")]
use wayland_client::{Connection, Dispatch, QueueHandle};

use nixclip_core::error::{NixClipError, Result};
use nixclip_core::pipeline::privacy::FilterResult;
use nixclip_core::pipeline::ContentProcessor;
use nixclip_core::{MimePayload, NewEntry, Representation};

use crate::AppState;

#[allow(dead_code)]
#[cfg(any(test, target_os = "linux"))]
type DirectMimeSource = wl_clipboard_rs::copy::MimeSource;

// ---------------------------------------------------------------------------
// ClipboardEvent
// ---------------------------------------------------------------------------

/// A single clipboard selection event received from the backend.
#[derive(Debug, Clone)]
pub struct ClipboardEvent {
    /// MIME types offered by the clipboard source.
    pub offered_mimes: Vec<String>,
    /// Actual data payloads read for relevant MIME types.
    pub payloads: Vec<MimePayload>,
    /// The desktop-entry or app-id of the source application, if available.
    pub source_app: Option<String>,
}

// ---------------------------------------------------------------------------
// ClipboardBackend trait
// ---------------------------------------------------------------------------

/// Abstraction over the platform clipboard integration.
///
/// This trait allows the watcher logic to be tested and compiled even when the
/// concrete Wayland protocol crates are unavailable (e.g. on macOS CI).
#[async_trait]
pub trait ClipboardBackend: Send + 'static {
    /// Start watching for clipboard changes and send each event to `tx`.
    ///
    /// This method should run indefinitely (or until an error occurs).
    #[allow(dead_code)]
    async fn watch(&mut self, tx: mpsc::Sender<ClipboardEvent>) -> Result<()>;
}

// ---------------------------------------------------------------------------
// WaylandBackend
// ---------------------------------------------------------------------------

/// Wayland-based clipboard backend using ext-data-control or wlr-data-control.
///
/// NOTE: The Wayland protocol crate types (wayland-client, wayland-protocols,
/// wayland-protocols-wlr) are Linux-only.  The implementation below is
/// structured for correctness on a Wayland system.  On non-Linux builds the
/// backend simply returns an error explaining that Wayland is unavailable.
pub struct WaylandBackend {
    protocol: DirectDataControlProtocol,
    #[allow(dead_code)]
    seat_count: usize,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DirectDataControlProtocol {
    ExtDataControlV1,
    WlrDataControlV1,
}

#[allow(dead_code)]
impl DirectDataControlProtocol {
    fn global_name(self) -> &'static str {
        match self {
            Self::ExtDataControlV1 => "ext_data_control_manager_v1",
            Self::WlrDataControlV1 => "zwlr_data_control_manager_v1",
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct WaylandProbe {
    protocol: Option<DirectDataControlProtocol>,
    seat_count: usize,
}

#[allow(dead_code)]
impl WaylandProbe {
    fn from_interfaces<'a>(interfaces: impl IntoIterator<Item = &'a str>) -> Self {
        let mut seat_count = 0usize;
        let mut saw_ext = false;
        let mut saw_wlr = false;

        for interface in interfaces {
            match interface {
                "wl_seat" => seat_count += 1,
                "ext_data_control_manager_v1" => saw_ext = true,
                "zwlr_data_control_manager_v1" => saw_wlr = true,
                _ => {}
            }
        }

        let protocol = if saw_ext {
            Some(DirectDataControlProtocol::ExtDataControlV1)
        } else if saw_wlr {
            Some(DirectDataControlProtocol::WlrDataControlV1)
        } else {
            None
        };

        Self {
            protocol,
            seat_count,
        }
    }
}

impl WaylandBackend {
    #[cfg(target_os = "linux")]
    fn probe() -> Result<WaylandProbe> {
        let connection = Connection::connect_to_env()
            .map_err(|e| NixClipError::Wayland(format!("connect to Wayland display: {e}")))?;
        let (globals, mut event_queue) =
            registry_queue_init::<WaylandRegistryState>(&connection)
                .map_err(|e| NixClipError::Wayland(format!("enumerate Wayland globals: {e}")))?;
        let mut state = WaylandRegistryState;
        event_queue
            .roundtrip(&mut state)
            .map_err(|e| NixClipError::Wayland(format!("roundtrip Wayland globals: {e}")))?;

        Ok(WaylandProbe::from_interfaces(
            globals
                .contents()
                .clone_list()
                .iter()
                .map(|global| global.interface.as_str()),
        ))
    }

    #[cfg(not(target_os = "linux"))]
    fn probe() -> Result<WaylandProbe> {
        Err(NixClipError::Wayland(
            "Wayland clipboard is only available on Linux".into(),
        ))
    }

    /// Connect to the Wayland display and locate the data-control manager.
    ///
    /// Looks for `ext_data_control_manager_v1` first, then falls back to
    /// `zwlr_data_control_manager_v1`.
    pub fn connect() -> Result<Self> {
        let probe = Self::probe()?;

        let protocol = probe.protocol.ok_or_else(|| {
            NixClipError::Wayland(
                "compositor does not expose ext_data_control_manager_v1 or zwlr_data_control_manager_v1"
                    .into(),
            )
        })?;

        if probe.seat_count == 0 {
            return Err(NixClipError::Wayland(
                "compositor exposes data-control but no wl_seat globals were advertised".into(),
            ));
        }

        Ok(Self {
            protocol,
            seat_count: probe.seat_count,
        })
    }
}

#[async_trait]
impl ClipboardBackend for WaylandBackend {
    async fn watch(&mut self, _tx: mpsc::Sender<ClipboardEvent>) -> Result<()> {
        Err(NixClipError::Wayland(format!(
            "direct {} capture is detected but not implemented: \
                 watcher needs a persistent Wayland event-queue thread that owns \
                 wl_seat/data-control device objects and drains offer FDs on selection events \
                 (detected {} seat global(s))",
            self.protocol.global_name(),
            self.seat_count
        )))
    }
}

#[cfg(target_os = "linux")]
#[derive(Debug)]
struct WaylandRegistryState;

#[cfg(target_os = "linux")]
impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for WaylandRegistryState {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

// ---------------------------------------------------------------------------
// WlPasteBackend — subprocess-based backend using wl-clipboard
// ---------------------------------------------------------------------------

/// Subprocess-based clipboard backend using `wl-paste` and `wl-copy`.
///
/// Works on any Wayland compositor that provides the `wl-clipboard` package.
/// This is the same pragmatic approach used by `cliphist` and similar tools.
/// It shells out to `wl-paste --watch` for change notifications, queries MIME
/// types and content per-change, and uses `wl-copy` for restore.
pub struct WlPasteBackend;

impl WlPasteBackend {
    /// Check if `wl-paste` is available in `$PATH`.
    pub fn available() -> bool {
        std::process::Command::new("wl-paste")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// Check if `wl-copy` is available in `$PATH`.
    pub fn copy_available() -> bool {
        std::process::Command::new("wl-copy")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

/// MIME types we actively fetch content for on each clipboard change.
const RELEVANT_MIMES: &[&str] = &[
    "text/plain",
    "text/html",
    "image/png",
    "image/jpeg",
    "text/uri-list",
    "x-special/gnome-copied-files",
];

/// Standalone async task for the wl-paste watcher (Send-safe, spawnable with tokio::spawn).
async fn wlpaste_watch_task(tx: mpsc::Sender<ClipboardEvent>) {
    let mut backend = WlPasteBackend;
    if let Err(e) = backend.watch_impl(tx).await {
        error!(error = %e, "wl-paste backend failed");
    }
}

impl WlPasteBackend {
    async fn watch_impl(&mut self, tx: mpsc::Sender<ClipboardEvent>) -> Result<()> {
        use tokio::io::{AsyncBufReadExt, BufReader};
        use tokio::process::Command;

        info!("starting wl-paste clipboard watcher");

        let mut child = Command::new("wl-paste")
            .args(["--watch", "echo", "__NIXCLIP_CHANGE__"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| NixClipError::Wayland(format!("failed to spawn wl-paste: {e}")))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| NixClipError::Wayland("wl-paste stdout not captured".into()))?;
        let mut lines = BufReader::new(stdout).lines();

        while let Ok(Some(line)) = lines.next_line().await {
            if line.trim() != "__NIXCLIP_CHANGE__" {
                continue;
            }

            // Query available MIME types.
            let types = match wl_list_types().await {
                Ok(t) if !t.is_empty() => t,
                _ => continue,
            };

            // Fetch content for each relevant MIME type offered.
            let mut payloads = Vec::new();
            for mime in RELEVANT_MIMES {
                if types.iter().any(|t| t == mime) {
                    if let Ok(data) = wl_fetch(mime).await {
                        if !data.is_empty() {
                            payloads.push(MimePayload {
                                mime: mime.to_string(),
                                data,
                            });
                        }
                    }
                }
            }

            if payloads.is_empty() {
                continue;
            }

            let event = ClipboardEvent {
                offered_mimes: types,
                payloads,
                source_app: None, // best-effort: could query org.gnome.Shell.Introspect
            };

            if tx.send(event).await.is_err() {
                debug!("event channel closed, stopping wl-paste watcher");
                break;
            }
        }

        let _ = child.kill().await;
        Ok(())
    }

    async fn set_selection(&self, representations: Vec<Representation>) -> Result<()> {
        use tokio::io::AsyncWriteExt;
        use tokio::process::Command;

        if representations.is_empty() {
            return Ok(());
        }

        // Restore the first (best) representation via wl-copy.
        let rep = &representations[0];

        let mut child = Command::new("wl-copy")
            .args(["--type", &rep.mime])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| NixClipError::Wayland(format!("failed to spawn wl-copy: {e}")))?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(&rep.data)
                .await
                .map_err(|e| NixClipError::Wayland(format!("wl-copy write error: {e}")))?;
        }

        let status = child
            .wait()
            .await
            .map_err(|e| NixClipError::Wayland(format!("wl-copy wait error: {e}")))?;

        if !status.success() {
            return Err(NixClipError::Wayland("wl-copy exited with error".into()));
        }

        info!(mime = %rep.mime, "clipboard restored via wl-copy");
        Ok(())
    }
}

#[async_trait]
impl ClipboardBackend for WlPasteBackend {
    async fn watch(&mut self, tx: mpsc::Sender<ClipboardEvent>) -> Result<()> {
        self.watch_impl(tx).await
    }
}

/// Query the MIME types currently offered by the clipboard selection.
async fn wl_list_types() -> Result<Vec<String>> {
    let output = tokio::process::Command::new("wl-paste")
        .arg("--list-types")
        .output()
        .await
        .map_err(|e| NixClipError::Wayland(format!("wl-paste --list-types: {e}")))?;

    if !output.status.success() {
        return Ok(Vec::new());
    }

    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect())
}

/// Fetch clipboard content for a specific MIME type.
async fn wl_fetch(mime: &str) -> Result<Vec<u8>> {
    let output = tokio::process::Command::new("wl-paste")
        .args(["--no-newline", "--type", mime])
        .output()
        .await
        .map_err(|e| NixClipError::Wayland(format!("wl-paste --type {mime}: {e}")))?;

    if !output.status.success() {
        return Ok(Vec::new());
    }

    Ok(output.stdout)
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Main watcher loop.
///
/// Tries clipboard backends in order of preference:
/// 1. Direct Wayland protocol (best performance, full MIME fidelity)
/// 2. `wl-paste`/`wl-copy` subprocess (works wherever wl-clipboard is installed)
///
/// If neither is available, the daemon continues without clipboard capture
/// (IPC, pruning, etc. still work).
pub async fn run(state: Arc<AppState>) -> Result<()> {
    let (tx, rx) = mpsc::channel::<ClipboardEvent>(64);

    match WaylandBackend::connect() {
        Ok(backend) => {
            info!(
                protocol = backend.protocol.global_name(),
                seat_count = backend.seat_count,
                "direct Wayland data-control support detected, but capture is not implemented yet; using wl-paste fallback if available"
            );
        }
        Err(error) => {
            info!(
                error = %error,
                "direct Wayland data-control backend unavailable or incomplete; falling back if possible"
            );
        }
    }

    // wl-paste/wl-copy subprocess backend.
    if WlPasteBackend::available() {
        info!("using wl-paste/wl-copy subprocess backend");
        tokio::spawn(wlpaste_watch_task(tx));
        process_events(state, rx).await;
        return Ok(());
    }

    warn!(
        "no clipboard backend available; running without capture. \
         Install wl-clipboard or run on a Wayland compositor with data-control support."
    );
    std::future::pending::<()>().await;
    Ok(())
}

/// Run the event processing loop, receiving from the given channel.
async fn process_events(state: Arc<AppState>, mut rx: mpsc::Receiver<ClipboardEvent>) {
    info!("clipboard watcher started");
    while let Some(event) = rx.recv().await {
        if let Err(e) = handle_event(&state, event).await {
            warn!(error = %e, "failed to process clipboard event");
        }
    }
    info!("clipboard watcher stopped");
}

/// Process a single clipboard event.
///
/// Privacy filtering is split into two phases so that content-based regex
/// patterns (e.g. API-key detection) run *after* the content processor has
/// produced the preview text, while cheaper app-name / MIME-type checks
/// still short-circuit before the expensive processing step.
async fn handle_event(state: &AppState, event: ClipboardEvent) -> Result<()> {
    let ClipboardEvent {
        offered_mimes,
        payloads,
        source_app,
    } = event;

    // Skip if screen is locked.
    if state.is_locked.load(Ordering::Relaxed) {
        debug!("screen is locked, skipping clipboard event");
        return Ok(());
    }

    // Phase 1: pre-content privacy check (app-name + MIME-type rules).
    // This avoids the cost of content processing for events that would be
    // rejected based on their source alone.
    {
        let filter = state.privacy_filter.read().await;
        let pre_result = filter.check_pre_content(source_app.as_deref(), &offered_mimes);
        if pre_result == FilterResult::Reject {
            debug!(
                source_app = ?source_app,
                "privacy filter rejected clipboard event (pre-content)"
            );
            return Ok(());
        }
    }

    // Process content (classification, hashing, thumbnails, etc.).
    let processed = ContentProcessor::process(payloads, source_app.clone())?;

    // Phase 2: content-based privacy check (regex patterns on preview text).
    // Patterns like API-key detectors need the preview text that the content
    // processor just produced.
    let ephemeral = {
        let filter = state.privacy_filter.read().await;
        filter.check_content_patterns(processed.preview_text.as_deref()) == FilterResult::Ephemeral
    };
    if ephemeral {
        debug!("privacy filter flagged content as ephemeral");
    }

    // Build the NewEntry.
    let new_entry = NewEntry {
        content_class: processed.content_class,
        preview_text: processed.preview_text.clone(),
        canonical_hash: processed.canonical_hash,
        representations: processed.representations.clone(),
        source_app: source_app.clone(),
        ephemeral,
        metadata: processed.metadata.clone(),
    };

    // Insert into the store (via spawn_blocking since rusqlite is !Send-safe
    // across await points when wrapped in std::sync::Mutex).
    let summary = {
        let store = state
            .store
            .lock()
            .map_err(|e| NixClipError::Pipeline(format!("store lock poisoned: {e}")))?;
        store.insert(new_entry)?
    };

    if let Some(entry_id) = summary {
        debug!(id = entry_id, class = %processed.content_class, "new entry stored");

        // Build a summary for broadcast. We construct a minimal one from what
        // we already know to avoid another DB query.
        let broadcast = nixclip_core::EntrySummary {
            id: entry_id,
            created_at: chrono::Utc::now().timestamp_millis(),
            last_seen_at: chrono::Utc::now().timestamp_millis(),
            pinned: false,
            ephemeral,
            content_class: processed.content_class,
            preview_text: processed.preview_text,
            source_app,
            thumbnail: processed.thumbnail,
            match_ranges: vec![],
            metadata: processed.metadata,
        };
        let _ = state.new_entry_tx.send(broadcast);
    } else {
        debug!("entry deduplicated (same as most recent)");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Clipboard restore
// ---------------------------------------------------------------------------

/// Restore representations to the system clipboard.
///
/// Uses direct Wayland data-control restore when possible so multi-MIME
/// entries preserve their original offered representations.
pub async fn restore_to_clipboard(representations: Vec<Representation>) -> Result<()> {
    if representations.is_empty() {
        return Err(NixClipError::Pipeline(
            "no representations to restore".into(),
        ));
    }

    match restore_via_data_control(representations.clone()).await {
        Ok(()) => {
            info!("clipboard restored via direct Wayland data-control backend");
            Ok(())
        }
        Err(error) => {
            warn!(
                error = %error,
                "direct Wayland restore failed; falling back to wl-copy if available"
            );
            restore_via_wl_copy(representations).await
        }
    }
}

#[cfg(target_os = "linux")]
async fn restore_via_data_control(representations: Vec<Representation>) -> Result<()> {
    let sources = direct_mime_sources(&representations);

    tokio::task::spawn_blocking(move || {
        let options = wl_clipboard_rs::copy::Options::new();
        options.copy_multi(sources).map_err(|error| {
            NixClipError::Wayland(format!("wl-clipboard-rs restore failed: {error}"))
        })
    })
    .await
    .map_err(|error| NixClipError::Wayland(format!("direct restore task failed: {error}")))?
}

#[cfg(not(target_os = "linux"))]
async fn restore_via_data_control(_representations: Vec<Representation>) -> Result<()> {
    Err(NixClipError::Wayland(
        "direct Wayland restore is only available on Linux".into(),
    ))
}

#[allow(dead_code)]
#[cfg(any(test, target_os = "linux"))]
fn direct_mime_sources(representations: &[Representation]) -> Vec<DirectMimeSource> {
    representations
        .iter()
        .map(|representation| wl_clipboard_rs::copy::MimeSource {
            source: wl_clipboard_rs::copy::Source::Bytes(
                representation.data.clone().into_boxed_slice(),
            ),
            mime_type: wl_clipboard_rs::copy::MimeType::Specific(representation.mime.clone()),
        })
        .collect()
}

async fn restore_via_wl_copy(representations: Vec<Representation>) -> Result<()> {
    if !WlPasteBackend::copy_available() {
        return Err(NixClipError::Wayland(
            "no clipboard restore backend available; install wl-clipboard or finish the direct data-control backend"
                .into(),
        ));
    }

    let backend = WlPasteBackend;
    backend.set_selection(representations).await
}

#[cfg(test)]
mod tests {
    use nixclip_core::config::IgnoreConfig;
    use nixclip_core::pipeline::PrivacyFilter;

    use super::*;

    fn default_filter() -> PrivacyFilter {
        PrivacyFilter::new(&IgnoreConfig::default()).expect("default filter")
    }

    #[test]
    fn pattern_matches_require_preview_text_in_second_pass() {
        let filter = default_filter();
        let offered_mimes = vec!["text/plain".to_string()];
        let preview = format!("sk-{}", "X".repeat(48));

        assert!(!matches!(
            filter.check(None, &offered_mimes, None),
            FilterResult::Ephemeral
        ));
        assert!(matches!(
            filter.check(None, &offered_mimes, Some(&preview)),
            FilterResult::Ephemeral
        ));
    }

    #[test]
    fn ignored_apps_are_rejected_before_processing() {
        let filter = default_filter();
        let offered_mimes = vec!["text/plain".to_string()];

        assert!(matches!(
            filter.check(
                Some("org.keepassxc.KeePassXC"),
                &offered_mimes,
                Some("normal text"),
            ),
            FilterResult::Reject
        ));
    }

    #[test]
    fn restore_backend_reports_missing_wl_copy() {
        if cfg!(target_os = "linux") && !WlPasteBackend::copy_available() {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime");
            let err = runtime
                .block_on(restore_via_wl_copy(vec![Representation {
                    mime: "text/plain".into(),
                    data: b"hello".to_vec(),
                }]))
                .expect_err("wl-copy should be required for fallback restore");
            assert!(err
                .to_string()
                .contains("no clipboard restore backend available"));
        }
    }

    #[test]
    fn direct_restore_preserves_all_mime_representations() {
        let sources = direct_mime_sources(&[
            Representation {
                mime: "text/plain".into(),
                data: b"plain".to_vec(),
            },
            Representation {
                mime: "text/html".into(),
                data: b"<b>plain</b>".to_vec(),
            },
            Representation {
                mime: "image/png".into(),
                data: vec![1, 2, 3, 4],
            },
        ]);

        let offered_mimes = sources
            .iter()
            .map(|source| match &source.mime_type {
                wl_clipboard_rs::copy::MimeType::Specific(mime) => mime.as_str(),
                other => panic!("unexpected MIME source variant: {other:?}"),
            })
            .collect::<Vec<_>>();

        assert_eq!(offered_mimes, vec!["text/plain", "text/html", "image/png"]);
    }

    #[test]
    fn wayland_probe_prefers_ext_data_control() {
        let probe = WaylandProbe::from_interfaces([
            "wl_compositor",
            "zwlr_data_control_manager_v1",
            "ext_data_control_manager_v1",
            "wl_seat",
        ]);

        assert_eq!(
            probe.protocol,
            Some(DirectDataControlProtocol::ExtDataControlV1)
        );
        assert_eq!(probe.seat_count, 1);
    }

    #[test]
    fn wayland_probe_falls_back_to_wlr() {
        let probe = WaylandProbe::from_interfaces([
            "wl_compositor",
            "zwlr_data_control_manager_v1",
            "wl_seat",
            "wl_seat",
        ]);

        assert_eq!(
            probe.protocol,
            Some(DirectDataControlProtocol::WlrDataControlV1)
        );
        assert_eq!(probe.seat_count, 2);
    }

    #[test]
    fn wayland_probe_reports_missing_protocol() {
        let probe = WaylandProbe::from_interfaces(["wl_compositor", "wl_seat"]);

        assert_eq!(probe.protocol, None);
        assert_eq!(probe.seat_count, 1);
    }
}
