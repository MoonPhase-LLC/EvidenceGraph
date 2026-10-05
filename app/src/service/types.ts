/**
 * Mirrors `app/src-tauri/src/supervisor::SupervisorState`'s
 * `#[serde(tag = "state", rename_all = "snake_case")]` JSON shape exactly.
 * The frontend never receives connection details (host/port/token) at any
 * state, including `ready` -- only this narrow status shape, per S1-04's
 * "smaller exposed capability" decision (see `check_service_health`'s doc
 * comment in `lib.rs`).
 */
export type SupervisorState =
  | { state: "starting" }
  | { state: "awaiting_endpoint" }
  | { state: "verifying_identity" }
  | { state: "installing_credential" }
  | { state: "ready" }
  | { state: "failed"; reason: string }
  | { state: "stopped" };

/** Shape of the authenticated `GET /health` body (`service/src/evidencegraph_service/routes/health.py`). */
export type HealthResponse = {
  status: string;
  service: string;
  version: string;
};
