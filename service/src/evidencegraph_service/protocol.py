"""Private-channel wire protocol (S1-04).

Tauri (the parent) and this service's supervised-mode child
(`supervised.py`) exchange a small, fixed-order sequence of framed JSON
messages over the child's inherited stdin (parent -> child) and stdout
(child -> parent). Structured logs continue to go to stderr
(`logging_config.py`), a completely separate stream -- protocol messages
and log lines can never be confused with each other because they never
share a stream.

Wire format (must match `app/src-tauri/src/supervisor/protocol.rs`
byte-for-byte):

    frame = u32_big_endian(len(payload)) ++ payload
    payload = UTF-8-encoded JSON object, always containing at least
              {"v": 1, "type": "<message-type>"}

`len(payload)` is bounded by `MAX_FRAME_BYTES`; a frame claiming to be
larger is a protocol violation, not a value to attempt reading. Every
message declares protocol version 1 (`"v": 1`) -- a version mismatch is
also a protocol violation (fail closed), not a compatibility feature.

Message order (see `docs/SPRINT_1_BACKLOG.md` S1-04 and
`docs/DECISIONS.md` D-025 for the full handshake this participates in):

    parent -> child   startup_secret          (must be the first message)
    child  -> parent  endpoint_ready
    ... (D-025 HTTP challenge/response happens out-of-band, over the
         bound HTTP endpoint, not this channel -- see `challenge.py`) ...
    parent -> child   install_session_token
    child  -> parent  ready
    parent -> child   shutdown                (optional, at app exit)
    child  -> parent  shutdown_ack

Any message out of this order, any duplicate, any unknown `type`, EOF
before `ready`, or a malformed/oversized frame is fatal to the child
process (`ProtocolError`, uncaught by design at the call site in
`supervised.py`, which exits non-zero) -- "unknown, malformed, duplicated,
and out-of-order messages fail startup."
"""

from __future__ import annotations

import json
import struct
from typing import Protocol

from . import b64url

PROTOCOL_VERSION = 1

#: Real messages here are tiny (a handful of short fields); this bound
#: exists purely so a malformed/hostile length prefix can never make the
#: child attempt to allocate or read an unbounded amount of data.
MAX_FRAME_BYTES = 4096

_LENGTH_STRUCT = struct.Struct(">I")  # big-endian uint32


class ProtocolError(Exception):
    """Any private-channel framing/ordering/content violation. Always
    fatal -- there is no partial-recovery path. The message is always a
    fixed, bounded, non-secret category string."""


class BinaryStream(Protocol):
    """Structural type for the raw stdin/stdout buffer objects this module
    operates on (`sys.stdin.buffer` / `sys.stdout.buffer` in production;
    `io.BytesIO` in tests)."""

    def read(self, n: int, /) -> bytes: ...
    def write(self, data: bytes, /) -> int: ...
    def flush(self) -> None: ...


def write_frame(stream: BinaryStream, message: dict[str, object]) -> None:
    payload = json.dumps(message, separators=(",", ":")).encode("utf-8")
    if len(payload) > MAX_FRAME_BYTES:
        raise ProtocolError("frame_too_large")
    stream.write(_LENGTH_STRUCT.pack(len(payload)))
    stream.write(payload)
    stream.flush()


def _read_exact(stream: BinaryStream, n: int) -> bytes:
    chunks = bytearray()
    while len(chunks) < n:
        chunk = stream.read(n - len(chunks))
        if not chunk:
            raise ProtocolError("channel_closed")
        chunks.extend(chunk)
    return bytes(chunks)


def read_frame(stream: BinaryStream) -> dict[str, object]:
    (length,) = _LENGTH_STRUCT.unpack(_read_exact(stream, _LENGTH_STRUCT.size))
    if length == 0 or length > MAX_FRAME_BYTES:
        raise ProtocolError("frame_length_out_of_bounds")
    payload = _read_exact(stream, length)
    try:
        message = json.loads(payload.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise ProtocolError("frame_not_valid_json") from exc
    if not isinstance(message, dict):
        raise ProtocolError("frame_not_an_object")
    if message.get("v") != PROTOCOL_VERSION:
        raise ProtocolError("unsupported_protocol_version")
    if not isinstance(message.get("type"), str):
        raise ProtocolError("frame_missing_type")
    return message


# --- Typed helpers for the child's side of the exchange -------------------


def read_startup_secret(stream: BinaryStream) -> bytes:
    """Reads and strictly validates the mandatory first message. Anything
    else -- wrong type, missing/malformed `secret` field, EOF -- is a
    `ProtocolError`, and the caller must never open a socket first."""
    message = read_frame(stream)
    if message["type"] != "startup_secret":
        raise ProtocolError(f"expected_startup_secret_got_{message['type']}")
    secret_b64 = message.get("secret")
    try:
        return b64url.decode(secret_b64, field="startup_secret")
    except b64url.DecodeError as exc:
        raise ProtocolError(str(exc)) from exc


def write_endpoint_ready(stream: BinaryStream, *, host: str, port: int) -> None:
    write_frame(
        stream, {"v": PROTOCOL_VERSION, "type": "endpoint_ready", "host": host, "port": port}
    )


def read_install_token_or_shutdown(stream: BinaryStream) -> tuple[str, str | None]:
    """Reads exactly one message from the two types valid at this point in
    the sequence. Returns `("install_session_token", token)` or
    `("shutdown", None)`. Any other `type` -- including a second
    `startup_secret` -- is a `ProtocolError`: this channel's message order
    is fixed, not a general-purpose queue."""
    message = read_frame(stream)
    msg_type = message["type"]
    if not isinstance(msg_type, str):
        raise ProtocolError("frame_missing_type")
    if msg_type == "install_session_token":
        token = message.get("token")
        if not isinstance(token, str) or not token:
            raise ProtocolError("install_session_token_missing_token")
        return msg_type, token
    if msg_type == "shutdown":
        return msg_type, None
    raise ProtocolError(f"unexpected_message_type_{msg_type}")


def write_ready(stream: BinaryStream) -> None:
    write_frame(stream, {"v": PROTOCOL_VERSION, "type": "ready"})


def write_shutdown_ack(stream: BinaryStream) -> None:
    write_frame(stream, {"v": PROTOCOL_VERSION, "type": "shutdown_ack"})


#: Small, fixed set of reasons the child may report before it can even
#: install a credential (e.g. a malformed `startup_secret` message) --
#: never a raw exception message, which could echo back attacker/parent-
#: controlled content.
_ERROR_REASONS = frozenset(
    {
        "invalid_startup_secret",
        "bind_failed",
        "internal_error",
    }
)


def write_error(stream: BinaryStream, *, reason: str) -> None:
    if reason not in _ERROR_REASONS:
        reason = "internal_error"
    write_frame(stream, {"v": PROTOCOL_VERSION, "type": "error", "reason": reason})
