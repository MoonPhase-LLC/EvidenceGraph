//! Private-channel wire protocol (S1-04). Mirrors
//! `service/src/evidencegraph_service/protocol.py` byte-for-byte -- see
//! that module's docstring for the full framing/ordering contract this
//! implements. Only framing and the specific message shapes live here;
//! message *ordering* is enforced by the caller (`supervisor/mod.rs`),
//! which reads and writes messages in the one fixed sequence the
//! handshake requires -- there is no general-purpose message queue here.
//!
//! Every `ProtocolError` variant here is a fixed, content-free tag --
//! never a received message's `type`, a field value, or any other
//! child-controlled content. An earlier version of this module carried
//! `UnexpectedType(String)`/`MissingField(&'static str)` payloads and
//! interpolated the received type into `Display`, which is exactly the
//! class of leak S1-04 review finding 4 flagged; every call site in
//! `supervisor::mod` now maps a `ProtocolError` to a
//! [`super::errors::FailureReason`] before it can reach a log or
//! `SupervisorState::Failed`, so nothing here needs -- or is permitted --
//! to carry request content to do that mapping usefully.
//!
//! Objects are parsed with [`parse_object_rejecting_duplicates`], never
//! plain `serde_json::from_slice::<Value>`: the latter silently collapses
//! a duplicate key to its last occurrence, which is exactly the kind of
//! ambiguity a strict wire schema (S1-04 review finding 5) must not
//! accept silently.

use super::b64url;
use serde::de::{Deserializer as _, Error as DeError, MapAccess, Visitor};
use serde_json::{Map, Value};
use std::fmt;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const PROTOCOL_VERSION: u64 = 1;

/// Matches `protocol.MAX_FRAME_BYTES` in the Python implementation.
pub const MAX_FRAME_BYTES: u32 = 4096;

/// Matches `protocol.STARTUP_SECRET_LENGTH`/`config._REQUIRED_HOST` on the
/// Python side and `supervisor::STARTUP_SECRET_LEN`/`process::spawn`'s own
/// bind contract on this side.
const REQUIRED_HOST: &str = "127.0.0.1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolError {
    ChannelClosed,
    FrameTooLarge,
    FrameLengthOutOfBounds,
    FrameNotValidJson,
    /// Valid JSON, but not an object, or an object with a duplicate key --
    /// deliberately not distinguished further (both are "the framing
    /// contract was violated before we ever got to field-level checks").
    FrameMalformedObject,
    UnsupportedProtocolVersion,
    FrameMissingType,
    UnexpectedMessageType,
    /// Missing/extra/wrong-typed field, or a field value outside its
    /// required range/shape (e.g. a non-loopback `host`, an out-of-range
    /// `port`).
    UnexpectedSchema,
    HostNotLoopback,
    Io,
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Every branch is a fixed literal -- see module docs.
        let s = match self {
            Self::ChannelClosed => "channel_closed",
            Self::FrameTooLarge => "frame_too_large",
            Self::FrameLengthOutOfBounds => "frame_length_out_of_bounds",
            Self::FrameNotValidJson => "frame_not_valid_json",
            Self::FrameMalformedObject => "frame_malformed_object",
            Self::UnsupportedProtocolVersion => "unsupported_protocol_version",
            Self::FrameMissingType => "frame_missing_type",
            Self::UnexpectedMessageType => "unexpected_message_type",
            Self::UnexpectedSchema => "unexpected_schema",
            Self::HostNotLoopback => "host_not_loopback",
            Self::Io => "private_channel_io_error",
        };
        f.write_str(s)
    }
}

impl std::error::Error for ProtocolError {}

impl From<std::io::Error> for ProtocolError {
    fn from(_: std::io::Error) -> Self {
        // The underlying `io::Error` is deliberately discarded here, not
        // wrapped -- its `Display` can include OS-provided text that
        // isn't one of this module's fixed categories.
        Self::Io
    }
}

/// A `serde` `Visitor` that parses a JSON object while rejecting any
/// duplicate key -- `serde_json::Value`'s normal map-building silently
/// keeps only the *last* occurrence of a repeated key, which is exactly
/// the ambiguity a strict wire schema must not accept. Any non-object
/// top-level value also fails here (via the `Visitor` trait's default
/// `visit_*` methods, which we don't override), collapsed into the same
/// [`ProtocolError::FrameMalformedObject`] as a duplicate key -- both are
/// "the object-level framing contract was violated."
struct NoDuplicateKeysVisitor;

