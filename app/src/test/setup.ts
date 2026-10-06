import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, vi } from "vitest";

// Testing Library's async utilities (used by user-event) finish each action
// by waiting on a `setTimeout(0)`, and only advance the clock themselves when
// they detect *Jest's* fake timers. Under Vitest's fake timers that timeout
// would never fire. This minimal `jest` stand-in lets Testing Library advance
// Vitest's clock; it is removed after each test (`unstubGlobals`).
beforeEach(() => {
  vi.stubGlobal("jest", { advanceTimersByTime: (ms: number) => vi.advanceTimersByTime(ms) });
});

// Vitest runs without globals, so Testing Library cannot register its own
// automatic cleanup: unmount here, then remove the IPC mock and any fake
// timers so no test leaks state into the next.
afterEach(() => {
  cleanup();
  clearMocks();
  vi.useRealTimers();
});
