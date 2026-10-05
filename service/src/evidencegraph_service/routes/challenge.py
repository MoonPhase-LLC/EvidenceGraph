"""POST /__startup/challenge -- D-025's bounded, single-use, pre-
authentication identity challenge (S1-04).

See `challenge.py` for the HMAC construction, the atomic single-use/
lifetime/attempt bounds enforced by `StartupChallengeState`, and why
*every* rejected request here -- malformed JSON, duplicate/unknown keys,
wrong types, wrong-length encoding, not just a well-formed-but-wrong
nonce -- consumes one attempt via `reject_malformed`/`handle_request`
before this route returns. This module is only the thin HTTP transport
around that: strict, bounded request parsing and a uniform rejection
response, mirroring `auth.py`'s own philosophy (fixed response bodies, no
oracle, no per-request content in logs).

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

#: The one fixed, bounded reason string `StartupChallengeState` uses for
#: "already closed" -- checked by identity/equality below to decide
#: 404-vs-400, never by inspecting arbitrary exception text.
_CLOSED_REASON = "challenge_not_open"


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


def _reject_duplicate_keys(pairs: list[tuple[str, object]]) -> dict[str, object]:
    """`json.loads(..., object_pairs_hook=...)` sees every key exactly as
    it appeared in the input, before the standard dict-building step would
    silently collapse duplicates (last-value-wins) -- this is what makes
    detecting a duplicate key possible at all."""
    seen: dict[str, object] = {}
    for key, value in pairs:
        if key in seen:
            raise ValueError("duplicate_key")
        seen[key] = value
    return seen


def _extract_nonce_or_reject(body: bytes | None, challenge_state: StartupChallengeState) -> str:
    """Returns the request's `nonce` field on a strictly well-formed body:
    valid UTF-8 JSON, no duplicate keys, an object with *exactly* the key
    `nonce` (no more, no fewer) and nothing else, string-typed. Any
    structural problem -- including the oversized-body case signaled by
    `body is None` -- calls `challenge_state.reject_malformed()` (counting
    one attempt) and raises `ChallengeError`, so every malformed request
    is accounted for the same way a well-formed-but-wrong nonce is.
    """
    if body is None:
        challenge_state.reject_malformed()
        raise ChallengeError("body_too_large")

    try:
        payload = json.loads(body.decode("utf-8"), object_pairs_hook=_reject_duplicate_keys)
    except (UnicodeDecodeError, ValueError) as exc:
        challenge_state.reject_malformed()
        raise ChallengeError("malformed_request_body") from exc

    if not isinstance(payload, dict) or set(payload.keys()) != {"nonce"}:
        challenge_state.reject_malformed()
        raise ChallengeError("unexpected_schema")

    nonce_b64 = payload["nonce"]
    if not isinstance(nonce_b64, str):
        challenge_state.reject_malformed()
        raise ChallengeError("unexpected_schema")

    return nonce_b64


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

    try:
        nonce_b64 = _extract_nonce_or_reject(body, challenge_state)
        response_b64 = challenge_state.handle_request(nonce_b64=nonce_b64)
    except ChallengeError as exc:
        # A race against the open/lifetime/attempts check above (another
        # request closed it in between) surfaces the same fixed reason
        # `reject_malformed`/`handle_request` themselves raise for it --
        # checked by identity, never by inspecting free-form exception
        # text, and mapped back to the same "doesn't exist" response as
        # the up-front check.
        if str(exc) == _CLOSED_REASON:
            return JSONResponse(_NOT_FOUND_BODY, status_code=404)
        return JSONResponse(_REJECTED_BODY, status_code=400)

    return JSONResponse({"response": response_b64}, status_code=200)
