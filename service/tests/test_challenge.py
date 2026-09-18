from __future__ import annotations

import hmac
import threading
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
    raw = bytes(range(16))
    return raw, b64url.encode(raw)


def test_handle_request_matches_independent_construction() -> None:
    state = challenge.StartupChallengeState(secret=SECRET, host=HOST, port=PORT)
    nonce_raw, nonce_b64 = _nonce()

    response_b64 = state.handle_request(nonce_b64=nonce_b64)

    expected = _expected_response(secret=SECRET, host=HOST, port=PORT, nonce=nonce_raw)
    assert b64url.decode(response_b64, field="response") == expected


def test_response_differs_for_different_endpoint() -> None:
    nonce_raw, nonce_b64 = _nonce()
    state_a = challenge.StartupChallengeState(secret=SECRET, host=HOST, port=PORT)
    state_b = challenge.StartupChallengeState(secret=SECRET, host=HOST, port=PORT + 1)

    response_a = state_a.handle_request(nonce_b64=nonce_b64)
    response_b = state_b.handle_request(nonce_b64=nonce_b64)

    assert response_a != response_b


def test_response_differs_for_different_secret() -> None:
    nonce_raw, nonce_b64 = _nonce()
    state_a = challenge.StartupChallengeState(secret=SECRET, host=HOST, port=PORT)
    state_b = challenge.StartupChallengeState(secret=bytes(range(1, 33)), host=HOST, port=PORT)

    assert state_a.handle_request(nonce_b64=nonce_b64) != state_b.handle_request(
        nonce_b64=nonce_b64
    )


def test_response_differs_for_different_nonce() -> None:
    _, nonce_a = _nonce()
    nonce_b = b64url.encode(bytes(reversed(range(16))))

    response_a = challenge.StartupChallengeState(
        secret=SECRET, host=HOST, port=PORT
    ).handle_request(nonce_b64=nonce_a)
    response_b = challenge.StartupChallengeState(
        secret=SECRET, host=HOST, port=PORT
    ).handle_request(nonce_b64=nonce_b)

    assert response_a != response_b


def test_challenge_is_single_use() -> None:
    """Replay protection: a second call -- even with a valid nonce --
    after the first success is rejected, not answered again."""
    state = challenge.StartupChallengeState(secret=SECRET, host=HOST, port=PORT)
    _, nonce_b64 = _nonce()

    state.handle_request(nonce_b64=nonce_b64)

    assert state.is_open is False
    with pytest.raises(challenge.ChallengeError):
        state.handle_request(nonce_b64=nonce_b64)


def test_succeeded_flag_only_true_after_a_real_success() -> None:
    state = challenge.StartupChallengeState(secret=SECRET, host=HOST, port=PORT)
    assert state.succeeded is False

    _, nonce_b64 = _nonce()
    state.handle_request(nonce_b64=nonce_b64)

    assert state.succeeded is True


def test_malformed_nonce_is_rejected_and_still_consumes_an_attempt() -> None:
    state = challenge.StartupChallengeState(secret=SECRET, host=HOST, port=PORT)

    with pytest.raises(challenge.ChallengeError):
        state.handle_request(nonce_b64="not valid base64url!!")

    # Still open (malformed input isn't itself the "used up" success path),
    # but the attempt was consumed.
    assert state.is_open is True
    assert state._attempts == 1


@pytest.mark.parametrize("length", [0, 1, 15, 17, 32, 64])
def test_handle_request_rejects_wrong_length_nonce(length: int) -> None:
    """Exact-length wire schema enforcement (S1-04 review finding 5): a
    nonce that decodes to anything other than exactly `NONCE_LENGTH`
    bytes is rejected, not truncated/padded/accepted leniently."""
    state = challenge.StartupChallengeState(secret=SECRET, host=HOST, port=PORT)
    wrong_length_nonce = b64url.encode(bytes(length))
    assert length != challenge.NONCE_LENGTH

    with pytest.raises(challenge.ChallengeError):
        state.handle_request(nonce_b64=wrong_length_nonce)


def test_exact_length_nonce_is_accepted() -> None:
    state = challenge.StartupChallengeState(secret=SECRET, host=HOST, port=PORT)
    exact_nonce = b64url.encode(bytes(challenge.NONCE_LENGTH))

    state.handle_request(nonce_b64=exact_nonce)  # must not raise


def test_too_many_attempts_closes_the_challenge() -> None:
    state = challenge.StartupChallengeState(secret=SECRET, host=HOST, port=PORT)

    for _ in range(challenge.MAX_ATTEMPTS):
        with pytest.raises(challenge.ChallengeError):
            state.handle_request(nonce_b64="bad!!")

    assert state.is_open is False
    with pytest.raises(challenge.ChallengeError):
        state.handle_request(nonce_b64="bad!!")


