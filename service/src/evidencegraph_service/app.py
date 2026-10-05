"""Application factory.

Separated from process execution (`main.py`/`supervised.py`) so tests --
and either process entry point -- can construct an app instance directly
from an explicit `Settings` object, without relying on real process
environment variables or starting a real server.

Two modes, one factory:

- **Standalone** (S1-03, `main.py`): `credential_store`/`startup_challenge`
  omitted. A `CredentialStore` is created and immediately pre-populated
  from `Settings.session_token`, preserving S1-03's exact behavior --
  every existing test and code path is unaffected by S1-04.
- **Supervised** (S1-04, `supervised.py`): the caller passes an *empty*
  `CredentialStore` (installed later, over the private channel, only after
  the D-025 challenge succeeds) and a `StartupChallengeState` plus
  `supervised=True`. The one pre-authentication route
  (`routes/challenge.py`) is mounted and exempted from
  `SessionTokenAuthMiddleware` -- and only that one route, and only while
  the challenge is open.
"""

from __future__ import annotations

import logging

from fastapi import FastAPI
from starlette.requests import Request
from starlette.responses import JSONResponse

from .auth import SessionTokenAuthMiddleware
from .challenge import StartupChallengeState
from .config import Settings
from .credentials import CredentialStore
from .routes.challenge import CHALLENGE_PATH
from .routes.challenge import router as challenge_router
from .routes.health import router as health_router

logger = logging.getLogger("evidencegraph_service")


def create_app(
    settings: Settings | None = None,
    *,
    credential_store: CredentialStore | None = None,
    startup_challenge: StartupChallengeState | None = None,
    supervised: bool = False,
) -> FastAPI:
    resolved_settings = settings if settings is not None else Settings()
    docs_enabled = resolved_settings.environment == "development"

    resolved_store = (
        credential_store
        if credential_store is not None
        else CredentialStore.preinstalled(
            resolved_settings.session_token.get_secret_value()
            if resolved_settings.session_token is not None
            else None
        )
    )

    app = FastAPI(
        title="EvidenceGraph Local Service",
        version="0.1.0",
        debug=False,
        docs_url="/docs" if docs_enabled else None,
        redoc_url="/redoc" if docs_enabled else None,
        openapi_url="/openapi.json" if docs_enabled else None,
    )
    app.state.settings = resolved_settings
    app.state.credential_store = resolved_store
    # Always present (possibly `None`) so `routes/challenge.py` never needs
    # `getattr`/`hasattr` -- a missing attribute and an explicit `None` mean
    # the same thing there (route unavailable), so this keeps them the same
    # code path.
    app.state.startup_challenge = startup_challenge

    # Outermost of our own middleware: every request, including ones to
    # undefined paths, is checked before routing ever occurs. The
    # challenge-path exemption is `None`/`None` in standalone mode, which
    # `SessionTokenAuthMiddleware` treats as "no exemption at all" --
    # zero unauthenticated routes, exactly like S1-03.
    app.add_middleware(
        SessionTokenAuthMiddleware,
        credential_store=resolved_store,
        challenge_path=CHALLENGE_PATH if startup_challenge is not None else None,
        challenge_state=startup_challenge,
    )

    app.include_router(health_router)
    if startup_challenge is not None:
        app.include_router(challenge_router)

    @app.exception_handler(Exception)
    async def _unhandled_exception_handler(request: Request, exc: Exception) -> JSONResponse:
        # Minimal, constant response body regardless of the real error --
        # never a stack trace, file path, or exception message. Deliberately
        # never touches `request.url` (or anything derived from it): building
        # that property re-parses the raw `Host` header via
        # `urllib.parse.urlsplit`, which raises `ValueError` for a
        # syntactically invalid authority (e.g. `Host: [non-IP-text]`) -- a
        # handler that runs *because something already failed* must not be
        # able to fail the same way itself and turn a clean error response
        # into an unhandled exception/traceback. `request.method` is a plain
        # ASGI-scope read (no URL parsing) and the exception *class name*
        # only (no message/args, which could echo back request/config
        # content) goes to the structured log -- both bounded, predefined
        # values, never attacker-controlled request content.
        logger.error(
            "unhandled_exception",
            extra={"method": request.method, "exc_type": type(exc).__name__},
        )
        return JSONResponse({"detail": "Internal Server Error"}, status_code=500)

    # Supervised mode's credential store is *expected* to be empty at this
    # exact point -- the private-channel handshake that installs it hasn't
    # run yet, and `supervised.py` logs its own accurate startup sequence.
    # Warning here would misleadingly suggest a misconfiguration.
    if not supervised and not resolved_store.is_installed:
        logger.warning(
            "no_session_credential_configured",
            extra={
                "hint": (
                    "every request will be rejected until "
                    "EVIDENCEGRAPH_SESSION_TOKEN is set (standalone mode) or a "
                    "supervising process installs one (S1-04)"
                )
            },
        )

    return app
