"""Session-token authentication middleware.

`docs/DECISIONS.md` D-009 (delivery amended by D-025): every request to the
local service, including `GET /health`, must carry a bearer token matching
the installed session credential; missing/malformed/mismatched tokens
return 401. Missing credential configuration must never disable
authentication -- it must fail closed instead (every request rejected).

This is deliberately implemented as ASGI middleware, not a per-route
`Depends()`, so a future route added without updating an allowlist is
still protected by construction -- there is no opt-out mechanism here.
D-025's bounded startup challenge is the sole pre-authentication protocol
exception in the target architecture, and it is not implemented by this
ticket (see `docs/SPRINT_1_BACKLOG.md` S1-03/S1-04): this service, as
shipped here, has zero unauthenticated routes.
"""

from __future__ import annotations

import hmac
import logging
from collections.abc import Awaitable, Callable

from starlette.middleware.base import BaseHTTPMiddleware
from starlette.requests import Request
from starlette.responses import JSONResponse, Response
from starlette.types import ASGIApp

from .config import Settings

logger = logging.getLogger("evidencegraph_service.auth")

_AUTH_SCHEME = "Bearer"

# Identical body for every rejection reason: an attacker (or a curious
# local process) gets no signal about *why* a request was rejected --
# whether no credential is configured yet, the header is malformed, or the
# token is simply wrong all look the same from the outside.
_UNAUTHORIZED_BODY = {"detail": "Unauthorized"}


class SessionTokenAuthMiddleware(BaseHTTPMiddleware):
    def __init__(self, app: ASGIApp, settings: Settings) -> None:
        super().__init__(app)
        self._settings = settings

    async def dispatch(
        self, request: Request, call_next: Callable[[Request], Awaitable[Response]]
    ) -> Response:
        rejection_reason = self._rejection_reason(request)
        if rejection_reason is not None:
            logger.warning(
                "auth_rejected",
                extra={
                    "reason": rejection_reason,
                    "path": request.url.path,
                    "method": request.method,
                },
            )
            return JSONResponse(_UNAUTHORIZED_BODY, status_code=401)
        return await call_next(request)

    def _rejection_reason(self, request: Request) -> str | None:
        """Returns None if authenticated, else a category string for logs.

        The returned reason is always a fixed category label, never the
        supplied header or token value.
        """
        if not self._settings.has_valid_credential_configured:
            return "no_credential_configured"

        header = request.headers.get("authorization")
        if not header:
            return "missing_authorization_header"

        scheme, _, supplied_token = header.partition(" ")
        if scheme != _AUTH_SCHEME or not supplied_token:
            return "malformed_authorization_header"

        expected_token = self._settings.session_token.get_secret_value()  # type: ignore[union-attr]
        if not hmac.compare_digest(supplied_token.encode("utf-8"), expected_token.encode("utf-8")):
            return "token_mismatch"

        return None