impl<'de> Visitor<'de> for NoDuplicateKeysVisitor {
    type Value = Map<String, Value>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON object with no duplicate keys")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut result = Map::new();
        while let Some((key, value)) = map.next_entry::<String, Value>()? {
            if result.contains_key(&key) {
                return Err(A::Error::custom("duplicate key"));
            }
            result.insert(key, value);
        }
        Ok(result)
    }
}

fn parse_object_rejecting_duplicates(bytes: &[u8]) -> Result<Map<String, Value>, ProtocolError> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    match deserializer.deserialize_any(NoDuplicateKeysVisitor) {
        Ok(map) => Ok(map),
        Err(e) if e.is_syntax() || e.is_eof() => Err(ProtocolError::FrameNotValidJson),
        Err(_) => Err(ProtocolError::FrameMalformedObject),
    }
}

pub async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    message: &Value,
) -> Result<(), ProtocolError> {
    let payload = serde_json::to_vec(message).map_err(|_| ProtocolError::FrameNotValidJson)?;
    if payload.len() as u32 > MAX_FRAME_BYTES {
        return Err(ProtocolError::FrameTooLarge);
    }
    writer.write_u32(payload.len() as u32).await?;
    writer.write_all(&payload).await?;
    writer.flush().await?;
    Ok(())
}

/// Reads one frame and strictly validates its envelope: a JSON object
/// with no duplicate keys, an integer (never bool/float -- `Value::
/// as_u64` returns `None` for both JSON variants, so this is already
/// correct without an extra check) `"v" == PROTOCOL_VERSION`, and a
/// string `"type"`. Does **not** check the exact key set -- that depends
/// on which message type this turns out to be, which the caller doesn't
/// know until after this returns; callers must follow up with
/// [`require_exact_keys`] once they've routed on the `"type"` value.
async fn read_frame<R: AsyncRead + Unpin>(
    reader: &mut R,
) -> Result<Map<String, Value>, ProtocolError> {
    let length = reader
        .read_u32()
        .await
        .map_err(|_| ProtocolError::ChannelClosed)?;
    if length == 0 || length > MAX_FRAME_BYTES {
        return Err(ProtocolError::FrameLengthOutOfBounds);
    }
    let mut buf = vec![0u8; length as usize];
    reader
        .read_exact(&mut buf)
        .await
        .map_err(|_| ProtocolError::ChannelClosed)?;

    let message = parse_object_rejecting_duplicates(&buf)?;

    if message.get("v").and_then(Value::as_u64) != Some(PROTOCOL_VERSION) {
        Err(ProtocolError::UnsupportedProtocolVersion)
    } else if !message.get("type").is_some_and(Value::is_string) {
        Err(ProtocolError::FrameMissingType)
    } else {
        Ok(message)
    }
}

fn require_exact_keys(
    message: &Map<String, Value>,
    expected: &[&str],
) -> Result<(), ProtocolError> {
    if message.len() != expected.len() || !expected.iter().all(|k| message.contains_key(*k)) {
        return Err(ProtocolError::UnexpectedSchema);
    }
    Ok(())
}

fn message_type(value: &Map<String, Value>) -> &str {
    value["type"].as_str().unwrap_or("")
}

// --- Typed helpers for the parent's side of the exchange -------------------

pub async fn write_startup_secret<W: AsyncWrite + Unpin>(
    writer: &mut W,
    secret: &[u8],
) -> Result<(), ProtocolError> {
    write_frame(
        writer,
        &serde_json::json!({"v": PROTOCOL_VERSION, "type": "startup_secret", "secret": b64url::encode(secret)}),
    )
    .await
}

pub struct EndpointReady {
    pub host: String,
    pub port: u16,
}

