# EvidenceGraph — Architecture

Status: Sprint 0 draft. Critically evaluates the proposed stack rather than accepting it
uncritically; see `DECISIONS.md` for the ADRs behind each call made here.

## 1. Component Overview

```mermaid
flowchart TB
    subgraph Desktop["Tauri Desktop Shell (Rust host)"]
        FE["Frontend: React + TypeScript\n(Tailwind + shadcn/ui)"]
    end

    subgraph Local["Local Analysis Service (Python + FastAPI)\nbound to 127.0.0.1 only"]
        API["API layer"]
        Parse["Document Parsing / Chunking"]
        Classify["Artifact Classification"]
        Retrieve["Candidate Control Retrieval"]
        Pipeline["AI Pipeline Orchestration"]
        FW["Framework Engine"]
        DB[("SQLite via SQLAlchemy")]
    end

    subgraph Runtime["Local Model Runtime"]
        MP["ModelProvider abstraction"]
        LCPP["llama.cpp (GGUF)"]
    end

    FE <-->|"local HTTP/IPC, localhost only"| API
    API --> Parse --> Classify --> Retrieve --> Pipeline
    Pipeline --> MP --> LCPP
    Pipeline --> FW
    API --> DB
    FW --> DB
    Pipeline --> DB

    FS[("Local filesystem:\nevidence, models, DB file")]
    DB -.-> FS
    LCPP -.-> FS
```

## 2. Major Components

- **Desktop shell (Tauri, Rust host)**: process/window management, filesystem permission
  boundary, launches and supervises the local analysis service as a child process.
- **Frontend (React + TypeScript)**: all UI — assessment management, evidence upload, review,
  graph exploration, model/settings management. Talks to the local analysis service only.
- **Local analysis service (Python + FastAPI)**: owns the database, document parsing, framework
  engine, and AI pipeline orchestration. Bound to `127.0.0.1` (localhost) only — see
  `SECURITY.md`.
- **Model runtime (`ModelProvider` + llama.cpp)**: isolated behind an interface so the pipeline
  never depends on llama.cpp specifics directly. See `MODEL_RUNTIME.md`.
- **Framework engine**: loads framework definitions (Framework → Family → Control → Enhancement)
  from data files under `/frameworks`, not from application code.
- **SQLite database**: single local file, owned exclusively by the Python service (frontend never
  touches it directly).

## 3. Trust and Process Boundaries

```mermaid
flowchart LR
    subgraph Untrusted["Untrusted input"]
        U1["Uploaded evidence\n(PDF/DOCX/TXT/CSV)"]
        U2["Imported GGUF models"]
        U3["LLM output"]
    end
    subgraph TB1["Trust boundary: file ingestion"]
        Parser["Sandboxed parsers"]
    end
    subgraph TB2["Trust boundary: model output"]
        Validator["Deterministic schema validation"]
    end
    U1 --> Parser --> App["Application logic"]
    U2 -->|"checksum/signature check"| Runtime["Model runtime"]
    Runtime --> U3 --> Validator --> App
```

Two boundaries matter most:

1. **File ingestion boundary**: every uploaded file is untrusted bytes until parsed by a
   size-limited, type-verified, sandboxed-as-practical parser. Extracted text is still treated as
   *data*, never as instructions, downstream (see `SECURITY.md`).
2. **Model output boundary**: LLM output is untrusted/probabilistic. It must pass deterministic
   schema validation before being persisted as a Mapping Candidate. A model that returns malformed
   JSON, an unknown control ID, or an out-of-range confidence is rejected, not "best-effort
   parsed."

Process boundary: the frontend (Tauri/React) never directly parses evidence, never directly talks
to the model runtime, and never directly touches the database. Everything evidence-related goes
through the local analysis service, which is the only component with filesystem/database/model
access beyond what the OS file picker exposes to the frontend for selecting files to upload.

## 4. Desktop / Frontend / Backend Interaction

- Tauri launches the FastAPI service as a local child process on app start and terminates it on
  app exit.
- Frontend communicates with the service over HTTP restricted to `127.0.0.1` on a locally
  allocated port (not a fixed well-known port, to reduce collision/hijack risk — open question,
  see `DECISIONS.md`).
- No remote network calls originate from the local analysis service in V0.1 except: (a) model
  catalog fetch/download, which is an explicit, visible, user-initiated network action, never
  silent.

## 5. Database Ownership

