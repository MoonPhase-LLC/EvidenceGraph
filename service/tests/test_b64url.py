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


# --- Canonical encoding (S1-04 review round 3, finding 4) -------------------
#
# 16 bytes encode to 22 chars and 32 bytes to 43 chars; in both cases the
# final character carries unused low-order bits (4 and 2 respectively),
# which must be zero in the canonical encoding. "A" decodes to 0 and "B" to
# 1, so `"A" * n + "B"` sets only an unused bit: a lenient decoder would
# map it to the same bytes as `"A" * (n + 1)`.


@pytest.mark.parametrize(
    ("noncanonical", "expected_length"),
    [("A" * 21 + "B", 16), ("A" * 42 + "B", 32)],
)
def test_decode_rejects_nonzero_unused_bits(noncanonical: str, expected_length: int) -> None:
    with pytest.raises(b64url.DecodeError) as exc_info:
        b64url.decode(noncanonical, field="nonce")
    assert str(exc_info.value) == "nonce_invalid_encoding"
    with pytest.raises(b64url.DecodeError):
        b64url.decode_exact(noncanonical, field="nonce", expected_length=expected_length)


@pytest.mark.parametrize(
    ("canonical", "expected_length"),
    [("A" * 22, 16), ("A" * 21 + "Q", 16), ("A" * 43, 32), ("A" * 42 + "E", 32)],
)
def test_decode_accepts_canonical_equivalents(canonical: str, expected_length: int) -> None:
    raw = b64url.decode_exact(canonical, field="nonce", expected_length=expected_length)
    assert b64url.encode(raw) == canonical


@pytest.mark.parametrize("length", [1, 2, 15, 16, 31, 32, 33, 64])
def test_encoded_values_always_decode_canonically(length: int) -> None:
    raw = bytes((i * 37 + 11) % 256 for i in range(length))
    assert b64url.decode(b64url.encode(raw), field="x") == raw
