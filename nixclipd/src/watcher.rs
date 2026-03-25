//! Wayland clipboard watcher.
//!
//! Connects to the Wayland compositor and monitors clipboard selection changes.
//! Each change is processed through the privacy filter and content processor,
//! then inserted into the clip store and broadcast to subscribers.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use nixclip_core::error::{NixClipError, Result};
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
#[allow(async_fn_in_trait)]
pub trait ClipboardBackend: Send + 'static {
    /// Start watching for clipboard changes and send each event to `tx`.
    ///
    /// This method should run indefinitely (or until an error occurs).
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
            return Err(NixClipError::Wayland(
                "Wayland clipboard is only available on Linux".into(),
            ));
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
// Public entry point
// ---------------------------------------------------------------------------

/// Main watcher loop.
///
/// Connects to the clipboard backend, receives events, processes them through
/// the privacy filter and content processor, then stores and broadcasts.
pub async fn run(state: Arc<AppState>) -> Result<()> {
    let mut backend = match WaylandBackend::connect() {
        Ok(b) => b,
        Err(e) => {
            // Non-fatal: log and return so the daemon can still serve IPC, etc.
            warn!(error = %e, "clipboard watcher unavailable; running without capture");
            // Park forever so the task stays alive but idle.
            std::future::pending::<()>().await;
            return Ok(());
        }
    };

    let (tx, mut rx) = mpsc::channel::<ClipboardEvent>(64);

    // Spawn the backend watcher.
    let watch_handle = tokio::spawn(async move {
        if let Err(e) = backend.watch(tx).await {
            error!(error = %e, "clipboard backend watch failed");
        }
    });

    info!("clipboard watcher started");

    // Process incoming clipboard events.
    while let Some(event) = rx.recv().await {
        if let Err(e) = handle_event(&state, event).await {
            warn!(error = %e, "failed to process clipboard event");
        }
    }

    // If the channel closed, the backend exited.
    drop(watch_handle);
    info!("clipboard watcher stopped");
    Ok(())
}

/// Process a single clipboard event.
async fn handle_event(state: &AppState, event: ClipboardEvent) -> Result<()> {
    // Skip if screen is locked.
    if state.is_locked.load(Ordering::Relaxed) {
        debug!("screen is locked, skipping clipboard event");
        return Ok(());
    }

    // Run privacy filter.
    {
        let filter = state.privacy_filter.read().await;
        if filter.should_ignore(&event.offered_mimes, event.source_app.as_deref()) {
            debug!(
                source_app = ?event.source_app,
                "privacy filter rejected clipboard event"
            );
            return Ok(());
        }
    }

    // Process content (classification, hashing, thumbnails, etc.).
    let processed = ContentProcessor::process(event.payloads, event.source_app.clone())?;

    // Build the NewEntry.
    let new_entry = NewEntry {
        content_class: processed.content_class,
        preview_text: processed.preview_text.clone(),
        canonical_hash: processed.canonical_hash,
        representations: processed.representations.clone(),
        source_app: event.source_app,
        ephemeral: false,
    };

    // Insert into the store (via spawn_blocking since rusqlite is !Send-safe
    // across await points when wrapped in std::sync::Mutex).
    let summary = {
        let store = state.store.lock().map_err(|e| {
            NixClipError::Pipeline(format!("store lock poisoned: {e}"))
        })?;
        store.insert(&new_entry)?
    };

    debug!(id = summary.id, class = %summary.content_class, "new entry stored");

    // Broadcast to subscribers (ignore send errors -- no subscribers is fine).
    let _ = state.new_entry_tx.send(summary);

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

    let backend = WaylandBackend::connect()?;

    // Convert Representation -> the backend format.
    let reps: Vec<Representation> = representations
        .into_iter()
        .map(|r| Representation {
            mime: r.mime,
            data: r.data,
        })
        .collect();

    backend.set_selection(reps).await
}
