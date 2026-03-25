//! IPC message types and frame protocol for NixClip.
//!
//! All communication between the daemon, CLI, and UI occurs over a Unix domain
//! socket using a simple framed protocol:
//!
//!   [u32 big-endian length][MessagePack-encoded payload]
//!
//! Messages are serialized with `rmp_serde` using named fields so that
//! previously-unknown fields can be safely ignored by older peers.

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::config::Config;
use crate::error::{NixClipError, Result};
use crate::{EntryId, EntrySummary, RestoreMode};

/// Wire-protocol version advertised by this build.
pub const PROTOCOL_VERSION: u32 = 1;

/// Maximum allowed frame payload size (64 MiB).
const MAX_FRAME_SIZE: u32 = 64 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Client -> Server messages
// ---------------------------------------------------------------------------

/// Messages sent from a CLI / UI client to the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientMessage {
    /// Begin a subscription; the daemon will push `NewEntry` events until the
    /// connection is closed.
    Subscribe { version: u32 },

    /// Full-text / filtered search of the history.
    Query {
        version: u32,
        text: Option<String>,
        content_class: Option<String>,
        offset: u32,
        limit: u32,
    },

    /// Restore a history entry to the clipboard (and optionally paste it).
    Restore {
        version: u32,
        id: EntryId,
        mode: RestoreMode,
    },

    /// Permanently delete one or more history entries.
    Delete { version: u32, ids: Vec<EntryId> },

    /// Pin or unpin a single history entry.
    Pin {
        version: u32,
        id: EntryId,
        pinned: bool,
    },

    /// Remove every unpinned entry from the history.
    ClearUnpinned { version: u32 },

    /// Request the daemon's current configuration.
    GetConfig { version: u32 },

    /// Apply a partial TOML patch to the daemon's configuration.
    SetConfig { version: u32, patch: toml::Value },

    /// Fetch a single entry by ID.
    GetEntry { version: u32, id: EntryId },
}

impl ClientMessage {
    /// Construct a versioned `Subscribe` message.
    pub fn subscribe() -> Self {
        Self::Subscribe {
            version: PROTOCOL_VERSION,
        }
    }

    /// Construct a versioned `Query` message.
    pub fn query(
        text: Option<String>,
        content_class: Option<String>,
        offset: u32,
        limit: u32,
    ) -> Self {
        Self::Query {
            version: PROTOCOL_VERSION,
            text,
            content_class,
            offset,
            limit,
        }
    }

    /// Construct a versioned `Restore` message.
    pub fn restore(id: EntryId, mode: RestoreMode) -> Self {
        Self::Restore {
            version: PROTOCOL_VERSION,
            id,
            mode,
        }
    }

    /// Construct a versioned `Delete` message.
    pub fn delete(ids: Vec<EntryId>) -> Self {
        Self::Delete {
            version: PROTOCOL_VERSION,
            ids,
        }
    }

    /// Construct a versioned `Pin` message.
    pub fn pin(id: EntryId, pinned: bool) -> Self {
        Self::Pin {
            version: PROTOCOL_VERSION,
            id,
            pinned,
        }
    }

    /// Construct a versioned `ClearUnpinned` message.
    pub fn clear_unpinned() -> Self {
        Self::ClearUnpinned {
            version: PROTOCOL_VERSION,
        }
    }

    /// Construct a versioned `GetConfig` message.
    pub fn get_config() -> Self {
        Self::GetConfig {
            version: PROTOCOL_VERSION,
        }
    }

    /// Construct a versioned `SetConfig` message.
    pub fn set_config(patch: toml::Value) -> Self {
        Self::SetConfig {
            version: PROTOCOL_VERSION,
            patch,
        }
    }

    /// Construct a versioned `GetEntry` message.
    pub fn get_entry(id: EntryId) -> Self {
        Self::GetEntry {
            version: PROTOCOL_VERSION,
            id,
        }
    }

