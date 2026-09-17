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

Rejection logging and the ASGI-level exception handler (`app.py`) never
touch `request.url` (or anything derived from it). Starlette builds that
URL by splicing the raw `Host` header into a string and re-parsing it with
`urllib.parse.urlsplit`, which raises `ValueError` for a syntactically
invalid authority (e.g. `Host: [non-IP-text]`) -- an unauthenticated,
attacker-controlled header must never be able to make the auth path itself
raise. `request.headers`/`request.method` are plain ASGI-scope reads and
never trigger that parse.
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

#: Canonical scheme name used both for comparison (case-insensitively, per
#: RFC 9110 SS11.1: auth-scheme is a token and tokens are matched
#: case-insensitively) and for the `WWW-Authenticate` response header.
_AUTH_SCHEME = "Bearer"
_AUTH_SCHEME_LOWER = _AUTH_SCHEME.lower()

_WWW_AUTHENTICATE_HEADERS = {"WWW-Authenticate": _AUTH_SCHEME}


def _parse_authorization_header(header: str) -> tuple[str, str] | None:
    """Splits `<scheme> <credentials>` per RFC 9110 SS11.4/SS11.6.2:
    `credentials = auth-scheme [1*SP (token68 / #auth-param)]` -- the
    scheme is separated from the credentials by one-or-more SP characters
    (not tab, not other whitespace). Returns `None` if the header doesn't
    have that shape at all. The scheme is returned as-is (case comparison
    is the caller's job); the token is returned verbatim -- not stripped or
    normalized beyond consuming the 1*SP separator itself.
    """
    scheme, separator_found, rest = header.partition(" ")
    if not separator_found:
        return None
    token = rest.lstrip(" ")
    if not token:
        return None
    return scheme, token


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
            # Fixed event name plus a reason drawn from a small, predefined
            # set of category strings (see `_rejection_reason`) -- never
            # request content. No path/Host/header value is logged here.
            logger.warning("auth_rejected", extra={"reason": rejection_reason})
            return JSONResponse(
                _UNAUTHORIZED_BODY, status_code=401, headers=_WWW_AUTHENTICATE_HEADERS
            )
        return await call_next(request)

    def _rejection_reason(self, request: Request) -> str | None:
        """Returns None if authenticated, else a category string for logs.

        The returned reason is always a fixed category label, never the
        supplied header or token value. Only `request.headers` is read --
        never `request.url`, so a malformed `Host` header (or any other
        request metadata) can never make this raise.
        """
        if not self._settings.has_valid_credential_configured:
            return "no_credential_configured"

        header = request.headers.get("authorization")
        if not header:
            return "missing_authorization_header"

        parsed = _parse_authorization_header(header)
        if parsed is None:
            return "malformed_authorization_header"

        scheme, supplied_token = parsed
        if scheme.lower() != _AUTH_SCHEME_LOWER:
            return "malformed_authorization_header"

        expected_token = self._settings.session_token.get_secret_value()  # type: ignore[union-attr]
        if not hmac.compare_digest(supplied_token.encode("utf-8"), expected_token.encode("utf-8")):
            return "token_mismatch"

        return None
