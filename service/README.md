# EvidenceGraph Local Service

Standalone Python/FastAPI local analysis service skeleton. Bound to a literal loopback address
only (`127.0.0.1`/`::1`, never `0.0.0.0`); every request, including `GET /health`, requires the
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
`docs/DECISIONS.md` D-009: there is no unauthenticated health exemption). Pick any value at least
16 characters long for local/manual testing:

```sh
EVIDENCEGRAPH_SESSION_TOKEN=replace-with-any-string-16-chars-or-more uv run evidencegraph-service
```

The service prints a structured JSON startup log line to stderr and then listens on
`127.0.0.1:51823` by default. Configuration is entirely environment-variable driven (all
`EVIDENCEGRAPH_*`, see `src/evidencegraph_service/config.py`):

| Variable | Default | Notes |
|---|---|---|
| `EVIDENCEGRAPH_HOST` | `127.0.0.1` | Must be a literal loopback address; anything else refuses to start. |
| `EVIDENCEGRAPH_PORT` | `51823` | 1-65535. |
| `EVIDENCEGRAPH_ENVIRONMENT` | `production` | `development` additionally exposes `/docs`, `/redoc`, `/openapi.json` (still authenticated). |
| `EVIDENCEGRAPH_SESSION_TOKEN` | *(unset)* | Standalone/test credential only. In supervised (Tauri-launched) mode, S1-04 installs this over a private channel instead — never via this variable. Missing or under 16 characters means every request is rejected (fail closed), not that auth is skipped. |

Exercise it once running:

```sh
curl http://127.0.0.1:51823/health   # -> 401, no token supplied
curl -H "Authorization: Bearer replace-with-any-string-16-chars-or-more" http://127.0.0.1:51823/health
```

## Tests, lint, and types

```sh
uv run pytest
uv run ruff check .
uv run ruff format --check .
uv run mypy
```