    /// Return the `version` field regardless of the variant.
    pub fn version(&self) -> u32 {
        match self {
            Self::Subscribe { version }
            | Self::ClearUnpinned { version }
            | Self::GetConfig { version } => *version,
            Self::Query { version, .. }
            | Self::Restore { version, .. }
            | Self::Delete { version, .. }
            | Self::Pin { version, .. }
            | Self::SetConfig { version, .. }
            | Self::GetEntry { version, .. } => *version,
        }
    }
}

// ---------------------------------------------------------------------------
// Server -> Client messages
// ---------------------------------------------------------------------------

/// Messages sent from the daemon to a connected client.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ServerMessage {
    /// Push notification for a newly captured clipboard entry (subscribers).
    NewEntry { version: u32, entry: EntrySummary },

    /// Response to a `Query` request.
    QueryResult {
        version: u32,
        entries: Vec<EntrySummary>,
        total: u32,
    },

    /// Response to a `Restore` request.
    RestoreResult {
        version: u32,
        success: bool,
        error: Option<String>,
    },

    /// Response to a `GetConfig` or `SetConfig` request.
    ConfigValue { version: u32, config: Config },

    /// Response to a `GetEntry` request.
    EntryDetail {
        version: u32,
        entry: Option<EntrySummary>,
    },

    /// Generic error response.
    Error { version: u32, message: String },

    /// Generic success acknowledgement (used for `Delete`, `Pin`, etc.).
    Ok { version: u32 },
}

impl ServerMessage {
    /// Construct a versioned `NewEntry` message.
    pub fn new_entry(entry: EntrySummary) -> Self {
        Self::NewEntry {
            version: PROTOCOL_VERSION,
            entry,
        }
    }

    /// Construct a versioned `QueryResult` message.
    pub fn query_result(entries: Vec<EntrySummary>, total: u32) -> Self {
        Self::QueryResult {
            version: PROTOCOL_VERSION,
            entries,
            total,
        }
    }

    /// Construct a versioned `RestoreResult` message indicating success.
    pub fn restore_ok() -> Self {
        Self::RestoreResult {
            version: PROTOCOL_VERSION,
            success: true,
            error: None,
        }
    }

    /// Construct a versioned `RestoreResult` message indicating failure.
    pub fn restore_err(message: impl Into<String>) -> Self {
        Self::RestoreResult {
            version: PROTOCOL_VERSION,
            success: false,
            error: Some(message.into()),
        }
    }

    /// Construct a versioned `ConfigValue` message.
    pub fn config_value(config: Config) -> Self {
        Self::ConfigValue {
            version: PROTOCOL_VERSION,
            config,
        }
    }

    /// Construct a versioned `EntryDetail` message.
    pub fn entry_detail(entry: Option<EntrySummary>) -> Self {
        Self::EntryDetail {
            version: PROTOCOL_VERSION,
            entry,
        }
    }

    /// Construct a versioned `Error` message.
    pub fn error(message: impl Into<String>) -> Self {
        Self::Error {
            version: PROTOCOL_VERSION,
            message: message.into(),
        }
    }

    /// Construct a versioned `Ok` message.
    pub fn ok() -> Self {
        Self::Ok {
            version: PROTOCOL_VERSION,
        }
    }

    /// Return the `version` field regardless of the variant.
    pub fn version(&self) -> u32 {
        match self {
            Self::NewEntry { version, .. }
            | Self::QueryResult { version, .. }
            | Self::RestoreResult { version, .. }
            | Self::EntryDetail { version, .. }
            | Self::ConfigValue { version, .. }
            | Self::Error { version, .. }
            | Self::Ok { version } => *version,
        }
    }
}

// ---------------------------------------------------------------------------
// Serialization helpers
// ---------------------------------------------------------------------------

