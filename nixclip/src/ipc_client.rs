//! IPC client that bridges between the GLib main loop (UI thread) and a
//! background tokio runtime for communicating with the nixclipd daemon.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gtk4::glib;
use tokio::net::UnixStream;
use tokio::sync::mpsc;

use nixclip_core::ipc::{self, ClientMessage, ServerMessage};
use nixclip_core::{ContentClass, EntryId, RestoreMode};

// ---------------------------------------------------------------------------
// Request / Response envelope
// ---------------------------------------------------------------------------

/// A request sent from the GLib thread to the background tokio runtime.
struct IpcRequest {
    message: ClientMessage,
    /// Sender that lives on the GLib side; the background thread sends the raw
    /// response back through it.
    reply: glib::Sender<Result<ServerMessage, String>>,
}

// ---------------------------------------------------------------------------
// UiIpcClient
// ---------------------------------------------------------------------------

/// The IPC handle used by UI code running on the GLib main loop.
///
/// Internally it spawns a dedicated tokio runtime on a background thread and
/// bridges requests/responses via channels.
pub struct UiIpcClient {
    /// Channel to send requests into the background tokio runtime.
    request_tx: mpsc::UnboundedSender<IpcRequest>,
}

impl UiIpcClient {
    /// Create a new client and immediately attempt to connect to the given
    /// socket path in the background.
    pub fn new(socket_path: &Path) -> Self {
        let (request_tx, request_rx) = mpsc::unbounded_channel::<IpcRequest>();
        let path = socket_path.to_path_buf();

        // Spawn a background thread that owns a single-threaded tokio runtime.
        std::thread::Builder::new()
            .name("nixclip-ipc".into())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("failed to build tokio runtime for IPC");
                rt.block_on(ipc_loop(path, request_rx));
            })
            .expect("failed to spawn IPC thread");

        Self { request_tx }
    }

    // -----------------------------------------------------------------------
    // Public helpers — each method fires a request and routes the response
    // back to the GLib main loop via a callback.
    // -----------------------------------------------------------------------

    /// Query clipboard history.
    ///
    /// The callback receives `(entries, total)` where `total` is the full
    /// count of matching entries (which may exceed the returned page).
    pub fn query(
        &self,
        text: Option<String>,
        class: Option<ContentClass>,
        limit: u32,
        callback: impl Fn(Result<(Vec<nixclip_core::EntrySummary>, u32), String>) + 'static,
    ) {
        let msg = ClientMessage::query(
            text,
            class.map(|c| c.as_str().to_string()),
            0,
            limit,
        );

        self.send(msg, move |res| match res {
            Ok(ServerMessage::QueryResult { entries, total, .. }) => {
                callback(Ok((entries, total)))
            }
            Ok(ServerMessage::Error { message, .. }) => callback(Err(message)),
            Ok(other) => callback(Err(format!("unexpected response: {other:?}"))),
            Err(e) => callback(Err(e)),
        });
    }

    /// Restore a clipboard entry.
    pub fn restore(
        &self,
        id: EntryId,
        mode: RestoreMode,
        callback: impl Fn(Result<bool, String>) + 'static,
    ) {
        let msg = ClientMessage::restore(id, mode);

        self.send(msg, move |res| match res {
            Ok(ServerMessage::RestoreResult { success, error, .. }) => {
                if success {
                    callback(Ok(true));
                } else {
                    callback(Err(error.unwrap_or_else(|| "restore failed".into())));
                }
            }
            Ok(ServerMessage::Error { message, .. }) => callback(Err(message)),
            Ok(other) => callback(Err(format!("unexpected response: {other:?}"))),
            Err(e) => callback(Err(e)),
        });
    }

    /// Delete a clipboard entry.
    pub fn delete(
        &self,
        id: EntryId,
        callback: impl Fn(Result<(), String>) + 'static,
    ) {
        let msg = ClientMessage::delete(vec![id]);

        self.send(msg, move |res| match res {
            Ok(ServerMessage::Ok { .. }) => callback(Ok(())),
            Ok(ServerMessage::Error { message, .. }) => callback(Err(message)),
            Ok(other) => callback(Err(format!("unexpected response: {other:?}"))),
            Err(e) => callback(Err(e)),
        });
    }

    /// Pin or unpin a clipboard entry.
    pub fn pin(
        &self,
        id: EntryId,
        pinned: bool,
        callback: impl Fn(Result<(), String>) + 'static,
    ) {
        let msg = ClientMessage::pin(id, pinned);

        self.send(msg, move |res| match res {
            Ok(ServerMessage::Ok { .. }) => callback(Ok(())),
            Ok(ServerMessage::Error { message, .. }) => callback(Err(message)),
            Ok(other) => callback(Err(format!("unexpected response: {other:?}"))),
            Err(e) => callback(Err(e)),
        });
    }

    /// Clear all unpinned entries.
    pub fn clear_unpinned(
        &self,
        callback: impl Fn(Result<(), String>) + 'static,
    ) {
        let msg = ClientMessage::clear_unpinned();

        self.send(msg, move |res| match res {
            Ok(ServerMessage::Ok { .. }) => callback(Ok(())),
            Ok(ServerMessage::Error { message, .. }) => callback(Err(message)),
            Ok(other) => callback(Err(format!("unexpected response: {other:?}"))),
            Err(e) => callback(Err(e)),
        });
    }

    // -----------------------------------------------------------------------
    // Internal
    // -----------------------------------------------------------------------

    /// Send a request to the background runtime. When a response arrives it
    /// is dispatched back to the GLib main loop via the provided callback.
    fn send(
        &self,
        message: ClientMessage,
        callback: impl Fn(Result<ServerMessage, String>) + 'static,
    ) {
        let (glib_tx, glib_rx) = glib::MainContext::channel::<Result<ServerMessage, String>>(
            glib::Priority::DEFAULT,
        );

        // Attach receiver to GLib main loop.
        glib_rx.attach(None, move |result| {
            callback(result);
            glib::ControlFlow::Break
        });

        let req = IpcRequest {
            message,
            reply: glib_tx,
        };

        if self.request_tx.send(req).is_err() {
            tracing::error!("IPC background thread has exited; cannot send request");
        }
    }
}

