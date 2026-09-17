# EvidenceGraph Local Service

Standalone Python/FastAPI local analysis service skeleton. Bound to exactly `127.0.0.1` -- the
documented S1-03 bind contract (`docs/SPRINT_1_BACKLOG.md` S1-03, `docs/ARCHITECTURE.md` §4,
`docs/SECURITY.md` T-09) -- and no other address or loopback form (`::1`, other `127.x.x.x`
addresses, `localhost`, `0.0.0.0`, etc.); every request, including `GET /health`, requires the
installed session credential. See the root [`CONTRIBUTING.md`](../CONTRIBUTING.md) for toolchain
versions and [`docs/SPRINT_1_BACKLOG.md`](../docs/SPRINT_1_BACKLOG.md) S1-03 for this ticket's
scope, and [`docs/DECISIONS.md`](../docs/DECISIONS.md) D-009/D-018/D-025 for the full
authentication/startup architecture this is one piece of.

**Standalone only, for now.** This process is not yet launched or supervised by the Tauri desktop
shell (`app/`) — that is S1-04. Run it by hand for local development/testing, with the session
token supplied via an environment variable, exactly as documented below.

## Setup

Requires [`uv`](https://docs.astral.sh/uv/) and Python 3.11+ (3.12 recommended — see root
`CONTRIBUTING.md`). From this directory:

```sh
uv sync
```

This creates `.venv/` and installs exact versions pinned in `uv.lock`.

## Running

A session token is required before any request succeeds — including `/health` (see
`docs/DECISIONS.md` D-009: there is no unauthenticated health exemption).

**Accepted credential format:** an ASCII string of at least 16 characters, using only the
unpadded Base64URL alphabet (`A-Z`, `a-z`, `0-9`, `-`, `_`; RFC 4648 §5, no `=` padding). This is
exactly the alphabet `secrets.token_urlsafe()` produces, so every generated credential round-trips
cleanly through an `Authorization: Bearer <token>` header (RFC 9110 §11.3's `token68` grammar). A
value outside this format is treated the same as no credential configured — every request fails
closed with `401`, and the invalid value is never logged. Generate a temporary development-only
credential without committing or printing a production secret:

```sh
python -c "import secrets; print(secrets.token_urlsafe(32))"
```

```sh
EVIDENCEGRAPH_SESSION_TOKEN=$(python -c "import secrets; print(secrets.token_urlsafe(32))") uv run evidencegraph-service
```

The service prints a structured JSON startup log line to stderr and then listens on
`127.0.0.1:51823` by default. Configuration is entirely environment-variable driven (all
`EVIDENCEGRAPH_*`, see `src/evidencegraph_service/config.py`):

| Variable | Default | Notes |
|---|---|---|
| `EVIDENCEGRAPH_HOST` | `127.0.0.1` | Must be exactly `127.0.0.1` — the documented S1-03 bind contract; any other value (`::1`, another `127.x.x.x` address, `localhost`, `0.0.0.0`, a LAN/public address, ...) refuses to start, before any listener opens. |
| `EVIDENCEGRAPH_PORT` | `51823` | 1-65535. |
| `EVIDENCEGRAPH_ENVIRONMENT` | `production` | `development` additionally exposes `/docs`, `/redoc`, `/openapi.json` (still authenticated). |
| `EVIDENCEGRAPH_SESSION_TOKEN` | *(unset)* | Standalone/test credential only, in the format documented above. In supervised (Tauri-launched) mode, S1-04 installs this over a private channel instead — never via this variable. Missing, too short, or wrong-format means every request is rejected (fail closed), not that auth is skipped. |

Exercise it once running (`Bearer` is matched case-insensitively, per RFC 9110 §11.1, and every
`401` response includes a `WWW-Authenticate: Bearer` header):

```sh
curl -i http://127.0.0.1:51823/health   # -> 401, no token supplied
curl -H "Authorization: Bearer <your-generated-token>" http://127.0.0.1:51823/health
```

## Entry points

Both of the following start the identical service with identical configuration behavior — the
module entry point is a thin shim over the same `main()` the console script uses:

```sh
uv run evidencegraph-service
uv run python -m evidencegraph_service
```

## Tests, lint, and types

```sh
uv run pytest
uv run ruff check .
uv run ruff format --check .
uv run mypy
```
