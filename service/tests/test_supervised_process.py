"""Real-process integration tests for the S1-04 supervised entry point
(`supervised.py`): drives an actual `python -m evidencegraph_service
--supervised` subprocess over real OS pipes, playing the "Tauri parent"
role end-to-end -- private-channel framing, the D-025 HTTP challenge, and
the authenticated `/health` round trip. `test_server_process.py` is this
file's S1-03 sibling (standalone mode); this one is supervised mode.

The Rust-side implementation of everything `_FakeTauri` does here lives in
`app/src-tauri/src/supervisor/`. This file exists so the documented wire
protocol and HMAC challenge construction are proven end-to-end, from a
second, independent implementation of the "Tauri" side, before/alongside
the Rust one -- if this file's `_expected_response` (copied from the
`challenge.py` docstring, not imported from its implementation) ever
disagreed with what the real child computes, that would mean the
documented byte construction is ambiguous or wrong, which is exactly the
kind of mismatch that would otherwise only surface as "the real Rust
handshake mysteriously never authenticates."
"""

from __future__ import annotations

import concurrent.futures
import hmac
import os
import socket
import subprocess
import sys
from collections.abc import Iterator
from dataclasses import dataclass, field
from hashlib import sha256
from typing import IO, Any

import httpx
import pytest

from evidencegraph_service import b64url, protocol
from evidencegraph_service.challenge import DOMAIN_SEPARATOR

_READY_TIMEOUT_SECONDS = 15.0
_SHORT_TIMEOUT_SECONDS = 5.0


def _expected_response(*, secret: bytes, host: str, port: int, nonce: bytes) -> bytes:
    endpoint = f"{host}:{port}".encode("ascii")
    message = DOMAIN_SEPARATOR + b"\x00" + endpoint + b"\x00" + nonce
    return hmac.new(secret, message, sha256).digest()


def _read_frame_with_timeout(stream: IO[bytes], *, timeout: float) -> dict[str, Any]:
    """`protocol.read_frame` blocks indefinitely on a dead/hung child; runs
    it on a helper thread so a broken test (or a genuine regression) fails
    fast instead of hanging the whole suite. The helper thread is not
    joined on timeout -- it unblocks on its own once the child's stdout
    pipe eventually closes (process termination), which every test
    guarantees via its fixture teardown."""
    executor = concurrent.futures.ThreadPoolExecutor(max_workers=1)
    future = executor.submit(protocol.read_frame, stream)
    try:
        return future.result(timeout=timeout)
    except concurrent.futures.TimeoutError as exc:
        raise TimeoutError(f"no frame received within {timeout}s") from exc
    finally:
        executor.shutdown(wait=False)


def _spawn() -> subprocess.Popen[bytes]:
    env = dict(os.environ)
    for key in list(env):
        if key.startswith("EVIDENCEGRAPH_"):
            del env[key]
    return subprocess.Popen(  # noqa: S603 -- fixed argv, no shell, no untrusted input
        [sys.executable, "-m", "evidencegraph_service", "--supervised"],
        env=env,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )


@dataclass
class _FakeTauri:
    """Plays the parent role over a real spawned child's stdio -- the
    Python-side proof that the documented protocol/challenge is actually
    implementable and correct, ahead of the Rust implementation."""

    process: subprocess.Popen[bytes]
    secret: bytes = field(default_factory=lambda: os.urandom(32))
    host: str | None = None
    port: int | None = None
    captured: bool = False
    stdout_text: str = ""
    stderr_text: str = ""

    @property
    def _stdin(self) -> IO[bytes]:
        assert self.process.stdin is not None
        return self.process.stdin

    @property
    def _stdout(self) -> IO[bytes]:
        assert self.process.stdout is not None
        return self.process.stdout

    def send_startup_secret(self) -> None:
        protocol.write_frame(
            self._stdin,
            {"v": 1, "type": "startup_secret", "secret": b64url.encode(self.secret)},
        )

    def send_raw(self, message: dict[str, object]) -> None:
        protocol.write_frame(self._stdin, message)

    def read_endpoint_ready(self, *, timeout: float = _READY_TIMEOUT_SECONDS) -> tuple[str, int]:
        message = _read_frame_with_timeout(self._stdout, timeout=timeout)
        assert message["type"] == "endpoint_ready"
        host, port = message["host"], message["port"]
        assert isinstance(host, str)
        assert isinstance(port, int)
        self.host, self.port = host, port
        return host, port

    def base_url(self) -> str:
        assert self.host is not None and self.port is not None
        return f"http://{self.host}:{self.port}"

    def do_challenge(self, *, nonce: bytes | None = None) -> httpx.Response:
        nonce = nonce if nonce is not None else os.urandom(16)
        return httpx.post(
            f"{self.base_url()}/__startup/challenge",
            json={"nonce": b64url.encode(nonce)},
            timeout=_SHORT_TIMEOUT_SECONDS,
        )

    def expected_response_for(self, nonce: bytes, *, secret: bytes | None = None) -> str:
        assert self.host is not None and self.port is not None
        return b64url.encode(
            _expected_response(
                secret=secret if secret is not None else self.secret,
                host=self.host,
                port=self.port,
                nonce=nonce,
            )
        )

    def send_install_token(self, token: str) -> None:
        protocol.write_frame(self._stdin, {"v": 1, "type": "install_session_token", "token": token})

    def read_ready(self, *, timeout: float = _READY_TIMEOUT_SECONDS) -> None:
        message = _read_frame_with_timeout(self._stdout, timeout=timeout)
        assert message == {"v": 1, "type": "ready"}

    def send_shutdown(self) -> None:
        protocol.write_frame(self._stdin, {"v": 1, "type": "shutdown"})

    def read_shutdown_ack(self, *, timeout: float = _SHORT_TIMEOUT_SECONDS) -> None:
        message = _read_frame_with_timeout(self._stdout, timeout=timeout)
        assert message == {"v": 1, "type": "shutdown_ack"}

    def close_stdin(self) -> None:
        try:
            self._stdin.close()
        except OSError:
            pass

    def terminate_and_capture(self) -> None:
        if self.captured:
            return
        self.close_stdin()
        self.process.terminate()
        try:
            stdout, stderr = self.process.communicate(timeout=10)
        except subprocess.TimeoutExpired:
            self.process.kill()
            stdout, stderr = self.process.communicate(timeout=10)
        self.stdout_text = stdout.decode("latin-1", errors="replace")
        self.stderr_text = stderr.decode("latin-1", errors="replace")
        self.captured = True


