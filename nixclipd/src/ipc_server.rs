//! IPC server on a Unix domain socket.
//!
//! Binds to [`Config::socket_path()`], accepts connections, verifies peer
//! credentials, and dispatches [`ClientMessage`]s to the appropriate handler.

use std::sync::Arc;

use tokio::net::{UnixListener, UnixStream};
use tracing::{debug, error, info, warn};

use nixclip_core::config::Config;
use nixclip_core::error::{NixClipError, Result};
use nixclip_core::ipc::{
    recv_message, send_message, ClientMessage, ServerMessage,
};
use nixclip_core::{ContentClass, Query, RestoreMode};

use crate::watcher;
use crate::AppState;

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Start the IPC server, listening for client connections.
pub async fn run(state: Arc<AppState>) -> Result<()> {
    let socket_path = Config::socket_path();

    // Remove a stale socket file from a previous run.
    if socket_path.exists() {
        info!(path = %socket_path.display(), "removing stale socket");
        std::fs::remove_file(&socket_path).map_err(|e| {
            NixClipError::Io(e)
        })?;
    }

    // Ensure the parent directory exists.
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            NixClipError::Io(e)
        })?;
    }

    let listener = UnixListener::bind(&socket_path)?;

    // Set socket permissions to owner-only (0o700 on the parent dir is the
    // norm for XDG_RUNTIME_DIR; we also restrict the socket itself).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o700);
        if let Err(e) = std::fs::set_permissions(&socket_path, perms) {
            warn!(error = %e, "failed to set socket permissions");
        }
    }

    info!(path = %socket_path.display(), "IPC server listening");

    loop {
        match listener.accept().await {
            Ok((stream, _addr)) => {
                let s = state.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_client(s, stream).await {
                        debug!(error = %e, "client connection ended");
                    }
                });
            }
            Err(e) => {
                error!(error = %e, "failed to accept IPC connection");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Peer credential verification
// ---------------------------------------------------------------------------

/// Verify that the peer has the same UID as this process.
///
/// Returns `Ok(())` if credentials match, or an error otherwise.
#[cfg(unix)]
fn verify_peer_credentials(stream: &UnixStream) -> Result<()> {
    use std::os::unix::io::AsRawFd;

    let fd = stream.as_raw_fd();

    // Use libc::getsockopt with SO_PEERCRED on Linux.
    #[cfg(target_os = "linux")]
    {
        let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        let ret = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                &mut cred as *mut _ as *mut libc::c_void,
                &mut len,
            )
        };
        if ret != 0 {
            return Err(NixClipError::Ipc(format!(
                "getsockopt(SO_PEERCRED) failed: {}",
                std::io::Error::last_os_error()
            )));
        }

        let my_uid = unsafe { libc::getuid() };
        if cred.uid != my_uid {
            return Err(NixClipError::Ipc(format!(
                "peer UID {} does not match daemon UID {}",
                cred.uid, my_uid
            )));
        }

        return Ok(());
    }

    // On macOS / other Unix, use LOCAL_PEERCRED or skip (non-fatal for dev).
    #[cfg(not(target_os = "linux"))]
    {
        let _ = fd;
        debug!("peer credential check not implemented on this platform; allowing connection");
        Ok(())
    }
}

#[cfg(not(unix))]
fn verify_peer_credentials(_stream: &UnixStream) -> Result<()> {
    Ok(())
}

// ---------------------------------------------------------------------------
// Client handler
// ---------------------------------------------------------------------------

