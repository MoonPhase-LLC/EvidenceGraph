//! Private-channel wire protocol (S1-04). Mirrors
//! `service/src/evidencegraph_service/protocol.py` byte-for-byte -- see
//! that module's docstring for the full framing/ordering contract this
//! implements. Only framing and the specific message shapes live here;
//! message *ordering* is enforced by the caller (`supervisor/mod.rs`),
//! which reads and writes messages in the one fixed sequence the
//! handshake requires -- there is no general-purpose message queue here.

use super::b64url;
use serde_json::{json, Value};
use std::fmt;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const PROTOCOL_VERSION: u64 = 1;

/// Matches `protocol.MAX_FRAME_BYTES` in the Python implementation.
pub const MAX_FRAME_BYTES: u32 = 4096;

#[derive(Debug)]
pub enum ProtocolError {
    ChannelClosed,
    FrameTooLarge,
    FrameLengthOutOfBounds,
    NotValidJson,
    NotAnObject,
    UnsupportedVersion,
    MissingType,
    UnexpectedType(String),
    MissingField(&'static str),
    Io(std::io::Error),
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ChannelClosed => write!(f, "private channel closed"),
            Self::FrameTooLarge => write!(f, "frame too large"),
            Self::FrameLengthOutOfBounds => write!(f, "frame length out of bounds"),
            Self::NotValidJson => write!(f, "frame is not valid JSON"),
            Self::NotAnObject => write!(f, "frame is not a JSON object"),
            Self::UnsupportedVersion => write!(f, "unsupported protocol version"),
            Self::MissingType => write!(f, "frame missing type"),
            Self::UnexpectedType(t) => write!(f, "unexpected message type: {t}"),
            Self::MissingField(field) => write!(f, "frame missing field: {field}"),
            Self::Io(e) => write!(f, "private channel I/O error: {e}"),
        }
    }
}

impl std::error::Error for ProtocolError {}

impl From<std::io::Error> for ProtocolError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

pub async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    message: &Value,
) -> Result<(), ProtocolError> {
    let payload = serde_json::to_vec(message).map_err(|_| ProtocolError::NotValidJson)?;
    if payload.len() as u32 > MAX_FRAME_BYTES {
        return Err(ProtocolError::FrameTooLarge);
    }
    writer.write_u32(payload.len() as u32).await?;
    writer.write_all(&payload).await?;
    writer.flush().await?;
    Ok(())
}

async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Value, ProtocolError> {
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
    let value: Value = serde_json::from_slice(&buf).map_err(|_| ProtocolError::NotValidJson)?;
    if !value.is_object() {
        return Err(ProtocolError::NotAnObject);
    }
    if value.get("v").and_then(Value::as_u64) != Some(PROTOCOL_VERSION) {
        return Err(ProtocolError::UnsupportedVersion);
    }
    if !value.get("type").is_some_and(Value::is_string) {
        return Err(ProtocolError::MissingType);
    }
    Ok(value)
}

fn message_type(value: &Value) -> &str {
    value["type"].as_str().unwrap_or("")
}

fn require_type(value: &Value, expected: &str) -> Result<(), ProtocolError> {
    let actual = message_type(value);
    if actual != expected {
        return Err(ProtocolError::UnexpectedType(actual.to_string()));
    }
    Ok(())
}

// --- Typed helpers for the parent's side of the exchange -------------------

pub async fn write_startup_secret<W: AsyncWrite + Unpin>(
    writer: &mut W,
    secret: &[u8],
) -> Result<(), ProtocolError> {
    write_frame(
        writer,
        &json!({"v": PROTOCOL_VERSION, "type": "startup_secret", "secret": b64url::encode(secret)}),
    )
    .await
}

pub struct EndpointReady {
    pub host: String,
    pub port: u16,
}

/// Reads and strictly validates the mandatory first child->parent
/// message. Anything else -- wrong type, missing/malformed fields, EOF --
/// is a `ProtocolError`, and the caller must never treat the child as
/// trustworthy first.
pub async fn read_endpoint_ready<R: AsyncRead + Unpin>(
    reader: &mut R,
) -> Result<EndpointReady, ProtocolError> {
    let value = read_frame(reader).await?;
    require_type(&value, "endpoint_ready")?;
    let host = value
        .get("host")
        .and_then(Value::as_str)
        .ok_or(ProtocolError::MissingField("host"))?;
    let port = value
        .get("port")
        .and_then(Value::as_u64)
        .ok_or(ProtocolError::MissingField("port"))?;
    let port = u16::try_from(port).map_err(|_| ProtocolError::MissingField("port"))?;
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
        &json!({"v": PROTOCOL_VERSION, "type": "install_session_token", "token": token}),
    )
    .await
}

pub async fn read_ready<R: AsyncRead + Unpin>(reader: &mut R) -> Result<(), ProtocolError> {
    let value = read_frame(reader).await?;
    require_type(&value, "ready")
}

pub async fn write_shutdown<W: AsyncWrite + Unpin>(writer: &mut W) -> Result<(), ProtocolError> {
    write_frame(writer, &json!({"v": PROTOCOL_VERSION, "type": "shutdown"})).await
}

pub async fn read_shutdown_ack<R: AsyncRead + Unpin>(reader: &mut R) -> Result<(), ProtocolError> {
    let value = read_frame(reader).await?;
    require_type(&value, "shutdown_ack")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[tokio::test]
    async fn write_then_read_endpoint_ready_round_trips() {
        let mut buf = Vec::new();
        write_frame(
            &mut buf,
            &json!({"v": 1, "type": "endpoint_ready", "host": "127.0.0.1", "port": 54321}),
        )
        .await
        .unwrap();
        let mut cursor = Cursor::new(buf);
        let endpoint = read_endpoint_ready(&mut cursor).await.unwrap();
        assert_eq!(endpoint.host, "127.0.0.1");
        assert_eq!(endpoint.port, 54321);
    }

    #[tokio::test]
    async fn read_endpoint_ready_rejects_wrong_type() {
        let mut buf = Vec::new();
        write_frame(&mut buf, &json!({"v": 1, "type": "ready"}))
            .await
            .unwrap();
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
        write_frame(
            &mut buf,
            &json!({"v": 2, "type": "endpoint_ready", "host": "x", "port": 1}),
        )
        .await
        .unwrap();
        let mut cursor = Cursor::new(buf);
        assert!(read_endpoint_ready(&mut cursor).await.is_err());
    }

    #[tokio::test]
    async fn read_frame_rejects_non_object() {
        let mut buf = Vec::new();
        let payload = serde_json::to_vec(&json!([1, 2, 3])).unwrap();
        buf.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        buf.extend_from_slice(&payload);
        let mut cursor = Cursor::new(buf);
        assert!(read_endpoint_ready(&mut cursor).await.is_err());
    }

    #[tokio::test]
    async fn write_frame_rejects_oversized_message() {
        let huge =
            json!({"v": 1, "type": "ready", "padding": "x".repeat(MAX_FRAME_BYTES as usize * 2)});
        let mut buf = Vec::new();
        assert!(write_frame(&mut buf, &huge).await.is_err());
    }

    #[tokio::test]
    async fn read_ready_and_shutdown_ack_shapes() {
        let mut buf = Vec::new();
        write_frame(&mut buf, &json!({"v": 1, "type": "ready"}))
            .await
            .unwrap();
        write_frame(&mut buf, &json!({"v": 1, "type": "shutdown_ack"}))
            .await
            .unwrap();
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
}
