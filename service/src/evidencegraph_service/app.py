"""Application factory.

Separated from process execution (`main.py`) so tests -- and any future
supervising process -- can construct an app instance directly from an
explicit `Settings` object, without relying on real process environment
variables or starting a real server.
"""

from __future__ import annotations

import logging

from fastapi import FastAPI
from starlette.requests import Request
from starlette.responses import JSONResponse

from .auth import SessionTokenAuthMiddleware
from .config import Settings
from .routes.health import router as health_router

logger = logging.getLogger("evidencegraph_service")


def create_app(settings: Settings | None = None) -> FastAPI:
    resolved_settings = settings if settings is not None else Settings()
    docs_enabled = resolved_settings.environment == "development"

    app = FastAPI(
        title="EvidenceGraph Local Service",
        version="0.1.0",
        debug=False,
        docs_url="/docs" if docs_enabled else None,
        redoc_url="/redoc" if docs_enabled else None,
        openapi_url="/openapi.json" if docs_enabled else None,
    )
    app.state.settings = resolved_settings

    # Outermost of our own middleware: every request, including ones to
    # undefined paths, is checked before routing ever occurs.
    app.add_middleware(SessionTokenAuthMiddleware, settings=resolved_settings)

    app.include_router(health_router)

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

    if not resolved_settings.has_valid_credential_configured:
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
