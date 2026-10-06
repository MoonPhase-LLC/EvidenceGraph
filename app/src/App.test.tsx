import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { StrictMode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App";
import { HEALTHY, advance, mockServiceIpc, settle } from "@/test/serviceIpc";

// Component tests of the real app shell over Tauri's official IPC mock.
// They check DOM structure and content only: they do not prove what a
// screen reader speaks, or rendered colors/contrast.

const POLL_MS = 250;

beforeEach(() => {
  vi.useFakeTimers();
});

function renderApp() {
  // `delay: null`: no internal setTimeout between events, so user-event never
  // waits on the fake clock (timers are advanced explicitly by the tests).
  const user = userEvent.setup({ delay: null });
  render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
  return user;
}

function serviceSection() {
  return screen.getByRole("region", { name: "Local service" });
}

/** The two live regions in the service card: [supervisor status, health result]. */
function liveRegions() {
  const [status, health] = within(serviceSection()).getAllByRole("status");
  return { status, health };
}

function checkAgainButton() {
  return screen.getByRole("button", { name: "Check local service health again" });
}

/** Renders the app, reaches `ready`, and settles the surviving initial health check. */
async function renderReadyApp() {
  const ipc = mockServiceIpc({ state: "ready" });
  const user = renderApp();
  await settle();
  // StrictMode double-runs effects; the last request is the live one.
  await settle(() => ipc.health.forEach((r) => r.resolve(HEALTHY)));
  return { ipc, user };
}

describe("app shell smoke test", () => {
  it("renders identity, navigation, main content, and the service status", async () => {
    await renderReadyApp();

    expect(screen.getByRole("heading", { level: 1, name: "Welcome to EvidenceGraph" })).toBeVisible();
    expect(screen.getByRole("navigation", { name: "Main" })).toBeVisible();
    expect(screen.getByRole("main")).toBeVisible();
    expect(screen.getByText("Assessment features are not available yet")).toBeVisible();
    const section = serviceSection();
    expect(within(section).getByText("Service ready")).toBeVisible();
    expect(within(section).getByText("evidencegraph-service")).toBeVisible();
    expect(within(section).getByText("0.1.0")).toBeVisible();
  });

  it("shows starting progress before the service is ready", async () => {
    mockServiceIpc({ state: "awaiting_endpoint" });
    renderApp();
    await settle();

    expect(within(serviceSection()).getByText("Starting service")).toBeVisible();
    expect(screen.queryByRole("button", { name: "Check local service health again" })).toBeNull();
  });

  it("shows plain-language failure wording with the code as secondary detail", async () => {
    const { ipc } = await renderReadyApp();

    ipc.setStatus({ state: "failed", reason: "authenticated_connection_lost" });
    await advance(POLL_MS);

    const section = serviceSection();
    expect(within(section).getByText("Service unavailable")).toBeVisible();
    expect(within(section).getByText(/could not start or stopped unexpectedly/)).toBeVisible();
    expect(within(section).getByText("authenticated_connection_lost")).toBeVisible();
    expect(within(section).queryByText("evidencegraph-service")).toBeNull();
  });
});

describe("unavailable navigation", () => {
  it.each(["Assessments", "Settings"])("%s is focusable, marked unavailable, and does nothing", async (label) => {
    const { ipc, user } = await renderReadyApp();
    const item = screen.getByRole("button", { name: new RegExp(`^${label}`) });

    expect(item).toHaveAttribute("aria-disabled", "true");
    // jsdom has no layout, so it joins the two stacked labels without the
    // space a real browser's accessible-name computation may add; the exact
    // name is asserted in real Chromium by the Playwright smoke test.
    expect(item).toHaveTextContent(`${label}Not available yet`);

    await user.click(item);
    item.focus();
    await user.keyboard("{Enter}{ }");

    expect(item).toHaveFocus();
    expect(screen.getByRole("heading", { level: 1, name: "Welcome to EvidenceGraph" })).toBeVisible();
    expect(ipc.unexpected).toEqual([]);
  });
});

describe("re-checking health", () => {
  it("keeps focus on the button while pending and ignores duplicate activation", async () => {
    const { ipc, user } = await renderReadyApp();
    const before = ipc.health.length;
    const button = checkAgainButton();
    button.focus();

    await user.keyboard("{Enter}");
    expect(ipc.health).toHaveLength(before + 1);
    expect(button).toHaveAttribute("aria-disabled", "true");
    expect(button).toHaveFocus();

    await user.keyboard("{Enter}{ }");
    expect(ipc.health).toHaveLength(before + 1); // ignored while pending

    await settle(() => ipc.health[before].resolve(HEALTHY));
    expect(button).not.toHaveAttribute("aria-disabled");
    expect(button).toHaveFocus();
  });
});

describe("health live region", () => {
  it("exists, empty, before the service is ready", async () => {
    mockServiceIpc({ state: "starting" });
    renderApp();
    await settle();

    const { health } = liveRegions();
    expect(health).toHaveAttribute("aria-live", "polite");
    expect(health).toHaveAttribute("aria-atomic", "true");
    expect(health).toHaveTextContent("");
  });

  it("empties while pending and refills for repeated identical outcomes, in the same element", async () => {
    const { ipc, user } = await renderReadyApp();
    const { health } = liveRegions();
    const passed = "Health check passed: evidencegraph-service 0.1.0 reported ok.";
    expect(health).toHaveTextContent(passed);

    for (const outcome of ["success", "success", "rejection", "rejection", "success"] as const) {
      const next = ipc.health.length;
      checkAgainButton().focus();
      await user.keyboard("{Enter}");
      expect(liveRegions().health).toBe(health); // stable node
      expect(health).toHaveTextContent("");

      await settle(() =>
        outcome === "success" ? ipc.health[next].resolve(HEALTHY) : ipc.health[next].reject("health_request_failed"),
      );
      expect(health).toHaveTextContent(outcome === "success" ? passed : "Health check failed.");
    }
  });

  it("announces a rejection while the supervisor status region stays unchanged", async () => {
    const { ipc, user } = await renderReadyApp();
    const { status, health } = liveRegions();
    const next = ipc.health.length;

    checkAgainButton().focus();
    await user.keyboard("{Enter}");
    await settle(() => ipc.health[next].reject("health_request_failed"));

    expect(status).toHaveTextContent("Service ready");
    expect(health).toHaveTextContent("Health check failed.");
    expect(within(serviceSection()).getByText("health_request_failed")).toBeVisible();
    expect(within(serviceSection()).queryByText("evidencegraph-service")).toBeNull();
  });

  it("stays empty when a pending check settles after the service left ready", async () => {
    const { ipc, user } = await renderReadyApp();
    const { status, health } = liveRegions();
    const next = ipc.health.length;
    checkAgainButton().focus();
    await user.keyboard("{Enter}");

    ipc.setStatus({ state: "failed", reason: "authenticated_connection_lost" });
    await advance(POLL_MS);
    await settle(() => ipc.health[next].resolve(HEALTHY));

    expect(status).toHaveTextContent("Service unavailable");
    expect(health).toHaveTextContent("");
  });
});
