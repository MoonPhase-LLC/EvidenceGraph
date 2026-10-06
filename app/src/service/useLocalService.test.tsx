import { renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { HEALTHY, advance, mockServiceIpc, settle } from "@/test/serviceIpc";
import { useLocalService } from "./useLocalService";

// These tests read the hook's returned state directly. The UI only renders
// health while `ready`, so a stale write after leaving `ready` would be
// invisible in a DOM test; it is not invisible here.

const POLL_MS = 250;
const FAILED = { state: "failed", reason: "authenticated_connection_lost" } as const;

beforeEach(() => {
  vi.useFakeTimers();
});

describe("status polling", () => {
  it("keeps polling after reaching ready", async () => {
    const ipc = mockServiceIpc({ state: "ready" });
    const { result } = renderHook(() => useLocalService());
    await settle();
    expect(result.current.status).toEqual({ state: "ready" });
    const afterFirst = ipc.calls.status;

    await advance(POLL_MS * 4);

    expect(ipc.calls.status).toBe(afterFirst + 4);
  });

  it.each([
    ["failed", FAILED],
    ["stopped", { state: "stopped" }],
  ] as const)("stops polling at the terminal state %s", async (_, terminal) => {
    const ipc = mockServiceIpc({ state: "ready" });
    const { result } = renderHook(() => useLocalService());
    await settle();

    ipc.setStatus(terminal);
    await advance(POLL_MS);
    expect(result.current.status).toEqual(terminal);
    const atTerminal = ipc.calls.status;

    await advance(POLL_MS * 8);

    expect(ipc.calls.status).toBe(atTerminal);
  });

  it("stops polling on unmount", async () => {
    const ipc = mockServiceIpc({ state: "starting" });
    const { unmount } = renderHook(() => useLocalService());
    await settle();
    unmount();
    const atUnmount = ipc.calls.status;

    await advance(POLL_MS * 8);

    expect(ipc.calls.status).toBe(atUnmount);
  });

  it("leaves exactly one polling loop under StrictMode's mount/unmount/mount", async () => {
    const ipc = mockServiceIpc({ state: "ready" });
    renderHook(() => useLocalService(), { reactStrictMode: true });
    await settle();
    const settled = ipc.calls.status;

    await advance(POLL_MS * 6);

    expect(ipc.calls.status).toBe(settled + 6);
  });
});

describe("health results", () => {
  it("does not request health until ready", async () => {
    const ipc = mockServiceIpc({ state: "verifying_identity" });
    const { result } = renderHook(() => useLocalService());
    await settle();
    await advance(POLL_MS * 2);

    expect(ipc.health).toHaveLength(0);
    expect(result.current.health).toBeNull();
  });

  it("shows a successful result while ready", async () => {
    const ipc = mockServiceIpc({ state: "ready" });
    const { result } = renderHook(() => useLocalService());
    await settle();
    expect(result.current.healthPending).toBe(true);

    await settle(() => ipc.health[0].resolve(HEALTHY));

    expect(result.current).toMatchObject({ health: HEALTHY, healthError: null, healthPending: false });
  });

  it("clears health when leaving ready", async () => {
    const ipc = mockServiceIpc({ state: "ready" });
    const { result } = renderHook(() => useLocalService());
    await settle();
    await settle(() => ipc.health[0].resolve(HEALTHY));

    ipc.setStatus(FAILED);
    await advance(POLL_MS);

    expect(result.current).toMatchObject({
      status: FAILED,
      health: null,
      healthError: null,
      healthPending: false,
    });
  });

  it("clears an earlier success when a re-check is rejected", async () => {
    const ipc = mockServiceIpc({ state: "ready" });
    const { result } = renderHook(() => useLocalService());
    await settle();
    await settle(() => ipc.health[0].resolve(HEALTHY));

    await settle(() => result.current.recheckHealth());
    await settle(() => ipc.health[1].reject("health_request_failed"));

    expect(result.current).toMatchObject({
      status: { state: "ready" },
      health: null,
      healthError: "health_request_failed",
      healthPending: false,
    });
  });

  it.each([
    ["success", (ipc: ReturnType<typeof mockServiceIpc>) => ipc.health[0].resolve(HEALTHY)],
    ["rejection", (ipc: ReturnType<typeof mockServiceIpc>) => ipc.health[0].reject("health_request_failed")],
  ])("ignores a late %s that settles after a terminal transition", async (_, settleLate) => {
    const ipc = mockServiceIpc({ state: "ready" });
    const { result } = renderHook(() => useLocalService());
    await settle();
    expect(ipc.health).toHaveLength(1); // still pending

    ipc.setStatus(FAILED);
    await advance(POLL_MS);
    await settle(() => settleLate(ipc));

    expect(result.current).toMatchObject({
      status: FAILED,
      health: null,
      healthError: null,
      healthPending: false,
    });
  });

  it.each([
    ["success", (ipc: ReturnType<typeof mockServiceIpc>) => ipc.health[0].resolve(HEALTHY)],
    ["rejection", (ipc: ReturnType<typeof mockServiceIpc>) => ipc.health[0].reject("health_request_failed")],
  ])("handles a late %s that settles after unmount without errors", async (_, settleLate) => {
    // React 18+ silently drops state updates on unmounted components, so the
    // hook's `cancelled` guard is not observable here: this checks that the
    // late settlement causes no error and no further render, not the guard.
    const ipc = mockServiceIpc({ state: "ready" });
    const errors = vi.spyOn(console, "error");
    const { result, unmount } = renderHook(() => useLocalService());
    await settle();
    const lastRendered = result.current;
    unmount();

    await settle(() => settleLate(ipc));

    expect(result.current).toBe(lastRendered); // no further render happened
    expect(errors).not.toHaveBeenCalled();
  });

  it("never lets a superseded request overwrite a newer success", async () => {
    const ipc = mockServiceIpc({ state: "ready" });
    const { result } = renderHook(() => useLocalService());
    await settle();

    await settle(() => result.current.recheckHealth()); // supersedes health[0]
    await settle(() => ipc.health[1].resolve(HEALTHY));
    await settle(() => ipc.health[0].reject("health_request_failed"));

    expect(result.current).toMatchObject({ health: HEALTHY, healthError: null, healthPending: false });
  });

  it("never lets a superseded request overwrite a newer rejection", async () => {
    const ipc = mockServiceIpc({ state: "ready" });
    const { result } = renderHook(() => useLocalService());
    await settle();

    await settle(() => result.current.recheckHealth());
    await settle(() => ipc.health[1].reject("health_request_failed"));
    await settle(() => ipc.health[0].resolve(HEALTHY));

    expect(result.current).toMatchObject({
      health: null,
      healthError: "health_request_failed",
      healthPending: false,
    });
  });

  it("uses only the surviving request under StrictMode's double effect", async () => {
    const ipc = mockServiceIpc({ state: "ready" });
    const { result } = renderHook(() => useLocalService(), { reactStrictMode: true });
    await settle();
    const live = ipc.health.length - 1;

    // Settle every abandoned request with a failure first, then the live one.
    await settle(() => ipc.health.slice(0, live).forEach((r) => r.reject("health_request_failed")));
    expect(result.current.healthError).toBeNull();
    await settle(() => ipc.health[live].resolve(HEALTHY));

    expect(result.current).toMatchObject({ health: HEALTHY, healthError: null, healthPending: false });
  });

  it("makes no IPC calls other than the two existing commands", async () => {
    const ipc = mockServiceIpc({ state: "ready" });
    const { result } = renderHook(() => useLocalService());
    await settle();
    await settle(() => ipc.health[0].resolve(HEALTHY));
    await settle(() => result.current.recheckHealth());
    await advance(POLL_MS * 2);

    expect(ipc.unexpected).toEqual([]);
  });
});
