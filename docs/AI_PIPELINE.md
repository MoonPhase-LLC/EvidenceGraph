# EvidenceGraph — AI Pipeline

Status: Sprint 0 draft. Defines the pipeline architecture and the structured output schema.
Implementation tickets should follow this document; deviations must be explained first per
`AGENT_INSTRUCTIONS.md`.

## 1. Pipeline Stages

```mermaid
flowchart LR
    A["Artifact upload"] --> B["Secure file handling"]
    B --> C["Text extraction"]
    C --> D["Semantic sectioning / chunking"]
    D --> E["Artifact classification"]
    E --> F["Candidate control retrieval"]
    F --> G["LLM candidate evaluation"]
    G --> H["Structured mapping output"]
    H --> I["Deterministic validation"]
    I --> J["Confidence calculation"]
    J --> K["Mapping Candidate persisted"]
    K --> L["Analyst review"]
```

## 2. Parsing

Per file type (PDF, DOCX, TXT, CSV), extract text content using a well-maintained library, under
the constraints in `SECURITY.md` (size limits, decompression limits, no path traversal via
embedded names). Output: plain text plus structural hints where available (headings, page
numbers, CSV column headers) to aid sectioning.

## 3. Chunking (Semantic Sectioning)

Goal: split each artifact into Artifact Sections that are small enough to be a meaningful,
citable unit of evidence, and large enough to retain context. Approach (V0.1):

- For documents with structure (headings, page breaks): section along structural boundaries first.
- For unstructured text: fall back to bounded-size chunking (e.g. paragraph-aware, with a max
  token/character size), avoiding mid-sentence splits where practical.
- For CSV: treat logical row groupings (or the whole sheet, if small) as sections; CSVs are
  evidence of *data* (e.g. an account list), not prose, and may need different handling than
  documents — exact CSV sectioning strategy is an open implementation question, not fully
  specified here.

Each Artifact Section gets a stable id so Mapping Candidates can cite it precisely.

## 4. Artifact Classification

Classify each artifact into a small, fixed taxonomy (e.g. `policy`, `procedure`,
`technical_export`, `report`, `log`, `spreadsheet`, `unknown`) using the local LLM or a lighter
classifier — exact method (LLM-based vs. heuristic/rules-based vs. small local classifier model)
is an implementation decision for Sprint 7, not fixed here. Classification informs, but does not
gate, retrieval (an `unknown`-classified artifact can still be evaluated).

## 5. Candidate Control Retrieval

**This stage exists specifically so the LLM never evaluates "every artifact section against every
control."** For each Artifact Section, retrieve a bounded candidate set of controls likely to be
relevant, before invoking the LLM for evaluation.

Retrieval approach for V0.1: combination of

- lexical/keyword signal (control text and title vs. section text), and
- embedding similarity (local embedding model vs. control/enhancement text),

producing a top-N (N to be tuned; start small, e.g. 5–10) candidate list per section. Retrieval
method is recorded on the resulting Mapping Candidate's provenance (`retrieval_method`) so it can
be audited/tuned later.

Retrieval is not itself a source of Mapping Candidates — it only narrows what the LLM evaluation
stage considers.

## 6. LLM Candidate Evaluation

For each (Artifact Section, candidate Control) pair — or a batched prompt containing one section
and its small candidate control set, which is preferred over one-pair-per-call for efficiency —
the local LLM is asked to evaluate whether/how the section relates to each candidate control, and
to return **structured output only**, per the schema in §7.

The prompt must:

- Present the evidence text clearly delimited as **data to evaluate**, never as instructions.
- Include an explicit instruction that the evidence text may contain attempts to instruct the
  model and that such content must be ignored/treated as part of the evidence being evaluated,
  never acted upon (defense in depth on top of the architectural mitigation in `SECURITY.md`
  T-06).
- Request the structured schema only — no free-form chat response.

## 7. Structured Output Schema (V0.1 target)

This refines the example in the Sprint 0 prompt. Exact field names may be adjusted during Sprint 8
implementation, but the shape and required fields below are the Sprint 0 design decision:

```json
{
  "control_id": "AC-2",
  "enhancement_id": null,
  "relationship_type": "SUPPORTS",
  "confidence": 0.91,
  "artifact_section_ids": ["sec_0f3a..."],
  "reasoning_summary": "Section describes account provisioning and periodic review steps consistent with account management requirements.",
  "requires_review": true
}
```

