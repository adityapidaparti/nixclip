/// Integration tests for IPC message encoding/decoding and the frame protocol.
use nixclip_core::ipc::{
    ClientMessage, ServerMessage,
    encode_message, decode_message,
    write_frame, read_frame,
    send_message, recv_message,
    PROTOCOL_VERSION,
};
use nixclip_core::{ContentClass, EntryId, RestoreMode};

// ===========================================================================
// encode_message / decode_message round-trips
// ===========================================================================

// ---------------------------------------------------------------------------
// ClientMessage
// ---------------------------------------------------------------------------

#[test]
fn client_subscribe_round_trip() {
    let msg = ClientMessage::subscribe();
    let bytes = encode_message(&msg).expect("encode Subscribe");
    let decoded: ClientMessage = decode_message(&bytes).expect("decode Subscribe");
    assert!(
        matches!(decoded, ClientMessage::Subscribe { version: PROTOCOL_VERSION }),
        "decoded variant should be Subscribe with correct version"
    );
}

#[test]
fn client_query_round_trip_with_filters() {
    let msg = ClientMessage::query(
        Some("search term".into()),
        Some("text".into()),
        5,
        25,
    );
    let bytes = encode_message(&msg).expect("encode Query");
    let decoded: ClientMessage = decode_message(&bytes).expect("decode Query");
    match decoded {
        ClientMessage::Query { version, text, content_class, offset, limit } => {
            assert_eq!(version, PROTOCOL_VERSION);
            assert_eq!(text.as_deref(), Some("search term"));
            assert_eq!(content_class.as_deref(), Some("text"));
            assert_eq!(offset, 5);
            assert_eq!(limit, 25);
        }
        other => panic!("expected Query, got {other:?}"),
    }
}

#[test]
fn client_query_round_trip_no_filters() {
    let msg = ClientMessage::query(None, None, 0, 50);
    let bytes = encode_message(&msg).expect("encode Query no-filters");
    let decoded: ClientMessage = decode_message(&bytes).expect("decode Query no-filters");
    match decoded {
        ClientMessage::Query { text, content_class, offset, limit, .. } => {
            assert!(text.is_none());
            assert!(content_class.is_none());
            assert_eq!(offset, 0);
            assert_eq!(limit, 50);
        }
        other => panic!("expected Query, got {other:?}"),
    }
}

#[test]
fn client_restore_round_trip() {
    let id: EntryId = 42;
    let msg = ClientMessage::restore(id, RestoreMode::PlainText);
    let bytes = encode_message(&msg).expect("encode Restore");
    let decoded: ClientMessage = decode_message(&bytes).expect("decode Restore");
    match decoded {
        ClientMessage::Restore { version, id: decoded_id, mode } => {
            assert_eq!(version, PROTOCOL_VERSION);
            assert_eq!(decoded_id, id);
            assert_eq!(mode, RestoreMode::PlainText);
        }
        other => panic!("expected Restore, got {other:?}"),
    }
}

#[test]
fn client_restore_original_mode_round_trip() {
    let msg = ClientMessage::restore(1, RestoreMode::Original);
    let bytes = encode_message(&msg).expect("encode");
    let decoded: ClientMessage = decode_message(&bytes).expect("decode");
    match decoded {
        ClientMessage::Restore { mode, .. } => assert_eq!(mode, RestoreMode::Original),
        other => panic!("expected Restore, got {other:?}"),
    }
}

#[test]
fn client_delete_round_trip() {
    let ids: Vec<EntryId> = vec![1, 2, 3];
    let msg = ClientMessage::delete(ids.clone());
    let bytes = encode_message(&msg).expect("encode Delete");
    let decoded: ClientMessage = decode_message(&bytes).expect("decode Delete");
    match decoded {
        ClientMessage::Delete { ids: decoded_ids, .. } => {
            assert_eq!(decoded_ids, ids);
        }
        other => panic!("expected Delete, got {other:?}"),
    }
}

#[test]
fn client_delete_empty_ids() {
    let msg = ClientMessage::delete(vec![]);
    let bytes = encode_message(&msg).expect("encode");
    let decoded: ClientMessage = decode_message(&bytes).expect("decode");
    match decoded {
        ClientMessage::Delete { ids, .. } => assert!(ids.is_empty()),
        other => panic!("expected Delete, got {other:?}"),
    }
}

#[test]
fn client_pin_true_round_trip() {
    let msg = ClientMessage::pin(99, true);
    let bytes = encode_message(&msg).expect("encode Pin");
    let decoded: ClientMessage = decode_message(&bytes).expect("decode Pin");
    match decoded {
        ClientMessage::Pin { id, pinned, .. } => {
            assert_eq!(id, 99);
            assert!(pinned);
        }
        other => panic!("expected Pin, got {other:?}"),
    }
}

