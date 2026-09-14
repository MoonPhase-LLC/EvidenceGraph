# EvidenceGraph — Compliance Model

Status: Sprint 0 draft. This defines the conceptual data/relationship model for compliance
reasoning. It is framework-agnostic; NIST 800-53-specific content lives under `/frameworks`.
See `DATABASE.md` for the resulting proposed schema.

## 1. Design Goal

Support a compliance knowledge graph without hardcoding NIST-specific assumptions, while keeping
V0.1's actual relationship surface **minimal**. The entities and relationships below are the
target model; V0.1 implements a subset (marked **[V0.1]**) and the rest is deferred but not
precluded (marked **[Future]**).

## 2. Core Entities

### Framework **[V0.1]**
A compliance framework definition (e.g. "NIST SP 800-53 Rev. 5"). Has an id, name, version, and
source metadata. Framework *content* (families/controls/enhancements) is data, not code — see
`ARCHITECTURE.md` §"Framework Engine."

### Control Family **[V0.1]**
A grouping of controls within a framework (e.g. "AC — Access Control"). Belongs to one Framework.

### Control **[V0.1]**
A single control within a family (e.g. "AC-2 — Account Management"). Has official framework text
(verbatim, versioned, never edited by the app or the AI).

### Control Enhancement **[V0.1]**
A refinement of a control (e.g. "AC-2(1)"). Belongs to one Control. Modeled the same way as a
Control for relationship purposes (an enhancement can be independently mapped to evidence).

### Assessment **[V0.1]**
A single engagement: a named container binding one Framework to a set of uploaded evidence and
the resulting mappings/decisions. Assessments are isolated from one another (no cross-assessment
data sharing — see `SECURITY.md` "cross-assessment contamination").

