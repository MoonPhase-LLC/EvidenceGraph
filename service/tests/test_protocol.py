from __future__ import annotations

import io
import struct

import pytest

from evidencegraph_service import b64url, protocol

VALID_SECRET = b64url.encode(bytes(range(32)))
VALID_TOKEN = "test-only-session-token-abcdef12"


def _frame_bytes(message: dict[str, object]) -> bytes:
    buf = io.BytesIO()
    protocol.write_frame(buf, message)
    return buf.getvalue()


def test_write_then_read_round_trips() -> None:
    buf = io.BytesIO(_frame_bytes({"v": 1, "type": "ready"}))
    message = protocol.read_frame(buf)
    assert message == {"v": 1, "type": "ready"}


def test_read_frame_rejects_eof_before_length_prefix() -> None:
    with pytest.raises(protocol.ProtocolError):
        protocol.read_frame(io.BytesIO(b""))


def test_read_frame_rejects_eof_mid_payload() -> None:
    buf = io.BytesIO(struct.pack(">I", 100) + b"short")
    with pytest.raises(protocol.ProtocolError):
        protocol.read_frame(buf)


def test_read_frame_rejects_oversized_length_prefix() -> None:
    buf = io.BytesIO(struct.pack(">I", protocol.MAX_FRAME_BYTES + 1))
    with pytest.raises(protocol.ProtocolError):
        protocol.read_frame(buf)


def test_read_frame_rejects_zero_length() -> None:
    buf = io.BytesIO(struct.pack(">I", 0))
    with pytest.raises(protocol.ProtocolError):
        protocol.read_frame(buf)


def test_read_frame_rejects_non_json() -> None:
    payload = b"not json at all"
    buf = io.BytesIO(struct.pack(">I", len(payload)) + payload)
    with pytest.raises(protocol.ProtocolError):
        protocol.read_frame(buf)


def test_read_frame_rejects_non_object_json() -> None:
    buf = io.BytesIO(_raw_json_frame([1, 2, 3]))
    with pytest.raises(protocol.ProtocolError):
        protocol.read_frame(buf)


def test_read_frame_rejects_wrong_protocol_version() -> None:
    buf = io.BytesIO(_raw_json_frame({"v": 2, "type": "ready"}))
    with pytest.raises(protocol.ProtocolError):
        protocol.read_frame(buf)


def test_read_frame_rejects_missing_type() -> None:
    buf = io.BytesIO(_raw_json_frame({"v": 1}))
    with pytest.raises(protocol.ProtocolError):
        protocol.read_frame(buf)


def _raw_json_frame(obj: object) -> bytes:
    import json

    payload = json.dumps(obj).encode("utf-8")
    return struct.pack(">I", len(payload)) + payload


def test_write_frame_rejects_oversized_message() -> None:
    huge = {"v": 1, "type": "ready", "padding": "x" * (protocol.MAX_FRAME_BYTES * 2)}
    with pytest.raises(protocol.ProtocolError):
        protocol.write_frame(io.BytesIO(), huge)


def test_read_startup_secret_round_trips() -> None:
    buf = io.BytesIO()
    protocol.write_frame(buf, {"v": 1, "type": "startup_secret", "secret": VALID_SECRET})
    buf.seek(0)
    assert protocol.read_startup_secret(buf) == bytes(range(32))


def test_read_startup_secret_rejects_wrong_message_type() -> None:
    buf = io.BytesIO()
    protocol.write_frame(buf, {"v": 1, "type": "ready"})
    buf.seek(0)
    with pytest.raises(protocol.ProtocolError):
        protocol.read_startup_secret(buf)


def test_read_startup_secret_rejects_malformed_secret_field() -> None:
    buf = io.BytesIO()
    protocol.write_frame(buf, {"v": 1, "type": "startup_secret", "secret": "not base64url!!"})
    buf.seek(0)
    with pytest.raises(protocol.ProtocolError):
        protocol.read_startup_secret(buf)


def test_read_startup_secret_rejects_missing_secret_field() -> None:
    buf = io.BytesIO()
    protocol.write_frame(buf, {"v": 1, "type": "startup_secret"})
    buf.seek(0)
    with pytest.raises(protocol.ProtocolError):
        protocol.read_startup_secret(buf)


