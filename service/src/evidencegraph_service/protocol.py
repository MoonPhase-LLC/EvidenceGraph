"""Private-channel wire protocol (S1-04).

Tauri (the parent) and this service's supervised-mode child
(`supervised.py`) exchange a small, fixed-order sequence of framed JSON
messages over the child's inherited stdin (parent -> child) and stdout
(child -> parent). Structured logs continue to go to stderr
(`logging_config.py`), a completely separate stream -- protocol messages
and log lines can never be confused with each other because they never
share a stream, and this module never writes anything to `stdout` except
the exact protocol frames defined here (nothing here calls `print`, and
`write_frame` is the only function that touches the child's stdout at
all).

Wire format (must match `app/src-tauri/src/supervisor/protocol.rs`
byte-for-byte):

    frame = u32_big_endian(len(payload)) ++ payload
    payload = UTF-8-encoded JSON object, always containing at least
              {"v": 1, "type": "<message-type>"}

`len(payload)` is bounded by `MAX_FRAME_BYTES`; a frame claiming to be
larger is a protocol violation, not a value to attempt reading. Every
message declares protocol version 1 as a JSON *integer* (`"v": 1`) -- a
JSON boolean or float in that position is rejected the same as a wrong
integer (Python's `bool` is a subtype of `int`, so this module always
checks `type(value) is int`, never `isinstance(value, int)`, to keep
`true`/`false` from silently passing as `1`/`0`). A JSON object is parsed
with duplicate keys rejected outright (see `_strict_object_pairs_hook`) -- never
silently collapsed to last-value-wins -- and, per message type, only the
exact expected key set is accepted; an extra or missing key is a protocol
violation.

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

Every `ProtocolError` this module raises carries one of a small, fixed set
of category strings, defined once as `_Reason` constants below -- never an
f-string built from the received message's `type`, field contents, or any
other attacker/parent-controlled value. This is deliberate: these
messages can end up in structured logs (`supervised.py`) and, indirectly,
in the frontend-visible `Failed` reason on the Rust side -- neither may
ever echo back received content.
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

#: Exact required decoded length of the startup secret -- part of S1-04's
#: strict wire schema. Matches the 32 random bytes Tauri generates
#: (`app/src-tauri/src/supervisor/mod.rs::STARTUP_SECRET_LEN`).
STARTUP_SECRET_LENGTH = 32

_LENGTH_STRUCT = struct.Struct(">I")  # big-endian uint32

# --- Fixed, bounded error reasons -------------------------------------------
# Every one of these is a literal string constant. Nothing in this module
# ever builds a `ProtocolError` from `f"...{attacker_or_parent_value}..."`.

_REASON_CHANNEL_CLOSED = "channel_closed"
_REASON_FRAME_TOO_LARGE = "frame_too_large"
_REASON_FRAME_LENGTH_OUT_OF_BOUNDS = "frame_length_out_of_bounds"
_REASON_FRAME_NOT_VALID_JSON = "frame_not_valid_json"
_REASON_FRAME_DUPLICATE_KEY = "frame_duplicate_key"
_REASON_FRAME_NOT_AN_OBJECT = "frame_not_an_object"
_REASON_UNSUPPORTED_PROTOCOL_VERSION = "unsupported_protocol_version"
_REASON_FRAME_MISSING_TYPE = "frame_missing_type"
_REASON_UNEXPECTED_MESSAGE_TYPE = "unexpected_message_type"
_REASON_UNEXPECTED_SCHEMA = "unexpected_schema"


class ProtocolError(Exception):
    """Any private-channel framing/ordering/content violation. Always
    fatal -- there is no partial-recovery path. The message is always one
    of the fixed `_REASON_*` category strings above -- never request
    content, a received message type, a field value, or a path."""


class BinaryStream(Protocol):
    """Structural type for the raw stdin/stdout buffer objects this module
    operates on (`sys.stdin.buffer` / `sys.stdout.buffer` in production;
    `io.BytesIO` in tests)."""

    def read(self, n: int, /) -> bytes: ...
    def write(self, data: bytes, /) -> int: ...
    def flush(self) -> None: ...


def _strict_object_pairs_hook(pairs: list[tuple[str, object]]) -> dict[str, object]:
    """`json.loads(..., object_pairs_hook=...)` sees every key exactly as
    it appeared in the input, before the standard dict-building step would
    silently collapse duplicates (last-value-wins). Raising here is what
    makes a duplicate key a parse failure instead of a silently-accepted
    ambiguity."""
    seen: dict[str, object] = {}
    for key, value in pairs:
        if key in seen:
            raise ValueError(_REASON_FRAME_DUPLICATE_KEY)
        seen[key] = value
    return seen


def write_frame(stream: BinaryStream, message: dict[str, object]) -> None:
    payload = json.dumps(message, separators=(",", ":")).encode("utf-8")
    if len(payload) > MAX_FRAME_BYTES:
        raise ProtocolError(_REASON_FRAME_TOO_LARGE)
    stream.write(_LENGTH_STRUCT.pack(len(payload)))
    stream.write(payload)
    stream.flush()


def _read_exact(stream: BinaryStream, n: int) -> bytes:
    chunks = bytearray()
    while len(chunks) < n:
        chunk = stream.read(n - len(chunks))
        if not chunk:
            raise ProtocolError(_REASON_CHANNEL_CLOSED)
        chunks.extend(chunk)
    return bytes(chunks)


def read_frame(stream: BinaryStream) -> dict[str, object]:
    """Reads one frame and strictly validates its envelope: a JSON object
    with no duplicate keys, an integer (never bool/float) `"v" ==
    PROTOCOL_VERSION`, and a string `"type"`. Does **not** check the
    exact key set -- that depends on which message type this turns out to
    be, which the caller doesn't know until after this returns; callers
    must follow up with `_require_exact_keys` once they've routed on
    `message["type"]`.
    """
    (length,) = _LENGTH_STRUCT.unpack(_read_exact(stream, _LENGTH_STRUCT.size))
    if length == 0 or length > MAX_FRAME_BYTES:
        raise ProtocolError(_REASON_FRAME_LENGTH_OUT_OF_BOUNDS)
    payload = _read_exact(stream, length)
    try:
        message = json.loads(payload.decode("utf-8"), object_pairs_hook=_strict_object_pairs_hook)
    except (UnicodeDecodeError, ValueError) as exc:
        raise ProtocolError(_REASON_FRAME_NOT_VALID_JSON) from exc
    if not isinstance(message, dict):
        raise ProtocolError(_REASON_FRAME_NOT_AN_OBJECT)
    version = message.get("v")
    if type(version) is not int or version != PROTOCOL_VERSION:
        # `type(...) is not int` (not `isinstance`) deliberately excludes
        # `bool` -- `True == 1` in Python, so `isinstance(True, int) and
        # True == 1` would incorrectly accept `{"v": true}`.
        raise ProtocolError(_REASON_UNSUPPORTED_PROTOCOL_VERSION)
    if not isinstance(message.get("type"), str):
        raise ProtocolError(_REASON_FRAME_MISSING_TYPE)
    return message


def _require_exact_keys(message: dict[str, object], expected: frozenset[str]) -> None:
    if set(message.keys()) != expected:
        raise ProtocolError(_REASON_UNEXPECTED_SCHEMA)


# --- Typed helpers for the child's side of the exchange -------------------

_STARTUP_SECRET_KEYS = frozenset({"v", "type", "secret"})
_INSTALL_TOKEN_KEYS = frozenset({"v", "type", "token"})
_SHUTDOWN_KEYS = frozenset({"v", "type"})


def read_startup_secret(stream: BinaryStream) -> bytes:
    """Reads and strictly validates the mandatory first message. Anything
    else -- wrong type, missing/malformed/wrong-length `secret` field,
    unknown/missing keys, EOF -- is a `ProtocolError`, and the caller must
    never open a socket first."""
    message = read_frame(stream)
    if message["type"] != "startup_secret":
        raise ProtocolError(_REASON_UNEXPECTED_MESSAGE_TYPE)
    _require_exact_keys(message, _STARTUP_SECRET_KEYS)
    secret_b64 = message.get("secret")
    try:
        return b64url.decode_exact(
            secret_b64, field="startup_secret", expected_length=STARTUP_SECRET_LENGTH
        )
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
    is fixed, not a general-purpose queue. The message is routed on
    `type` first, then checked against *that* type's own exact key set --
    an `install_session_token` with an extra field, or a `shutdown` with
    any field beyond `v`/`type`, is rejected either way.
    """
    message = read_frame(stream)
    msg_type = message["type"]
    if not isinstance(msg_type, str):
        raise ProtocolError(_REASON_FRAME_MISSING_TYPE)  # unreachable: read_frame already checked
    if msg_type == "install_session_token":
        _require_exact_keys(message, _INSTALL_TOKEN_KEYS)
        token = message.get("token")
        if not isinstance(token, str) or not token:
            raise ProtocolError(_REASON_UNEXPECTED_SCHEMA)
        return msg_type, token
    if msg_type == "shutdown":
        _require_exact_keys(message, _SHUTDOWN_KEYS)
        return msg_type, None
    raise ProtocolError(_REASON_UNEXPECTED_MESSAGE_TYPE)


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
