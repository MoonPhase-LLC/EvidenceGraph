import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

/**
 * Mirrors `app/src-tauri/src/supervisor::SupervisorState`'s
 * `#[serde(tag = "state", rename_all = "snake_case")]` JSON shape exactly.
 * The frontend never receives connection details (host/port/token) at any
 * state, including `ready` -- only this narrow status shape, per S1-04's
 * "smaller exposed capability" decision (see `check_service_health`'s doc
 * comment in `lib.rs`).
 */
type SupervisorState =
  | { state: "starting" }
  | { state: "awaiting_endpoint" }
  | { state: "verifying_identity" }
  | { state: "installing_credential" }
  | { state: "ready" }
  | { state: "failed"; reason: string }
  | { state: "stopped" };

/** Shape of the authenticated `GET /health` body (`service/src/evidencegraph_service/routes/health.py`). */
type HealthResponse = {
  status: string;
  service: string;
  version: string;
};

const STARTING_LABELS: Partial<Record<SupervisorState["state"], string>> = {
  starting: "Starting local service...",
  awaiting_endpoint: "Waiting for service endpoint...",
  verifying_identity: "Verifying service identity...",
  installing_credential: "Installing session credential...",
};

// No Tauri event is emitted for state changes (`supervisor` module is
// deliberately Tauri-independent -- see its module docs); polling a
// cheap, in-memory-only command is simpler and just as responsive at
// this interval for a startup sequence that takes well under a second in
// the common case.
const POLL_INTERVAL_MS = 250;

function App() {
  const [status, setStatus] = useState<SupervisorState>({ state: "starting" });
  const [health, setHealth] = useState<HealthResponse | null>(null);
  const [healthError, setHealthError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;

    async function poll() {
      while (!cancelled) {
        try {
          const current = await invoke<SupervisorState>("get_service_status");
          if (cancelled) return;
          setStatus(current);
          // Keep observing after `ready`: the authenticated connection can
          // be lost later (D-026), which moves the supervisor to `failed`,
          // and this poll is the only thing that shows the UI that. Only
          // the terminal states end the poll.
          if (current.state === "failed" || current.state === "stopped") {
            return;
          }
        } catch {
          // Transient IPC failure -- keep polling rather than surfacing a
          // one-off glitch as a hard failure.
        }
        await new Promise((resolve) => setTimeout(resolve, POLL_INTERVAL_MS));
      }
    }

    void poll();
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    // The frontend never attempts this until the state is exactly
    // `ready` -- there is nothing to call before then, and
    // `check_service_health` itself also independently refuses
    // (`Err("service_not_ready")`) if called too early.
    if (status.state !== "ready") {
      // Leaving `ready` (connection lost, shutdown) revokes service access,
      // so a previously received health result is stale and must not keep
      // being displayed as if the service were still good.
      setHealth(null);
      setHealthError(null);
      return;
    }

    let cancelled = false;
    invoke<HealthResponse>("check_service_health")
      .then((result) => {
        if (!cancelled) setHealth(result);
      })
      .catch((err: unknown) => {
        if (!cancelled) setHealthError(String(err));
      });
    return () => {
      cancelled = true;
    };
  }, [status.state]);

  return (
    <main className="flex min-h-screen flex-col items-center justify-center gap-4 bg-background px-6 text-center text-foreground">
      <h1 className="text-3xl font-semibold tracking-tight">EvidenceGraph</h1>
      <p className="max-w-md text-sm text-muted-foreground">
        This is a placeholder screen. Assessment creation, evidence upload, and
        AI-assisted control mapping are not implemented yet.
      </p>
      <ServiceStatusPanel status={status} health={health} healthError={healthError} />
    </main>
  );
}

function ServiceStatusPanel({
  status,
  health,
  healthError,
}: {
  status: SupervisorState;
  health: HealthResponse | null;
  healthError: string | null;
}) {
  if (status.state === "failed") {
    return (
      <div className="max-w-md rounded-md border border-destructive/40 bg-destructive/10 px-4 py-3 text-sm text-destructive">
        <p className="font-medium">Local service failed to start</p>
        {/* `reason` is always one of the fixed, bounded, non-secret
            category strings the Rust supervisor produces -- never raw
            request content, a stack trace, or a credential. */}
        <p className="mt-1 text-xs opacity-80">{status.reason}</p>
      </div>
    );
  }

  if (status.state === "ready") {
    return (
      <div className="max-w-md rounded-md border border-emerald-600/30 bg-emerald-600/10 px-4 py-3 text-sm">
        <p className="font-medium text-emerald-700 dark:text-emerald-400">Local service ready</p>
        {health && (
          <p className="mt-1 text-xs text-muted-foreground">
            {health.service} v{health.version} &mdash; {health.status}
          </p>
        )}
        {healthError && (
          <p className="mt-1 text-xs text-destructive">Health check failed: {healthError}</p>
        )}
      </div>
    );
  }

  if (status.state === "stopped") {
    return <p className="text-xs text-muted-foreground">Local service stopped.</p>;
  }

  return (
    <div className="max-w-md rounded-md border border-border bg-muted/40 px-4 py-3 text-sm text-muted-foreground">
      {STARTING_LABELS[status.state] ?? "Starting local service..."}
    </div>
  );
}

export default App;
