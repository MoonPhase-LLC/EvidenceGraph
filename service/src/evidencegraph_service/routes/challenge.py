"""POST /__startup/challenge -- D-025's bounded, single-use, pre-
authentication identity challenge (S1-04).

See `challenge.py` for the HMAC construction and the single-use/lifetime/
attempt bounds enforced by `StartupChallengeState`; this module is only the
thin HTTP transport around it -- strict, bounded request parsing and a
uniform rejection response, mirroring `auth.py`'s own philosophy (fixed
response bodies, no oracle, no per-request content in logs).

Mounted only in supervised mode (`supervised.py`, via `create_app(...,
startup_challenge=...)`). Standalone mode never registers this router at
all, and `create_app` defaults `app.state.startup_challenge` to `None`,
which this handler treats identically to "closed" -- preserving S1-03's
invariant that the standalone service has zero unauthenticated routes. An
unauthenticated challenge endpoint would also be meaningless (and unsafe)
in standalone mode, since there is no startup secret to challenge against.
"""

from __future__ import annotations

import json

from fastapi import APIRouter, Request
from starlette.responses import JSONResponse

from ..challenge import ChallengeError, StartupChallengeState

router = APIRouter()

CHALLENGE_PATH = "/__startup/challenge"

#: Bounded independent of anything Starlette/FastAPI imposes by default --
#: the real request body is one short JSON field, `{"nonce": "<=~86
#: chars>"}`.
MAX_BODY_BYTES = 512

#: Identical body for every rejection reason (malformed JSON, oversized
#: body, bad nonce, closed/expired/exhausted challenge) -- no oracle, same
#: principle as `auth._UNAUTHORIZED_BODY`.
_REJECTED_BODY = {"detail": "Bad Request"}
_NOT_FOUND_BODY = {"detail": "Not Found"}


async def _read_bounded_body(request: Request) -> bytes | None:
    """Returns the body, or `None` if it exceeds `MAX_BODY_BYTES` --
    reading via `request.stream()` so an oversized body is detected and
    abandoned incrementally, never buffered in full first."""
    chunks = bytearray()
    async for chunk in request.stream():
        chunks.extend(chunk)
        if len(chunks) > MAX_BODY_BYTES:
            return None
    return bytes(chunks)


@router.post(CHALLENGE_PATH)
async def startup_challenge(request: Request) -> JSONResponse:
    challenge_state: StartupChallengeState | None = request.app.state.startup_challenge

    # Closed, exhausted, expired, or never mounted for this mode: behave as
    # if the route doesn't exist, not "exists but rejects" -- this is the
    # "permanently close/disable the challenge route once authentication
    # becomes ready" requirement.
    if challenge_state is None or not challenge_state.is_open:
        return JSONResponse(_NOT_FOUND_BODY, status_code=404)

    body = await _read_bounded_body(request)
    if body is None:
        return JSONResponse(_REJECTED_BODY, status_code=400)

    try:
        payload = json.loads(body.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError):
        return JSONResponse(_REJECTED_BODY, status_code=400)
    if not isinstance(payload, dict):
        return JSONResponse(_REJECTED_BODY, status_code=400)

    nonce_b64 = payload.get("nonce")
    if not isinstance(nonce_b64, str):
        return JSONResponse(_REJECTED_BODY, status_code=400)

    try:
        response_b64 = challenge_state.compute_response(nonce_b64=nonce_b64)
    except ChallengeError:
        return JSONResponse(_REJECTED_BODY, status_code=400)

    return JSONResponse({"response": response_b64}, status_code=200)
