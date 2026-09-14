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
data sharing — see `SECURITY.md` "cross-assessment contamination", and §8 below for the specific
invariants that enforce it).

An Assessment also has a **scope**: which of the Framework's controls are actually applicable to
this engagement. V0.1 default scope is "every control in the loaded framework" (no baseline
filtering), but the entity explicitly supports a narrower scope so this isn't hardcoded as
all-or-nothing at the schema level. See "Assessment Scope" below and `OPEN_QUESTIONS.md` P-1/C-2
for whether V0.1's UI actually exposes scope narrowing (a product decision) versus just defaulting
to "all controls" while the representation exists.

### Assessment Scope **[V0.1]**
The set of Controls/Enhancements considered "applicable" (in scope for gap analysis and review) for
a given Assessment. Represented as either (a) an explicit `assessment_control_scope` join
(Assessment ↔ Control/Enhancement, "in scope") when a subset is selected, or (b) an implicit "all
controls in the assessment's framework" when no explicit scope rows exist — the latter is V0.1's
default and the only mode exposed unless/until `OPEN_QUESTIONS.md` P-1 is resolved to allow
baseline/subset selection at assessment creation. This exists specifically so gap analysis
(§"Finding / Gap" below) has a real answer to "a gap relative to *what* control set" instead of
silently assuming every framework control always applies.

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

**This limitation is intentional and should not be expanded speculatively.** V0.1 has no policy
versioning, no policy statement extraction, no policy lifecycle (draft/effective/superseded), no
policy-specific relationship types, and no policy-to-procedure linkage — "policy" is a label on an
Artifact, nothing more. Do not build any part of a larger policy-management subsystem (approval
workflows, policy authoring, version history, statement-level extraction) under this entity in
V0.1, even incrementally, without an explicit, separately-scoped product decision — this is a
deliberate scope boundary per `CLAUDE.md`, not an oversight to be filled in opportunistically.

### Policy Statement **[Future]**
An individual statement/requirement extracted from a Policy. Deferred past V0.1 — V0.1 maps
Artifact Sections to Controls directly rather than extracting discrete policy statements first.
This keeps the V0.1 pipeline to one level of extraction (document → sections) instead of two
(document → statements → controls).

### AnalysisRun **[V0.1]**
One row per analysis attempt (a run of the pipeline against an artifact, or a batch of artifacts
analyzed together). Exists so the system can distinguish states the original model could not
represent: an artifact that was **never analyzed**, one where analysis **succeeded but produced no
mappings** (a valid, non-error outcome — see `AI_PIPELINE.md` §11), one where analysis **failed**
(model unavailable, validation failures on every candidate, etc.), and one whose mappings have been
**superseded** by a later run (e.g. after a model upgrade or a framework update). Status:
`succeeded` / `succeeded_no_mappings` / `failed` / `superseded`. Also the anchor point for
immutable analysis-configuration identity (see §5) — this is what makes "which model/framework/
prompt version actually produced this mapping" answerable rather than duplicated loosely across
every Mapping Candidate row. See `DECISIONS.md` D-012/D-013.

### Mapping Candidate **[V0.1]**
A suggestion of a relationship between an Artifact Section (or Policy Statement, once that entity
exists) and a Control or Control Enhancement. Carries relationship type, confidence, reasoning
summary, and full provenance (see §5). Has a `source`: `ai` (produced by an `AnalysisRun`, the
default and most common case) or `analyst_manual` (created directly by the analyst when AI
retrieval/evaluation missed a relationship they can see — see `DECISIONS.md` D-015). A manual
mapping still goes through the same Analyst Decision lifecycle below; nothing is auto-approved
merely because a human authored it directly instead of confirming an AI suggestion.

### Approved Mapping **[V0.1, modeled as a Mapping Candidate + Analyst Decision, not a separate table]**
The product concept of "an accepted relationship" is realized as a Mapping Candidate whose most
recent Analyst Decision is `approved`. See Decision D-005 in `DECISIONS.md` for why this is not a
separate table.

### Analyst Decision **[V0.1]**
A human decision on a Mapping Candidate: `approved`, `rejected`, or `needs_review`, with a
timestamp and (V0.1, single-user) the local user identity. Multiple decisions can exist over time
for the same candidate (e.g. reopened after `needs_review`); the current status is the latest
decision. Decisions are **deterministically append-only**: a decision row is never updated or
deleted, only superseded by a newer row with a later `decided_at` for the same
`mapping_candidate_id`; "current status" is always a derived read (latest row per candidate), never
a mutated field. This is the audit-trail guarantee `PRODUCT.md` §4/§8 and `COMPLIANCE_MODEL.md` §4
depend on.