### Artifact **[V0.1]**
An uploaded piece of evidence (a PDF, DOCX, TXT, or CSV file) within an assessment. Has: file
hash, original filename, detected/confirmed MIME type, ingestion timestamp, and an AI-assigned
(and optionally analyst-corrected) classification (e.g. "policy," "procedure," "technical
export," "log," "report," "spreadsheet," "unknown").

### Artifact Section **[V0.1]**
A semantically chunked portion of an artifact (e.g. a policy section, a CSV's logical grouping,
a page range) produced during parsing/chunking. Mappings reference specific sections, not whole
artifacts, wherever the parser can section the document — this is what makes provenance
meaningful (see `AI_PIPELINE.md` §"Chunking").

### Policy **[Future, schema allowed in V0.1]**
A named governance document distinct from a generic Artifact — conceptually an Artifact that has
been identified as a policy, with its own statements. V0.1 may treat "policy" purely as an
Artifact classification value rather than a first-class separate entity; see Decision D-004 in
`DECISIONS.md`.

### Policy Statement **[Future]**
An individual statement/requirement extracted from a Policy. Deferred past V0.1 — V0.1 maps
Artifact Sections to Controls directly rather than extracting discrete policy statements first.
This keeps the V0.1 pipeline to one level of extraction (document → sections) instead of two
(document → statements → controls).

### Mapping Candidate **[V0.1]**
An AI-generated, not-yet-reviewed suggestion of a relationship between an Artifact Section (or
Policy Statement, once that entity exists) and a Control or Control Enhancement. Carries
relationship type, confidence, reasoning summary, and full AI provenance (see §5).

### Approved Mapping **[V0.1, modeled as a Mapping Candidate + Analyst Decision, not a separate table]**
The product concept of "an accepted relationship" is realized as a Mapping Candidate whose most
recent Analyst Decision is `approved`. See Decision D-005 in `DECISIONS.md` for why this is not a
separate table.

### Analyst Decision **[V0.1]**
A human decision on a Mapping Candidate: `approved`, `rejected`, or `needs_review`, with a
timestamp and (V0.1, single-user) the local user identity. Multiple decisions can exist over time
for the same candidate (e.g. reopened after `needs_review`); the current status is the latest
decision. Decisions are never deleted, only superseded — preserves audit trail.

### Finding / Gap **[V0.1, computed — not a persisted AI output]**
A control (in an assessment's scope) with no `approved` mapping, or only low-confidence /
`needs_review` mappings. Computed from existing data at query time in V0.1 rather than stored as
its own AI-generated artifact — this avoids a second class of "AI assertion" needing its own
provenance and review lifecycle in V0.1. See `DECISIONS.md` D-006.

### Provenance **[V0.1, embedded in Mapping Candidate, not a separate table]**
Attached to every Mapping Candidate: source artifact id, source section id(s), model identifier
+ version, confidence score, reasoning summary, retrieval method that surfaced the candidate
control, and timestamp. See §5.

## 3. Relationship Types

The prompt material proposes a large relationship vocabulary (SUPPORTS, PARTIALLY_SUPPORTS,
CONFLICTS_WITH, IMPLEMENTS, REFERENCES, SUPERSEDES, STALE, INSUFFICIENT, REQUIRES_REVIEW).
Recommendation: **do not implement all of these in V0.1.** Several conflate a *relationship type*
with a *review status* or a *data-quality flag*, which is a category error worth avoiding in the
schema.

### V0.1 relationship types (Artifact Section ↔ Control/Enhancement)

- `SUPPORTS` — evidence indicates the control is implemented.
- `PARTIALLY_SUPPORTS` — evidence indicates partial/incomplete implementation.
- `REFERENCES` — evidence mentions the control/topic without clearly supporting or refuting it
  (useful as a low-confidence catch-all rather than forcing SUPPORTS on weak matches).
- `CONFLICTS_WITH` — evidence appears to contradict claimed implementation (e.g. a policy says
  MFA is required; an account export shows accounts without MFA). Kept in V0.1 because it is a
  distinct and valuable signal, not just low confidence.

### Explicitly deferred to Future, and why

- `IMPLEMENTS` — overlaps heavily with `SUPPORTS` for the Artifact↔Control case; more meaningful
  for a future Policy Statement ↔ Control layer ("this statement implements this control")
  once Policy Statement exists as an entity.
- `SUPERSEDES` — an Artifact↔Artifact relationship (versioning), not Artifact↔Control. Deferred
  until artifact versioning/history is designed.
- `STALE` — this is a *property of a mapping over time* (e.g. "evidence is >12 months old"), not
  a relationship type chosen by the AI at mapping time. Model as a computed attribute on a
  Mapping Candidate (e.g. `evidence_age_flag`) in a future sprint, not as an enum value here.
- `INSUFFICIENT` — this is confidence-below-threshold plus `needs_review`, not a new relationship
  type. Represent via low `confidence` + `needs_review` decision status instead of a fifth enum
  value.
- `REQUIRES_REVIEW` — this is an Analyst Decision status (`needs_review`), not a relationship
  type. Keeping it out of the relationship enum avoids two fields meaning overlapping things.

### Other relationship scopes — all Future/out of scope for V0.1

- Control ↔ Policy, Control ↔ Policy Statement, Control ↔ Procedure as distinct edges — deferred
  with the Policy Statement entity itself.
- Artifact ↔ Artifact (e.g. duplicate-of, supersedes, references) — deferred; V0.1 only tracks
  exact-hash duplicates (a flag, not a graph edge).
- Framework ↔ Framework crosswalks (e.g. NIST 800-53 control X ≈ ISO 27001 control Y) — explicitly
  future work once a second framework exists.

## 4. AI Suggestion vs. Human Determination

This separation is load-bearing for the product's compliance posture and must be visible in both
the data model and the UI:

| | AI Suggestion (Mapping Candidate) | Human Determination (Analyst Decision) |
|---|---|---|
| Produced by | Model Provider + pipeline | The local human analyst |
| Claim strength | "Evidence appears to support AC-2" | "This mapping is approved / rejected / needs review" |
| Mutable? | Immutable once generated (re-running analysis creates a new candidate, doesn't edit an old one) | Append-only history of decisions |
| Authoritative for compliance status? | **No, never** | **Yes — this is the only authoritative signal** |
| Can the app compute "AC-2 is compliant"? | No | No — no entity in this model represents "control is compliant"; only "control has an approved mapping." Compliance determination is an audit/human judgment outside this app's authority, per `PRODUCT.md` principle 3. |

A Mapping Candidate's `confidence` and `reasoning_summary` must never be rendered in the UI in a
way that implies a determination (e.g. never "AC-2: COMPLIANT" — always "AC-2: 1 approved mapping,
0.91 confidence, analyst-approved").

## 5. Provenance Fields (on every Mapping Candidate)

- `artifact_id`
- `artifact_section_id` (one or more)
- `control_id` (and `enhancement_id` if applicable)
- `model_provider` + `model_identifier` + `model_version`
- `confidence` (0.0–1.0)
- `relationship_type`
- `reasoning_summary` (short, human-readable, generated text — not official framework language)
- `retrieval_method` (how this control was selected as a candidate — see `AI_PIPELINE.md`)
- `created_at`
- current review status (derived from latest Analyst Decision, default `needs_review` until
  acted on)

## 6. Framework Genericity Rule

No table, field, or code path in this model may assume NIST-specific structure (e.g. hardcoded
family prefixes like "AC-", hardcoded enhancement numbering conventions). Framework, Family,
Control, and Enhancement are generic; NIST-specific *data* lives in `/frameworks/nist-800-53-rev5`
per `ARCHITECTURE.md`.

## 7. "Expected Evidence" Guidance

A future layer may associate a Control with example evidence types (e.g. AC-2 → "user account
inventory"). This is **product-generated guidance**, must be stored and rendered in a way that is
visibly distinct from official framework text, and must never be described to the user as part of
the NIST standard itself. Not implemented in V0.1's data model beyond reserving this distinction
conceptually; see `DECISIONS.md`.

See also: `DATABASE.md` for the schema this model implies, `AI_PIPELINE.md` for how Mapping
Candidates are produced, `ARCHITECTURE.md` for framework data separation.