/// Reads and strictly validates the mandatory first child->parent
/// message. Anything else -- wrong type, missing/extra/wrong-typed
/// fields, a non-loopback `host`, an out-of-range `port`, EOF -- is a
/// `ProtocolError`, and the caller must never treat the child as
/// trustworthy first. `host` is checked against [`REQUIRED_HOST`]
/// *here*, at parse time (S1-04 review finding 5's "require host exactly
/// 127.0.0.1"), not left to a later business-logic check the caller could
/// forget -- a successfully returned `EndpointReady` is guaranteed
/// loopback by construction, so callers never need to re-check it.
pub async fn read_endpoint_ready<R: AsyncRead + Unpin>(
    reader: &mut R,
) -> Result<EndpointReady, ProtocolError> {
    let message = read_frame(reader).await?;
    if message_type(&message) != "endpoint_ready" {
        return Err(ProtocolError::UnexpectedMessageType);
    }
    require_exact_keys(&message, &["v", "type", "host", "port"])?;

    let host = message
        .get("host")
        .and_then(Value::as_str)
        .ok_or(ProtocolError::UnexpectedSchema)?;
    if host != REQUIRED_HOST {
        return Err(ProtocolError::HostNotLoopback);
    }

    let port = message
        .get("port")
        .and_then(Value::as_u64)
        .ok_or(ProtocolError::UnexpectedSchema)?;
    let port = u16::try_from(port).map_err(|_| ProtocolError::UnexpectedSchema)?;
    if port == 0 {
        return Err(ProtocolError::UnexpectedSchema);
    }

    Ok(EndpointReady {
        host: host.to_string(),
        port,
    })
}

pub async fn write_install_session_token<W: AsyncWrite + Unpin>(
    writer: &mut W,
    token: &str,
) -> Result<(), ProtocolError> {
    write_frame(
        writer,
        &serde_json::json!({"v": PROTOCOL_VERSION, "type": "install_session_token", "token": token}),
    )
    .await
}

pub async fn read_ready<R: AsyncRead + Unpin>(reader: &mut R) -> Result<(), ProtocolError> {
    let message = read_frame(reader).await?;
    if message_type(&message) != "ready" {
        return Err(ProtocolError::UnexpectedMessageType);
    }
    require_exact_keys(&message, &["v", "type"])
}

pub async fn write_shutdown<W: AsyncWrite + Unpin>(writer: &mut W) -> Result<(), ProtocolError> {
    write_frame(
        writer,
        &serde_json::json!({"v": PROTOCOL_VERSION, "type": "shutdown"}),
    )
    .await
}

