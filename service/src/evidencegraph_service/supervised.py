"""Supervised-mode process entry point (S1-04).

`python -m evidencegraph_service --supervised` (see `__main__.py`) -- the
mode Tauri actually launches, via an explicit, non-PATH-resolved absolute
path (`app/src-tauri/src/supervisor/process.rs`). Implements the child's
half of the full D-018/D-025 startup handshake:

    1. Read the startup secret from the parent over the private channel
       (must be the first thing this process does -- no socket is opened
       before this succeeds).
    2. Bind `127.0.0.1:0` (OS-assigned port) and keep the bound socket --
       never "pick a port, close it, rebind"; ownership stays atomic all
       the way through to Uvicorn (`server.serve(sockets=[sock])`).
    3. Report the bound endpoint back over the private channel.
    4. Serve the D-025 HTTP challenge route (`routes/challenge.py`) --
       the sole pre-authentication endpoint -- until the parent's
       identity-verification handshake completes.
    5. Receive and install the session token over the private channel
       (only after a successful challenge -- the challenge and the
       protocol-message ordering are independently enforced; see
       `challenge.py` and `protocol.py`).
    6. Acknowledge readiness. Every route is authenticated from this point
       on; the challenge route is closed and behaves as if it never
       existed.
    7. Continue serving until a `shutdown` message arrives (graceful) or
       the private channel closes/EOFs (the parent is gone -- shut down
       rather than run on with nobody able to reach it cleanly).

Any failure before step 6 is fail-closed: this process exits non-zero
without ever having exposed a working, authenticated service. There is no
fallback, retry, or best-effort continuation anywhere in this sequence.
"""

from __future__ import annotations

import asyncio
import logging
import socket
import sys
import threading
from dataclasses import dataclass
from typing import NoReturn

import uvicorn

from . import protocol
from .app import create_app
from .challenge import StartupChallengeState
from .config import Settings
from .credentials import CredentialStore
from .logging_config import configure_logging

logger = logging.getLogger("evidencegraph_service.supervised")

_STARTUP_ERROR_EXIT_CODE = 1


def _fail(reason: str) -> NoReturn:
    """Logs a bounded reason, best-effort notifies the parent over the
    private channel (if stdout is still writable), then exits non-zero.
    Every pre-readiness failure path in this module funnels through here,
    so the process always fails the exact same way: fail closed, no
    partial state, no socket left listening for anything to connect to.
    """
    logger.error("supervised_startup_failed", extra={"reason": reason})
    try:
        protocol.write_error(sys.stdout.buffer, reason=reason)
    except Exception:  # noqa: S110 -- best-effort only; must not mask the exit below
        pass
    raise SystemExit(_STARTUP_ERROR_EXIT_CODE)


@dataclass
class _ProtocolOutcome:
    """Shared, single-writer (the protocol thread) result the main thread
    reads after `server.serve()` returns, to decide the process exit code.
    `clean_shutdown` is `True` if and only if an explicit `shutdown`
    message was received and acknowledged -- any other reason the loop
    stopped (protocol violation, malformed message, channel closed/EOF,
    unexpected error) leaves it `False`, regardless of whether a session
    credential happened to already be installed at that point."""

    clean_shutdown: bool = False