def test_write_endpoint_ready_shape() -> None:
    buf = io.BytesIO()
    protocol.write_endpoint_ready(buf, host="127.0.0.1", port=54321)
    buf.seek(0)
    message = protocol.read_frame(buf)
    assert message == {"v": 1, "type": "endpoint_ready", "host": "127.0.0.1", "port": 54321}


def test_read_install_token_or_shutdown_install() -> None:
    buf = io.BytesIO()
    protocol.write_frame(buf, {"v": 1, "type": "install_session_token", "token": VALID_TOKEN})
    buf.seek(0)
    msg_type, token = protocol.read_install_token_or_shutdown(buf)
    assert msg_type == "install_session_token"
    assert token == VALID_TOKEN


def test_read_install_token_or_shutdown_shutdown() -> None:
    buf = io.BytesIO()
    protocol.write_frame(buf, {"v": 1, "type": "shutdown"})
    buf.seek(0)
    msg_type, token = protocol.read_install_token_or_shutdown(buf)
    assert msg_type == "shutdown"
    assert token is None


def test_read_install_token_or_shutdown_rejects_unknown_type() -> None:
    buf = io.BytesIO()
    protocol.write_frame(buf, {"v": 1, "type": "evict_everyone"})
    buf.seek(0)
    with pytest.raises(protocol.ProtocolError):
        protocol.read_install_token_or_shutdown(buf)


def test_read_install_token_or_shutdown_rejects_duplicate_startup_secret() -> None:
    """The channel's message order is fixed; a second `startup_secret`
    message at this point is out-of-order, not a harmless re-send."""
    buf = io.BytesIO()
    protocol.write_frame(buf, {"v": 1, "type": "startup_secret", "secret": VALID_SECRET})
    buf.seek(0)
    with pytest.raises(protocol.ProtocolError):
        protocol.read_install_token_or_shutdown(buf)


def test_read_install_token_or_shutdown_rejects_missing_token() -> None:
    buf = io.BytesIO()
    protocol.write_frame(buf, {"v": 1, "type": "install_session_token"})
    buf.seek(0)
    with pytest.raises(protocol.ProtocolError):
        protocol.read_install_token_or_shutdown(buf)


def test_write_ready_and_shutdown_ack_shapes() -> None:
    buf = io.BytesIO()
    protocol.write_ready(buf)
    buf.seek(0)
    assert protocol.read_frame(buf) == {"v": 1, "type": "ready"}

    buf = io.BytesIO()
    protocol.write_shutdown_ack(buf)
    buf.seek(0)
    assert protocol.read_frame(buf) == {"v": 1, "type": "shutdown_ack"}


def test_write_error_falls_back_to_internal_error_for_unknown_reason() -> None:
    buf = io.BytesIO()
    protocol.write_error(buf, reason="something-attacker-controlled")
    buf.seek(0)
    message = protocol.read_frame(buf)
    assert message == {"v": 1, "type": "error", "reason": "internal_error"}


# --- S1-04 review finding 5: strict wire schema -----------------------------


def _raw_frame(payload: bytes) -> bytes:
    return struct.pack(">I", len(payload)) + payload


def test_read_frame_rejects_duplicate_v_key() -> None:
    buf = io.BytesIO(_raw_frame(b'{"v":1,"v":1,"type":"ready"}'))
    with pytest.raises(protocol.ProtocolError):
        protocol.read_frame(buf)


def test_read_frame_rejects_duplicate_type_key() -> None:
    buf = io.BytesIO(_raw_frame(b'{"v":1,"type":"ready","type":"shutdown"}'))
    with pytest.raises(protocol.ProtocolError):
        protocol.read_frame(buf)


def test_read_frame_rejects_duplicate_key_in_a_typed_field() -> None:
    """Not just `v`/`type` -- any duplicate key anywhere in the object."""
    buf = io.BytesIO(
        _raw_frame(b'{"v":1,"type":"startup_secret","secret":"AAAA","secret":"BBBBBBBBBBBBBBBB"}')
    )
    with pytest.raises(protocol.ProtocolError):
        protocol.read_startup_secret(buf)