An Analyst Decision of `approved` records agreement with the mapping's `relationship_type` — it is
**not** the same thing as a judgment that the evidence is *sufficient* for the control (see
"Evidence Sufficiency" below); those are kept as separate concepts even though V0.1's UI may
surface them together.

### Evidence Sufficiency **[V0.1, distinct concept — not a new persisted field beyond what's needed for the gap view]**
Whether a control's *approved coverage*, taken as a whole, is enough to consider the control
addressed. This is explicitly **not** the same thing as any single Mapping Candidate's
`relationship_type` or `confidence`, and not the same thing as an Analyst Decision on one candidate:
a control could have one `approved` `PARTIALLY_SUPPORTS` mapping (an analyst-confirmed relationship
that is still, by its own type, incomplete) — the relationship type and confidence describe *that
one candidate*; sufficiency is a judgment about the control's *overall* coverage across all of its
approved mappings. V0.1 computes sufficiency the same way it computes gaps (§"Finding / Gap"
below): a control is "sufficiently covered" if it has at least one `approved` `SUPPORTS` mapping,
"partially covered" if its approved mappings are `PARTIALLY_SUPPORTS`-only, and "not covered"
otherwise. This is a computed view, not an analyst-editable field, in V0.1 — see
`OPEN_QUESTIONS.md` if a future sprint needs an explicit analyst override of the computed
sufficiency judgment.

