// Test-only entry for the Playwright browser smoke test. Installs Tauri's
// official IPC mock in place of the Rust side, then loads the app's real
// production entry point (`src/main.tsx`) unchanged. Nothing here ships:
// the production build never references this file.
import { mockIPC } from "@tauri-apps/api/mocks";

type Status = { state: string; reason?: string };
type Control = {
  status: Status;
  health: "ok" | "reject";
  calls: string[];
  setStatus(next: Status): void;
};

const HEALTHY = { status: "ok", service: "evidencegraph-service", version: "0.1.0" };

const control: Control = {
  status: { state: "ready" },
  health: "ok",
  calls: [],
  setStatus(next) {
    control.status = next;
  },
};
// Exposed so the spec can drive state transitions.
(window as unknown as { __e2e: Control }).__e2e = control;

mockIPC((cmd) => {
  control.calls.push(cmd);
  if (cmd === "get_service_status") return control.status;
  if (cmd === "check_service_health") {
    if (control.health === "reject") throw "health_request_failed";
    return HEALTHY;
  }
  throw new Error(`unexpected IPC command: ${cmd}`);
});

await import("../src/main.tsx");