SQLite file lives in the app's local data directory. Only the Python service opens it (via
SQLAlchemy). No other process, including the frontend, reads or writes it directly. This keeps a
single writer, avoids SQLite multi-process contention, and keeps the schema an internal
implementation detail the frontend never depends on directly.

## 6. Model Runtime

See `MODEL_RUNTIME.md` for detail. Architecturally: the AI pipeline depends only on a
`ModelProvider` interface (roughly: `generate(prompt, schema) -> structured_output`,
`health_check()`, `list_available()`), not on llama.cpp specifics. V0.1 ships exactly one
implementation, `LlamaCppProvider`, run as a local subprocess/server bound to localhost.

## 7. Document Pipeline

Artifact → secure file handling → text extraction → semantic sectioning/chunking → artifact
classification → candidate control retrieval → LLM candidate evaluation → structured mapping
output → deterministic validation → confidence calculation → analyst review. Detailed in
`AI_PIPELINE.md`.

## 8. Framework Engine

Framework content (families, controls, enhancements, official text) is **data**, loaded from
`/frameworks/<framework-id>/`, not hardcoded into application logic. The application logic that
walks Framework → Family → Control → Enhancement, resolves candidate controls, and renders
control detail must work for any framework conforming to the (yet-to-be-finalized-in-detail)
framework data format — it must not special-case NIST identifiers (e.g. must not assume all
control IDs match `^[A-Z]{2}-\d+$`, since future frameworks will not share that shape).

## 9. AI Pipeline (Summary)

See `AI_PIPELINE.md`. Architecturally significant point: retrieval happens *before* LLM
evaluation to bound the prompt to a small candidate set per artifact section, rather than a
single mega-prompt containing all controls and all evidence.

## 10. Graph Representation

For V0.1's scope (single assessment, hundreds not millions of nodes), the relationship graph is
represented relationally in SQLite (Mapping Candidate rows are the edges; Control/Artifact rows
are the nodes) and assembled into a graph structure at query time for the frontend to render.
**No dedicated graph database in V0.1** — see `DECISIONS.md` D-002. This may need revisiting if
graph exploration performance or query complexity outgrows relational queries, but that
justification does not exist yet.

## 11. Offline Behavior

After initial app install and model download, the application must function with no network
access: assessment CRUD, evidence upload/parsing, AI analysis, review, and graph exploration are
all local operations. The only features that inherently require network access are: model catalog
browsing and model download. The UI must make clear which actions require network access.

## 12. Future Extensibility

- Additional frameworks: add a new `/frameworks/<id>/` data set; no engine code changes required
  if the framework engine is built to the genericity rule in §8.
- Additional model providers: implement `ModelProvider`; no pipeline changes required.
- Multi-user/collaboration: would require moving off single-writer SQLite and adding an identity
  model — explicitly deferred, not designed against in V0.1's schema beyond not actively
  precluding it.

## 13. Stack Evaluation

The proposed stack (Tauri, React/TypeScript, Tailwind/shadcn, Python/FastAPI local service,
SQLite/SQLAlchemy, llama.cpp/GGUF) is accepted for V0.1 with the following notes:

- **Tauri over Electron**: appropriate — smaller footprint, Rust host reduces attack surface for
  the permission-sensitive parts (filesystem, process spawn) versus a full Node/Chromium host
  process. Reasonable fit for a security-focused desktop tool.
- **Python/FastAPI local service over doing everything in the Rust/Tauri host**: accepted because
  the document parsing, embeddings, and llama.cpp ecosystem tooling is materially more mature in
  Python. Tradeoff: introduces an IPC/process boundary (mitigated by binding strictly to
  localhost) and a second runtime to package/ship. This is the single biggest packaging risk for
  V0.1 (bundling a Python runtime inside a Tauri app) and should be validated early in Sprint 1
  rather than assumed to be smooth.
- **SQLite/SQLAlchemy**: appropriate for a local-first, single-writer desktop app. No objection.
- **llama.cpp/GGUF**: reasonable default for broad local hardware support (CPU-only through
  GPU-accelerated) without a heavier serving stack. Accepted as the sole V0.1 provider behind
  `ModelProvider`.
- **No graph database, no message queue, no Kubernetes, no microservices**: correct call for this
  scope; see `DECISIONS.md` D-002.

Open architectural question flagged, not resolved here: exact mechanism for Tauri↔FastAPI process
supervision and port allocation. See `docs/OPEN_QUESTIONS.md`.
