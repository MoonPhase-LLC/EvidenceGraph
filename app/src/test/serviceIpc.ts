import { act } from "@testing-library/react";
import { mockIPC } from "@tauri-apps/api/mocks";
import { vi } from "vitest";
import type { HealthResponse, SupervisorState } from "@/service/types";

// Test-only helpers for the frontend's IPC boundary. Uses Tauri's official
// `mockIPC`, so the real `invoke` path in `src/service/commands.ts` runs
// unchanged; only the Rust side is replaced.

export type Deferred<T> = {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (reason: unknown) => void;
};

export function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

export const HEALTHY: HealthResponse = {
  status: "ok",
  service: "evidencegraph-service",
  version: "0.1.0",
};

/**
 * Mocks the two Tauri commands the frontend uses. Status is answered
 * immediately from `setStatus`; every health request gets its own deferred
 * promise in `health`, settled explicitly by the test, so async ordering is
 * controlled rather than timed. Any other command is recorded and rejected.
 */
export function mockServiceIpc(initial: SupervisorState = { state: "ready" }) {
  let status = initial;
  const calls = { status: 0 };
  const health: Deferred<HealthResponse>[] = [];
  const unexpected: string[] = [];

  mockIPC((cmd) => {
    if (cmd === "get_service_status") {
      calls.status++;
      return status;
    }
    if (cmd === "check_service_health") {
      const request = deferred<HealthResponse>();
      health.push(request);
      return request.promise;
    }
    unexpected.push(cmd);
    throw new Error(`unexpected IPC command: ${cmd}`);
  });

  return {
    setStatus(next: SupervisorState) {
      status = next;
    },
    calls,
    health,
    unexpected,
  };
}

/** Advances fake timers inside `act`, flushing promise callbacks in between. */
export async function advance(ms: number) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
}

/** Runs `fn` (e.g. settling a deferred) inside `act` and flushes the result. */
export async function settle(fn: () => void = () => {}) {
  await act(async () => {
    fn();
    await vi.advanceTimersByTimeAsync(0);
  });
}