@pytest.fixture
def tauri() -> Iterator[_FakeTauri]:
    fake = _FakeTauri(process=_spawn())
    try:
        yield fake
    finally:
        fake.terminate_and_capture()


VALID_TOKEN = "supervised-integration-test-token-1"


def test_full_handshake_and_authenticated_health_round_trip(tauri: _FakeTauri) -> None:
    tauri.send_startup_secret()
    host, port = tauri.read_endpoint_ready()
    assert host == "127.0.0.1"
    assert 1 <= port <= 65535

    nonce = os.urandom(16)
    response = tauri.do_challenge(nonce=nonce)
    assert response.status_code == 200
    assert response.json()["response"] == tauri.expected_response_for(nonce)

    tauri.send_install_token(VALID_TOKEN)
    tauri.read_ready()

    authed = httpx.get(
        f"{tauri.base_url()}/health",
        headers={"Authorization": f"Bearer {VALID_TOKEN}"},
        timeout=_SHORT_TIMEOUT_SECONDS,
    )
    assert authed.status_code == 200
    assert authed.json() == {
        "status": "ok",
        "service": "evidencegraph-service",
        "version": "0.1.0",
    }

    # Challenge route is now permanently closed. An unauthenticated probe
    # of it looks identical to any other unauthenticated request (401),
    # not a distinguishing 404 -- see `test_challenge_route.py` for the
    # authenticated-caller case, which does get 404 from the route itself.
    closed = tauri.do_challenge()
    assert closed.status_code == 401

    tauri.send_shutdown()
    tauri.read_shutdown_ack()
    exit_code = tauri.process.wait(timeout=_SHORT_TIMEOUT_SECONDS)
    assert exit_code == 0

    with pytest.raises(OSError):
        socket.create_connection((host, port), timeout=1)


def test_port_is_os_assigned_and_unaffected_by_an_unrelated_listener(tauri: _FakeTauri) -> None:
    blocker = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    try:
        blocker.bind(("127.0.0.1", 0))
        blocker.listen(1)
        blocked_port = blocker.getsockname()[1]

        tauri.send_startup_secret()
        _host, port = tauri.read_endpoint_ready()

        assert port != blocked_port
        assert port != 0
        with socket.create_connection(("127.0.0.1", port), timeout=_SHORT_TIMEOUT_SECONDS):
            pass  # the child's own port is independently reachable
    finally:
        blocker.close()


def test_response_computed_with_wrong_secret_does_not_match(tauri: _FakeTauri) -> None:
    tauri.send_startup_secret()
    tauri.read_endpoint_ready()
    nonce = os.urandom(16)

    response = tauri.do_challenge(nonce=nonce)

    wrong_expected = tauri.expected_response_for(nonce, secret=os.urandom(32))
    assert response.json()["response"] != wrong_expected
    assert response.json()["response"] == tauri.expected_response_for(nonce)


def test_replayed_challenge_is_rejected(tauri: _FakeTauri) -> None:
    tauri.send_startup_secret()
    tauri.read_endpoint_ready()

    first = tauri.do_challenge()
    second = tauri.do_challenge()  # fresh nonce, but the challenge is spent

    assert first.status_code == 200
    assert second.status_code == 401  # unauthenticated + closed looks like any 401


