"""D-025 domain-separated HMAC-SHA-256 startup identity challenge (S1-04).

This module implements the *child's* half of `docs/DECISIONS.md` D-018/
D-025's identity-verification handshake: Tauri (the parent) proves the
process answering on the bound loopback port is the exact child it spawned
-- not an unrelated or impersonating process that happened to occupy the
reported port -- before it will send that process any session credential
or evidence.

Exact byte construction (must match `app/src-tauri/src/supervisor/
challenge.rs` byte-for-byte, or the two sides compute different HMACs over
identical inputs and every legitimate handshake fails closed):

    message  = DOMAIN_SEPARATOR + 0x00 + endpoint_bytes + 0x00 + nonce_bytes
    response = HMAC-SHA-256(key=startup_secret_bytes, msg=message)

- `DOMAIN_SEPARATOR` is the fixed ASCII constant below -- distinct from any
  other HMAC/comparison use in this codebase and versioned, so a future
  protocol revision can never be misinterpreted as this one.
- `endpoint_bytes` is `f"{host}:{port}"` (`canonical_endpoint` below)
  ASCII-encoded: decimal port, no leading zeros, no scheme, no whitespace.
  For this service `host` is always the literal `127.0.0.1` (S1-03's
  `config._REQUIRED_HOST` contract).
- `nonce_bytes` is the *raw decoded bytes* of the nonce Tauri generated --
  never the Base64URL text itself -- so encoding choices on either side
  can't change the signed content.
- `0x00` field separators make the three-part construction unambiguous:
  none of the fields can ever contain a NUL byte, so there is exactly one
  way to have produced any given `message`.
- The startup secret is the HMAC *key*, never part of the signed message,
  and is never transmitted over HTTP in any form (D-018).
- Both the nonce (in the request) and the response (in the reply) are
  unpadded Base64URL (RFC 4648 SS5) -- the same alphabet S1-03 already
  requires for the session token (`config.is_valid_session_token_format`).

This module only *computes* the response for a given nonce -- it never
compares a response to anything, because the child is not the party doing
the verifying. Tauri independently recomputes the same HMAC from its own
copies of the secret/endpoint/nonce and compares in constant time on its
side (`supervisor/challenge.rs`); this module's only defensive
responsibility is refusing to compute *more than one* valid response for
this child instance at all (see `StartupChallengeState`), which is what
makes a captured (nonce, response) pair useless for replay.
"""

from __future__ import annotations

import hmac
import threading
import time
from hashlib import sha256

from . import b64url

#: Fixed ASCII domain separator -- see module docstring for the exact byte
#: construction this participates in.
DOMAIN_SEPARATOR = b"EvidenceGraph-S1-04-startup-challenge-v1"
_FIELD_SEPARATOR = b"\x00"

#: "Bound ... allowed attempts, and startup lifetime" -- independent,
#: defensive backstops on this side, not a substitute for Tauri's own
#: overall startup timeout (`supervisor/mod.rs`).
MAX_ATTEMPTS = 3
MAX_LIFETIME_SECONDS = 30.0


class ChallengeError(Exception):
    """A malformed, oversized, closed, or otherwise rejected challenge
    request. The message is always one of a small set of fixed, bounded,
    non-secret category strings -- never request content -- safe to use as
    a log field, though the route handler intentionally doesn't log
    per-request detail here either, consistent with S1-03's auth-rejection
    logging philosophy."""


def canonical_endpoint(host: str, port: int) -> str:
    """The exact string both sides must byte-for-byte agree on for a given
    bound endpoint. See module docstring."""
    return f"{host}:{port}"


def _compute_response(*, secret: bytes, endpoint: bytes, nonce: bytes) -> bytes:
    message = DOMAIN_SEPARATOR + _FIELD_SEPARATOR + endpoint + _FIELD_SEPARATOR + nonce
    return hmac.new(secret, message, sha256).digest()


class StartupChallengeState:
    """Per-child-instance challenge state. Exactly one instance exists per
    supervised process (constructed once, in `supervised.py`, immediately
    after the socket is bound); there is no cross-instance state, so a
    restarted child always starts with a fresh, independent challenge --
    a response computed against one instance's secret/endpoint can never
    verify against another's.
    """

    def __init__(self, *, secret: bytes, host: str, port: int) -> None:
        self._secret = secret
        self._endpoint = canonical_endpoint(host, port).encode("ascii")
        self._lock = threading.Lock()
        self._opened_at = time.monotonic()
        self._attempts = 0
        self._open = True
        self._succeeded = False

    def _open_locked(self) -> bool:
        """Caller must hold `self._lock`."""
        if not self._open:
            return False
        if time.monotonic() - self._opened_at > MAX_LIFETIME_SECONDS:
            self._open = False
            return False
        return True

    @property
    def is_open(self) -> bool:
        with self._lock:
            return self._open_locked()

    @property
    def succeeded(self) -> bool:
        with self._lock:
            return self._succeeded

    def compute_response(self, *, nonce_b64: str) -> str:
        """Computes and returns the Base64URL(unpadded) HMAC response for
        `nonce_b64`. This is a **single-use** operation: the challenge
        closes itself the moment it produces one successful response, so a
        captured (nonce, response) pair can never be replayed against this
        route again, and a second legitimate-looking request (race,
        retry, or attacker) after the first success is rejected the same
        way a stale/expired one is. Raises `ChallengeError` for a closed/
        expired/attempts-exhausted challenge or a malformed nonce; consumes
        one attempt for any request accepted past the open/lifetime check,
        regardless of whether the nonce itself turns out to be malformed.
        """
        with self._lock:
            if not self._open_locked():
                raise ChallengeError("challenge_not_open")
            self._attempts += 1
            if self._attempts >= MAX_ATTEMPTS:
                # This is the last attempt this challenge will ever accept,
                # succeed or fail -- close proactively (before releasing
                # the lock) so a concurrent caller can't sneak in one more
                # attempt while this one is still being computed.
                self._open = False

        try:
            nonce = b64url.decode(nonce_b64, field="nonce")
        except b64url.DecodeError as exc:
            raise ChallengeError(str(exc)) from exc
        response = _compute_response(secret=self._secret, endpoint=self._endpoint, nonce=nonce)

        with self._lock:
            self._succeeded = True
            self._open = False  # single-use: this was the one valid response.

        return b64url.encode(response)

    def close(self) -> None:
        """Idempotent, explicit close -- called once the resulting session
        token is actually installed (`supervised.py`), as a second,
        independent enforcement of "permanently close the challenge route
        once authentication becomes ready" on top of the single-use
        self-close in `compute_response`."""
        with self._lock:
            self._open = False
