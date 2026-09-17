"""Regression coverage for the malformed-`Host`-header crash.

Unlike the rest of this suite, these tests exercise a *real* Uvicorn server
listening on a real loopback socket, started via `python -m
evidencegraph_service` (exactly the advertised module entry point) as a
subprocess -- not `fastapi.testclient.TestClient`, which talks to the ASGI
app in-process and never goes through real HTTP/1.1 request-line parsing or
a real socket at all. The bug this guards against only reproduces over a
real transport: Starlette lazily builds `request.url` by splicing the raw
`Host` header into a string and re-parsing it with
`urllib.parse.urlsplit`, which raises `ValueError` for a syntactically
invalid authority such as `Host: [non-IP-text]`. `TestClient` requests
never trigger the code path that first surfaced this (see `auth.py`/
`app.py`), so only a real client speaking real HTTP/1.1 proves the fix.
"""

from __future__ import annotations

import json
import os
import socket
import subprocess
import sys
import time
from collections.abc import Iterator
from dataclasses import dataclass, field

import pytest

from .conftest import VALID_TOKEN

WRONG_TOKEN = "wrong-token-entirely-different-value12"

# Several shapes of malformed `Host` header, all of which the underlying
# HTTP server (h11/uvicorn) happily accepts as a header *value* -- the
# failure was always in application-level URL reconstruction, never at the
# transport layer -- including the exact reproduction from the review.
MALFORMED_HOSTS = [
    "[non-IP-text]",
    "[not-a-valid-ipv6-address]",
    "[]",
    "[gibberish",  # unterminated bracket
]

_READY_TIMEOUT_SECONDS = 15.0
_SOCKET_TIMEOUT_SECONDS = 5.0


def _free_loopback_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
        probe.bind(("127.0.0.1", 0))
        port: int = probe.getsockname()[1]
        return port


def _wait_until_accepting_connections(port: int, timeout: float) -> None:
    deadline = time.monotonic() + timeout
    last_error: OSError | None = None
    while time.monotonic() < deadline:
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=0.5):
                return
        except OSError as exc:
            last_error = exc
            time.sleep(0.1)
    raise TimeoutError(f"service never started listening on 127.0.0.1:{port}") from last_error


@dataclass
class _RawResponse:
    status_code: int
    headers: dict[str, str]
    body: bytes

    def json(self) -> object:
        return json.loads(self.body)


@dataclass
class _RunningService:
    port: int
    process: subprocess.Popen[str]
    stdout_text: str = field(default="")
    stderr_text: str = field(default="")
    captured: bool = field(default=False)

    def raw_request(
        self,
        *,
        host_header: str,
        path: str = "/health",
        method: str = "GET",
        authorization: str | None = None,
    ) -> _RawResponse:
        lines = [f"{method} {path} HTTP/1.1", f"Host: {host_header}", "Connection: close"]
        if authorization is not None:
            lines.append(f"Authorization: {authorization}")
        request_text = "\r\n".join(lines) + "\r\n\r\n"

        with socket.create_connection(
            ("127.0.0.1", self.port), timeout=_SOCKET_TIMEOUT_SECONDS
        ) as sock:
            sock.sendall(request_text.encode("latin-1"))
            sock.settimeout(_SOCKET_TIMEOUT_SECONDS)
            chunks: list[bytes] = []
            try:
                while True:
                    data = sock.recv(4096)
                    if not data:
                        break
                    chunks.append(data)
            except TimeoutError:
                pass
        raw = b"".join(chunks)
        return _parse_response(raw)


def _parse_response(raw: bytes) -> _RawResponse:
    header_blob, _, body = raw.partition(b"\r\n\r\n")
    header_lines = header_blob.split(b"\r\n")
    status_line = header_lines[0].decode("latin-1")
    status_code = int(status_line.split(" ", 2)[1])
    headers: dict[str, str] = {}
    for line in header_lines[1:]:
        if not line:
            continue
        key, _, value = line.decode("latin-1").partition(":")
        headers[key.strip().lower()] = value.strip()
    return _RawResponse(status_code=status_code, headers=headers, body=body)