def test_expired_challenge_is_rejected(monkeypatch: pytest.MonkeyPatch) -> None:
    state = challenge.StartupChallengeState(secret=SECRET, host=HOST, port=PORT)
    _, nonce_b64 = _nonce()

    future = state._opened_at + challenge.MAX_LIFETIME_SECONDS + 1
    monkeypatch.setattr(time, "monotonic", lambda: future)

    assert state.is_open is False
    with pytest.raises(challenge.ChallengeError):
        state.handle_request(nonce_b64=nonce_b64)


def test_close_is_idempotent_and_prevents_further_use() -> None:
    state = challenge.StartupChallengeState(secret=SECRET, host=HOST, port=PORT)
    state.close()
    state.close()

    assert state.is_open is False
    _, nonce_b64 = _nonce()
    with pytest.raises(challenge.ChallengeError):
        state.handle_request(nonce_b64=nonce_b64)


def test_canonical_endpoint_format() -> None:
    assert challenge.canonical_endpoint("127.0.0.1", 51823) == "127.0.0.1:51823"


# --- S1-04 review finding 6: malformed requests must count as attempts -----


def test_reject_malformed_consumes_an_attempt() -> None:
    state = challenge.StartupChallengeState(secret=SECRET, host=HOST, port=PORT)

    state.reject_malformed()

    assert state._attempts == 1
    assert state.is_open is True


def test_reject_malformed_raises_when_already_closed() -> None:
    state = challenge.StartupChallengeState(secret=SECRET, host=HOST, port=PORT)
    state.close()

    with pytest.raises(challenge.ChallengeError):
        state.reject_malformed()


def test_reject_malformed_and_handle_request_share_one_attempt_budget() -> None:
    """An attacker cannot get unlimited free malformed probes before the
    well-formed-nonce guessing budget starts counting down -- both draw
    from the same counter."""
    state = challenge.StartupChallengeState(secret=SECRET, host=HOST, port=PORT)
    assert challenge.MAX_ATTEMPTS == 3

    state.reject_malformed()  # attempt 1/3
    state.reject_malformed()  # attempt 2/3
    assert state.is_open is True

    _, nonce_b64 = _nonce()
    state.handle_request(nonce_b64=nonce_b64)  # attempt 3/3 -- succeeds and closes

    assert state.is_open is False
    assert state.succeeded is True


def test_malformed_attempts_alone_can_exhaust_the_budget() -> None:
    state = challenge.StartupChallengeState(secret=SECRET, host=HOST, port=PORT)

    for _ in range(challenge.MAX_ATTEMPTS):
        state.reject_malformed()

    assert state.is_open is False
    assert state.succeeded is False
    _, nonce_b64 = _nonce()
    with pytest.raises(challenge.ChallengeError):
        state.handle_request(nonce_b64=nonce_b64)


# --- S1-04 review finding 7: atomic consumption (concurrency regression) ---


def test_concurrent_requests_produce_at_most_one_success() -> None:
    """Regression test for a real race: an earlier version of this method
    released the lock between the open/attempt check and computing the
    HMAC response, so two concurrent callers could both observe "still
    open", both pass the check, and both go on to compute and return a
    valid response before either one closed the challenge -- defeating
    the single-use guarantee entirely under concurrency. The fix holds one
    lock for the whole operation (check, attempt accounting, decode,
    compute, close). This drives many threads at the *same* challenge
    instance, synchronized to start as close to simultaneously as
    possible, each with a distinct valid nonce, and asserts only one can
    ever succeed."""
    state = challenge.StartupChallengeState(secret=SECRET, host=HOST, port=PORT)
    thread_count = 32
    nonces_b64 = [b64url.encode(bytes([i % 256]) * 16) for i in range(thread_count)]
    results: list[object] = [None] * thread_count
    barrier = threading.Barrier(thread_count)

    def _attempt(i: int) -> None:
        barrier.wait()
        try:
            results[i] = state.handle_request(nonce_b64=nonces_b64[i])
        except challenge.ChallengeError as exc:
            results[i] = exc

    threads = [threading.Thread(target=_attempt, args=(i,)) for i in range(thread_count)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()

    successes = [r for r in results if isinstance(r, str)]
    failures = [r for r in results if isinstance(r, challenge.ChallengeError)]

    assert len(successes) == 1, f"expected exactly one success, got {len(successes)}"
    assert len(failures) == thread_count - 1
    assert state.succeeded is True
    assert state.is_open is False
