# EvidenceGraph — Local Model Runtime

Status: Sprint 0 draft.

## 1. `ModelProvider` Abstraction

The AI pipeline (`AI_PIPELINE.md`) depends only on capability interfaces, never on a specific
runtime, and never on the assumption that one model instance must serve every capability
(`DECISIONS.md` D-011 — this revises the original single-interface sketch, which bundled
generation and embedding together as if one loaded model always does both):

```
GenerationCapable:
  generate(prompt: str, schema: JSONSchema, timeout_s: float) -> StructuredResult | ProviderError
  health_check() -> HealthStatus                        # up/down, model loaded, version info
  describe() -> ModelDescriptor                          # id, version, context window, capabilities

EmbeddingCapable:
  embed(text: str) -> list[float]                      # for retrieval, §AI_PIPELINE.md §5
  health_check() -> HealthStatus
  describe() -> ModelDescriptor
```

A concrete provider (e.g. `LlamaCppProvider`) may implement one or both interfaces. The pipeline
asks for "the active generation provider" and "the active embedding provider" independently — they
may be the same underlying process/model if a chosen GGUF model is adequate at both, or two
separate `LlamaCppProvider` instances loaded with different models, whichever gives acceptable
retrieval and generation quality (an empirical Sprint 5/6 decision, informed by an early
retrieval-quality check — see `SPRINTS.md` Sprint 8).

V0.1 ships exactly one provider implementation: `LlamaCppProvider`. The interface exists so that
`OllamaProvider`, `VLLMProvider`, or an `EnterprisePrivateProvider` could be added later without
touching pipeline code — not because V0.1 needs more than one.

`ModelProvider` implementations must never make external network calls as part of `generate`/`embed`.
Authenticated loopback IPC is permitted; external calls are confined to the explicit model
download/catalog flow (§3, §6).

## 2. `LlamaCppProvider`

Runs llama.cpp as a local subprocess (server mode), bound to `127.0.0.1` on a locally allocated
port, and communicates with it over that local HTTP interface, or via direct bindings — exact
mechanism (subprocess + HTTP vs. Python bindings) is an implementation decision for Sprint 5, not
fixed here; either is acceptable as long as it stays local-only and fits the `ModelProvider`
interface.

Structured output: rely on llama.cpp's grammar/JSON-schema-constrained sampling where available,
to increase the odds that `generate()` returns schema-conformant output. Even so, the pipeline's
deterministic validation (`AI_PIPELINE.md` §8) is mandatory — constrained sampling reduces but
does not eliminate the need to validate.

## 3. Model Catalog

A manifest describing available/recommended models, e.g. (size in bytes, four and a half billion):

```json
{
  "model_id": "example-7b-instruct-q4_k_m",
  "display_name": "Example 7B Instruct (Q4_K_M)",
  "format": "gguf",
  "size_bytes": 4500000000,
  "roles": ["generation"],
  "min_ram_gb": 8,
  "recommended_ram_gb": 16,
  "min_vram_gb": null,
  "recommended_vram_gb": 6,
  "context_window": 8192,
  "tier": "BALANCED",
  "download_url": "...",
  "sha256": "...",
  "source": "..."
}
```

`roles` (`DECISIONS.md` D-023) declares which capability/capabilities a catalog entry supports:
`["generation"]`, `["embedding"]`, or both. Capability-specific fields apply only to the relevant
role(s). Both roles require an explicit maximum input-token limit and tokenizer identity;
generation additionally budgets output tokens within its `context_window`. Embedding inputs must
be chunked within their model's limit, never silently truncated. Embedding entries also carry
`embedding_dimensions`, `embedding_normalization` (e.g. `"none"` or `"l2"`), and
`embedding_similarity_metric` (e.g. `"cosine"` or `"dot_product"`). Normalization transforms a
vector; a similarity metric compares vectors. They are separate configuration fields (D-025).

The catalog is data (a manifest file, versioned with the app or fetched from a pinned source —
exact hosting TBD), not hardcoded model-selection logic. This is what lets recommendations be
based on each model's actual stated requirements rather than an if/else chain like "12GB VRAM
always means model X" (explicitly called out as an anti-pattern in the Sprint 0 brief).

## 4. Hardware Detection

Best-effort detection of:

- CPU (cores, architecture)
- RAM (total, available)
- GPU presence and vendor
- VRAM (where the platform/driver exposes it — not guaranteed, especially cross-vendor)
- OS and version
- available disk space (for model storage)

Detection failures degrade gracefully (field = "unknown") rather than blocking the flow. Exact
libraries/APIs used for detection on Windows are a Sprint 6 implementation decision.

## 5. Recommendation Logic

Given detected hardware and the model catalog, compute which catalog entries are:

- **Recommended** (hardware meets or exceeds `recommended_*` fields)
- **Feasible but not ideal** (meets `min_*` but not `recommended_*`)
- **Not feasible** (below `min_*`)

Map catalog entries to the FAST/BALANCED/ACCURATE/CUSTOM tiers via a `tier` field on each catalog
entry (author-assigned when the catalog is curated), not computed from raw specs — tier is a
product/UX grouping, feasibility is a hardware-fit computation; keep them as separate concerns.