/// Handle a single client connection.
async fn handle_client(state: Arc<AppState>, stream: UnixStream) -> Result<()> {
    // Verify the peer is the same user.
    verify_peer_credentials(&stream)?;

    let (mut reader, mut writer) = stream.into_split();

    loop {
        // Read one framed message from the client.
        let msg: ClientMessage = match recv_message(&mut reader).await {
            Ok(m) => m,
            Err(NixClipError::Ipc(ref s)) if s.contains("connection closed") => {
                // Clean disconnect.
                return Ok(());
            }
            Err(e) => return Err(e),
        };

        debug!(msg_type = std::any::type_name::<ClientMessage>(), "received IPC message");

        let response = match msg {
            ClientMessage::Subscribe { .. } => {
                handle_subscribe(state.clone(), &mut writer).await;
                // subscribe takes over the connection loop; when it returns
                // the client disconnected.
                return Ok(());
            }
            ClientMessage::Query {
                text,
                content_class,
                offset,
                limit,
                ..
            } => handle_query(&state, text, content_class, offset, limit).await,
            ClientMessage::Restore { id, mode, .. } => {
                handle_restore(&state, id, mode).await
            }
            ClientMessage::Delete { ids, .. } => handle_delete(&state, ids).await,
            ClientMessage::Pin { id, pinned, .. } => {
                handle_pin(&state, id, pinned).await
            }
            ClientMessage::ClearUnpinned { .. } => handle_clear_unpinned(&state).await,
            ClientMessage::GetConfig { .. } => handle_get_config(&state).await,
            ClientMessage::SetConfig { patch, .. } => {
                handle_set_config(&state, patch).await
            }
        };

        send_message(&mut writer, &response).await?;
    }
}

// ---------------------------------------------------------------------------
// Message handlers
// ---------------------------------------------------------------------------

