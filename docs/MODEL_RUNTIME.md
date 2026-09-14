# EvidenceGraph — Local Model Runtime

Status: Sprint 0 draft.

## 1. `ModelProvider` Abstraction

The AI pipeline (`AI_PIPELINE.md`) depends only on this interface, never on a specific runtime:

```
ModelProvider:
  generate(prompt: str, schema: JSONSchema, timeout_s: float) -> StructuredResult | ProviderError
  embed(text: str) -> list[float]                      # for retrieval, §AI_PIPELINE.md §5
  health_check() -> HealthStatus                        # up/down, model loaded, version info
  describe() -> ModelDescriptor                          # id, version, context window, capabilities
```

V0.1 ships exactly one implementation: `LlamaCppProvider`. The interface exists so that
`OllamaProvider`, `VLLMProvider`, or an `EnterprisePrivateProvider` could be added later without
touching pipeline code — not because V0.1 needs more than one.

`ModelProvider` implementations must never make network calls as part of `generate`/`embed`
(inference itself is always local); network calls are confined to the separate model
download/catalog flow (§5).

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

A manifest describing available/recommended models, e.g.:

```json
{
  "model_id": "example-7b-instruct-q4_k_m",
  "display_name": "Example 7B Instruct (Q4_K_M)",
  "format": "gguf",
  "size_bytes": 4_500_000_000,
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

- Start: launch `LlamaCppProvider` subprocess for the selected model; confirm via `health_check()`
  before the pipeline is allowed to use it.
- Stop: on app exit, and when the user explicitly switches models (stop current before starting
  next — avoid two model processes competing for the same hardware resources simultaneously in
  V0.1; running multiple models concurrently is not a V0.1 requirement).
- Switching: requires stopping the current provider instance and starting a new one; in-flight
  pipeline work should be allowed to fail cleanly rather than silently pointing at the new model
  mid-evaluation.

## 9. Health Checking

`health_check()` should confirm the runtime process is alive **and** capable of serving a
minimal generation request, not just "process exists" — a hung subprocess should be detectable.
Used at: model start confirmation, and optionally a periodic/on-demand check surfaced in Settings.

## 10. Localhost Security

The llama.cpp server (if run in server mode) must bind to `127.0.0.1` only, exactly like the
FastAPI service (`SECURITY.md` T-09). No LAN or public interface binding in V0.1 under any
configuration.

## 11. Offline Mode

Once a model is downloaded/imported and verified, all `ModelProvider` operations (`generate`,
`embed`, `health_check`) must function with no network access. Only §6 (download) and catalog
browsing require network access, and both are clearly distinguished in the UI from
fully-offline-capable actions per `ARCHITECTURE.md` §11.

See also: `AI_PIPELINE.md` for how `ModelProvider` is used, `SECURITY.md` T-08/T-09 for the
threats this design addresses.