def test_malformed_nonce_returns_400_without_closing_the_challenge(tauri: _FakeTauri) -> None:
    tauri.send_startup_secret()
    tauri.read_endpoint_ready()

    bad = httpx.post(
        f"{tauri.base_url()}/__startup/challenge",
        json={"nonce": "not valid base64url!!"},
        timeout=_SHORT_TIMEOUT_SECONDS,
    )
    assert bad.status_code == 400

    # Still within MAX_ATTEMPTS -- a subsequent valid attempt still works.
    good = tauri.do_challenge()
    assert good.status_code == 200


def test_malformed_startup_secret_message_exits_without_opening_a_port(
    tauri: _FakeTauri,
) -> None:
    tauri.send_raw({"v": 1, "type": "ready"})  # wrong message type, first message

    # The child notices immediately and reports a bounded error frame --
    # it never gets far enough to send `endpoint_ready` at all.
    message = _read_frame_with_timeout(tauri._stdout, timeout=_SHORT_TIMEOUT_SECONDS)
    assert message["type"] == "error"

    exit_code = tauri.process.wait(timeout=_SHORT_TIMEOUT_SECONDS)
    assert exit_code != 0


def test_install_token_before_challenge_succeeds_is_rejected(tauri: _FakeTauri) -> None:
    tauri.send_startup_secret()
    tauri.read_endpoint_ready()
    # Deliberately skip the HTTP challenge entirely.

    tauri.send_install_token(VALID_TOKEN)

    # The child treats this as a protocol violation and exits -- it does
    # not send `ready` (the pipe simply closes, which the child-exit
    # assertion below is the real signal for).
    exit_code = tauri.process.wait(timeout=_READY_TIMEOUT_SECONDS)
    assert exit_code != 0


def test_shutdown_before_readiness_still_exits_cleanly(tauri: _FakeTauri) -> None:
    tauri.send_startup_secret()
    tauri.read_endpoint_ready()
    # No challenge, no credential -- an abandoned startup is still a valid
    # reason to shut down gracefully.

    tauri.send_shutdown()
    tauri.read_shutdown_ack()
    exit_code = tauri.process.wait(timeout=_SHORT_TIMEOUT_SECONDS)
    assert exit_code == 0


def test_private_channel_closing_early_stops_the_child(tauri: _FakeTauri) -> None:
    tauri.send_startup_secret()
    tauri.read_endpoint_ready()

    tauri.close_stdin()  # parent gone, mid-handshake, no shutdown message

    exit_code = tauri.process.wait(timeout=_READY_TIMEOUT_SECONDS)
    assert exit_code != 0


def test_no_secret_or_probe_content_and_no_traceback_in_logs(tauri: _FakeTauri) -> None:
    tauri.send_startup_secret()
    tauri.read_endpoint_ready()
    nonce = os.urandom(16)
    tauri.do_challenge(nonce=nonce)
    tauri.do_challenge()  # rejected (closed) -- exercises a failure log line too
    tauri.send_install_token(VALID_TOKEN)
    tauri.read_ready()
    tauri.send_shutdown()
    tauri.read_shutdown_ack()
    tauri.process.wait(timeout=_SHORT_TIMEOUT_SECONDS)

    tauri.terminate_and_capture()
    combined = tauri.stdout_text + tauri.stderr_text

    assert b64url.encode(tauri.secret) not in combined
    assert VALID_TOKEN not in combined
    assert b64url.encode(nonce) not in combined
    assert "Traceback" not in combined


_SECRET_MARKER = "MARKER-SECRET-VALUE-DO-NOT-LEAK-9f3a7c1e"


def test_secret_markers_in_malformed_frames_never_appear_in_logs() -> None:
    """S1-04 review finding 4 regression test: place a recognizable
    synthetic value inside *every* interesting field of a malformed first
    message and confirm it never surfaces in captured stdout/stderr. An
    earlier version of this code built `ProtocolError`/log messages with
    f-strings embedding the received `type` or field content directly
    (e.g. `f"expected_startup_secret_got_{message['type']}"`); this drives
    a marker through each of those historical injection points and proves
    none of them echo it back.
    """
    malformed_frames: list[dict[str, object]] = [
        {"v": 1, "type": _SECRET_MARKER, "secret": "A" * 43},  # marker as `type`
        {"v": 1, "type": "startup_secret", "secret": _SECRET_MARKER},  # marker as `secret`
        {  # marker as an unexpected extra field
            "v": 1,
            "type": "startup_secret",
            "secret": "A" * 43,
            "extra": _SECRET_MARKER,
        },
        {"v": _SECRET_MARKER, "type": "startup_secret", "secret": "A" * 43},  # marker as `v`
    ]

    for frame in malformed_frames:
        fake = _FakeTauri(process=_spawn())
        try:
            fake.send_raw(frame)
            fake.process.wait(timeout=_READY_TIMEOUT_SECONDS)
        finally:
            fake.terminate_and_capture()

        combined = fake.stdout_text + fake.stderr_text
        assert _SECRET_MARKER not in combined, f"marker leaked for frame {frame!r}"
        assert "Traceback" not in combined
    assert "ValueError" not in combined