### Finding / Gap **[V0.1, computed — not a persisted AI output; renamed for clarity below]**
A control (in an assessment's scope, see "Assessment Scope" above) with no `approved` `SUPPORTS`/
`PARTIALLY_SUPPORTS` mapping. Computed from existing data at query time in V0.1 rather than stored
as its own AI-generated artifact — this avoids a second class of "AI assertion" needing its own
provenance and review lifecycle in V0.1. See `DECISIONS.md` D-006.

**Coverage semantics (important, and previously under-specified — `DECISIONS.md` D-014):** only
`approved` mappings with `relationship_type` `SUPPORTS` or `PARTIALLY_SUPPORTS` count toward
coverage. An `approved` `CONFLICTS_WITH` mapping never satisfies coverage — it is a confirmed
conflict, which is a *different* signal surfaced alongside the gap view, not a substitute for it. An
`approved` `REFERENCES` mapping also never satisfies coverage — the analyst confirmed the evidence
mentions the topic, not that it demonstrates implementation. A control with only approved
`CONFLICTS_WITH`/`REFERENCES` mappings (and no approved `SUPPORTS`/`PARTIALLY_SUPPORTS`) is still a
gap.

**Derived view vs. future "Finding":** what this document calls "Finding / Gap" in V0.1 is strictly
an ephemeral, computed query result — it has no id, no lifecycle, and no independent existence
beyond the query that produced it. A future sprint may introduce a first-class, persisted
**Finding** entity (e.g. one the analyst explicitly opens, assigns, tracks status on, or attaches
remediation notes to, independent of the underlying mappings changing) — that would be a genuinely
new, larger-lifecycle entity, not just a renamed gap view, and is explicitly **not** built in V0.1.
Do not conflate the two: "gap view" (V0.1, computed, no persistence) and "Finding" (future,
persisted, has its own review/remediation lifecycle) are different concepts that happen to share a
name in casual usage.

### Provenance **[V0.1, split across Mapping Candidate and AnalysisRun, neither a separate "provenance" table]**
Attached to every Mapping Candidate: source artifact id, source section id(s), `source` (ai/manual),
confidence score, reasoning summary, retrieval method, and timestamp. For AI-sourced candidates,
the model/framework/parser/prompt configuration identity lives on the referenced `AnalysisRun`
rather than being duplicated per candidate. See §5.

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
- `CONFLICTS_WITH` — the evidence in **a single artifact section** appears to contradict what the
  candidate control requires (e.g. a section stating "MFA is optional" evaluated against a control
  requiring MFA, using the model's own understanding of the control's requirement). Kept in V0.1
  because it is a distinct and valuable signal, not just low confidence. **Not** a comparison
  between two different artifacts (e.g. a policy document vs. a separate technical export) — that
  is cross-artifact contradiction analysis, which V0.1's per-artifact-section pipeline design does
  not support and does not attempt (see `DECISIONS.md` D-014, `AI_PIPELINE.md` §12,
  `OPEN_QUESTIONS.md`). An earlier draft's example conflated the two; corrected here.

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

## 5. Provenance Fields

On every Mapping Candidate:

- `artifact_id`
- `artifact_section_id` (one or more)
- `control_id` (and `enhancement_id` if applicable)
- `source` (`ai` or `analyst_manual` — see "Mapping Candidate" above, `DECISIONS.md` D-015)
- `analysis_run_id` (required when `source = ai`; null for `analyst_manual` — see below)
- `confidence` (0.0–1.0; only meaningful for `source = ai`)
- `relationship_type`
- `reasoning_summary` (short, human-readable text — AI-generated for `source = ai`, analyst-written
  for `source = analyst_manual`; never official framework language)
- `retrieval_method` (how this control was selected as a candidate — see `AI_PIPELINE.md`; null for
  `analyst_manual`)
- `created_at`
- current review status (derived from latest Analyst Decision, default `needs_review` until
  acted on)

On every `AnalysisRun` (immutable analysis-configuration identity, `DECISIONS.md` D-012/D-013 —
this replaces duplicating `model_provider`/`model_identifier`/`model_version` on every individual
Mapping Candidate row):

- `model_provider` + `model_identifier` + `model_version` + `model_digest` (checksum of the loaded
  weights, from the model catalog manifest's `sha256` where available — `MODEL_RUNTIME.md` §6)
- `framework_id` + `framework_version` used for control resolution during this run
- `parser_version` and `chunker_version` that produced the sections evaluated
- `prompt_template_version`
- relevant non-default inference settings actually used
- `status` (`succeeded` / `succeeded_no_mappings` / `failed` / `superseded`)
- `started_at` / `completed_at`

This is what makes "why did this mapping look different after we upgraded the model" answerable,
and what lets the system distinguish "never analyzed" from "analyzed, found nothing" from "analysis
failed" from "analyzed under a now-superseded configuration" — none of which the original
Mapping-Candidate-only provenance model could represent.

## 6. Framework Genericity Rule

No table, field, or code path in this model may assume NIST-specific structure (e.g. hardcoded
family prefixes like "AC-", hardcoded enhancement numbering conventions). Framework, Family,
Control, and Enhancement are generic; NIST-specific *data* lives in `/frameworks/nist-800-53-rev5`
per `ARCHITECTURE.md`. Control Family is **optional** (a Control may attach directly to its
Framework) rather than required, so a future framework that doesn't group controls into families
isn't forced into NIST's shape — see `DECISIONS.md` D-016. This is a proportional, partial fix;
full arbitrary-depth hierarchy generalization remains an open question (`OPEN_QUESTIONS.md` A-5)
deferred until a second framework's real data justifies it.

## 7. "Expected Evidence" Guidance

A future layer may associate a Control with example evidence types (e.g. AC-2 → "user account
inventory"). This is **product-generated guidance**, must be stored and rendered in a way that is
visibly distinct from official framework text, and must never be described to the user as part of
the NIST standard itself. Not implemented in V0.1's data model beyond reserving this distinction
conceptually; see `DECISIONS.md`.

## 8. Cross-Assessment and Cross-Framework Isolation Invariants

These must hold for every Assessment, enforced by schema constraints where SQLite allows it and by
application-layer checks otherwise (see `DATABASE.md` for the concrete FK/constraint mechanics):

1. Every `Artifact`, `Artifact Section`, `Mapping Candidate`, and `Analyst Decision` is reachable
   from exactly one `Assessment` via its foreign-key chain. No table represents evidence or
   decisions as globally shared across assessments (`SECURITY.md` T-16).
2. A `Mapping Candidate`'s `control_id` (and `enhancement_id`) must belong to the same
   `Framework` as its `Assessment`'s `framework_id`. An assessment scoped to NIST 800-53 Rev. 5 must
   never be able to produce or store a mapping candidate pointing at a control from a different
   framework, even after a second framework exists. This is not automatically implied by the
   individual foreign keys alone (`artifact → assessment` and `mapping_candidate → control` are
   separate chains) and must be an explicit check — either a composite constraint or an
   application-layer validation enforced at write time, not just at read time.
3. Duplicate detection (`artifact.sha256_hash`) is scoped per-assessment, never global — see
   `DATABASE.md` §5.
4. These invariants exist specifically so that adding a second framework or running concurrent
   assessments never risks one client's evidence, mappings, or framework context leaking into
   another's — the single most consequential failure mode for a multi-client compliance tool, even
   though V0.1 is architecturally single-assessment-at-a-time in its UI.

See also: `DATABASE.md` for the schema this model implies, `AI_PIPELINE.md` for how Mapping
Candidates are produced, `ARCHITECTURE.md` for framework data separation.
