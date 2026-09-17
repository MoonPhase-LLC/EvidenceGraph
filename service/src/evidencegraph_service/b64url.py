"""Shared unpadded-Base64URL (RFC 4648 SS5) helpers.

The one text encoding this codebase uses anywhere secret-adjacent bytes
cross a text-based channel: the session-token format itself (`config.py`,
S1-03), the private-channel startup secret (`protocol.py`, S1-04), and the
D-025 challenge nonce/response (`challenge.py`, S1-04) all share this exact
alphabet and this exact decoder -- one implementation, so "what counts as
valid Base64URL here" can never quietly drift between them.
"""

from __future__ import annotations

import base64
import re

#: `A-Z a-z 0-9 - _`, no `=` padding -- see `config.py`'s
#: `is_valid_session_token_format` docstring for the full rationale.
TOKEN_PATTERN = re.compile(r"^[A-Za-z0-9_-]+$")

#: Generous upper bound for any single Base64URL-encoded field this
#: codebase decodes (secrets/nonces/responses are ~32-64 raw bytes, i.e.
#: ~43-86 encoded chars) -- exists only to bound parsing work on a value
#: that hasn't been validated yet, per the private-channel/challenge
#: "messages are bounded in size and strictly parsed" requirement.
MAX_FIELD_LENGTH = 256


class DecodeError(Exception):
    """Malformed or oversized Base64URL text. The message is always one of
    a small set of fixed, bounded, non-secret category strings (the field
    name plus a reason) -- never the offending value itself."""


def encode(raw: bytes) -> str:
    return base64.urlsafe_b64encode(raw).decode("ascii").rstrip("=")


def decode(value: object, *, field: str) -> bytes:
    """Strictly parses `value` as unpadded Base64URL. Rejects anything
    that isn't a non-empty, bounded-length string matching `TOKEN_PATTERN`
    *before* ever calling into the decoder -- never a best-effort decode
    of untrusted input."""
    if not isinstance(value, str) or not value or len(value) > MAX_FIELD_LENGTH:
        raise DecodeError(f"{field}_invalid_length")
    if not TOKEN_PATTERN.fullmatch(value):
        raise DecodeError(f"{field}_invalid_encoding")
    padded = value + "=" * (-len(value) % 4)
    try:
        return base64.urlsafe_b64decode(padded)
    except ValueError as exc:  # binascii.Error subclasses ValueError
        raise DecodeError(f"{field}_invalid_encoding") from exc