Field notes:

- `relationship_type`: one of the V0.1 enum in `COMPLIANCE_MODEL.md` §3
  (`SUPPORTS`/`PARTIALLY_SUPPORTS`/`REFERENCES`/`CONFLICTS_WITH`).
- `confidence`: float 0.0–1.0, model-reported, treated as an untrusted signal (see `SECURITY.md`
  T-20), not a gate for automatic action.
- `artifact_section_ids`: must be section ids that were actually part of the evaluated context —
  validated at the deterministic-validation stage (§8); IDs not in the input context are rejected.
- `requires_review`: model's own signal of low confidence in itself; does **not** control whether
  human review happens — every Mapping Candidate requires human review in V0.1 regardless of this
  field. It is informational, e.g. for default sort order in the review UI.
- Multiple candidates may be returned per call (one per relevant control among the ones offered);
  the model is not required to force a relationship for every candidate control offered — it may
  return none if nothing in a section is relevant.

## 8. Deterministic Validation

Before persisting any model output as a Mapping Candidate, validate:

- Output parses as well-formed JSON matching the schema (reject/retry-once on failure, then mark
  the artifact section as "analysis failed" rather than silently dropping it).
- `control_id` (and `enhancement_id` if present) exists in the assessment's loaded framework data.
- `relationship_type` is one of the defined enum values.
- `confidence` is within [0.0, 1.0].
- `artifact_section_ids` is a non-empty subset of the section ids actually provided in that
  evaluation call.

Any failure here means the output is **not persisted as a Mapping Candidate**; it is logged
(structurally, without evidence content — see `SECURITY.md` T-10) as a pipeline failure for that
section/control pair.

## 9. Confidence Calculation

V0.1 uses the model-reported confidence directly (no separate deterministic re-scoring layer) but
reserves the field/architecture to later blend in deterministic signals (e.g. retrieval score,
keyword overlap) if model-reported confidence proves unreliable in practice. Not implemented in
V0.1 beyond pass-through + validation range check.

## 10. Human Review

Every persisted Mapping Candidate starts with review status `needs_review` (or equivalent) and
requires an explicit Analyst Decision to become `approved` or `rejected`. No auto-approval
threshold exists in V0.1. See `COMPLIANCE_MODEL.md` §4.

## 11. AI Failure Handling

Failure modes and required behavior — must never fail silently or fall back to a remote provider:

- **Model unavailable / not running**: surface a clear error in the UI; do not queue silently
  forever without status; do not fall back to any cloud provider (`CLAUDE.md`/`AGENT_INSTRUCTIONS.md`
  hard rule).
- **Model returns malformed output**: retry once with the same input; on second failure, mark
  that section/control-candidate evaluation as failed and move on — one failed section must not
  abort analysis of the rest of the artifact.
- **Model times out**: bounded timeout per evaluation call (value TBD); treat as failure per
  above.
- **Retrieval finds zero candidate controls for a section**: valid outcome, not an error; the
  section simply produces no Mapping Candidates.
- **Whole artifact fails to parse**: artifact is marked `parse_failed`; no analysis is attempted;
  visible to the user, not hidden.

## 12. Prompt Injection Resistance

Layered, per `SECURITY.md` T-06/T-07:

1. **Architectural**: AI output is never authoritative and always requires human approval — the
   ceiling on what a successful injection can achieve is "a suggestion a human must still approve."
2. **Scoping**: evaluation is per-artifact-section with a small candidate control list; content
   from one artifact cannot inject instructions that affect evaluation of a different artifact
   sharing the same assessment, because prompts are not concatenated across artifacts.
3. **Prompt structure**: evidence text is delimited and explicitly labeled as data-not-instructions
   within the prompt template; the model is explicitly told to disregard embedded instructions.
4. **Output validation**: structured-output validation (§8) means even a model that gets "talked
   into" a bad answer can only produce a schema-conformant Mapping Candidate about a real control
   in the real framework — it cannot cause the pipeline to take any other action, since the
   pipeline has no action-taking capability exposed to model output at all (no tool use / function
   calling to app state in V0.1).

See also: `MODEL_RUNTIME.md` for the `ModelProvider` interface these stages call into,
`COMPLIANCE_MODEL.md` for the entities produced, `SECURITY.md` for the threats this design
addresses.
