import { useCallback, useEffect, useState } from "react";
import { checkServiceHealth, getServiceStatus } from "./commands";
import type { HealthResponse, SupervisorState } from "./types";

// No Tauri event is emitted for state changes (the `supervisor` module is
// deliberately Tauri-independent -- see its module docs); polling a cheap,
// in-memory-only command is simpler and just as responsive at this interval.
const POLL_INTERVAL_MS = 250;

export type LocalService = {
  status: SupervisorState;
  /** Last successful health result; only ever non-null while `ready`. */
  health: HealthResponse | null;
  /** Fixed error code from the last failed health check, if any. */
  healthError: string | null;
  healthPending: boolean;
  /** Runs the authenticated health check again. A no-op unless `ready`. */
  recheckHealth: () => void;
};

/**
 * Observes the local service supervisor and its authenticated health check.
 *
 * Invariants (carried over from S1-04 and relied on by the UI):
 * - Status polling continues after `ready`: the authenticated connection can
 *   be lost later (D-026), which moves the supervisor to `failed`, and this
 *   poll is the only thing that shows the UI that. Only the terminal states
 *   (`failed`, `stopped`) end it.
 * - Leaving `ready` clears any health result or error: it describes a
 *   connection that no longer exists.
 * - A health request still pending when the state changes, a re-check is
 *   requested, or the component unmounts can never write its result: each
 *   request belongs to one effect run, and that run's cleanup marks it
 *   cancelled before any later state is applied.
 */
export function useLocalService(): LocalService {
  const [status, setStatus] = useState<SupervisorState>({ state: "starting" });
  const [health, setHealth] = useState<HealthResponse | null>(null);
  const [healthError, setHealthError] = useState<string | null>(null);
  const [healthPending, setHealthPending] = useState(false);
  const [healthRequest, setHealthRequest] = useState(0);

  useEffect(() => {
    let cancelled = false;

    async function poll() {
      while (!cancelled) {
        try {
          const current = await getServiceStatus();
          if (cancelled) return;
          setStatus(current);
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
    // Never attempted until the state is exactly `ready`; the command also
    // independently refuses (`service_not_ready`) if called too early.
    if (status.state !== "ready") {
      setHealth(null);
      setHealthError(null);
      setHealthPending(false);
      return;
    }

    let cancelled = false;
    setHealthPending(true);
    checkServiceHealth()
      .then((result) => {
        if (cancelled) return;
        setHealth(result);
        setHealthError(null);
      })
      .catch((err: unknown) => {
        if (cancelled) return;
        // A failed check must not leave an earlier success on screen.
        setHealth(null);
        setHealthError(String(err));
      })
      .finally(() => {
        if (!cancelled) setHealthPending(false);
      });
    return () => {
      cancelled = true;
    };
  }, [status.state, healthRequest]);

  const recheckHealth = useCallback(() => {
    setHealthRequest((n) => n + 1);
  }, []);

  return { status, health, healthError, healthPending, recheckHealth };
}