@pytest.mark.parametrize("version", [True, False])
def test_read_frame_rejects_boolean_version(version: bool) -> None:
    """`True == 1` in Python -- a naive `value == PROTOCOL_VERSION` check
    (or `isinstance(value, int)`, since `bool` subclasses `int`) would
    incorrectly accept `{"v": true}` as protocol version 1."""
    buf = io.BytesIO(_raw_json_frame({"v": version, "type": "ready"}))
    with pytest.raises(protocol.ProtocolError):
        protocol.read_frame(buf)


def test_read_frame_rejects_float_version() -> None:
    buf = io.BytesIO(_raw_json_frame({"v": 1.0, "type": "ready"}))
    with pytest.raises(protocol.ProtocolError):
        protocol.read_frame(buf)


def test_read_frame_rejects_string_version() -> None:
    buf = io.BytesIO(_raw_json_frame({"v": "1", "type": "ready"}))
    with pytest.raises(protocol.ProtocolError):
        protocol.read_frame(buf)


@pytest.mark.parametrize("length", [0, 1, 16, 31, 33, 64])
def test_read_startup_secret_rejects_wrong_length_secret(length: int) -> None:
    wrong_length_secret = b64url.encode(bytes(length))
    buf = io.BytesIO()
    protocol.write_frame(buf, {"v": 1, "type": "startup_secret", "secret": wrong_length_secret})
    buf.seek(0)
    with pytest.raises(protocol.ProtocolError):
        protocol.read_startup_secret(buf)


def test_read_startup_secret_rejects_unknown_extra_key() -> None:
    buf = io.BytesIO()
    protocol.write_frame(
        buf,
        {"v": 1, "type": "startup_secret", "secret": VALID_SECRET, "extra": "field"},
    )
    buf.seek(0)
    with pytest.raises(protocol.ProtocolError):
        protocol.read_startup_secret(buf)


def test_read_install_session_token_rejects_unknown_extra_key() -> None:
    buf = io.BytesIO()
    protocol.write_frame(
        buf,
        {"v": 1, "type": "install_session_token", "token": VALID_TOKEN, "extra": "field"},
    )
    buf.seek(0)
    with pytest.raises(protocol.ProtocolError):
        protocol.read_install_token_or_shutdown(buf)


def test_read_shutdown_rejects_extra_token_field() -> None:
    """A real `shutdown` message never carries a `token` -- one that does
    is rejected, not silently accepted with the field ignored."""
    buf = io.BytesIO()
    protocol.write_frame(buf, {"v": 1, "type": "shutdown", "token": VALID_TOKEN})
    buf.seek(0)
    with pytest.raises(protocol.ProtocolError):
        protocol.read_install_token_or_shutdown(buf)


def test_read_install_token_or_shutdown_rejects_boolean_version() -> None:
    buf = io.BytesIO(
        _raw_json_frame({"v": True, "type": "install_session_token", "token": VALID_TOKEN})
    )
    with pytest.raises(protocol.ProtocolError):
        protocol.read_install_token_or_shutdown(buf)


def test_multiple_frames_can_be_read_sequentially() -> None:
    """Simulates the real ordered exchange over one continuous stream."""
    buf = io.BytesIO()
    protocol.write_frame(buf, {"v": 1, "type": "startup_secret", "secret": VALID_SECRET})
    protocol.write_endpoint_ready(buf, host="127.0.0.1", port=1234)
    protocol.write_frame(buf, {"v": 1, "type": "install_session_token", "token": VALID_TOKEN})
    protocol.write_ready(buf)
    buf.seek(0)

    assert protocol.read_startup_secret(buf) == bytes(range(32))
    assert protocol.read_frame(buf) == {
        "v": 1,
        "type": "endpoint_ready",
        "host": "127.0.0.1",
        "port": 1234,
    }
    assert protocol.read_install_token_or_shutdown(buf) == ("install_session_token", VALID_TOKEN)
    assert protocol.read_frame(buf) == {"v": 1, "type": "ready"}