#[test]
fn client_pin_false_round_trip() {
    let msg = ClientMessage::pin(7, false);
    let bytes = encode_message(&msg).expect("encode");
    let decoded: ClientMessage = decode_message(&bytes).expect("decode");
    match decoded {
        ClientMessage::Pin { id, pinned, .. } => {
            assert_eq!(id, 7);
            assert!(!pinned);
        }
        other => panic!("expected Pin, got {other:?}"),
    }
}

#[test]
fn client_clear_unpinned_round_trip() {
    let msg = ClientMessage::clear_unpinned();
    let bytes = encode_message(&msg).expect("encode ClearUnpinned");
    let decoded: ClientMessage = decode_message(&bytes).expect("decode ClearUnpinned");
    assert!(matches!(decoded, ClientMessage::ClearUnpinned { version: PROTOCOL_VERSION }));
}

#[test]
fn client_get_config_round_trip() {
    let msg = ClientMessage::get_config();
    let bytes = encode_message(&msg).expect("encode GetConfig");
    let decoded: ClientMessage = decode_message(&bytes).expect("decode GetConfig");
    assert!(matches!(decoded, ClientMessage::GetConfig { version: PROTOCOL_VERSION }));
}

#[test]
fn client_set_config_round_trip() {
    let patch = toml::Value::Table({
        let mut t = toml::map::Map::new();
        t.insert("max_entries".into(), toml::Value::Integer(200));
        t
    });
    let msg = ClientMessage::set_config(patch.clone());
    let bytes = encode_message(&msg).expect("encode SetConfig");
    let decoded: ClientMessage = decode_message(&bytes).expect("decode SetConfig");
    match decoded {
        ClientMessage::SetConfig { patch: dp, .. } => assert_eq!(dp, patch),
        other => panic!("expected SetConfig, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// ClientMessage::version()
// ---------------------------------------------------------------------------

#[test]
fn client_version_accessor_all_variants() {
    assert_eq!(ClientMessage::subscribe().version(), PROTOCOL_VERSION);
    assert_eq!(ClientMessage::clear_unpinned().version(), PROTOCOL_VERSION);
    assert_eq!(ClientMessage::get_config().version(), PROTOCOL_VERSION);
    assert_eq!(ClientMessage::query(None, None, 0, 10).version(), PROTOCOL_VERSION);
    assert_eq!(ClientMessage::restore(1, RestoreMode::Original).version(), PROTOCOL_VERSION);
    assert_eq!(ClientMessage::delete(vec![]).version(), PROTOCOL_VERSION);
    assert_eq!(ClientMessage::pin(1, true).version(), PROTOCOL_VERSION);
    let patch = toml::Value::Boolean(false);
    assert_eq!(ClientMessage::set_config(patch).version(), PROTOCOL_VERSION);
}

// ---------------------------------------------------------------------------
// ServerMessage
// ---------------------------------------------------------------------------

#[test]
fn server_ok_round_trip() {
    let msg = ServerMessage::ok();
    let bytes = encode_message(&msg).expect("encode Ok");
    let decoded: ServerMessage = decode_message(&bytes).expect("decode Ok");
    assert!(matches!(decoded, ServerMessage::Ok { version: PROTOCOL_VERSION }));
}

#[test]
fn server_error_round_trip() {
    let msg = ServerMessage::error("something went wrong");
    let bytes = encode_message(&msg).expect("encode Error");
    let decoded: ServerMessage = decode_message(&bytes).expect("decode Error");
    match decoded {
        ServerMessage::Error { message, version } => {
            assert_eq!(message, "something went wrong");
            assert_eq!(version, PROTOCOL_VERSION);
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[test]
fn server_restore_ok_round_trip() {
    let msg = ServerMessage::restore_ok();
    let bytes = encode_message(&msg).expect("encode RestoreResult ok");
    let decoded: ServerMessage = decode_message(&bytes).expect("decode RestoreResult ok");
    match decoded {
        ServerMessage::RestoreResult { success, error, .. } => {
            assert!(success);
            assert!(error.is_none());
        }
        other => panic!("expected RestoreResult, got {other:?}"),
    }
}

#[test]
fn server_restore_err_round_trip() {
    let msg = ServerMessage::restore_err("clipboard unavailable");
    let bytes = encode_message(&msg).expect("encode RestoreResult err");
    let decoded: ServerMessage = decode_message(&bytes).expect("decode RestoreResult err");
    match decoded {
        ServerMessage::RestoreResult { success, error, .. } => {
            assert!(!success);
            assert_eq!(error.as_deref(), Some("clipboard unavailable"));
        }
        other => panic!("expected RestoreResult, got {other:?}"),
    }
}

#[test]
fn server_config_value_round_trip() {
    let config = nixclip_core::config::Config::default();
    let msg = ServerMessage::config_value(config.clone());
    let bytes = encode_message(&msg).expect("encode ConfigValue");
    let decoded: ServerMessage = decode_message(&bytes).expect("decode ConfigValue");
    match decoded {
        ServerMessage::ConfigValue { config: dc, .. } => {
            assert_eq!(dc.general.max_entries, config.general.max_entries);
        }
        other => panic!("expected ConfigValue, got {other:?}"),
    }
}

#[test]
fn server_query_result_round_trip() {
    use nixclip_core::EntrySummary;
    let summary = EntrySummary {
        id: 1,
        created_at: 1_000_000,
        last_seen_at: 1_000_001,
        pinned: false,
        ephemeral: false,
        content_class: ContentClass::Text,
        preview_text: Some("hello".to_string()),
        source_app: None,
        thumbnail: None,
    };
    let msg = ServerMessage::query_result(vec![summary], 1);
    let bytes = encode_message(&msg).expect("encode QueryResult");
    let decoded: ServerMessage = decode_message(&bytes).expect("decode QueryResult");
    match decoded {
        ServerMessage::QueryResult { entries, total, .. } => {
            assert_eq!(total, 1);
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].preview_text.as_deref(), Some("hello"));
        }
        other => panic!("expected QueryResult, got {other:?}"),
    }
}

#[test]
fn server_new_entry_round_trip() {
    use nixclip_core::EntrySummary;
    let summary = EntrySummary {
        id: 5,
        created_at: 2_000_000,
        last_seen_at: 2_000_000,
        pinned: true,
        ephemeral: false,
        content_class: ContentClass::Url,
        preview_text: Some("https://example.com".to_string()),
        source_app: Some("org.test.App".to_string()),
        thumbnail: None,
    };
    let msg = ServerMessage::new_entry(summary.clone());
    let bytes = encode_message(&msg).expect("encode NewEntry");
    let decoded: ServerMessage = decode_message(&bytes).expect("decode NewEntry");
    match decoded {
        ServerMessage::NewEntry { entry, .. } => {
            assert_eq!(entry.id, 5);
            assert!(entry.pinned);
            assert_eq!(entry.content_class, ContentClass::Url);
        }
        other => panic!("expected NewEntry, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// ServerMessage::version()
// ---------------------------------------------------------------------------

#[test]
fn server_version_accessor_all_variants() {
    assert_eq!(ServerMessage::ok().version(), PROTOCOL_VERSION);
    assert_eq!(ServerMessage::error("x").version(), PROTOCOL_VERSION);
    assert_eq!(ServerMessage::restore_ok().version(), PROTOCOL_VERSION);
    assert_eq!(ServerMessage::restore_err("x").version(), PROTOCOL_VERSION);
    assert_eq!(
        ServerMessage::config_value(nixclip_core::config::Config::default()).version(),
        PROTOCOL_VERSION
    );
}

// ===========================================================================
// write_frame / read_frame
// ===========================================================================

#[tokio::test]
async fn write_then_read_frame_round_trip() {
    let payload = b"frame test payload";
    let mut buf: Vec<u8> = Vec::new();
    write_frame(&mut buf, payload).await.expect("write_frame");

    // The first 4 bytes must encode the length.
    let written_len = u32::from_be_bytes(buf[..4].try_into().unwrap());
    assert_eq!(written_len, payload.len() as u32);

    let mut cursor = std::io::Cursor::new(buf);
    let received = read_frame(&mut cursor).await.expect("read_frame");
    assert_eq!(received.as_slice(), payload);
}

#[tokio::test]
async fn zero_length_frame_is_valid() {
    let mut buf: Vec<u8> = Vec::new();
    write_frame(&mut buf, &[]).await.expect("write empty frame");

    let mut cursor = std::io::Cursor::new(buf);
    let received = read_frame(&mut cursor).await.expect("read empty frame");
    assert!(received.is_empty(), "zero-length frame should return empty vec");
}

#[tokio::test]
async fn read_frame_clean_eof_returns_ipc_error() {
    // Completely empty reader — peer closed before writing anything.
    let buf: Vec<u8> = Vec::new();
    let mut cursor = std::io::Cursor::new(buf);
    let err = read_frame(&mut cursor).await.expect_err("should fail on EOF");
    assert!(
        matches!(&err, nixclip_core::NixClipError::Ipc(s) if s.contains("connection closed")),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn read_frame_rejects_oversized_length_header() {
    // Write a length header larger than MAX_FRAME_SIZE (64 MiB).
    let oversized: u32 = 64 * 1024 * 1024 + 1;
    let buf = oversized.to_be_bytes().to_vec();

    let mut cursor = std::io::Cursor::new(buf);
    let err = read_frame(&mut cursor).await.expect_err("should fail on oversized");
    assert!(
        matches!(&err, nixclip_core::NixClipError::Ipc(s) if s.contains("frame too large")),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn write_frame_rejects_oversized_payload() {
    let big: Vec<u8> = vec![0u8; 64 * 1024 * 1024 + 1];
    let mut buf: Vec<u8> = Vec::new();
    let err = write_frame(&mut buf, &big).await.expect_err("should reject big payload");
    assert!(
        matches!(&err, nixclip_core::NixClipError::Ipc(s) if s.contains("frame too large")),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn read_frame_truncated_payload_is_io_error() {
    // Write a header claiming 100 bytes but only supply 10.
    let mut buf: Vec<u8> = 100u32.to_be_bytes().to_vec();
    buf.extend_from_slice(&[0u8; 10]);

    let mut cursor = std::io::Cursor::new(buf);
    let err = read_frame(&mut cursor).await.expect_err("truncated payload should error");
    // UnexpectedEof from read_exact → NixClipError::Io
    assert!(
        matches!(err, nixclip_core::NixClipError::Io(_)),
        "expected Io error, got {err}"
    );
}

#[tokio::test]
async fn multiple_frames_in_sequence() {
    let mut buf: Vec<u8> = Vec::new();
    write_frame(&mut buf, b"first").await.expect("write 1");
    write_frame(&mut buf, b"second").await.expect("write 2");
    write_frame(&mut buf, b"third").await.expect("write 3");

    let mut cursor = std::io::Cursor::new(buf);
    let f1 = read_frame(&mut cursor).await.expect("read 1");
    let f2 = read_frame(&mut cursor).await.expect("read 2");
    let f3 = read_frame(&mut cursor).await.expect("read 3");

    assert_eq!(f1, b"first");
    assert_eq!(f2, b"second");
    assert_eq!(f3, b"third");
}

// ===========================================================================
// send_message / recv_message (convenience wrappers)
// ===========================================================================

#[tokio::test]
async fn send_recv_client_message_round_trip() {
    let msg = ClientMessage::clear_unpinned();
    let mut buf: Vec<u8> = Vec::new();
    send_message(&mut buf, &msg).await.expect("send_message");

    let mut cursor = std::io::Cursor::new(buf);
    let received: ClientMessage = recv_message(&mut cursor).await.expect("recv_message");
    assert!(matches!(
        received,
        ClientMessage::ClearUnpinned { version: PROTOCOL_VERSION }
    ));
}

#[tokio::test]
async fn send_recv_server_message_round_trip() {
    let msg = ServerMessage::ok();
    let mut buf: Vec<u8> = Vec::new();
    send_message(&mut buf, &msg).await.expect("send_message");

    let mut cursor = std::io::Cursor::new(buf);
    let received: ServerMessage = recv_message(&mut cursor).await.expect("recv_message");
    assert!(matches!(received, ServerMessage::Ok { version: PROTOCOL_VERSION }));
}

#[tokio::test]
async fn send_recv_query_message() {
    let msg = ClientMessage::query(Some("rust".into()), None, 0, 20);
    let mut buf: Vec<u8> = Vec::new();
    send_message(&mut buf, &msg).await.expect("send");

    let mut cursor = std::io::Cursor::new(buf);
    let received: ClientMessage = recv_message(&mut cursor).await.expect("recv");
    match received {
        ClientMessage::Query { text, limit, .. } => {
            assert_eq!(text.as_deref(), Some("rust"));
            assert_eq!(limit, 20);
        }
        other => panic!("expected Query, got {other:?}"),
    }
}

// ===========================================================================
// tokio::io::duplex — bidirectional in-memory stream
// ===========================================================================

#[tokio::test]
async fn duplex_write_read_frame() {
    use tokio::io::AsyncWriteExt;

    let (mut client, mut server) = tokio::io::duplex(1024);

    let payload = b"duplex payload";
    write_frame(&mut client, payload).await.expect("write");
    // Drop the write half so the server can read EOF cleanly after the frame.
    client.shutdown().await.ok();

    let received = read_frame(&mut server).await.expect("read");
    assert_eq!(received.as_slice(), payload);
}

#[tokio::test]
async fn duplex_bidirectional_messages() {
    let (mut client, mut server) = tokio::io::duplex(4096);

    // Client sends a query.
    let query = ClientMessage::query(Some("test".into()), None, 0, 10);
    send_message(&mut client, &query).await.expect("client send");

    // Server receives the query.
    let received: ClientMessage = recv_message(&mut server).await.expect("server recv");
    assert!(matches!(received, ClientMessage::Query { .. }));

    // Server replies.
    let reply = ServerMessage::ok();
    send_message(&mut server, &reply).await.expect("server send");

    // Client receives the reply.
    let server_reply: ServerMessage = recv_message(&mut client).await.expect("client recv");
    assert!(matches!(server_reply, ServerMessage::Ok { .. }));
}