def _spawn_service(*, session_token: str | None) -> tuple[int, subprocess.Popen[str]]:
    port = _free_loopback_port()
    env = dict(os.environ)
    for key in list(env):
        if key.startswith("EVIDENCEGRAPH_"):
            del env[key]
    env["EVIDENCEGRAPH_HOST"] = "127.0.0.1"
    env["EVIDENCEGRAPH_PORT"] = str(port)
    if session_token is not None:
        env["EVIDENCEGRAPH_SESSION_TOKEN"] = session_token

    # The advertised `python -m evidencegraph_service` entry point (S1-03
    # review finding #5) -- exercised here for its own sake as well as to
    # get a real Uvicorn process.
    process = subprocess.Popen(  # noqa: S603 -- fixed argv, no shell, no untrusted input
        [sys.executable, "-m", "evidencegraph_service"],
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    try:
        _wait_until_accepting_connections(port, _READY_TIMEOUT_SECONDS)
    except TimeoutError:
        process.kill()
        process.communicate(timeout=5)
        raise
    return port, process


@pytest.fixture
def running_service() -> Iterator[_RunningService]:
    """A real subprocess with a valid session credential configured."""
    port, process = _spawn_service(session_token=VALID_TOKEN)
    service = _RunningService(port=port, process=process)
    try:
        yield service
    finally:
        _terminate_and_capture(service)


@pytest.fixture
def running_service_without_credential() -> Iterator[_RunningService]:
    """A real subprocess with no session credential configured at all."""
    port, process = _spawn_service(session_token=None)
    service = _RunningService(port=port, process=process)
    try:
        yield service
    finally:
        _terminate_and_capture(service)


def _terminate_and_capture(service: _RunningService) -> None:
    """Idempotent: safe to call once mid-test (to inspect logs) and again
    from fixture teardown -- a second call is a no-op."""
    if service.captured:
        return
    service.process.terminate()
    try:
        stdout, stderr = service.process.communicate(timeout=10)
    except subprocess.TimeoutExpired:
        service.process.kill()
        stdout, stderr = service.process.communicate(timeout=10)
    service.stdout_text = stdout
    service.stderr_text = stderr
    service.captured = True


_UNAUTHORIZED_BODY = {"detail": "Unauthorized"}


@pytest.mark.parametrize("malformed_host", MALFORMED_HOSTS)
def test_malformed_host_with_missing_token_returns_401(
    running_service: _RunningService, malformed_host: str
) -> None:
    response = running_service.raw_request(host_header=malformed_host)

    assert response.status_code == 401
    assert response.json() == _UNAUTHORIZED_BODY
    assert response.headers.get("www-authenticate") == "Bearer"


@pytest.mark.parametrize("malformed_host", MALFORMED_HOSTS)
def test_malformed_host_with_incorrect_token_returns_401(
    running_service: _RunningService, malformed_host: str
) -> None:
    response = running_service.raw_request(
        host_header=malformed_host, authorization=f"Bearer {WRONG_TOKEN}"
    )

    assert response.status_code == 401
    assert response.json() == _UNAUTHORIZED_BODY


@pytest.mark.parametrize("malformed_host", MALFORMED_HOSTS)
def test_malformed_host_with_no_server_credential_configured_returns_401(
    running_service_without_credential: _RunningService, malformed_host: str
) -> None:
    response = running_service_without_credential.raw_request(
        host_header=malformed_host, authorization=f"Bearer {VALID_TOKEN}"
    )

    assert response.status_code == 401
    assert response.json() == _UNAUTHORIZED_BODY


def test_malformed_host_does_not_block_a_correctly_authenticated_request(
    running_service: _RunningService,
) -> None:
    """The fix removes `Host` from the auth decision entirely -- a request
    that would otherwise succeed still succeeds regardless of `Host`."""
    response = running_service.raw_request(
        host_header="[non-IP-text]", authorization=f"Bearer {VALID_TOKEN}"
    )

    assert response.status_code == 200
    assert response.json() == {
        "status": "ok",
        "service": "evidencegraph-service",
        "version": "0.1.0",
    }


def test_ordinary_valid_authenticated_request_still_works(
    running_service: _RunningService,
) -> None:
    response = running_service.raw_request(
        host_header="127.0.0.1", authorization=f"Bearer {VALID_TOKEN}"
    )

    assert response.status_code == 200
    assert response.json() == {
        "status": "ok",
        "service": "evidencegraph-service",
        "version": "0.1.0",
    }


def test_no_probe_strings_or_tracebacks_appear_in_captured_logs(
    running_service: _RunningService,
) -> None:
    for malformed_host in MALFORMED_HOSTS:
        running_service.raw_request(host_header=malformed_host)
        running_service.raw_request(
            host_header=malformed_host, authorization=f"Bearer {WRONG_TOKEN}"
        )
    running_service.raw_request(host_header="127.0.0.1", authorization=f"Bearer {VALID_TOKEN}")

    _terminate_and_capture(running_service)
    combined_log_text = running_service.stdout_text + running_service.stderr_text

    for malformed_host in MALFORMED_HOSTS:
        assert malformed_host not in combined_log_text
    assert VALID_TOKEN not in combined_log_text
    assert WRONG_TOKEN not in combined_log_text
    assert "Traceback" not in combined_log_text
    assert "ValueError" not in combined_log_text
    assert "Internal Server Error" not in combined_log_text
