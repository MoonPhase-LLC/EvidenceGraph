import { invoke } from "@tauri-apps/api/core";
import type { HealthResponse, SupervisorState } from "./types";

// Typed wrappers around the only two Tauri commands the frontend uses
// (registered in `app/src-tauri/src/lib.rs`). Neither returns, nor accepts,
// any connection detail: the authenticated request happens entirely in Rust.

/** Read-only snapshot of the supervisor state. Safe to call at any time. */
export function getServiceStatus(): Promise<SupervisorState> {
  return invoke<SupervisorState>("get_service_status");
}

/**
 * Performs one authenticated `GET /health` round trip in Rust. Rejects with
 * a fixed, non-secret error code string (e.g. `service_not_ready`).
 */
export function checkServiceHealth(): Promise<HealthResponse> {
  return invoke<HealthResponse>("check_service_health");
}
