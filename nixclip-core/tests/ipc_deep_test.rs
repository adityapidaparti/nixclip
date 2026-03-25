/// Deep IPC protocol tests verifying wire-format details and edge cases.
///
/// These tests go beyond basic round-trip checks to validate:
///   - MessagePack named-field encoding (forward-compat)
///   - version field presence in every variant
///   - serde tag = "type" discriminant wire-format
///   - Frame boundary conditions
///   - Sequential frame I/O
///   - Partial / truncated frame handling
use nixclip_core::ipc::{
    ClientMessage, ServerMessage,
    encode_message, decode_message,
    write_frame, read_frame,
    PROTOCOL_VERSION,
};
use nixclip_core::{EntryId, RestoreMode};

// ===========================================================================
// Test 1 — MessagePack named-field encoding (not positional)
//
// `rmp_serde::to_vec_named` produces a MessagePack **map** for each struct /
// enum variant, keyed by field name strings.  `to_vec` (positional) would
// produce an **array**.  We verify the encoded form contains the string keys
// "version" and "type" so that a future receiver that adds new fields can
// still decode the message.
// ===========================================================================

#[test]
fn msgpack_uses_named_fields_not_positional() {
    let msg = ClientMessage::subscribe();
    let bytes = encode_message(&msg).expect("encode");

    // The bytes must contain the UTF-8 string "version" somewhere.
    // rmp_serde named encoding writes each field key as a msgpack str.
    let bytes_as_str = String::from_utf8_lossy(&bytes);
    assert!(
        bytes_as_str.contains("version"),
        "encoded bytes should contain the 'version' field key; \
         got {} bytes: {:?}",
        bytes.len(),
        &bytes[..bytes.len().min(64)]
    );
}

#[test]
fn msgpack_named_contains_type_key() {
    // The `#[serde(tag = "type")]` attribute means serde injects a "type"
    // field with the variant name into the map.
    let msg = ClientMessage::subscribe();
    let bytes = encode_message(&msg).expect("encode");

    let bytes_as_str = String::from_utf8_lossy(&bytes);
    assert!(
        bytes_as_str.contains("type"),
        "encoded bytes should contain the serde tag key 'type'; \
         got {} raw bytes",
        bytes.len()
    );
}

#[test]
fn msgpack_named_type_value_is_variant_name() {
    // Spot-check that the variant name is written as-is (e.g. "Subscribe").
    let msg = ClientMessage::subscribe();
    let bytes = encode_message(&msg).expect("encode");

    let bytes_as_str = String::from_utf8_lossy(&bytes);
    assert!(
        bytes_as_str.contains("Subscribe"),
        "encoded Subscribe variant should contain 'Subscribe' in the payload"
    );
}

#[test]
fn msgpack_query_named_fields_present() {
    let msg = ClientMessage::query(Some("rust".into()), None, 0, 10);
    let bytes = encode_message(&msg).expect("encode");

    let bytes_as_str = String::from_utf8_lossy(&bytes);
    // All field keys must appear in the map.
    for key in &["version", "type", "text", "offset", "limit"] {
        assert!(
            bytes_as_str.contains(key),
            "named encoding of Query should contain field key '{}'; \
             payload snippet: {:?}",
            key,
            &bytes[..bytes.len().min(128)]
        );
    }
}

// ===========================================================================
// Test 2 — All ClientMessage variants carry a version field
// ===========================================================================

#[test]
fn all_client_variants_have_version_field() {
    use toml::Value as TomlValue;

    let variants: Vec<ClientMessage> = vec![
        ClientMessage::subscribe(),
        ClientMessage::query(None, None, 0, 10),
        ClientMessage::restore(1 as EntryId, RestoreMode::Original),
        ClientMessage::delete(vec![]),
        ClientMessage::pin(1, true),
        ClientMessage::clear_unpinned(),
        ClientMessage::get_config(),
        ClientMessage::set_config(TomlValue::Boolean(false)),
    ];

    for variant in &variants {
        let v = variant.version();
        assert_eq!(
            v, PROTOCOL_VERSION,
            "variant {:?} has version {v}, expected {PROTOCOL_VERSION}",
            std::mem::discriminant(variant)
        );
    }
}

