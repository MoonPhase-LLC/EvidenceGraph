from __future__ import annotations

import pytest

from evidencegraph_service import b64url


def test_round_trip() -> None:
    raw = bytes(range(32))
    encoded = b64url.encode(raw)
    assert b64url.decode(encoded, field="x") == raw


def test_encode_never_contains_padding() -> None:
    for length in range(1, 40):
        assert "=" not in b64url.encode(bytes(range(length)))


@pytest.mark.parametrize(
    "value",
    [
        None,
        123,
        1.5,
        [],
        {},
        "",  # empty
        "has a space",
        "has\ttab",
        "has\nnewline",
        "non-ascii-é",
        "padded==",
        "invalid+chars/here",
        "x" * 300,  # too long
    ],
)
def test_decode_rejects_invalid_input(value: object) -> None:
    with pytest.raises(b64url.DecodeError):
        b64url.decode(value, field="thefield")


def test_decode_error_message_names_the_field_not_the_value() -> None:
    with pytest.raises(b64url.DecodeError) as exc_info:
        b64url.decode("has a space and a secret-looking-value", field="startup_secret")

    assert "startup_secret" in str(exc_info.value)
    assert "secret-looking-value" not in str(exc_info.value)
