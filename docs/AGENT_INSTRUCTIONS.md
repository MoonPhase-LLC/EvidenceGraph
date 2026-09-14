# EvidenceGraph — Agent Instructions

This file governs all coding agents (Claude Code, OpenAI Codex, or any other) working in this
repository. It is binding for all future implementation work, not just Sprint 0. See also the
root `CLAUDE.md`, which these rules are consistent with and expand on.

## 1. Read Before You Write

1. Read `docs/PRODUCT.md` before implementing any feature.
2. Read `docs/ARCHITECTURE.md` before making any architectural change.
3. Read the relevant subsystem doc before modifying that subsystem:
   - Compliance/data model changes → `docs/COMPLIANCE_MODEL.md`, `docs/DATABASE.md`
   - AI pipeline changes → `docs/AI_PIPELINE.md`
   - Model runtime changes → `docs/MODEL_RUNTIME.md`
   - Anything touching evidence handling or the local API → `docs/SECURITY.md`
   - Framework data → `docs/COMPLIANCE_MODEL.md` §6, `docs/ARCHITECTURE.md` §8
4. Check `docs/DECISIONS.md` for prior architectural decisions before re-deciding something
   already decided. If you disagree with a past decision, raise it explicitly rather than
   silently doing something else.
5. Check `docs/OPEN_QUESTIONS.md` before inventing behavior for anything listed there — ask the
   human product owner instead.

## 2. Dependencies and Frameworks

6. Do not introduce a new framework or library without justification. Before adding a dependency:
   confirm it's actually necessary, prefer established/well-maintained libraries, avoid large
   dependencies for small tasks, and record why it was added (commit message or PR description
   is sufficient — no separate dependency log required).
7. Do not perform major dependency upgrades or replace major frameworks without explicit
   product-owner approval.

## 3. Data and Schema

8. Do not change database schemas without migrations once implementation begins (no ad hoc schema
   edits against a running local database — see `DATABASE.md` §6).
9. Do not modify compliance framework data (`/frameworks/**`) unless explicitly tasked to do so.
10. Never fabricate or invent official NIST (or any framework's) control language. If official
    framework data isn't present yet, do not synthesize plausible-looking substitutes — leave the
    task for a dedicated, explicitly-scoped framework-ingestion ticket.

## 4. Privacy and Network Behavior

11. Never send evidence — uploaded files, extracted text, embeddings, findings, assessment data,
    or any prompt containing them — to an external/cloud service. See `CLAUDE.md` "Privacy Rules"
    and `docs/SECURITY.md`.
12. Never silently fall back to cloud AI if local inference fails or is unavailable. Surface the
    failure; do not route around it via a remote provider.
13. Never bind a local service (the FastAPI service, the model runtime) to anything other than
    `127.0.0.1` unless a task explicitly and specifically requires otherwise — and if it ever
    does, that's an architecture-level decision requiring product-owner approval first, not a
    routine implementation choice. Localhost binding is a network-reachability control, not
    authentication (`docs/SECURITY.md` T-09) — the FastAPI service must also enforce the per-launch
    shared-secret token from `docs/DECISIONS.md` D-009 on every request; do not remove or bypass
    that check under the assumption that localhost binding alone is sufficient.
14. Never place evidence content, extracted text, or prompts containing evidence into logs. Logs
    may contain structural metadata (file hash, size, status, error class) only. See
    `docs/SECURITY.md` T-10.

## 5. Treat as Untrusted

15. Treat uploaded documents (PDF/DOCX/TXT/CSV) as untrusted input at every layer — see
    `docs/SECURITY.md` §5 for the specific threats this implies (decompression bombs, path
    traversal, parser exploits).
16. Treat imported model files as untrusted; do not assume an imported GGUF file's provenance
    without the verification steps in `docs/MODEL_RUNTIME.md` §7.
17. Treat LLM output as untrusted and probabilistic. Always run it through the deterministic
    validation described in `docs/AI_PIPELINE.md` §8 before persisting it as data the rest of the
    app relies on.

## 6. AI Authority Boundaries

18. Preserve provenance for every AI-generated mapping: artifact, section(s), control, model,
    confidence, reasoning summary, and review status, per `docs/COMPLIANCE_MODEL.md` §5.
19. Never implement, suggest, or expose a code path that lets the AI (or the app on the AI's
    behalf) independently mark something as compliant, approved, or resolved. Only an explicit
    Analyst Decision (a human action) can do that. See `docs/COMPLIANCE_MODEL.md` §4.
20. Never render AI output in a way that implies a compliance determination rather than a
    suggestion (e.g. never "Compliant" — always something like "AI-suggested mapping, pending
    review").

## 7. Testing

21. Add tests for new backend behavior where practical.
22. Do not delete or weaken existing tests just to make CI pass. If a test fails: determine
    whether the implementation or the test is wrong, fix the actual issue, and explain any
    resulting behavior change in the PR description.

## 8. Scope Discipline

23. Stay inside the assigned ticket/task. Do not perform unrelated refactors alongside a feature
    change.
24. Do not redesign architecture without explicit approval. If you believe an architectural
    change is necessary, explain the issue (in the PR description or by raising it directly)
    before implementing it — do not silently implement a redesign.
25. Keep V0.1 scope boundaries intact — do not build toward `docs/PRODUCT.md` §6 non-goals (extra
    frameworks, cloud inference, multi-tenant/collaboration features, etc.) without an explicit
    task to do so.
26. Prefer incremental changes over large rewrites.

## 9. Git Workflow

27. Follow the branch/PR workflow in the root `CLAUDE.md`: feature branch named
    `claude/<short-description>`, minimal-scope changes, tests where appropriate, commit, push,
    open a PR targeting `main`. Never push directly to `main`, never force-push, never merge your
    own PR unless explicitly instructed.

## 10. Multi-Agent Coordination

28. This repository is developed with both Claude Code and OpenAI Codex (see `docs/SPRINTS.md`
    §"Agent Roles" and the Sprint 0 brief §16). Avoid two agents independently modifying the same
    subsystem at the same time; one agent implementing a feature and the other reviewing it is the
    expected pattern, not both implementing in parallel.
29. When a ticket's "implementation owner recommendation" (see `docs/SPRINT_1_BACKLOG.md`) names
    an agent, treat that as a strong default, not a hard rule — the human product owner can
    reassign.

## 11. When Uncertain

30. When a decision requires product, compliance, UX, or security judgment beyond what's
    documented, add it to `docs/OPEN_QUESTIONS.md` (or check whether it's already there) rather
    than inventing an answer. Ask the human product owner directly for anything blocking.
