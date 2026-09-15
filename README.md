# EvidenceGraph

A privacy-first cybersecurity compliance evidence analysis application.

EvidenceGraph helps cybersecurity compliance analysts analyze large collections of client
evidence — policies, procedures, technical artifacts, configurations, reports, spreadsheets, logs
— and understand how that evidence relates to cybersecurity framework controls. The first
supported framework is **NIST SP 800-53 Revision 5**.

## Project Status

**Sprint 0 — Architecture and Product Definition.** No application code exists yet. This
repository currently contains product, architecture, security, and planning documentation only.
See `docs/SPRINTS.md` for the full sprint plan and `docs/OPEN_QUESTIONS.md` for decisions still
pending product-owner input.

## Privacy Philosophy

EvidenceGraph is **local-first** and **privacy-first**:

- Customer evidence is processed, embedded, retrieved against, and evaluated by a locally-run LLM
  — nothing evidence-bearing is sent to a cloud AI provider.
- There is no automatic cloud fallback if local inference is unavailable — a failure is surfaced,
  never silently routed around.
- The AI suggests candidate relationships between evidence and controls, with confidence scores
  and reasoning; it never independently determines compliance. Every suggestion requires human
  analyst review and carries full provenance (artifact, section, model, confidence, reasoning,
  review status).
- Uploaded documents, imported models, and LLM output are all treated as **untrusted** input at
  every layer.

See `docs/SECURITY.md` for the full threat model and `docs/PRODUCT.md` for the complete set of
product principles this is built on.

## Developer Documentation

Start here, in order:

1. [`docs/PRODUCT.md`](docs/PRODUCT.md) — problem, users, value proposition, V0.1 scope and
   non-goals.
2. [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) — components, trust boundaries, stack
   evaluation.
3. [`docs/SECURITY.md`](docs/SECURITY.md) — threat model, mitigations, unresolved risks.
4. [`docs/COMPLIANCE_MODEL.md`](docs/COMPLIANCE_MODEL.md) — the entity/relationship model behind
   the compliance knowledge graph.
5. [`docs/AI_PIPELINE.md`](docs/AI_PIPELINE.md) — parsing, retrieval, LLM evaluation, structured
   output, validation.
6. [`docs/MODEL_RUNTIME.md`](docs/MODEL_RUNTIME.md) — `ModelProvider` abstraction and local model
   lifecycle.
7. [`docs/DATABASE.md`](docs/DATABASE.md) — proposed schema and ER diagram.
8. [`docs/USER_FLOWS.md`](docs/USER_FLOWS.md) — intended user flows and open UX questions.
9. [`docs/SPRINTS.md`](docs/SPRINTS.md) — full sprint plan, Sprint 0–16.
10. [`docs/SPRINT_1_BACKLOG.md`](docs/SPRINT_1_BACKLOG.md) — detailed Sprint 1 tickets.
11. [`docs/DECISIONS.md`](docs/DECISIONS.md) — architecture decision log (ADRs).
12. [`docs/OPEN_QUESTIONS.md`](docs/OPEN_QUESTIONS.md) — questions requiring product-owner
    decision, categorized.
13. [`docs/AGENT_INSTRUCTIONS.md`](docs/AGENT_INSTRUCTIONS.md) — binding rules for any coding
    agent working in this repository.

Any coding agent (human or AI) should read `docs/AGENT_INSTRUCTIONS.md` and the relevant
subsystem doc before making changes. See also the root [`CLAUDE.md`](CLAUDE.md) for the git
workflow and forbidden actions.

## Contributing / Environment Setup

See [`CONTRIBUTING.md`](CONTRIBUTING.md) for repository tooling conventions, minimum/recommended
Node.js/Rust/Python versions, and Windows development prerequisites for the planned Tauri
stack.

## Technology (proposed, evaluated in `docs/ARCHITECTURE.md`)

Tauri (desktop shell) · React + TypeScript + Tailwind/shadcn-ui (frontend) · Python + FastAPI
(local analysis service) · SQLite + SQLAlchemy (storage) · llama.cpp + GGUF (local inference).

## License

TBD.