// ---------------------------------------------------------------------------
// Background event loop
// ---------------------------------------------------------------------------

/// Runs on the background tokio runtime. Receives requests from the GLib
/// thread, sends them over the Unix socket, and forwards replies back.
async fn ipc_loop(socket_path: PathBuf, mut rx: mpsc::UnboundedReceiver<IpcRequest>) {
    // We lazily connect (and reconnect) on each request batch.
    let mut stream: Option<UnixStream> = None;

    while let Some(req) = rx.recv().await {
        // Ensure we have a live connection.
        let conn = match &mut stream {
            Some(s) => s,
            None => {
                match UnixStream::connect(&socket_path).await {
                    Ok(s) => {
                        stream = Some(s);
                        stream.as_mut().unwrap()
                    }
                    Err(e) => {
                        let msg = format!("cannot connect to daemon: {e}");
                        tracing::warn!("{}", msg);
                        let _ = req.reply.send(Err(msg));
                        continue;
                    }
                }
            }
        };

        // Split the stream for reading and writing.
        let (mut reader, mut writer) = conn.split();

        // Send the request.
        if let Err(e) = ipc::send_message(&mut writer, &req.message).await {
            let msg = format!("failed to send IPC message: {e}");
            tracing::warn!("{}", msg);
            let _ = req.reply.send(Err(msg));
            // Connection is likely broken; drop it so we reconnect next time.
            stream = None;
            continue;
        }

        // Read the response.
        match ipc::recv_message::<_, ServerMessage>(&mut reader).await {
            Ok(response) => {
                let _ = req.reply.send(Ok(response));
            }
            Err(e) => {
                let msg = format!("failed to read IPC response: {e}");
                tracing::warn!("{}", msg);
                let _ = req.reply.send(Err(msg));
                stream = None;
            }
        }
    }

    tracing::debug!("IPC background loop exiting");
}