**Combined footprint when generation and embedding are separate models (`DECISIONS.md` D-023):**
when the user's configuration selects a distinct generation model and embedding model intended to
run simultaneously, feasibility must be computed against their **combined** RAM/VRAM requirement,
not each in isolation — recommending two models that individually fit but jointly exceed available
memory is a feasibility computation bug, not an edge case to ignore. If the combined footprint
doesn't fit the detected hardware, the app supports **sequential loading** as a documented fallback:
load the embedding model only while indexing/retrieving, swap to the generation model for
evaluation, rather than silently failing or crashing. Hardware that can't support either the
combined or sequential path for any catalog entry is reported as infeasible, not left ambiguous.

## 6. Model Download

- User-initiated only; never automatic.
- Downloads verified against the manifest's `sha256` before the model is considered usable.
- Partial/interrupted downloads must be detectable and resumable or cleanly restartable (exact
  mechanism TBD) — must not silently leave a corrupt file that later loads successfully-looking
  but is actually truncated.
- Download progress and failure are visible in the UI (`USER_FLOWS.md` §3).

## 7. GGUF Import

User can point the app at an existing local GGUF file instead of downloading. Per `SECURITY.md`
T-08, imported files have **no checksum verification against a known-good manifest** (there is
nothing to verify against), so the UI must clearly warn that an imported model's provenance is
not verified by the app, distinct from a catalog download.

## 8. Model Start / Stop / Switching

- Start: launch `LlamaCppProvider` subprocess(es) for the selected generation and/or embedding
  model; confirm via `health_check()` before the pipeline is allowed to use each.
- Single active model **per capability** at a time (`DECISIONS.md` D-011, narrowing the original
  D-008 "single active model" statement): at most one generation provider and at most one embedding
  provider are running simultaneously — not one model process total. If the same model backs both
  capabilities, that's one process; if not, it's two. Either way, V0.1 does not run a fleet of
  concurrently-serving models, and switching the generation model does not require restarting the
  embedding model (or vice versa).
- Stop: on app exit, and when the user explicitly switches a model for a given capability (stop
  that capability's current provider before starting its replacement).
- Switching: requires stopping the current provider instance for that capability and starting a
  new one; in-flight pipeline work should be allowed to fail cleanly rather than silently pointing
  at the new model mid-evaluation.

## 9. Health Checking

`health_check()` must be capability-specific (`DECISIONS.md` D-023) — a generation-only or
embedding-only provider must never be health-checked via a call it doesn't support:

- `GenerationCapable.health_check()` performs a minimal, bounded real `generate()` call and
  confirms a well-formed response — not just "process exists"; a hung subprocess that accepts
  connections but never responds must be detectable.
- `EmbeddingCapable.health_check()` performs a minimal `embed()` call on a fixed short string and
  confirms the returned vector is finite (no NaN/Inf values) and has the dimensionality expected for
  the loaded model.

Used at: model start confirmation (per capability), and optionally a periodic/on-demand check
surfaced in Settings.

## 10. Localhost Security and Model-Server Authentication

The llama.cpp server (if run in server mode) must bind to `127.0.0.1` only, exactly like the
FastAPI service (`SECURITY.md` T-09). No LAN or public interface binding in V0.1 under any
configuration.

**Localhost binding is not authentication.** An earlier draft of this document claimed that
binding alone was sufficient because the model server "has exactly one intended caller" (FastAPI).
That is incorrect: any other local process can open a TCP connection to a `127.0.0.1`-bound port
and issue requests directly, regardless of who the design *intends* to be calling it — "intended
caller" is not an enforcement mechanism (`DECISIONS.md` D-017).

The model-runtime server therefore requires its own independently generated credential, separate
from the D-009 frontend↔FastAPI session token:

- FastAPI generates a fresh random credential each time it starts a model-runtime instance, and
  holds it entirely within the backend process boundary — it is **never** sent to the frontend and
  never leaves local process space.
- Every inference request, and any administrative/introspection endpoint, must present this
  credential; requests without it are rejected before any processing. A pure liveness-only health
  endpoint may be explicitly exempted if the runtime distinguishes one from inference-capable
  endpoints — document which endpoints are exempt, don't assume.
- The credential is passed to the child at spawn in a child-scoped environment variable, or through
  a private inherited handle supported by the runtime — never a CLI argument,
  never logged (`SECURITY.md` T-10).
- If llama.cpp's server mode has no native per-request auth, FastAPI fronts it with a thin
  authenticating wrapper in the same process boundary so that no unauthenticated path to the model
  server exists, even as a side channel around the wrapper.

Binding to loopback remains required as defense-in-depth, but is no longer described as sufficient
on its own — see `SECURITY.md` T-23.

## 11. Offline Mode

Once a model is downloaded/imported and verified, all `ModelProvider` operations (`generate`,
`embed`, `health_check`) must function with external network access blocked; authenticated loopback
IPC remains permitted. Only §6 (download) and catalog
browsing require network access, and both are clearly distinguished in the UI from
fully-offline-capable actions per `ARCHITECTURE.md` §11.

See also: `AI_PIPELINE.md` for how `ModelProvider` is used, `SECURITY.md` T-08/T-09 for the
threats this design addresses.