/// Subscribe: push `NewEntry` events to the client until it disconnects.
async fn handle_subscribe(
    state: Arc<AppState>,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
) {
    let mut rx = state.new_entry_tx.subscribe();

    loop {
        match rx.recv().await {
            Ok(entry) => {
                let msg = ServerMessage::new_entry(entry);
                if let Err(e) = send_message(writer, &msg).await {
                    debug!(error = %e, "subscriber disconnected");
                    return;
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                warn!(missed = n, "subscriber lagged; some events were dropped");
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                debug!("broadcast channel closed; ending subscription");
                return;
            }
        }
    }
}

/// Query the clipboard history.
async fn handle_query(
    state: &AppState,
    text: Option<String>,
    content_class: Option<String>,
    offset: u32,
    limit: u32,
) -> ServerMessage {
    // Parse the content class filter if provided.
    let class_filter: Option<ContentClass> = match content_class {
        Some(ref s) => match s.parse::<ContentClass>() {
            Ok(c) => Some(c),
            Err(e) => {
                return ServerMessage::error(format!("invalid content_class: {e}"));
            }
        },
        None => None,
    };

    let query = Query {
        text: text.clone(),
        content_class: class_filter,
        offset,
        limit,
    };

    // If the query has a text component, use the search engine first.
    if let Some(ref search_text) = text {
        // Try fuzzy search via SearchEngine.
        let search_result = {
            let search_text = search_text.clone();
            let engine = &state.search_engine;
            engine.search(&search_text, class_filter, offset, limit)
        };

        match search_result {
            Ok(result) => {
                return ServerMessage::query_result(result.entries, result.total);
            }
            Err(e) => {
                warn!(error = %e, "search engine failed; falling back to store query");
            }
        }
    }

    // Fall back to direct store query.
    let result = {
        let store = match state.store.lock() {
            Ok(s) => s,
            Err(e) => {
                return ServerMessage::error(format!("store lock poisoned: {e}"));
            }
        };
        store.query(query)
    };

    match result {
        Ok(qr) => ServerMessage::query_result(qr.entries, qr.total),
        Err(e) => ServerMessage::error(format!("query failed: {e}")),
    }
}

/// Restore a clipboard entry to the system clipboard.
async fn handle_restore(
    state: &AppState,
    id: nixclip_core::EntryId,
    mode: RestoreMode,
) -> ServerMessage {
    // Load representations from the store.
    let representations = {
        let store = match state.store.lock() {
            Ok(s) => s,
            Err(e) => {
                return ServerMessage::restore_err(format!("store lock poisoned: {e}"));
            }
        };
        store.get_representations(id)
    };

    let representations = match representations {
        Ok(reps) => reps,
        Err(e) => {
            return ServerMessage::restore_err(format!("failed to load entry: {e}"));
        }
    };

    if representations.is_empty() {
        return ServerMessage::restore_err("entry has no representations");
    }

    // Filter representations based on restore mode.
    let to_restore = match mode {
        RestoreMode::Original => representations,
        RestoreMode::PlainText => {
            representations
                .into_iter()
                .filter(|r| r.mime == "text/plain")
                .collect::<Vec<_>>()
        }
    };

    if to_restore.is_empty() {
        return ServerMessage::restore_err("no suitable representation for requested mode");
    }

    // Set clipboard via the watcher backend.
    match watcher::restore_to_clipboard(to_restore).await {
        Ok(()) => ServerMessage::restore_ok(),
        Err(e) => ServerMessage::restore_err(format!("clipboard restore failed: {e}")),
    }
}

/// Delete one or more entries by ID.
async fn handle_delete(
    state: &AppState,
    ids: Vec<nixclip_core::EntryId>,
) -> ServerMessage {
    let result = {
        let store = match state.store.lock() {
            Ok(s) => s,
            Err(e) => {
                return ServerMessage::error(format!("store lock poisoned: {e}"));
            }
        };
        store.delete(&ids)
    };

    match result {
        Ok(()) => ServerMessage::ok(),
        Err(e) => ServerMessage::error(format!("delete failed: {e}")),
    }
}

/// Pin or unpin an entry.
async fn handle_pin(
    state: &AppState,
    id: nixclip_core::EntryId,
    pinned: bool,
) -> ServerMessage {
    let result = {
        let store = match state.store.lock() {
            Ok(s) => s,
            Err(e) => {
                return ServerMessage::error(format!("store lock poisoned: {e}"));
            }
        };
        store.pin(id, pinned)
    };

    match result {
        Ok(()) => ServerMessage::ok(),
        Err(e) => ServerMessage::error(format!("pin failed: {e}")),
    }
}

/// Clear all unpinned entries.
async fn handle_clear_unpinned(state: &AppState) -> ServerMessage {
    let result = {
        let store = match state.store.lock() {
            Ok(s) => s,
            Err(e) => {
                return ServerMessage::error(format!("store lock poisoned: {e}"));
            }
        };
        store.clear_unpinned()
    };

    match result {
        Ok(()) => ServerMessage::ok(),
        Err(e) => ServerMessage::error(format!("clear unpinned failed: {e}")),
    }
}

/// Return the current configuration to the client.
async fn handle_get_config(state: &AppState) -> ServerMessage {
    let config = state.config.read().await.clone();
    ServerMessage::config_value(config)
}

/// Apply a partial TOML patch to the configuration and reload related state.
async fn handle_set_config(
    state: &AppState,
    patch: toml::Value,
) -> ServerMessage {
    // Merge the patch into the current config.
    let new_config = {
        let current = state.config.read().await;
        let current_value = match toml::Value::try_from(current.clone()) {
            Ok(v) => v,
            Err(e) => {
                return ServerMessage::error(format!(
                    "failed to serialize current config: {e}"
                ));
            }
        };
        let merged = merge_toml(current_value, patch);
        match merged.try_into::<Config>() {
            Ok(c) => c,
            Err(e) => {
                return ServerMessage::error(format!("invalid config after merge: {e}"));
            }
        }
    };

    // Save to disk.
    if let Err(e) = new_config.save(Config::config_path()) {
        return ServerMessage::error(format!("failed to save config: {e}"));
    }

    // Reload privacy filter.
    {
        let new_filter = match nixclip_core::pipeline::PrivacyFilter::new(&new_config.ignore) {
            Ok(f) => f,
            Err(e) => {
                return ServerMessage::error(format!("invalid privacy filter config: {e}"));
            }
        };
        let mut filter = state.privacy_filter.write().await;
        *filter = new_filter;
    }

    // Update the in-memory config.
    {
        let mut config = state.config.write().await;
        *config = new_config.clone();
    }

    info!("configuration updated via IPC");
    ServerMessage::config_value(new_config)
}

// ---------------------------------------------------------------------------
// TOML merging
// ---------------------------------------------------------------------------

/// Recursively merge `patch` into `base`.
///
/// Tables are merged key-by-key; scalar values in `patch` overwrite those in
/// `base`.
fn merge_toml(base: toml::Value, patch: toml::Value) -> toml::Value {
    match (base, patch) {
        (toml::Value::Table(mut base_table), toml::Value::Table(patch_table)) => {
            for (key, patch_val) in patch_table {
                let merged = if let Some(base_val) = base_table.remove(&key) {
                    merge_toml(base_val, patch_val)
                } else {
                    patch_val
                };
                base_table.insert(key, merged);
            }
            toml::Value::Table(base_table)
        }
        (_base, patch) => patch,
    }
}
