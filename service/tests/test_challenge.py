from __future__ import annotations

import hmac
import time
from hashlib import sha256

import pytest

from evidencegraph_service import b64url, challenge

SECRET = bytes(range(32))
HOST = "127.0.0.1"
PORT = 54321


def _expected_response(*, secret: bytes, host: str, port: int, nonce: bytes) -> bytes:
    """Independent re-implementation of the documented byte construction,
    deliberately not calling `challenge._compute_response` -- this is the
    cross-check that the documented construction is actually reproducible
    from the docstring alone, the same way `supervisor/challenge.rs` must
    reproduce it."""
    endpoint = f"{host}:{port}".encode("ascii")
    message = challenge.DOMAIN_SEPARATOR + b"\x00" + endpoint + b"\x00" + nonce
    return hmac.new(secret, message, sha256).digest()


def _nonce() -> tuple[bytes, str]:
    raw = bytes(range(16)) + bytes(reversed(range(16)))
    return raw, b64url.encode(raw)


def test_compute_response_matches_independent_construction() -> None:
    state = challenge.StartupChallengeState(secret=SECRET, host=HOST, port=PORT)
    nonce_raw, nonce_b64 = _nonce()

    response_b64 = state.compute_response(nonce_b64=nonce_b64)

    expected = _expected_response(secret=SECRET, host=HOST, port=PORT, nonce=nonce_raw)
    assert b64url.decode(response_b64, field="response") == expected


def test_response_differs_for_different_endpoint() -> None:
    nonce_raw, nonce_b64 = _nonce()
    state_a = challenge.StartupChallengeState(secret=SECRET, host=HOST, port=PORT)
    state_b = challenge.StartupChallengeState(secret=SECRET, host=HOST, port=PORT + 1)

    response_a = state_a.compute_response(nonce_b64=nonce_b64)
    response_b = state_b.compute_response(nonce_b64=nonce_b64)

    assert response_a != response_b


def test_response_differs_for_different_secret() -> None:
    nonce_raw, nonce_b64 = _nonce()
    state_a = challenge.StartupChallengeState(secret=SECRET, host=HOST, port=PORT)
    state_b = challenge.StartupChallengeState(secret=bytes(range(1, 33)), host=HOST, port=PORT)

    assert state_a.compute_response(nonce_b64=nonce_b64) != state_b.compute_response(
        nonce_b64=nonce_b64
    )


def test_response_differs_for_different_nonce() -> None:
    _, nonce_a = _nonce()
    other_raw = bytes(range(1, 17)) + bytes(range(17, 33))
    nonce_b = b64url.encode(other_raw)

    response_a = challenge.StartupChallengeState(
        secret=SECRET, host=HOST, port=PORT
    ).compute_response(nonce_b64=nonce_a)
    response_b = challenge.StartupChallengeState(
        secret=SECRET, host=HOST, port=PORT
    ).compute_response(nonce_b64=nonce_b)

    assert response_a != response_b


def test_challenge_is_single_use() -> None:
    """Replay protection: a second call -- even with a valid nonce --
    after the first success is rejected, not answered again."""
    state = challenge.StartupChallengeState(secret=SECRET, host=HOST, port=PORT)
    _, nonce_b64 = _nonce()

    state.compute_response(nonce_b64=nonce_b64)

    assert state.is_open is False
    with pytest.raises(challenge.ChallengeError):
        state.compute_response(nonce_b64=nonce_b64)


def test_succeeded_flag_only_true_after_a_real_success() -> None:
    state = challenge.StartupChallengeState(secret=SECRET, host=HOST, port=PORT)
    assert state.succeeded is False

    _, nonce_b64 = _nonce()
    state.compute_response(nonce_b64=nonce_b64)

    assert state.succeeded is True


def test_malformed_nonce_is_rejected_and_still_consumes_an_attempt() -> None:
    state = challenge.StartupChallengeState(secret=SECRET, host=HOST, port=PORT)

    with pytest.raises(challenge.ChallengeError):
        state.compute_response(nonce_b64="not valid base64url!!")

    # Still open (malformed input isn't itself the "used up" success path),
    # but the attempt was consumed.
    assert state.is_open is True
    assert state._attempts == 1


def test_too_many_attempts_closes_the_challenge() -> None:
    state = challenge.StartupChallengeState(secret=SECRET, host=HOST, port=PORT)

    for _ in range(challenge.MAX_ATTEMPTS):
        with pytest.raises(challenge.ChallengeError):
            state.compute_response(nonce_b64="bad!!")

    assert state.is_open is False
    with pytest.raises(challenge.ChallengeError):
        state.compute_response(nonce_b64="bad!!")


def test_expired_challenge_is_rejected(monkeypatch: pytest.MonkeyPatch) -> None:
    state = challenge.StartupChallengeState(secret=SECRET, host=HOST, port=PORT)
    _, nonce_b64 = _nonce()

    future = state._opened_at + challenge.MAX_LIFETIME_SECONDS + 1
    monkeypatch.setattr(time, "monotonic", lambda: future)

    assert state.is_open is False
    with pytest.raises(challenge.ChallengeError):
        state.compute_response(nonce_b64=nonce_b64)


def test_close_is_idempotent_and_prevents_further_use() -> None:
    state = challenge.StartupChallengeState(secret=SECRET, host=HOST, port=PORT)
    state.close()
    state.close()

    assert state.is_open is False
    _, nonce_b64 = _nonce()
    with pytest.raises(challenge.ChallengeError):
        state.compute_response(nonce_b64=nonce_b64)


def test_canonical_endpoint_format() -> None:
    assert challenge.canonical_endpoint("127.0.0.1", 51823) == "127.0.0.1:51823"
