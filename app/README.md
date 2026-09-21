# EvidenceGraph Desktop Shell

Tauri v2 desktop application shell with a React + TypeScript frontend
(Tailwind CSS v4 + shadcn/ui configured). See the root
[`CONTRIBUTING.md`](../CONTRIBUTING.md) for exact setup/build commands,
toolchain versions, and Windows prerequisites.

## Local service supervision (S1-04)

On launch, this app spawns, authenticates, and supervises the standalone
Python service in [`../service`](../service) -- see
`app/src-tauri/src/supervisor/mod.rs` for the full startup/shutdown
contract and `docs/DECISIONS.md` D-001/D-009/D-018/D-025 for the
architecture it implements. In brief:

- The child is launched by an explicit absolute path only, never PATH or a
  shell. In a dev build (`npm run tauri dev` / `cargo run` with
  `debug_assertions`), that's `../../service/.venv/Scripts/python.exe -m
  evidencegraph_service --supervised` -- run `uv sync` in `service/` first
  (see `service/README.md`), or the app fails closed with
  `executable_not_found`. In a release build it's a bundled sidecar
  executable S1-09 has not shipped yet, so release builds intentionally
  fail closed today.
- A per-launch startup secret and session token are generated fresh every
  run and exchanged over private inherited stdio pipes, never an
  environment variable, CLI argument, or HTTP request.
- Before any credential is installed, Tauri verifies the child's identity
  via a domain-separated HMAC-SHA-256 challenge over the child's own
  bound HTTP endpoint (D-025) -- the sole pre-authentication route,
  single-use and closed immediately after.
- The frontend never receives the session token, host, or port. It polls
  the `get_service_status` command for a `Starting → Ready`/`Failed`
  snapshot and calls `check_service_health`, which performs the
  authenticated `GET /health` round trip entirely in Rust.

## Recommended IDE Setup

- [VS Code](https://code.visualstudio.com/) + [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)
