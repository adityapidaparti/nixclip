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

use nixclip_core::error::{NixClipError, Result};
use nixclip_core::pipeline::privacy::FilterResult;
use nixclip_core::pipeline::ContentProcessor;
use nixclip_core::{MimePayload, NewEntry, Representation};

use crate::AppState;

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

    /// Set the Wayland clipboard selection to the given representations.
    async fn set_selection(&self, representations: Vec<Representation>) -> Result<()>;
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
    // On a real Wayland system these fields would hold:
    //   connection: wayland_client::Connection,
    //   manager: data_control_manager proxy,
    //   seat: wl_seat proxy,
    //
    // TODO: Store the Wayland Connection, EventQueue, data-control manager
    //       proxy, and seat proxy here once the exact crate API is pinned down.
    _private: (),
}

impl WaylandBackend {
    /// Connect to the Wayland display and locate the data-control manager.
    ///
    /// Looks for `ext_data_control_manager_v1` first, then falls back to
    /// `zwlr_data_control_manager_v1`.
    pub fn connect() -> Result<Self> {
        // TODO: Full Wayland integration.
        //
        // Outline of what this should do:
        //
        // 1. let connection = wayland_client::Connection::connect_to_env()
        //        .map_err(|e| NixClipError::Wayland(format!("connect: {e}")))?;
        // 2. let display = connection.display();
        // 3. let mut event_queue = connection.new_event_queue();
        // 4. Enumerate globals via wl_registry, looking for:
        //    - ext_data_control_manager_v1 (preferred, from staging protocols)
        //    - zwlr_data_control_manager_v1 (fallback, from wlr-protocols)
        //    - wl_seat (to get the default seat)
        // 5. Create a data_control_device from the manager + seat.
        // 6. Store connection, event_queue, manager, device, seat.
        //
        // For now we indicate that the Wayland connection is not yet available.

        #[cfg(not(target_os = "linux"))]
        {
            Err(NixClipError::Wayland(
                "Wayland clipboard is only available on Linux".into(),
            ))
        }

        #[cfg(target_os = "linux")]
        {
            // Attempt connection -- this will fail if $WAYLAND_DISPLAY is unset
            // or the compositor isn't running.
            //
            // TODO: Replace with actual wayland_client::Connection::connect_to_env()
            //       once the crate API is validated.
            Err(NixClipError::Wayland(
                "Wayland data-control integration not yet implemented; \
                 see TODO comments in watcher.rs"
                    .into(),
            ))
        }
    }
}

#[async_trait]
impl ClipboardBackend for WaylandBackend {
    async fn watch(&mut self, _tx: mpsc::Sender<ClipboardEvent>) -> Result<()> {
        // TODO: Full implementation outline:
        //
        // 1. Run the Wayland event loop (roundtrip / dispatch) in a
        //    spawn_blocking context or a dedicated thread.
        // 2. On data_control_device.selection event:
        //    a. Read the offered MIME types from the data_control_offer.
        //    b. For each relevant MIME (text/plain, text/html, image/png, etc.),
        //       open a pipe, call offer.receive(mime, write_fd), roundtrip,
        //       then read the data from the read end.
        //    c. Build a ClipboardEvent with the payloads.
        //    d. Send it through `tx`.
        // 3. Repeat until the connection is closed or an error occurs.

        Err(NixClipError::Wayland(
            "Wayland watcher not yet implemented".into(),
        ))
    }

    async fn set_selection(&self, _representations: Vec<Representation>) -> Result<()> {
        // TODO: Full implementation outline:
        //
        // 1. Create a new data_control_source.
        // 2. For each representation, advertise the MIME type via source.offer(mime).
        // 3. Set the source as the current selection:
        //    data_control_device.set_selection(source).
        // 4. When the compositor requests data (send event), write the
        //    corresponding representation bytes to the provided fd.

        Err(NixClipError::Wayland(
            "Wayland set_selection not yet implemented".into(),
        ))
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

    async fn set_selection(&self, representations: Vec<Representation>) -> Result<()> {
        WlPasteBackend::set_selection(self, representations).await
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

    if let Ok(mut backend) = WaylandBackend::connect() {
        info!("using direct Wayland data-control backend");
        tokio::spawn(async move {
            if let Err(error) = backend.watch(tx).await {
                error!(error = %error, "direct Wayland backend failed");
            }
        });
        process_events(state, rx).await;
        return Ok(());
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

    // Run privacy filter.
    {
        let filter = state.privacy_filter.read().await;
        if matches!(
            filter.check(source_app.as_deref(), &offered_mimes, None),
            FilterResult::Reject
        ) {
            debug!(
                source_app = ?source_app,
                "privacy filter rejected clipboard event"
            );
            return Ok(());
        }
    }

    // Process content (classification, hashing, thumbnails, etc.).
    let processed = ContentProcessor::process(payloads, source_app.clone())?;

    let filter_result = {
        let filter = state.privacy_filter.read().await;
        filter.check(
            source_app.as_deref(),
            &offered_mimes,
            processed.preview_text.as_deref(),
        )
    };
    if matches!(filter_result, FilterResult::Reject) {
        debug!(
            source_app = ?source_app,
            "privacy filter rejected clipboard content after preview generation"
        );
        return Ok(());
    }
    let is_ephemeral = matches!(filter_result, FilterResult::Ephemeral);
    if is_ephemeral {
        debug!("privacy filter flagged content as ephemeral");
    }

    // Build the NewEntry.
    let new_entry = NewEntry {
        content_class: processed.content_class,
        preview_text: processed.preview_text.clone(),
        canonical_hash: processed.canonical_hash,
        representations: processed.representations.clone(),
        source_app: source_app.clone(),
        ephemeral: is_ephemeral,
        metadata: processed.metadata.clone(),
    };

    // Insert into the store (via spawn_blocking since rusqlite is !Send-safe
    // across await points when wrapped in std::sync::Mutex).
    let summary = {
        let store = state.store.lock().map_err(|e| {
            NixClipError::Pipeline(format!("store lock poisoned: {e}"))
        })?;
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
            ephemeral: is_ephemeral,
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
/// Creates a new data source with the provided MIME types and sets it as the
/// current selection on the default seat.
pub async fn restore_to_clipboard(representations: Vec<Representation>) -> Result<()> {
    if representations.is_empty() {
        return Err(NixClipError::Pipeline(
            "no representations to restore".into(),
        ));
    }

    let reps: Vec<Representation> = representations
        .into_iter()
        .map(|r| Representation {
            mime: r.mime,
            data: r.data,
        })
        .collect();

    if let Ok(backend) = WaylandBackend::connect() {
        return backend.set_selection(reps.clone()).await;
    }

    if WlPasteBackend::available() {
        return WlPasteBackend.set_selection(reps).await;
    }

    Err(NixClipError::Wayland(
        "no clipboard restore backend available; install wl-clipboard or enable direct data-control support"
            .into(),
    ))
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
}