#[test]
fn all_client_variants_version_encoded_in_msgpack() {
    use toml::Value as TomlValue;

    let variants: Vec<ClientMessage> = vec![
        ClientMessage::subscribe(),
        ClientMessage::query(None, None, 0, 10),
        ClientMessage::restore(1 as EntryId, RestoreMode::PlainText),
        ClientMessage::delete(vec![1, 2]),
        ClientMessage::pin(3, false),
        ClientMessage::clear_unpinned(),
        ClientMessage::get_config(),
        ClientMessage::set_config(TomlValue::Integer(99)),
    ];

    for variant in variants {
        let bytes = encode_message(&variant).expect("encode");
        let bytes_as_str = String::from_utf8_lossy(&bytes);
        assert!(
            bytes_as_str.contains("version"),
            "encoded {:?} should contain 'version' key in named msgpack",
            std::mem::discriminant(&variant)
        );
    }
}

// ===========================================================================
// Test 3 — All ServerMessage variants carry a version field
// ===========================================================================

#[test]
fn all_server_variants_have_version_field() {
    use nixclip_core::{ContentClass, EntrySummary};

    let entry = EntrySummary {
        id: 1,
        created_at: 0,
        last_seen_at: 0,
        pinned: false,
        ephemeral: false,
        content_class: ContentClass::Text,
        preview_text: Some("test".into()),
        source_app: None,
        thumbnail: None,
    };

    let variants: Vec<ServerMessage> = vec![
        ServerMessage::new_entry(entry.clone()),
        ServerMessage::query_result(vec![entry.clone()], 1),
        ServerMessage::restore_ok(),
        ServerMessage::restore_err("oops"),
        ServerMessage::config_value(nixclip_core::config::Config::default()),
        ServerMessage::error("bad"),
        ServerMessage::ok(),
    ];

    for variant in &variants {
        let v = variant.version();
        assert_eq!(
            v, PROTOCOL_VERSION,
            "ServerMessage variant {:?} has version {v}, expected {PROTOCOL_VERSION}",
            std::mem::discriminant(variant)
        );
    }
}

#[test]
fn all_server_variants_version_encoded_in_msgpack() {
    use nixclip_core::{ContentClass, EntrySummary};

    let entry = EntrySummary {
        id: 2,
        created_at: 0,
        last_seen_at: 0,
        pinned: false,
        ephemeral: false,
        content_class: ContentClass::Text,
        preview_text: None,
        source_app: None,
        thumbnail: None,
    };

    let variants: Vec<ServerMessage> = vec![
        ServerMessage::new_entry(entry.clone()),
        ServerMessage::query_result(vec![], 0),
        ServerMessage::restore_ok(),
        ServerMessage::restore_err("e"),
        ServerMessage::config_value(nixclip_core::config::Config::default()),
        ServerMessage::error("e"),
        ServerMessage::ok(),
    ];

    for variant in variants {
        let bytes = encode_message(&variant).expect("encode");
        let bytes_as_str = String::from_utf8_lossy(&bytes);
        assert!(
            bytes_as_str.contains("version"),
            "encoded {:?} should contain 'version' key",
            std::mem::discriminant(&variant)
        );
    }
}

// ===========================================================================
// Test 4 — Frame with exactly 64 MiB payload (boundary test)
//
// The spec says max frame size is 64 MiB. A payload of exactly MAX_FRAME_SIZE
// bytes must be accepted; MAX_FRAME_SIZE + 1 must be rejected.
// ===========================================================================

#[tokio::test]
async fn write_frame_exactly_64mib_is_accepted() {
    const MAX: usize = 64 * 1024 * 1024;
    let payload = vec![0xABu8; MAX];

    // write_frame should not return an error.
    let mut buf: Vec<u8> = Vec::new();
    write_frame(&mut buf, &payload)
        .await
        .expect("exactly 64 MiB payload must be accepted");

    // Verify the 4-byte length header equals MAX.
    let written_len = u32::from_be_bytes(buf[..4].try_into().unwrap());
    assert_eq!(written_len as usize, MAX);
}