def _protocol_loop(
    *,
    credential_store: CredentialStore,
    challenge_state: StartupChallengeState,
    server: uvicorn.Server,
    outcome: _ProtocolOutcome,
) -> None:
    """Runs on a dedicated background thread for the entire lifetime of the
    server. Blocking stdin reads are fine here precisely because this
    thread is not the asyncio event loop running Uvicorn on the main
    thread -- one never blocks the other.
    """
    stdin = sys.stdin.buffer
    stdout = sys.stdout.buffer
    try:
        while True:
            msg_type, token = protocol.read_install_token_or_shutdown(stdin)
            if msg_type == "install_session_token":
                if token is None:
                    raise protocol.ProtocolError("install_session_token_missing_token")
                if not challenge_state.succeeded:
                    # Defense in depth: the parent's own message ordering
                    # already guarantees it only sends this after a
                    # successful challenge, but this process does not
                    # trust that ordering blindly (see `challenge.py`).
                    raise protocol.ProtocolError("install_before_challenge_succeeded")
                if not credential_store.install(token):
                    raise protocol.ProtocolError("credential_install_failed")
                challenge_state.close()
                protocol.write_ready(stdout)
                continue
            # Only "shutdown" remains -- `read_install_token_or_shutdown`
            # itself raises `ProtocolError` for any other message type.
            protocol.write_shutdown_ack(stdout)
            outcome.clean_shutdown = True
            server.should_exit = True
            return
    except protocol.ProtocolError as exc:
        logger.error("private_channel_protocol_error", extra={"reason": str(exc)})
        server.should_exit = True
    except Exception:
        # The parent process being gone (broken pipe/EOF) surfaces here
        # too, not just `ProtocolError` -- either way, a private channel
        # that stops working mid-session means shut down, not run on
        # headless with nobody able to reach it cleanly.
        logger.error("private_channel_unexpected_failure", extra={})
        server.should_exit = True


def main() -> None:
    configure_logging()

    stdin = sys.stdin.buffer
    stdout = sys.stdout.buffer

    try:
        secret = protocol.read_startup_secret(stdin)
    except protocol.ProtocolError:
        _fail("invalid_startup_secret")

    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    try:
        # Bind and retain ownership of this exact socket all the way to
        # `server.serve(sockets=[sock])` below -- never closed and
        # reopened, so there is no window for an unrelated process to
        # race for the same port between "choose" and "bind".
        sock.bind(("127.0.0.1", 0))
        sock.listen(128)
    except OSError:
        _fail("bind_failed")

    host, port = sock.getsockname()

    try:
        protocol.write_endpoint_ready(stdout, host=host, port=port)
    except protocol.ProtocolError:
        _fail("internal_error")

    try:
        settings = Settings(host=host, port=port, session_token=None)
        credential_store = CredentialStore()
        challenge_state = StartupChallengeState(secret=secret, host=host, port=port)

        app = create_app(
            settings,
            credential_store=credential_store,
            startup_challenge=challenge_state,
            supervised=True,
        )

        logger.info(
            "supervised_starting",
            extra={"host": host, "port": port, "environment": settings.environment},
        )

        config = uvicorn.Config(app, host=host, port=port, log_level="warning")
        server = uvicorn.Server(config)
        outcome = _ProtocolOutcome()

        protocol_thread = threading.Thread(
            target=_protocol_loop,
            kwargs={
                "credential_store": credential_store,
                "challenge_state": challenge_state,
                "server": server,
                "outcome": outcome,
            },
            daemon=True,
            name="evidencegraph-private-channel",
        )
        protocol_thread.start()

        asyncio.run(server.serve(sockets=[sock]))
        # By the time `serve()` has returned, whichever code path set
        # `server.should_exit` has already finished running (it does
        # nothing after setting that flag) -- this join is a bounded
        # belt-and-braces wait, not the actual synchronization point.
        protocol_thread.join(timeout=5.0)
    except Exception:
        # Never let an unexpected exception propagate out of `main()`
        # uncaught: Python's default handling for that would print a full
        # traceback (file/line/exception-message chain, potentially
        # echoing back something derived from a malformed message or
        # local state) to stderr just before exiting. A fixed, bounded
        # reason and a non-zero exit code carry the same "something went
        # wrong" signal to the parent without that risk.
        logger.error("supervised_unexpected_failure", extra={})
        raise SystemExit(_STARTUP_ERROR_EXIT_CODE) from None

    logger.info("supervised_stopped", extra={"clean_shutdown": outcome.clean_shutdown})
    if not outcome.clean_shutdown:
        raise SystemExit(_STARTUP_ERROR_EXIT_CODE)


if __name__ == "__main__":
    main()
