# EvidenceGraph — Architecture Decision Log

Lightweight ADR-style entries. Each decision: context, decision, consequences, status. Add new
entries at the bottom; do not renumber or delete past entries even if later superseded — mark them
Superseded and link forward instead.

---

## D-001: Accept the proposed stack (Tauri + React/TS + Python/FastAPI + SQLite + llama.cpp)

**Status:** Accepted

**Context:** Sprint 0 brief proposed a specific stack and asked for critical evaluation rather
than blind acceptance.

**Decision:** Accept the proposed stack as-is for V0.1, with the packaging risk of bundling a
Python runtime inside a Tauri app flagged as the top technical risk to validate early in Sprint 1
(a minimal "hello world" Tauri-spawns-FastAPI-and-talks-to-it slice should be one of the first
things built, before deeper feature work).

**Consequences:** Two-runtime desktop app (Rust/Tauri host + Python local service) adds packaging
complexity versus an all-JS or all-Rust app, but is justified by the maturity of Python's document
parsing / ML tooling ecosystem for this domain. See `ARCHITECTURE.md` §13.

---

## D-002: No dedicated graph database in V0.1

**Status:** Accepted

**Context:** The product is conceptually "a compliance knowledge graph," which could suggest a
graph database (Neo4j etc.).

**Decision:** Represent the graph relationally in SQLite; assemble graph structures at query time
for the frontend. Do not introduce a graph database, or any additional database engine, in V0.1.

**Consequences:** Simpler single-database architecture, consistent with `CLAUDE.md` engineering
rules (avoid unnecessary infrastructure). Revisit only if graph query complexity or scale
genuinely outgrows relational queries with evidence, not speculatively.

---

## D-003: Minimal V0.1 relationship-type enum

**Status:** Accepted

**Context:** The Sprint 0 brief listed nine candidate relationship types (SUPPORTS,
PARTIALLY_SUPPORTS, CONFLICTS_WITH, IMPLEMENTS, REFERENCES, SUPERSEDES, STALE, INSUFFICIENT,
REQUIRES_REVIEW) and asked for a critical, minimal recommendation rather than implementing all of
them.

**Decision:** V0.1 implements four: `SUPPORTS`, `PARTIALLY_SUPPORTS`, `REFERENCES`,
`CONFLICTS_WITH`. The other five are either review-status concepts (`REQUIRES_REVIEW`,
`INSUFFICIENT`), a time-based computed property (`STALE`), or belong to relationship scopes not
yet modeled (`IMPLEMENTS` for Policy Statement↔Control, `SUPERSEDES` for Artifact↔Artifact
versioning).

**Consequences:** Smaller enum reduces ambiguity for the LLM at evaluation time and avoids
conflating relationship type with review status or data-quality flags in the schema. See
`COMPLIANCE_MODEL.md` §3 for full reasoning.

---

## D-004: Policy is an Artifact classification value, not a separate entity, in V0.1

**Status:** Accepted

**Context:** The brief's relationship model lists Policy and Policy Statement as first-class
concepts with their own Control relationships.

**Decision:** Defer Policy and Policy Statement as separate entities. In V0.1, "policy" is simply
one value of `artifact.classification`; policies are mapped to controls the same way any other
artifact is — via Artifact Section → Control mappings.

**Consequences:** Keeps V0.1's extraction pipeline to one level (document → sections) instead of
two (document → statements → controls). A future sprint can add Policy Statement extraction as an
additional, optional refinement stage without breaking the existing Artifact Section model, since
policy statements would essentially be a finer-grained section type.

---

## D-005: "Approved Mapping" is a Mapping Candidate + latest Analyst Decision, not a separate table

**Status:** Accepted

**Context:** The brief's language ("mapping candidates," "approved mappings") could imply two
separate tables.

**Decision:** One `mapping_candidate` table plus an append-only `analyst_decision` table. A
mapping's current status is derived from its latest decision, not stored as a mutable status flag
overwritten in place, and not duplicated into a second "approved" table.

**Consequences:** Full decision history preserved (supports the audit-trail/provenance
requirement) without a data-duplication/sync problem between a "candidates" table and an
"approved" table. Query for "current status" requires a latest-row-per-group lookup — an accepted
cost given the auditability benefit. See `DATABASE.md` §2.

---

## D-006: Findings/Gaps are computed at query time, not persisted AI output

**Status:** Accepted

**Context:** The brief lists "findings/gaps" as a required concept alongside AI provenance.

**Decision:** Do not treat a gap as a stored, AI-authored assertion needing its own review
lifecycle. Compute "controls with no/weak evidence" from existing `control` + `mapping_candidate`
+ `analyst_decision` data at query time.

**Consequences:** Avoids a second parallel category of "thing the AI outputs that needs human
review" alongside Mapping Candidates, which would have doubled the provenance/review UX for
limited benefit in V0.1. If gap analysis logic becomes complex enough to warrant caching, that's a
performance optimization on top of this model, not a change to it.

---

## D-007: No authentication/authorization layer on the local API in V0.1

**Status:** Accepted, contingent

**Context:** The FastAPI local service exposes an HTTP API consumed by the frontend.

**Decision:** No auth token/session on the local API in V0.1. This is acceptable **only** because
the service is strictly bound to `127.0.0.1` (see `SECURITY.md` T-09) and the app is single-user.

**Consequences:** Simpler local IPC. This decision must be revisited immediately if the binding
assumption ever changes (e.g. any future requirement to expose the service beyond localhost) —
flagged explicitly in `AGENT_INSTRUCTIONS.md` rule 13.

---

## D-008: Single active model at a time in V0.1

**Status:** Accepted

**Context:** Model runtime could theoretically support running multiple models concurrently.

**Decision:** V0.1 runs exactly one model process at a time; switching models stops the current
one before starting the next.

**Consequences:** Simpler resource management (no concurrent GPU/RAM contention to reason about);
matches the actual V0.1 workflow (one active model per assessment session). Concurrent multi-model
support is not precluded architecturally (the `ModelProvider` abstraction doesn't assume
singleton), just not built in V0.1.

---

*(Further decisions should be appended below as they are made during Sprint 1+ implementation.)*