#[tokio::test]
async fn read_frame_exactly_64mib_is_accepted() {
    const MAX: usize = 64 * 1024 * 1024;
    let payload = vec![0xCDu8; MAX];

    let mut buf: Vec<u8> = Vec::new();
    write_frame(&mut buf, &payload).await.expect("write 64 MiB");

    let mut cursor = std::io::Cursor::new(buf);
    let received = read_frame(&mut cursor)
        .await
        .expect("reading exactly 64 MiB must succeed");

    assert_eq!(received.len(), MAX);
    assert_eq!(received[0], 0xCD);
    assert_eq!(received[MAX - 1], 0xCD);
}

#[tokio::test]
async fn write_frame_64mib_plus_one_is_rejected() {
    const TOO_BIG: usize = 64 * 1024 * 1024 + 1;
    let payload = vec![0u8; TOO_BIG];

    let mut buf: Vec<u8> = Vec::new();
    let err = write_frame(&mut buf, &payload)
        .await
        .expect_err("64 MiB + 1 must be rejected");

    assert!(
        matches!(&err, nixclip_core::NixClipError::Ipc(s) if s.contains("frame too large")),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn read_frame_length_of_64mib_plus_one_is_rejected() {
    // Write only the 4-byte length header claiming 64 MiB + 1; no payload.
    let oversized: u32 = 64 * 1024 * 1024 + 1;
    let buf = oversized.to_be_bytes().to_vec();

    let mut cursor = std::io::Cursor::new(buf);
    let err = read_frame(&mut cursor)
        .await
        .expect_err("length > 64 MiB must be rejected");

    assert!(
        matches!(&err, nixclip_core::NixClipError::Ipc(s) if s.contains("frame too large")),
        "unexpected error: {err}"
    );
}

// ===========================================================================
// Test 5 — Multiple sequential frame reads/writes
// ===========================================================================

#[tokio::test]
async fn multiple_sequential_frames_round_trip() {
    let payloads: &[&[u8]] = &[
        b"frame-one",
        b"",                 // zero-length is valid
        b"frame-three",
        b"the fourth frame with more content",
        b"\x00\x01\x02\x03\x04\x05", // binary payload
    ];

    let mut buf: Vec<u8> = Vec::new();
    for payload in payloads {
        write_frame(&mut buf, payload).await.expect("write frame");
    }

    let mut cursor = std::io::Cursor::new(buf);
    for (i, expected) in payloads.iter().enumerate() {
        let received = read_frame(&mut cursor)
            .await
            .unwrap_or_else(|e| panic!("read frame {i} failed: {e}"));
        assert_eq!(
            received.as_slice(),
            *expected,
            "frame {i} content mismatch"
        );
    }
}

#[tokio::test]
async fn sequential_send_recv_messages_round_trip() {
    use nixclip_core::ipc::{send_message, recv_message};

    let messages: Vec<ClientMessage> = vec![
        ClientMessage::subscribe(),
        ClientMessage::query(Some("hello".into()), None, 0, 10),
        ClientMessage::clear_unpinned(),
        ClientMessage::get_config(),
        ClientMessage::delete(vec![1, 2, 3]),
    ];

    let mut buf: Vec<u8> = Vec::new();
    for msg in &messages {
        send_message(&mut buf, msg).await.expect("send_message");
    }

    let mut cursor = std::io::Cursor::new(buf);
    for (i, expected) in messages.iter().enumerate() {
        let received: ClientMessage = recv_message(&mut cursor)
            .await
            .unwrap_or_else(|e| panic!("recv_message {i} failed: {e}"));

        assert_eq!(
            received.version(), expected.version(),
            "message {i} version mismatch"
        );
        // Check the variant discriminant matches.
        assert_eq!(
            std::mem::discriminant(&received),
            std::mem::discriminant(expected),
            "message {i} variant mismatch"
        );
    }
}

// ===========================================================================
// Test 6 — Partial frame read (connection drops mid-frame)
//
// The spec states read_frame must return an I/O error when the stream ends
// mid-payload (i.e., after the length header has been read but before all
// payload bytes arrive).  This maps to `NixClipError::Io` because Tokio's
// `read_exact` raises `UnexpectedEof` in that case.
// ===========================================================================

#[tokio::test]
async fn partial_frame_missing_payload_bytes_is_io_error() {
    // Header claims 200 bytes; only 50 are provided.
    let claimed_len: u32 = 200;
    let mut buf = claimed_len.to_be_bytes().to_vec();
    buf.extend_from_slice(&[0xFFu8; 50]); // only 50 of the 200 bytes

    let mut cursor = std::io::Cursor::new(buf);
    let err = read_frame(&mut cursor)
        .await
        .expect_err("truncated payload must error");

    assert!(
        matches!(err, nixclip_core::NixClipError::Io(_)),
        "expected NixClipError::Io for truncated payload, got: {err}"
    );
}

#[tokio::test]
async fn partial_frame_missing_length_bytes_is_io_error() {
    // Stream contains only 2 of the 4 length bytes, then EOF.
    // The first byte peek succeeds (returns 1 byte), then read_exact for the
    // remaining 3 bytes hits EOF after only 1 more byte.
    let buf: Vec<u8> = vec![0x00, 0x01]; // only 2 bytes, not 4

    let mut cursor = std::io::Cursor::new(buf);
    let err = read_frame(&mut cursor)
        .await
        .expect_err("truncated length header must error");

    // Mid-stream EOF in read_exact → Io(UnexpectedEof).
    assert!(
        matches!(err, nixclip_core::NixClipError::Io(_)),
        "expected NixClipError::Io for truncated length header, got: {err}"
    );
}

#[tokio::test]
async fn clean_eof_before_any_bytes_is_ipc_connection_closed() {
    let buf: Vec<u8> = Vec::new(); // no bytes at all
    let mut cursor = std::io::Cursor::new(buf);
    let err = read_frame(&mut cursor)
        .await
        .expect_err("clean EOF must be 'connection closed' error");

    assert!(
        matches!(&err, nixclip_core::NixClipError::Ipc(s) if s.contains("connection closed")),
        "unexpected error: {err}"
    );
}

// ===========================================================================
// Test 7 — Serde tag = "type": deserialize from manually-constructed MessagePack
//
// We construct raw MessagePack bytes (a map with "type" and "version" keys)
// and verify that the serde tagged enum deserializer correctly dispatches on
// the "type" field value — i.e., the discriminant logic is driven by the
// wire bytes, not just by Rust's encode/decode symmetry.
// ===========================================================================

#[test]
fn manual_msgpack_subscribe_deserializes_correctly() {
    // Manually build a MessagePack map:
    //   { "type": "Subscribe", "version": 1 }
    //
    // MessagePack fixmap with 2 entries: 0x82
    // fixstr "type"    → 0xa4 + bytes "type"
    // fixstr "Subscribe" → 0xa9 + bytes "Subscribe"
    // fixstr "version" → 0xa7 + bytes "version"
    // positive fixint 1 → 0x01
    let bytes: Vec<u8> = {
        let mut v = Vec::new();
        // fixmap, 2 entries
        v.push(0x82);
        // key: "type" (4 chars → fixstr 0xa0 | 4 = 0xa4)
        v.push(0xa4);
        v.extend_from_slice(b"type");
        // value: "Subscribe" (9 chars → fixstr 0xa0 | 9 = 0xa9)
        v.push(0xa9);
        v.extend_from_slice(b"Subscribe");
        // key: "version" (7 chars → 0xa7)
        v.push(0xa7);
        v.extend_from_slice(b"version");
        // value: 1 (positive fixint)
        v.push(0x01);
        v
    };

    let decoded: ClientMessage = decode_message(&bytes)
        .expect("manually constructed Subscribe msgpack must decode");

    assert!(
        matches!(decoded, ClientMessage::Subscribe { version: 1 }),
        "expected Subscribe {{ version: 1 }}, got {decoded:?}"
    );
}

#[test]
fn manual_msgpack_ok_deserializes_correctly() {
    // { "type": "Ok", "version": 1 }
    let bytes: Vec<u8> = {
        let mut v = Vec::new();
        // fixmap, 2 entries
        v.push(0x82);
        // key: "type"
        v.push(0xa4);
        v.extend_from_slice(b"type");
        // value: "Ok" (2 chars → 0xa2)
        v.push(0xa2);
        v.extend_from_slice(b"Ok");
        // key: "version"
        v.push(0xa7);
        v.extend_from_slice(b"version");
        // value: 1
        v.push(0x01);
        v
    };

    let decoded: ServerMessage =
        decode_message(&bytes).expect("manually constructed Ok msgpack must decode");

    assert!(
        matches!(decoded, ServerMessage::Ok { version: 1 }),
        "expected Ok {{ version: 1 }}, got {decoded:?}"
    );
}

#[test]
fn manual_msgpack_clear_unpinned_deserializes_correctly() {
    // { "type": "ClearUnpinned", "version": 1 }
    let bytes: Vec<u8> = {
        let mut v = Vec::new();
        v.push(0x82); // fixmap 2 entries
        v.push(0xa4);
        v.extend_from_slice(b"type");
        // "ClearUnpinned" = 13 chars → fixstr 0xa0 | 13 = 0xad
        v.push(0xad);
        v.extend_from_slice(b"ClearUnpinned");
        v.push(0xa7);
        v.extend_from_slice(b"version");
        v.push(0x01);
        v
    };

    let decoded: ClientMessage =
        decode_message(&bytes).expect("manually constructed ClearUnpinned must decode");

    assert!(
        matches!(decoded, ClientMessage::ClearUnpinned { version: 1 }),
        "expected ClearUnpinned {{ version: 1 }}, got {decoded:?}"
    );
}

#[test]
fn manual_msgpack_unknown_extra_field_is_ignored() {
    // Forward compat: a map with an extra unknown field "future_field" must
    // still decode cleanly.  This verifies the named-field contract.
    //
    // { "type": "Subscribe", "version": 1, "future_field": 42 }
    let bytes: Vec<u8> = {
        let mut v = Vec::new();
        v.push(0x83); // fixmap 3 entries
        v.push(0xa4);
        v.extend_from_slice(b"type");
        v.push(0xa9);
        v.extend_from_slice(b"Subscribe");
        v.push(0xa7);
        v.extend_from_slice(b"version");
        v.push(0x01);
        // extra unknown field
        v.push(0xac); // fixstr 12 = "future_field"
        v.extend_from_slice(b"future_field");
        v.push(0x2a); // positive fixint 42
        v
    };

    let decoded: ClientMessage =
        decode_message(&bytes).expect("unknown extra fields must be ignored (forward compat)");

    assert!(
        matches!(decoded, ClientMessage::Subscribe { version: 1 }),
        "extra field should be ignored; got {decoded:?}"
    );
}

#[test]
fn manual_msgpack_wrong_type_value_errors() {
    // A "type" value that doesn't match any variant must fail deserialization.
    let bytes: Vec<u8> = {
        let mut v = Vec::new();
        v.push(0x82);
        v.push(0xa4);
        v.extend_from_slice(b"type");
        // "NonExistentVariant" = 18 chars
        v.push(0xb2); // fixstr 18
        v.extend_from_slice(b"NonExistentVariant");
        v.push(0xa7);
        v.extend_from_slice(b"version");
        v.push(0x01);
        v
    };

    let result: Result<ClientMessage, _> = decode_message(&bytes);
    assert!(
        result.is_err(),
        "unknown variant name must fail deserialization"
    );
}

// ===========================================================================
// Bonus: frame length is written in big-endian byte order
// ===========================================================================

#[tokio::test]
async fn frame_length_prefix_is_big_endian() {
    let payload = b"be-test";
    let mut buf: Vec<u8> = Vec::new();
    write_frame(&mut buf, payload).await.expect("write_frame");

    // Extract the raw 4 bytes and parse manually.
    let raw_len_bytes: [u8; 4] = buf[..4].try_into().unwrap();
    let be_len = u32::from_be_bytes(raw_len_bytes);
    let le_len = u32::from_le_bytes(raw_len_bytes);

    assert_eq!(
        be_len as usize,
        payload.len(),
        "big-endian parse must equal payload length"
    );
    // These only differ when the length bytes are not palindromic, which is
    // true for any length >= 256 — but for a short payload the BE value is
    // just equal to the actual length regardless of endianness since it fits
    // in one byte.  We confirm the BE interpretation equals the payload length.
    let _ = le_len; // Not asserting LE; just confirming BE is correct.
}