pub async fn read_shutdown_ack<R: AsyncRead + Unpin>(reader: &mut R) -> Result<(), ProtocolError> {
    let message = read_frame(reader).await?;
    if message_type(&message) != "shutdown_ack" {
        return Err(ProtocolError::UnexpectedMessageType);
    }
    require_exact_keys(&message, &["v", "type"])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    async fn write_raw(buf: &mut Vec<u8>, message: &Value) {
        write_frame(buf, message).await.unwrap();
    }

    #[tokio::test]
    async fn write_then_read_endpoint_ready_round_trips() {
        let mut buf = Vec::new();
        write_raw(
            &mut buf,
            &serde_json::json!({"v": 1, "type": "endpoint_ready", "host": "127.0.0.1", "port": 54321}),
        )
        .await;
        let mut cursor = Cursor::new(buf);
        let endpoint = read_endpoint_ready(&mut cursor).await.unwrap();
        assert_eq!(endpoint.host, "127.0.0.1");
        assert_eq!(endpoint.port, 54321);
    }

    #[tokio::test]
    async fn read_endpoint_ready_rejects_wrong_type() {
        let mut buf = Vec::new();
        write_raw(&mut buf, &serde_json::json!({"v": 1, "type": "ready"})).await;
        let mut cursor = Cursor::new(buf);
        assert!(read_endpoint_ready(&mut cursor).await.is_err());
    }

    #[tokio::test]
    async fn read_frame_rejects_eof() {
        let mut cursor = Cursor::new(Vec::<u8>::new());
        assert!(read_endpoint_ready(&mut cursor).await.is_err());
    }

    #[tokio::test]
    async fn read_frame_rejects_oversized_length_prefix() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&(MAX_FRAME_BYTES + 1).to_be_bytes());
        let mut cursor = Cursor::new(buf);
        assert!(read_endpoint_ready(&mut cursor).await.is_err());
    }

    #[tokio::test]
    async fn read_frame_rejects_wrong_version() {
        let mut buf = Vec::new();
        write_raw(
            &mut buf,
            &serde_json::json!({"v": 2, "type": "endpoint_ready", "host": "127.0.0.1", "port": 1}),
        )
        .await;
        let mut cursor = Cursor::new(buf);
        assert!(read_endpoint_ready(&mut cursor).await.is_err());
    }

    #[tokio::test]
    async fn read_frame_rejects_non_object() {
        let mut buf = Vec::new();
        let payload = serde_json::to_vec(&serde_json::json!([1, 2, 3])).unwrap();
        buf.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        buf.extend_from_slice(&payload);
        let mut cursor = Cursor::new(buf);
        assert!(read_endpoint_ready(&mut cursor).await.is_err());
    }

    #[tokio::test]
    async fn write_frame_rejects_oversized_message() {
        let huge = serde_json::json!({"v": 1, "type": "ready", "padding": "x".repeat(MAX_FRAME_BYTES as usize * 2)});
        let mut buf = Vec::new();
        assert!(write_frame(&mut buf, &huge).await.is_err());
    }

    #[tokio::test]
    async fn read_ready_and_shutdown_ack_shapes() {
        let mut buf = Vec::new();
        write_raw(&mut buf, &serde_json::json!({"v": 1, "type": "ready"})).await;
        write_raw(
            &mut buf,
            &serde_json::json!({"v": 1, "type": "shutdown_ack"}),
        )
        .await;
        let mut cursor = Cursor::new(buf);
        read_ready(&mut cursor).await.unwrap();
        read_shutdown_ack(&mut cursor).await.unwrap();
    }

    #[tokio::test]
    async fn write_startup_secret_and_install_session_token_are_valid_base64url() {
        let mut buf = Vec::new();
        write_startup_secret(&mut buf, &[1, 2, 3, 4]).await.unwrap();
        write_install_session_token(&mut buf, "some-token-value-here-1234567890")
            .await
            .unwrap();
        let mut cursor = Cursor::new(buf);
        let secret_msg = read_frame(&mut cursor).await.unwrap();
        assert_eq!(secret_msg["type"], "startup_secret");
        let secret_b64 = secret_msg["secret"].as_str().unwrap();
        assert_eq!(b64url::decode(secret_b64).unwrap(), vec![1, 2, 3, 4]);

        let token_msg = read_frame(&mut cursor).await.unwrap();
        assert_eq!(token_msg["type"], "install_session_token");
        assert_eq!(token_msg["token"], "some-token-value-here-1234567890");
    }

    // --- S1-04 review finding 5: strict wire schema -------------------------

    #[tokio::test]
    async fn read_frame_rejects_duplicate_v_key() {
        let mut buf = Vec::new();
        let payload = b"{\"v\":1,\"v\":1,\"type\":\"ready\"}";
        buf.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        buf.extend_from_slice(payload);
        let mut cursor = Cursor::new(buf);
        assert!(read_ready(&mut cursor).await.is_err());
    }

    #[tokio::test]
    async fn read_frame_rejects_duplicate_key_in_a_typed_field() {
        let mut buf = Vec::new();
        let payload =
            b"{\"v\":1,\"type\":\"endpoint_ready\",\"host\":\"127.0.0.1\",\"host\":\"127.0.0.1\",\"port\":1}";
        buf.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        buf.extend_from_slice(payload);
        let mut cursor = Cursor::new(buf);
        assert!(read_endpoint_ready(&mut cursor).await.is_err());
    }

    #[tokio::test]
    async fn read_frame_rejects_boolean_version() {
        for version in [serde_json::json!(true), serde_json::json!(false)] {
            let mut buf = Vec::new();
            write_raw(
                &mut buf,
                &serde_json::json!({"v": version, "type": "ready"}),
            )
            .await;
            let mut cursor = Cursor::new(buf);
            assert!(read_ready(&mut cursor).await.is_err());
        }
    }

    #[tokio::test]
    async fn read_frame_rejects_float_version() {
        let mut buf = Vec::new();
        write_raw(&mut buf, &serde_json::json!({"v": 1.0, "type": "ready"})).await;
        let mut cursor = Cursor::new(buf);
        assert!(read_ready(&mut cursor).await.is_err());
    }

    #[tokio::test]
    async fn read_frame_rejects_string_version() {
        let mut buf = Vec::new();
        write_raw(&mut buf, &serde_json::json!({"v": "1", "type": "ready"})).await;
        let mut cursor = Cursor::new(buf);
        assert!(read_ready(&mut cursor).await.is_err());
    }

    #[tokio::test]
    async fn read_endpoint_ready_rejects_non_loopback_host() {
        for host in ["localhost", "0.0.0.0", "::1", "192.168.1.5", "127.0.0.2"] {
            let mut buf = Vec::new();
            write_raw(
                &mut buf,
                &serde_json::json!({"v": 1, "type": "endpoint_ready", "host": host, "port": 1}),
            )
            .await;
            let mut cursor = Cursor::new(buf);
            assert!(
                read_endpoint_ready(&mut cursor).await.is_err(),
                "host {host} should be rejected"
            );
        }
    }

    #[tokio::test]
    async fn read_endpoint_ready_rejects_port_zero() {
        let mut buf = Vec::new();
        write_raw(
            &mut buf,
            &serde_json::json!({"v": 1, "type": "endpoint_ready", "host": "127.0.0.1", "port": 0}),
        )
        .await;
        let mut cursor = Cursor::new(buf);
        assert!(read_endpoint_ready(&mut cursor).await.is_err());
    }

    #[tokio::test]
    async fn read_endpoint_ready_rejects_port_above_65535() {
        let mut buf = Vec::new();
        write_raw(
            &mut buf,
            &serde_json::json!({"v": 1, "type": "endpoint_ready", "host": "127.0.0.1", "port": 65536}),
        )
        .await;
        let mut cursor = Cursor::new(buf);
        assert!(read_endpoint_ready(&mut cursor).await.is_err());
    }

    #[tokio::test]
    async fn read_endpoint_ready_rejects_boolean_port() {
        let mut buf = Vec::new();
        write_raw(
            &mut buf,
            &serde_json::json!({"v": 1, "type": "endpoint_ready", "host": "127.0.0.1", "port": true}),
        )
        .await;
        let mut cursor = Cursor::new(buf);
        assert!(read_endpoint_ready(&mut cursor).await.is_err());
    }

    #[tokio::test]
    async fn read_endpoint_ready_rejects_unknown_extra_key() {
        let mut buf = Vec::new();
        write_raw(
            &mut buf,
            &serde_json::json!({"v": 1, "type": "endpoint_ready", "host": "127.0.0.1", "port": 1, "extra": "x"}),
        )
        .await;
        let mut cursor = Cursor::new(buf);
        assert!(read_endpoint_ready(&mut cursor).await.is_err());
    }

    #[tokio::test]
    async fn read_ready_rejects_unknown_extra_key() {
        let mut buf = Vec::new();
        write_raw(
            &mut buf,
            &serde_json::json!({"v": 1, "type": "ready", "extra": "x"}),
        )
        .await;
        let mut cursor = Cursor::new(buf);
        assert!(read_ready(&mut cursor).await.is_err());
    }

    #[tokio::test]
    async fn multiple_frames_can_be_read_sequentially() {
        let mut buf = Vec::new();
        write_startup_secret(&mut buf, &[1, 2, 3, 4]).await.unwrap();
        write_raw(
            &mut buf,
            &serde_json::json!({"v": 1, "type": "endpoint_ready", "host": "127.0.0.1", "port": 1234}),
        )
        .await;
        write_install_session_token(&mut buf, "some-token-value-here-1234567890")
            .await
            .unwrap();
        write_raw(&mut buf, &serde_json::json!({"v": 1, "type": "ready"})).await;
        let mut cursor = Cursor::new(buf);

        let secret_msg = read_frame(&mut cursor).await.unwrap();
        assert_eq!(secret_msg["type"], "startup_secret");
        let endpoint = read_endpoint_ready(&mut cursor).await.unwrap();
        assert_eq!(endpoint.port, 1234);
        let token_msg = read_frame(&mut cursor).await.unwrap();
        assert_eq!(token_msg["type"], "install_session_token");
        read_ready(&mut cursor).await.unwrap();
    }
}