/// Serialize `msg` to a MessagePack byte vector using named fields.
///
/// Named-field encoding keeps the format forward-compatible: receivers that
/// do not know a field will simply ignore it.
pub fn encode_message<T: Serialize>(msg: &T) -> Result<Vec<u8>> {
    rmp_serde::to_vec_named(msg)
        .map_err(|e| NixClipError::Serialization(format!("encode failed: {e}")))
}

/// Deserialize a MessagePack byte slice into `T`.
pub fn decode_message<T: DeserializeOwned>(data: &[u8]) -> Result<T> {
    rmp_serde::from_slice(data)
        .map_err(|e| NixClipError::Serialization(format!("decode failed: {e}")))
}

// ---------------------------------------------------------------------------
// Frame I/O
// ---------------------------------------------------------------------------

/// Write a length-prefixed frame to `writer` and flush.
///
/// Frame layout:
/// ```text
/// +----------------------+-------------------------+
/// |  length (u32, BE)    |  payload (length bytes) |
/// +----------------------+-------------------------+
/// ```
///
/// # Errors
///
/// Returns [`NixClipError::Ipc`] if `data` exceeds [`MAX_FRAME_SIZE`].
/// Returns [`NixClipError::Io`] on any underlying I/O failure.
pub async fn write_frame<W>(writer: &mut W, data: &[u8]) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    let len = data.len();
    if len > MAX_FRAME_SIZE as usize {
        return Err(NixClipError::Ipc(format!(
            "frame too large: {len} bytes (max {MAX_FRAME_SIZE})"
        )));
    }

    // Write the 4-byte big-endian length prefix.
    writer.write_u32(len as u32).await?;
    // Write the payload (zero-length payloads are valid).
    if len > 0 {
        writer.write_all(data).await?;
    }
    writer.flush().await?;
    Ok(())
}

/// Read a length-prefixed frame from `reader` and return the payload bytes.
///
/// # Errors
///
/// - [`NixClipError::Ipc`] `"connection closed"` -- the peer closed the
///   connection before sending any bytes of the length prefix.
/// - [`NixClipError::Ipc`] `"frame too large"` -- the advertised length
///   exceeds [`MAX_FRAME_SIZE`].
/// - [`NixClipError::Io`] -- the underlying I/O returned an error, or
///   the stream ended in the middle of the length prefix or payload.
pub async fn read_frame<R>(reader: &mut R) -> Result<Vec<u8>>
where
    R: AsyncReadExt + Unpin,
{
    // Read the 4-byte big-endian length prefix.
    // We peek at the very first byte manually so we can distinguish a clean
    // EOF (connection closed before any data) from a mid-read EOF (which
    // `read_exact` surfaces as an `UnexpectedEof` I/O error).
    let mut len_buf = [0u8; 4];
    match reader.read(&mut len_buf[..1]).await? {
        0 => {
            // Peer closed the connection cleanly before sending anything.
            return Err(NixClipError::Ipc("connection closed".into()));
        }
        _ => {
            // Got the first byte; read the remaining 3 bytes exactly.
            // A mid-stream EOF here becomes `std::io::ErrorKind::UnexpectedEof`,
            // which is mapped to `NixClipError::Io` via the `From` impl.
            reader.read_exact(&mut len_buf[1..]).await?;
        }
    }

    let length = u32::from_be_bytes(len_buf);

    if length > MAX_FRAME_SIZE {
        return Err(NixClipError::Ipc(format!(
            "frame too large: {length} bytes (max {MAX_FRAME_SIZE})"
        )));
    }

    // A zero-length frame is valid (e.g., a keep-alive ping).
    if length == 0 {
        return Ok(Vec::new());
    }

    // Allocate exactly the right amount and fill it.
    let mut payload = vec![0u8; length as usize];
    reader.read_exact(&mut payload).await?;
    Ok(payload)
}

// ---------------------------------------------------------------------------
// Convenience send / receive
// ---------------------------------------------------------------------------

/// Encode `msg` and send it as a framed message on `writer`.
pub async fn send_message<W, T>(writer: &mut W, msg: &T) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
    T: Serialize,
{
    let data = encode_message(msg)?;
    write_frame(writer, &data).await
}

