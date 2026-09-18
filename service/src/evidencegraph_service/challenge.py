"""D-025 domain-separated HMAC-SHA-256 startup identity challenge (S1-04)
-- Tauri's (parent) half. Mirrors
`service/src/evidencegraph_service/challenge.py` byte-for-byte; that
module's docstring is the canonical description of the exact byte
construction:

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
  can't change the signed content. Must decode to exactly `NONCE_LENGTH`
  bytes.
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
side (`supervisor/challenge.rs`); this module's own defensive
responsibilities are:

1. Refusing to compute *more than one* valid response for this child
   instance at all (see `StartupChallengeState`), which is what makes a
   captured (nonce, response) pair useless for replay.
2. Doing so **atomically**: attempt accounting, the open/lifetime check,
   nonce parsing, and HMAC computation all happen under a single lock
   acquisition (`handle_request`) -- two concurrent requests can never
   both observe "still open" and both go on to compute a valid response.
   An earlier version of this module checked-then-released-the-lock
   before computing, which allowed exactly that race; a concurrency
   regression test (`tests/test_challenge.py::
   test_concurrent_requests_produce_at_most_one_success`) pins this down.
3. Counting *every* rejected request against the attempt budget,
   including malformed JSON, wrong schema, and invalid encoding/length --
   not just a well-formed-but-wrong nonce -- via `reject_malformed`,
   called by `routes/challenge.py` for failures it detects (oversized
   body, non-UTF-8, malformed JSON, wrong schema) before there is a
   `nonce_b64` value to hand to `handle_request` at all.
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

#: Exact required decoded lengths -- part of S1-04's strict wire schema.
NONCE_LENGTH = 16
RESPONSE_LENGTH = 32  # SHA-256 digest size; not independently configurable.

#: "Bound ... allowed attempts, and startup lifetime" -- independent,
#: defensive backstops on this side, not a substitute for Tauri's own
#: overall startup timeout (`supervisor/mod.rs`). Every request that
#: reaches this route while open counts against `MAX_ATTEMPTS`, malformed
#: or not -- see `reject_malformed`/`handle_request`.
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

    def _consume_attempt_locked(self) -> None:
        """Caller must hold `self._lock`. Counts this call as one attempt
        and proactively closes the challenge if it was the last one this
        instance will ever accept -- shared by both the malformed-request
        path (`reject_malformed`) and the well-formed path
        (`handle_request`), so every request that reaches this state while
        open, valid or not, consumes exactly one attempt."""
        self._attempts += 1
        if self._attempts >= MAX_ATTEMPTS:
            self._open = False

    @property
    def is_open(self) -> bool:
        with self._lock:
            return self._open_locked()

    @property
    def succeeded(self) -> bool:
        with self._lock:
            return self._succeeded

    def reject_malformed(self) -> None:
        """Records one consumed attempt for a request the caller (the HTTP
        route) already knows is malformed before it has a `nonce_b64`
        value to hand to `handle_request` -- an oversized body, non-UTF-8
        bytes, invalid JSON, a duplicate/unknown key, or a wrong-typed
        `nonce` field. Raises `ChallengeError("challenge_not_open")` if
        the challenge was already closed, exactly like `handle_request`,
        so callers can use the same except-and-400 handling either way.
        """
        with self._lock:
            if not self._open_locked():
                raise ChallengeError("challenge_not_open")
            self._consume_attempt_locked()

    def handle_request(self, *, nonce_b64: str) -> str:
        """Computes and returns the Base64URL(unpadded) HMAC response for
        `nonce_b64`. This is a **single-use** operation: the challenge
        closes itself the moment it produces one successful response, so a
        captured (nonce, response) pair can never be replayed against this
        route again, and a second legitimate-looking request (race,
        retry, or attacker) after the first success is rejected the same
        way a stale/expired one is.

        The open check, attempt accounting, nonce decode/length
        validation, and HMAC computation all happen under one held lock --
        not released and reacquired between steps -- so two concurrent
        calls can never both observe "still open" and both go on to
        produce a valid response (see module docstring point 2 and
        `tests/test_challenge.py::
        test_concurrent_requests_produce_at_most_one_success`).

        Raises `ChallengeError` for a closed/expired/attempts-exhausted
        challenge or a malformed/wrong-length nonce; consumes one attempt
        for any request accepted past the open/lifetime check, regardless
        of whether the nonce itself turns out to be malformed.
        """
        with self._lock:
            if not self._open_locked():
                raise ChallengeError("challenge_not_open")
            self._consume_attempt_locked()

            try:
                nonce = b64url.decode_exact(nonce_b64, field="nonce", expected_length=NONCE_LENGTH)
            except b64url.DecodeError as exc:
                raise ChallengeError(str(exc)) from exc

            response = _compute_response(secret=self._secret, endpoint=self._endpoint, nonce=nonce)
            self._succeeded = True
            self._open = False  # single-use: this was the one valid response.

            return b64url.encode(response)

    def close(self) -> None:
        """Idempotent, explicit close -- called once the resulting session
        token is actually installed (`supervised.py`), as a second,
        independent enforcement of "permanently close the challenge route
        once authentication becomes ready" on top of the single-use
        self-close in `handle_request`."""
        with self._lock:
            self._open = False
