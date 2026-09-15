# Contributing to EvidenceGraph

This document covers repository conventions and the toolchain versions the upcoming
Tauri/Rust, React/TypeScript, and Python projects will target. See
[`docs/AGENT_INSTRUCTIONS.md`](docs/AGENT_INSTRUCTIONS.md) for the rules that govern any
coding agent working in this repository, and the root [`CLAUDE.md`](CLAUDE.md) for the git
workflow.

## Repository conventions

- **`.editorconfig`** defines whitespace, indentation, encoding, and line-ending rules per
  language. Most editors pick this up automatically (no plugin needed in VS Code, JetBrains
  IDEs support it natively). It does not reformat existing files by itself — it only guides
  new edits.
- **`.gitignore`** covers Node/frontend, Rust/Tauri, and Python build artifacts, plus common
  editor/OS clutter and local-secret filename patterns. Lockfiles (`Cargo.lock`,
  `package-lock.json`, etc.) are intentionally **not** ignored — see "Dependency lockfiles"
  below.
- Language-specific lint/format tooling (ESLint, ruff, mypy, rustfmt config, etc.) is
  introduced by the tickets that create each project, not here (see `docs/SPRINT_1_BACKLOG.md`
  S1-01's non-goals).

### Dependency lockfiles

When each project is initialized, commit its lockfile so builds are reproducible:

- `src-tauri/Cargo.lock` (Rust/Tauri — this is an application, not a library, so the lockfile
  should be committed per Cargo's own guidance).
- `package-lock.json` (or the lockfile for whichever Node package manager is chosen).
- A Python lockfile appropriate to whatever dependency manager is chosen for the FastAPI
  service (e.g. `uv.lock` or `poetry.lock`).

## Minimum and recommended tool versions

"Minimum supported" is the oldest version the project is expected to work with. "Recommended"
is what contributors should actually install for day-to-day development. These are documented
now, before the Tauri/React/FastAPI projects exist, so every later ticket starts from the same
baseline instead of each picking its own.

| Tool | Minimum supported | Recommended for development |
|---|---|---|
| Node.js | 22.x (Maintenance LTS) | 24.x (Active LTS) |
| Rust | 1.78 (stable) | Latest stable via `rustup update` |
| Python | 3.11 | 3.12 |

Rationale, checked against official sources as of 2026-09-15:

- **Node.js**: Node 22.x entered Maintenance LTS and remains supported (security fixes) until
  2027-04-30; Node 24.x is the current Active LTS line (supported until 2028-04-30), which is
  what the Tauri CLI and Vite tooling will be developed and tested against. Earlier lines
  (Node 20 and below) are already end-of-life. Source: the
  [Node.js Release schedule](https://github.com/nodejs/Release#readme).
- **Rust**: Tauri v2 was released with a minimum supported Rust version (MSRV) of 1.78; that is
  the floor for anything built against the Tauri v2 crates. In practice, contributors should
  install the latest stable Rust via `rustup` — Tauri's own Windows tooling has occasionally
  needed a newer toolchain than its published MSRV for specific build scenarios (see
  [tauri-apps/tauri#14433](https://github.com/tauri-apps/tauri/issues/14433)), and keeping
  `rustup` current avoids chasing that gap manually. Sources:
  [Tauri v2 Prerequisites](https://v2.tauri.app/start/prerequisites/) and
  [tauri-apps/tauri#11205](https://github.com/tauri-apps/tauri/pull/11205) (MSRV history).
- **Python**: Python 3.10 is within a few weeks of losing even security support (its
  end-of-life is 2026-10-31 per the [Python developer's guide release
  cycle](https://devguide.python.org/versions/)), so it is not a safe floor for a project
  starting now. Python 3.11 has security support until 2027-10-31, giving headroom for
  Sprint 1 through Sprint 16. Python 3.12 is recommended for development because it has
  support until 2028-10-31 and is the most broadly compatible version across the FastAPI /
  SQLAlchemy / llama.cpp Python-binding ecosystem at time of writing; Python 3.13/3.14 are
  newer but not yet the safest default for a project pulling in native-extension dependencies
  (llama.cpp bindings) that tend to lag new interpreter releases.

If these facts change (a version reaches end-of-life, or a dependency requires a newer
floor), update this table rather than letting each project silently diverge.

## Windows development prerequisites (Tauri stack)

These apply to contributors building the Tauri desktop shell on Windows, once
`S1-02` initializes it. **Not yet exercised in this repository** — no Tauri project exists at
this ticket's stage — this is documentation only, sourced from the
[official Tauri v2 Windows prerequisites](https://v2.tauri.app/start/prerequisites/) and the
[official Rust Windows install guidance](https://www.rust-lang.org/tools/install), checked
2026-09-15.

### Developer machine requirements (building/running the app from source)

- **Microsoft C++ Build Tools**: install the "Desktop development with C++" workload (via the
  Visual Studio Installer or the standalone Build Tools installer). Rust's MSVC toolchain
  depends on this.
- **Rust with the MSVC toolchain**: install via [rustup](https://rustup.rs/) and ensure the
  default host triple is an MSVC target (e.g. `x86_64-pc-windows-msvc`), not the GNU target.
- **Node.js**: an LTS release (see table above) — only needed because the frontend is
  React/TypeScript; Tauri itself does not require Node.
- **WebView2 Runtime**: needed to build/run the app locally if it isn't already present. See
  end-user note below — most current Windows installs already have it.

### End-user requirements (once the app is packaged and distributed — not relevant yet)

- **WebView2 Runtime**: Tauri renders the UI using Microsoft's WebView2. It ships
  preinstalled on Windows 10 (from version 1803 onward) and Windows 11. Only end users on
  older/unusual Windows configurations would need the separate "Evergreen Bootstrapper"
  installer, which the Tauri bundler can optionally include.
- End users do **not** need Rust, Node.js, or C++ Build Tools — those are build-time
  dependencies only, not runtime requirements for the packaged app.

## What you can actually run today

No application code exists yet (Sprint 0 was documentation/architecture only; this ticket,
S1-01, adds only repository tooling config). There is currently no `npm install`, `cargo
build`, `tauri dev`, or `pytest` to run — those commands become real starting with `S1-02`
(Tauri shell), `S1-03` (FastAPI service), and `S1-06` (test runners), per
[`docs/SPRINT_1_BACKLOG.md`](docs/SPRINT_1_BACKLOG.md). This document will be updated as those
projects are initialized.