/// Read one framed message from `reader` and decode it as `T`.
pub async fn recv_message<R, T>(reader: &mut R) -> Result<T>
where
    R: AsyncReadExt + Unpin,
    T: DeserializeOwned,
{
    let data = read_frame(reader).await?;
    decode_message(&data)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Serialization round-trips
    // -----------------------------------------------------------------------

    #[test]
    fn client_message_subscribe_round_trip() {
        let msg = ClientMessage::subscribe();
        let encoded = encode_message(&msg).expect("encode");
        let decoded: ClientMessage = decode_message(&encoded).expect("decode");
        assert!(matches!(
            decoded,
            ClientMessage::Subscribe {
                version: PROTOCOL_VERSION
            }
        ));
    }

    #[test]
    fn client_message_query_round_trip() {
        let msg = ClientMessage::query(Some("hello".into()), Some("text".into()), 0, 20);
        let encoded = encode_message(&msg).expect("encode");
        let decoded: ClientMessage = decode_message(&encoded).expect("decode");
        match decoded {
            ClientMessage::Query {
                text,
                content_class,
                offset,
                limit,
                ..
            } => {
                assert_eq!(text.as_deref(), Some("hello"));
                assert_eq!(content_class.as_deref(), Some("text"));
                assert_eq!(offset, 0);
                assert_eq!(limit, 20);
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn server_message_ok_round_trip() {
        let msg = ServerMessage::ok();
        let encoded = encode_message(&msg).expect("encode");
        let decoded: ServerMessage = decode_message(&encoded).expect("decode");
        assert!(matches!(
            decoded,
            ServerMessage::Ok {
                version: PROTOCOL_VERSION
            }
        ));
    }

    #[test]
    fn server_message_error_round_trip() {
        let msg = ServerMessage::error("something went wrong");
        let encoded = encode_message(&msg).expect("encode");
        let decoded: ServerMessage = decode_message(&encoded).expect("decode");
        match decoded {
            ServerMessage::Error { message, .. } => {
                assert_eq!(message, "something went wrong");
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // Frame protocol
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn write_then_read_frame_round_trip() {
        let payload = b"hello frame";
        let mut buf: Vec<u8> = Vec::new();
        write_frame(&mut buf, payload).await.expect("write_frame");

        // First 4 bytes must be the big-endian length.
        let written_len = u32::from_be_bytes(buf[..4].try_into().unwrap());
        assert_eq!(written_len, payload.len() as u32);

        let mut cursor = std::io::Cursor::new(buf);
        let received = read_frame(&mut cursor).await.expect("read_frame");
        assert_eq!(received.as_slice(), payload);
    }

    #[tokio::test]
    async fn zero_length_frame_is_valid() {
        let mut buf: Vec<u8> = Vec::new();
        write_frame(&mut buf, &[]).await.expect("write_frame empty");

        let mut cursor = std::io::Cursor::new(buf);
        let received = read_frame(&mut cursor).await.expect("read_frame empty");
        assert!(received.is_empty());
    }

    #[tokio::test]
    async fn read_frame_detects_clean_eof() {
        // Empty stream -- peer closed connection before writing anything.
        let buf: Vec<u8> = Vec::new();
        let mut cursor = std::io::Cursor::new(buf);
        let err = read_frame(&mut cursor)
            .await
            .expect_err("should be EOF error");
        assert!(
            matches!(err, NixClipError::Ipc(ref s) if s.contains("connection closed")),
            "unexpected error: {err}",
        );
    }

    #[tokio::test]
    async fn read_frame_rejects_oversized_frame() {
        // Write a length header that exceeds MAX_FRAME_SIZE, with no payload.
        // The guard must fire before any attempt to read payload bytes.
        let oversized_len: u32 = MAX_FRAME_SIZE + 1;
        let buf = oversized_len.to_be_bytes().to_vec();

        let mut cursor = std::io::Cursor::new(buf);
        let err = read_frame(&mut cursor)
            .await
            .expect_err("should be too-large error");
        assert!(
            matches!(err, NixClipError::Ipc(ref s) if s.contains("frame too large")),
            "unexpected error: {err}",
        );
    }

    #[tokio::test]
    async fn write_frame_rejects_oversized_payload() {
        // Allocate MAX_FRAME_SIZE + 1 bytes and confirm write_frame rejects it.
        let big: Vec<u8> = vec![0u8; (MAX_FRAME_SIZE + 1) as usize];
        let mut buf: Vec<u8> = Vec::new();
        let err = write_frame(&mut buf, &big)
            .await
            .expect_err("should reject oversized");
        assert!(
            matches!(err, NixClipError::Ipc(ref s) if s.contains("frame too large")),
            "unexpected error: {err}",
        );
    }

    #[tokio::test]
    async fn send_recv_message_round_trip() {
        let msg = ClientMessage::clear_unpinned();
        let mut buf: Vec<u8> = Vec::new();
        send_message(&mut buf, &msg).await.expect("send_message");

        let mut cursor = std::io::Cursor::new(buf);
        let received: ClientMessage = recv_message(&mut cursor).await.expect("recv_message");
        assert!(matches!(
            received,
            ClientMessage::ClearUnpinned {
                version: PROTOCOL_VERSION
            }
        ));
    }

    // -----------------------------------------------------------------------
    // Constructor helpers
    // -----------------------------------------------------------------------

    #[test]
    fn client_message_version_accessor() {
        assert_eq!(ClientMessage::subscribe().version(), PROTOCOL_VERSION);
        assert_eq!(ClientMessage::clear_unpinned().version(), PROTOCOL_VERSION);
        assert_eq!(ClientMessage::get_config().version(), PROTOCOL_VERSION);
        assert_eq!(ClientMessage::delete(vec![]).version(), PROTOCOL_VERSION);
    }

    #[test]
    fn server_message_version_accessor() {
        assert_eq!(ServerMessage::ok().version(), PROTOCOL_VERSION);
        assert_eq!(ServerMessage::error("x").version(), PROTOCOL_VERSION);
        assert_eq!(ServerMessage::restore_ok().version(), PROTOCOL_VERSION);
        assert_eq!(ServerMessage::restore_err("x").version(), PROTOCOL_VERSION);
    }

    #[test]
    fn restore_result_fields() {
        let ok = ServerMessage::restore_ok();
        match ok {
            ServerMessage::RestoreResult { success, error, .. } => {
                assert!(success);
                assert!(error.is_none());
            }
            _ => panic!("wrong variant"),
        }

        let err = ServerMessage::restore_err("oops");
        match err {
            ServerMessage::RestoreResult { success, error, .. } => {
                assert!(!success);
                assert_eq!(error.as_deref(), Some("oops"));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn set_config_round_trip() {
        let patch = toml::Value::Table({
            let mut t = toml::map::Map::new();
            t.insert("max_entries".into(), toml::Value::Integer(500));
            t
        });
        let msg = ClientMessage::set_config(patch.clone());
        let encoded = encode_message(&msg).expect("encode");
        let decoded: ClientMessage = decode_message(&encoded).expect("decode");
        match decoded {
            ClientMessage::SetConfig {
                patch: decoded_patch,
                ..
            } => {
                assert_eq!(decoded_patch, patch);
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn query_with_no_filters_round_trip() {
        let msg = ClientMessage::query(None, None, 10, 50);
        let encoded = encode_message(&msg).expect("encode");
        let decoded: ClientMessage = decode_message(&encoded).expect("decode");
        match decoded {
            ClientMessage::Query {
                text,
                content_class,
                offset,
                limit,
                ..
            } => {
                assert!(text.is_none());
                assert!(content_class.is_none());
                assert_eq!(offset, 10);
                assert_eq!(limit, 50);
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }
}
