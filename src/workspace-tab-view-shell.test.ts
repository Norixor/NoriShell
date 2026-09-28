import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";

setActivePinia(createPinia());

import { activeWorkspaceTabViewId, retainWorkspaceTabViewSummaries, setActiveWorkspaceTabView,
  setWorkspaceTabViewSummary } from "./workspace-tab-view-state";

const native = vi.hoisted(() => ({
  getView: vi.fn(), focus: vi.fn<(id: string) => Promise<void>>(async () => undefined),
  visible: vi.fn<(id: string, visible: boolean) => Promise<void>>(async () => undefined),
  create: vi.fn(async (id: string) => ({ id, label: id, ownerWindow: "main", created: true })),
  close: vi.fn(async () => undefined),
  handlers: new Map<string, (event: { payload: unknown }) => void>(),
  emitted: [] as Array<{ event: string; payload: Record<string, unknown> }>,
  autoAcknowledge: false,
  autoDeactivate: false,
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (event: string, handler: (event: { payload: unknown }) => void) => {
    native.handlers.set(event, handler);
    return () => { if (native.handlers.get(event) === handler) native.handlers.delete(event); };
  }),
  emitTo: vi.fn(async (_target: string, event: string, payload: Record<string, unknown>) => {
    native.emitted.push({ event, payload });
    if ((native.autoAcknowledge || native.autoDeactivate) && event === "workspace-tab-view-deactivate") {
      native.handlers.get("workspace-tab-view-deactivated")?.({ payload: {
        id: payload.id, viewLabel: payload.id, operationId: payload.operationId,
      } });
    }
    if (native.autoAcknowledge && event === "workspace-tab-view-activate") {
      native.handlers.get("workspace-tab-view-activated")?.({ payload: {
        id: payload.id, viewLabel: payload.id, operationId: payload.operationId,
      } });
    }
  }),
}));

vi.mock("./workspace-tab-boot-trace", () => ({ markTabBoot: vi.fn() }));
vi.mock("./workspace-tab-windows", () => ({
  getWorkspaceTabView: native.getView,
  createWorkspaceTabView: native.create,
  closeWorkspaceTabView: native.close,
  focusWorkspaceTabView: native.focus,
  focusWorkspaceWindowTarget: vi.fn(async () => undefined),
  setWorkspaceTabViewBounds: vi.fn(async () => undefined),
  setWorkspaceTabViewVisible: native.visible,
  snapshotWorkspaceTabs: vi.fn(async () => ({ owned: [], others: [] })),
  workspaceTabViewLabel: (id: string) => id,
  workspaceWindowLabel: () => "main",
}));

import { ref } from "vue";
import type { Router } from "vue-router";

import {
  activateWorkspaceTabView,
  createManagedWorkspaceTab,
  registerWorkspaceShellRouteSettled,
  requestCloseWorkspaceTabView,
  showWorkspaceShellRoute,
  startWorkspaceTabViewShell,
} from "./workspace-tab-view-shell";
import { pendingWorkspaceTabViewId, workspaceTabViewSummaries } from "./workspace-tab-view-state";
import { snapshotWorkspaceTabs } from "./workspace-tab-windows";

describe("native Workspace Tab selection", () => {
  afterEach(() => vi.useRealTimers());

  beforeEach(() => {
    native.getView.mockReset().mockImplementation(async (id: string) => ({ id, label: id, ownerWindow: "main" }));
    native.focus.mockReset().mockImplementation(async () => undefined);
    native.visible.mockReset().mockImplementation(async () => undefined);
    native.create.mockClear();
    native.handlers.clear();
    native.emitted.length = 0;
    native.autoAcknowledge = false;
    native.autoDeactivate = false;
    retainWorkspaceTabViewSummaries(new Set());
    for (const id of ["A", "B"]) setWorkspaceTabViewSummary({ id, viewLabel: id, kind: "terminal", label: id, stateLabel: "" });
    setActiveWorkspaceTabView(null);
  });

  it("waits for the previous view to release input before showing the next", async () => {
    setActiveWorkspaceTabView("A");
    const switching = activateWorkspaceTabView("B");
    await vi.waitFor(() => expect(native.emitted.some((item) => item.event === "workspace-tab-view-deactivate")).toBe(true));
    expect(activeWorkspaceTabViewId.value).toBe("A");
    expect(native.focus).not.toHaveBeenCalledWith("B");
    const request = native.emitted.find((item) => item.event === "workspace-tab-view-deactivate")!.payload;
    native.handlers.get("workspace-tab-view-deactivated")?.({ payload: {
      id: "A", viewLabel: "A", operationId: request.operationId,
    } });
    await vi.waitFor(() => expect(native.emitted.some((item) => item.event === "workspace-tab-view-activate")).toBe(true));
    expect(activeWorkspaceTabViewId.value).toBe("A");
    const activation = native.emitted.find((item) => item.event === "workspace-tab-view-activate")!.payload;
    native.handlers.get("workspace-tab-view-activated")?.({ payload: {
      id: "B", viewLabel: "B", operationId: activation.operationId,
    } });
    await switching;
    expect(activeWorkspaceTabViewId.value).toBe("B");
    expect(native.focus).toHaveBeenCalledWith("B");
  });

  it("restores the previous view's input after the new view rejects activation", async () => {
    setActiveWorkspaceTabView("A");
    const switching = activateWorkspaceTabView("B");
    const failed = expect(switching).rejects.toThrow("workspace_tab.activation_failed");
    await vi.waitFor(() => expect(native.emitted.some((item) => item.event === "workspace-tab-view-deactivate")).toBe(true));
    const release = native.emitted.find((item) => item.event === "workspace-tab-view-deactivate")!.payload;
    native.handlers.get("workspace-tab-view-deactivated")?.({ payload: {
      id: "A", viewLabel: "A", operationId: release.operationId,
    } });
    await vi.waitFor(() => expect(native.emitted.some((item) => item.event === "workspace-tab-view-activate" && item.payload.id === "B")).toBe(true));
    const requested = native.emitted.find((item) => item.event === "workspace-tab-view-activate" && item.payload.id === "B")!.payload;
    native.autoDeactivate = true;
    native.handlers.get("workspace-tab-view-activation-failed")?.({ payload: {
      id: "B", viewLabel: "B", operationId: requested.operationId, code: "workspace_tab.activation_failed",
    } });
    await vi.waitFor(() => expect(native.emitted.some((item) => item.event === "workspace-tab-view-activate" && item.payload.id === "A")).toBe(true));
    const restored = native.emitted.find((item) => item.event === "workspace-tab-view-activate" && item.payload.id === "A")!.payload;
    native.handlers.get("workspace-tab-view-activated")?.({ payload: {
      id: "A", viewLabel: "A", operationId: restored.operationId,
    } });
    await failed;
    expect(activeWorkspaceTabViewId.value).toBe("A");
    expect(native.focus).toHaveBeenLastCalledWith("A");
    expect(native.visible).toHaveBeenCalledWith("B", false);
  });

  it("ignores an earlier activation acknowledgment for the same view", async () => {
    const switching = activateWorkspaceTabView("B");
    await vi.waitFor(() => expect(native.emitted.some((item) => item.event === "workspace-tab-view-activate")).toBe(true));
    const request = native.emitted.find((item) => item.event === "workspace-tab-view-activate")!.payload;
    native.handlers.get("workspace-tab-view-activated")?.({ payload: {
      id: "B", viewLabel: "B", operationId: "earlier-operation",
    } });
    await Promise.resolve();
    expect(activeWorkspaceTabViewId.value).toBeNull();
    native.handlers.get("workspace-tab-view-activated")?.({ payload: {
      id: "B", viewLabel: "B", operationId: request.operationId,
    } });
    await switching;
    expect(activeWorkspaceTabViewId.value).toBe("B");
  });

  it("keeps no active view when activation and previous-view recovery both fail", async () => {
    setActiveWorkspaceTabView("A");
    const switching = activateWorkspaceTabView("B");
    const failed = expect(switching).rejects.toThrow("workspace_tab.activation_failed");
    await vi.waitFor(() => expect(native.emitted.some((item) => item.event === "workspace-tab-view-deactivate")).toBe(true));
    const release = native.emitted.find((item) => item.event === "workspace-tab-view-deactivate")!.payload;
    native.handlers.get("workspace-tab-view-deactivated")?.({ payload: {
      id: "A", viewLabel: "A", operationId: release.operationId,
    } });
    await vi.waitFor(() => expect(native.emitted.some((item) => item.event === "workspace-tab-view-activate" && item.payload.id === "B")).toBe(true));
    const target = native.emitted.find((item) => item.event === "workspace-tab-view-activate" && item.payload.id === "B")!.payload;
    native.autoDeactivate = true;
    native.handlers.get("workspace-tab-view-activation-failed")?.({ payload: {
      id: "B", viewLabel: "B", operationId: target.operationId, code: "workspace_tab.activation_failed",
    } });
    await vi.waitFor(() => expect(native.emitted.some((item) => item.event === "workspace-tab-view-activate" && item.payload.id === "A")).toBe(true));
    const restore = native.emitted.find((item) => item.event === "workspace-tab-view-activate" && item.payload.id === "A")!.payload;
    native.handlers.get("workspace-tab-view-activation-failed")?.({ payload: {
      id: "A", viewLabel: "A", operationId: restore.operationId, code: "workspace_tab.activation_failed",
    } });
    await failed;
    expect(activeWorkspaceTabViewId.value).toBeNull();
    expect(native.visible).toHaveBeenCalledWith("A", false);
    expect(native.visible).toHaveBeenCalledWith("B", false);
  });

  it("releases a newly activated view before restoring the old view after native focus fails", async () => {
    setActiveWorkspaceTabView("A");
    native.autoAcknowledge = true;
    native.focus.mockImplementation(async (id: string) => {
      if (id === "B") throw new Error("workspace_tab.native_focus_failed");
    });
    const switching = activateWorkspaceTabView("B");
    await expect(switching).rejects.toThrow("workspace_tab.native_focus_failed");
    const releases = native.emitted.filter((item) => item.event === "workspace-tab-view-deactivate");
    expect(releases.map((item) => item.payload.id)).toEqual(["A", "B"]);
    expect(activeWorkspaceTabViewId.value).toBe("A");
    expect(native.focus).toHaveBeenLastCalledWith("A");
  });

  it("releases a possibly activated target when its acknowledgment is lost", async () => {
    vi.useFakeTimers();
    setActiveWorkspaceTabView("A");
    native.autoDeactivate = true;
    const switching = activateWorkspaceTabView("B");
    const failed = expect(switching).rejects.toThrow("workspace_tab.view_activated_timeout");
    for (let attempt = 0; attempt < 10 && !native.emitted.some((item) => item.event === "workspace-tab-view-activate" && item.payload.id === "B"); attempt += 1) {
      await vi.advanceTimersByTimeAsync(0);
    }
    expect(native.emitted.some((item) => item.event === "workspace-tab-view-activate" && item.payload.id === "B")).toBe(true);
    await vi.advanceTimersByTimeAsync(20_000);
    const releases = native.emitted.filter((item) => item.event === "workspace-tab-view-deactivate");
    expect(releases.map((item) => item.payload.id)).toEqual(["A", "B"]);
    const restored = native.emitted.find((item) => item.event === "workspace-tab-view-activate" && item.payload.id === "A")!.payload;
    native.handlers.get("workspace-tab-view-activated")?.({ payload: {
      id: "A", viewLabel: "A", operationId: restored.operationId,
    } });
    await failed;
    expect(activeWorkspaceTabViewId.value).toBe("A");
  });

  it("does not restore the previous view when uncertain target release fails", async () => {
    vi.useFakeTimers();
    setActiveWorkspaceTabView("A");
    const switching = activateWorkspaceTabView("B");
    const failed = expect(switching).rejects.toThrow("workspace_tab.view_activated_timeout");
    for (let attempt = 0; attempt < 10 && !native.emitted.some((item) => item.event === "workspace-tab-view-deactivate"); attempt += 1) {
      await vi.advanceTimersByTimeAsync(0);
    }
    const firstRelease = native.emitted.find((item) => item.event === "workspace-tab-view-deactivate")!.payload;
    native.handlers.get("workspace-tab-view-deactivated")?.({ payload: {
      id: "A", viewLabel: "A", operationId: firstRelease.operationId,
    } });
    for (let attempt = 0; attempt < 10 && !native.emitted.some((item) => item.event === "workspace-tab-view-activate" && item.payload.id === "B"); attempt += 1) {
      await vi.advanceTimersByTimeAsync(0);
    }
    await vi.advanceTimersByTimeAsync(20_000);
    const targetRelease = native.emitted.find((item) => item.event === "workspace-tab-view-deactivate" && item.payload.id === "B")!.payload;
    native.handlers.get("workspace-tab-view-deactivated")?.({ payload: {
      id: "B", viewLabel: "B", operationId: targetRelease.operationId, code: "workspace_tab.deactivation_failed",
    } });
    await failed;
    expect(activeWorkspaceTabViewId.value).toBeNull();
    expect(native.emitted.some((item) => item.event === "workspace-tab-view-activate" && item.payload.id === "A")).toBe(false);
    expect(native.visible).toHaveBeenCalledWith("A", false);
    expect(native.visible).toHaveBeenCalledWith("B", false);
  });

  it("keeps the old Tab active when input release fails", async () => {
    setActiveWorkspaceTabView("A");
    const switching = activateWorkspaceTabView("B");
    const failed = expect(switching).rejects.toThrow("workspace_tab.input_focus_release_failed");
    await vi.waitFor(() => expect(native.emitted.some((item) => item.event === "workspace-tab-view-deactivate")).toBe(true));
    const request = native.emitted.find((item) => item.event === "workspace-tab-view-deactivate")!.payload;
    native.handlers.get("workspace-tab-view-deactivated")?.({ payload: {
      id: "A", viewLabel: "A", operationId: request.operationId,
      code: "workspace_tab.input_focus_release_failed",
    } });
    await failed;
    expect(activeWorkspaceTabViewId.value).toBe("A");
    expect(native.focus).not.toHaveBeenCalledWith("B");
  });

  it("finishes rapid selections on the most recently requested Tab", async () => {
    let finishFirst!: () => void;
    let firstShow = true;
    native.visible.mockImplementation((id: string, visible: boolean) => {
      if (id === "A" && visible && firstShow) {
        firstShow = false;
        return new Promise((resolve) => { finishFirst = () => resolve(undefined); });
      }
      return Promise.resolve();
    });
    native.autoAcknowledge = true;
    const first = activateWorkspaceTabView("A");
    const second = activateWorkspaceTabView("B");
    await vi.waitFor(() => expect(finishFirst).toBeTypeOf("function"));
    finishFirst();
    await Promise.all([first, second]);
    expect(activeWorkspaceTabViewId.value).toBe("B");
    expect(native.focus).toHaveBeenLastCalledWith("B");
  });

  it("shows a background Tab before requesting its resource-safe close", async () => {
    setActiveWorkspaceTabView("A");
    native.autoAcknowledge = true;
    const closing = requestCloseWorkspaceTabView("B");
    await vi.waitFor(() => expect(native.emitted.some((item) => item.event === "workspace-tab-view-close")).toBe(true));
    expect(activeWorkspaceTabViewId.value).toBe("B");
    expect(native.focus).toHaveBeenCalledWith("B");
    native.handlers.get("workspace-tab-view-close-cancelled")?.({ payload: { id: "B", viewLabel: "B" } });
    expect(await closing).toBe(false);
  });

  it("waits for native removal before completing a close", async () => {
    setActiveWorkspaceTabView("A");
    native.autoAcknowledge = true;
    let present = true;
    native.getView.mockImplementation(async (id: string) => present ? { id, label: id, ownerWindow: "main" } : null);
    const closing = requestCloseWorkspaceTabView("B");
    await vi.waitFor(() => expect(native.emitted.some((item) => item.event === "workspace-tab-view-close")).toBe(true));
    let settled = false;
    void closing.then(() => { settled = true; });
    await Promise.resolve();
    expect(settled).toBe(false);
    present = false;
    native.handlers.get("workspace-tab-state-changed")?.({ payload: "B" });
    expect(await closing).toBe(true);
  });

  it("shares one in-flight close request for repeated clicks", async () => {
    native.autoAcknowledge = true;
    const first = requestCloseWorkspaceTabView("B");
    const repeated = requestCloseWorkspaceTabView("B", true);
    expect(repeated).toBe(first);
    await vi.waitFor(() => expect(native.emitted.some((item) => item.event === "workspace-tab-view-close")).toBe(true));
    expect(native.emitted.filter((item) => item.event === "workspace-tab-view-close")).toHaveLength(1);
    native.handlers.get("workspace-tab-view-close-cancelled")?.({ payload: { id: "B", viewLabel: "B" } });
    expect(await first).toBe(false);
  });

  it("times out a close that still owns its Core view without removing resources", async () => {
    native.autoAcknowledge = true;
    vi.useFakeTimers();
    const closing = requestCloseWorkspaceTabView("B");
    const failed = expect(closing).rejects.toThrow("workspace_tab.close_timeout");
    for (let attempt = 0; attempt < 10 && !native.emitted.some((item) => item.event === "workspace-tab-view-close"); attempt += 1) {
      await vi.advanceTimersByTimeAsync(0);
    }
    expect(native.emitted.some((item) => item.event === "workspace-tab-view-close")).toBe(true);
    await vi.advanceTimersByTimeAsync(60_000);
    await failed;
    expect(native.getView).toHaveBeenCalledWith("B");
    expect(native.handlers.has("workspace-tab-view-close-failed")).toBe(false);
  });

  it("treats a close as complete when Core removed the view before the timeout check", async () => {
    native.autoAcknowledge = true;
    vi.useFakeTimers();
    let present = true;
    native.getView.mockImplementation(async (id: string) => present ? { id, label: id, ownerWindow: "main" } : null);
    const closing = requestCloseWorkspaceTabView("B");
    for (let attempt = 0; attempt < 10 && !native.emitted.some((item) => item.event === "workspace-tab-view-close"); attempt += 1) {
      await vi.advanceTimersByTimeAsync(0);
    }
    expect(native.emitted.some((item) => item.event === "workspace-tab-view-close")).toBe(true);
    present = false;
    await vi.advanceTimersByTimeAsync(60_000);
    expect(await closing).toBe(true);
  });
});

describe("leaving a Tab for a shell page", () => {
  let unregister: (() => void) | null = null;

  beforeEach(() => {
    native.visible.mockReset().mockImplementation(async () => undefined);
    native.handlers.clear();
    native.emitted.length = 0;
    native.autoAcknowledge = false;
    native.autoDeactivate = true;
    retainWorkspaceTabViewSummaries(new Set());
    for (const id of ["A", "B"]) setWorkspaceTabViewSummary({ id, viewLabel: id, kind: "terminal", label: id, stateLabel: "" });
    setActiveWorkspaceTabView("A");
  });
  afterEach(() => {
    unregister?.();
    unregister = null;
    vi.useRealTimers();
  });

  it("releases input, navigates, and hides the Tab only after the shell page painted", async () => {
    const order: string[] = [];
    let painted!: () => void;
    unregister = registerWorkspaceShellRouteSettled(() => new Promise((resolve) => {
      order.push("settle");
      painted = () => { order.push("painted"); resolve(); };
    }));
    native.visible.mockImplementation(async (id: string, visible: boolean) => { order.push(`${id}:${visible}`); });
    const leaving = showWorkspaceShellRoute(() => { order.push(`navigate:${String(activeWorkspaceTabViewId.value)}`); });
    await vi.waitFor(() => expect(order).toContain("settle"));
    // Released and deselected before navigation; still on screen while the page renders.
    expect(native.emitted.find((item) => item.event === "workspace-tab-view-deactivate")?.payload.id).toBe("A");
    expect(order).toEqual(["navigate:null", "settle"]);
    painted();
    await leaving;
    expect(order.slice(0, 3)).toEqual(["navigate:null", "settle", "painted"]);
    expect(order).toContain("A:false");
    expect(order).not.toContain("A:true");
    expect(activeWorkspaceTabViewId.value).toBeNull();
  });

  it("keeps the Tab active and does not navigate when input release fails", async () => {
    native.autoDeactivate = false;
    const navigate = vi.fn();
    const leaving = showWorkspaceShellRoute(navigate);
    const failed = expect(leaving).rejects.toThrow("workspace_tab.input_focus_release_failed");
    await vi.waitFor(() => expect(native.emitted.some((item) => item.event === "workspace-tab-view-deactivate")).toBe(true));
    const request = native.emitted.find((item) => item.event === "workspace-tab-view-deactivate")!.payload;
    native.handlers.get("workspace-tab-view-deactivated")?.({ payload: {
      id: "A", viewLabel: "A", operationId: request.operationId, code: "workspace_tab.input_focus_release_failed",
    } });
    await failed;
    expect(navigate).not.toHaveBeenCalled();
    expect(activeWorkspaceTabViewId.value).toBe("A");
    expect(native.visible).not.toHaveBeenCalledWith("A", false);
  });

  it("hides the Tab after a bounded wait when the shell page never reveals", async () => {
    vi.useFakeTimers();
    unregister = registerWorkspaceShellRouteSettled(() => new Promise(() => undefined));
    const leaving = showWorkspaceShellRoute(() => undefined);
    for (let attempt = 0; attempt < 10 && !native.emitted.some((item) => item.event === "workspace-tab-view-deactivate"); attempt += 1) {
      await vi.advanceTimersByTimeAsync(0);
    }
    await vi.advanceTimersByTimeAsync(100);
    expect(native.visible).not.toHaveBeenCalledWith("A", false);
    await vi.advanceTimersByTimeAsync(300);
    await leaving;
    expect(native.visible).toHaveBeenCalledWith("A", false);
  });

  it("still hides the Tab when navigation itself fails", async () => {
    unregister = registerWorkspaceShellRouteSettled(async () => undefined);
    await expect(showWorkspaceShellRoute(() => { throw new Error("navigation_failed"); })).rejects.toThrow("navigation_failed");
    expect(native.visible).toHaveBeenCalledWith("A", false);
    expect(activeWorkspaceTabViewId.value).toBeNull();
  });
});

describe("managed Tab creation", () => {
  const router = { currentRoute: ref({ path: "/terminal", fullPath: "/terminal" }), push: vi.fn() } as unknown as Router;
  let stopShell: () => void;

  beforeEach(async () => {
    native.getView.mockReset().mockResolvedValue(null);
    native.visible.mockReset().mockImplementation(async () => undefined);
    native.create.mockClear();
    native.handlers.clear();
    native.emitted.length = 0;
    native.autoAcknowledge = false;
    native.autoDeactivate = true;
    retainWorkspaceTabViewSummaries(new Set());
    setActiveWorkspaceTabView(null);
    stopShell = await startWorkspaceTabViewShell(router);
  });
  afterEach(() => stopShell());

  function signal(event: string, id: string) {
    native.handlers.get(event)?.({ payload: { id, viewLabel: id } });
  }

  it("selects the new Tab at once behind a placeholder and shows its view only once rendered", async () => {
    setWorkspaceTabViewSummary({ id: "A", viewLabel: "A", kind: "terminal", label: "A", stateLabel: "" });
    setActiveWorkspaceTabView("A");
    const creating = createManagedWorkspaceTab("B", "terminal", "/terminal", { behavior: "welcome" }, {},
      { label: "New connection", stateLabel: "Choose" });
    await vi.waitFor(() => expect(workspaceTabViewSummaries.value.map((item) => item.label)).toEqual(["A", "New connection"]));
    expect(activeWorkspaceTabViewId.value).toBe("B");
    expect(pendingWorkspaceTabViewId.value).toBe("B");
    // The old view releases input and hides so the placeholder is visible.
    await vi.waitFor(() => expect(native.visible).toHaveBeenCalledWith("A", false));
    expect(native.emitted.some((item) => item.event === "workspace-tab-view-deactivate" && item.payload.id === "A")).toBe(true);
    // The new view takes its bootstrap from Core; no ready/bootstrap round trip is needed.
    await vi.waitFor(() => expect(native.create).toHaveBeenCalledWith("B", "terminal", "/terminal", {},
      expect.objectContaining({ mode: "new", id: "B", seed: { behavior: "welcome" } })));
    await Promise.resolve();
    expect(native.emitted.some((item) => item.event === "workspace-tab-view-bootstrap")).toBe(false);
    expect(native.visible).not.toHaveBeenCalledWith("B", true);
    native.autoAcknowledge = true;
    signal("workspace-tab-view-bootstrapped", "B");
    await creating;
    expect(native.visible).toHaveBeenCalledWith("B", true);
    expect(pendingWorkspaceTabViewId.value).toBeNull();
    expect(activeWorkspaceTabViewId.value).toBe("B");
  });

  it("hides the previous Tab only after the opaque placeholder painted", async () => {
    const frames: FrameRequestCallback[] = [];
    vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => { frames.push(callback); return frames.length; });
    try {
      setWorkspaceTabViewSummary({ id: "A", viewLabel: "A", kind: "terminal", label: "A", stateLabel: "" });
      setActiveWorkspaceTabView("A");
      const creating = createManagedWorkspaceTab("B", "terminal", "/terminal", { behavior: "welcome" }, {},
        { label: "B", stateLabel: "" });
      await vi.waitFor(() => expect(frames).toHaveLength(1));
      expect(pendingWorkspaceTabViewId.value).toBe("B");
      expect(native.visible).not.toHaveBeenCalledWith("A", false);
      frames.shift()!(0);
      expect(native.visible).not.toHaveBeenCalledWith("A", false);
      frames.shift()!(16);
      await vi.waitFor(() => expect(native.visible).toHaveBeenCalledWith("A", false));
      native.autoAcknowledge = true;
      await vi.waitFor(() => expect(native.create).toHaveBeenCalled());
      signal("workspace-tab-view-bootstrapped", "B");
      await creating;
      // The new view is shown before the placeholder that it covers is removed.
      expect(native.visible).toHaveBeenCalledWith("B", true);
      expect(pendingWorkspaceTabViewId.value).toBeNull();
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("does not activate a Tab it created a second time when a late state refresh sees it", async () => {
    let releaseSnapshot!: () => void;
    vi.mocked(snapshotWorkspaceTabs).mockImplementationOnce(() => new Promise((resolve) => {
      releaseSnapshot = () => resolve({ owned: [{ id: "B", kind: "terminal", payload: {} }], others: [] } as
        unknown as Awaited<ReturnType<typeof snapshotWorkspaceTabs>>);
    }));
    // Core emits its create notification before the command returns; the refresh reads Core later.
    native.create.mockImplementationOnce(async (id: string) => {
      native.handlers.get("workspace-tab-state-changed")?.({ payload: id });
      return { id, label: id, ownerWindow: "main", created: true };
    });
    const creating = createManagedWorkspaceTab("B", "terminal", "/terminal", { behavior: "welcome" }, {},
      { label: "New connection", stateLabel: "Choose" });
    await vi.waitFor(() => expect(native.create).toHaveBeenCalled());
    native.autoAcknowledge = true;
    signal("workspace-tab-view-bootstrapped", "B");
    await creating;
    const activations = () => native.emitted.filter((item) => item.event === "workspace-tab-view-activate"
      && item.payload.id === "B").length;
    expect(activations()).toBe(1);
    native.getView.mockResolvedValue({ id: "B", label: "B", ownerWindow: "main" });
    releaseSnapshot();
    await new Promise((resolve) => setTimeout(resolve, 0));
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(activations()).toBe(1);
    expect(activeWorkspaceTabViewId.value).toBe("B");
  });

  it("removes the placeholder and returns to the previous Tab when the new view cannot be created", async () => {
    setWorkspaceTabViewSummary({ id: "A", viewLabel: "A", kind: "terminal", label: "A", stateLabel: "" });
    setActiveWorkspaceTabView("A");
    native.autoAcknowledge = true;
    native.create.mockRejectedValueOnce(new Error("workspace_tab.view_create_failed"));
    await expect(createManagedWorkspaceTab("B", "terminal", "/terminal", {}, {}, { label: "B", stateLabel: "" }))
      .rejects.toThrow("workspace_tab.view_create_failed");
    expect(pendingWorkspaceTabViewId.value).toBeNull();
    expect(workspaceTabViewSummaries.value.map((item) => item.id)).toEqual(["A"]);
    expect(activeWorkspaceTabViewId.value).toBe("A");
    expect(native.visible).toHaveBeenLastCalledWith("A", true);
  });

  it("puts a created Tab in its source's Header position without an intermediate Tab", async () => {
    for (const id of ["first", "page:newPage:source", "last"]) {
      setWorkspaceTabViewSummary({ id, viewLabel: id, kind: id.startsWith("page") ? "page" : "terminal", label: id, stateLabel: "" });
    }
    setActiveWorkspaceTabView("page:newPage:source");
    const creating = createManagedWorkspaceTab("target", "terminal", "/terminal", { behavior: "welcome" }, {},
      { label: "target", stateLabel: "" }, { replaces: "page:newPage:source" });
    await vi.waitFor(() => expect(native.create).toHaveBeenCalled());
    // The target takes the source's Header place at once.
    expect(workspaceTabViewSummaries.value.map((item) => item.id)).toEqual(["first", "target", "last"]);
    native.autoAcknowledge = true;
    signal("workspace-tab-view-bootstrapped", "target");
    await creating;
    expect(workspaceTabViewSummaries.value.map((item) => item.id)).toEqual(["first", "target", "last"]);
    expect(native.visible).toHaveBeenCalledWith("page:newPage:source", false);
  });

  it("disposes the replaced New page even when opening the Host in its target Tab fails", async () => {
    native.close.mockClear();
    setWorkspaceTabViewSummary({ id: "page:newPage:source", viewLabel: "page:newPage:source", kind: "page", label: "New", stateLabel: "" });
    setActiveWorkspaceTabView("page:newPage:source");
    native.handlers.get("workspace-tab-shell-action")?.({ payload: {
      id: "page:newPage:source", viewLabel: "page:newPage:source", ownerWindow: "main", operationId: "op",
      action: { type: "navigate", path: "/terminal", query: { hostId: "host-1" } }, replaceSource: true,
    } });
    await vi.waitFor(() => expect(native.create).toHaveBeenCalled());
    const targetId = native.create.mock.calls[0]![0];
    native.autoAcknowledge = true;
    signal("workspace-tab-view-bootstrapped", targetId);
    await vi.waitFor(() => expect(native.emitted.some((item) => item.event === "workspace-tab-activate-owned")).toBe(true));
    const request = native.emitted.find((item) => item.event === "workspace-tab-activate-owned")!.payload;
    native.handlers.get("workspace-tab-activate-owned-result")?.({ payload: {
      id: targetId, operationId: request.operationId, code: "workspace_tab.activation_failed",
    } });
    await vi.waitFor(() => expect(native.close).toHaveBeenCalledWith("page:newPage:source"));
    expect(workspaceTabViewSummaries.value.some((item) => item.id === "page:newPage:source")).toBe(false);
  });
});

describe("closing Tab handoff", () => {
  const router = { currentRoute: ref({ path: "/terminal", fullPath: "/terminal" }), push: vi.fn() } as unknown as Router;
  let stopShell: () => void;

  async function startWith(ids: string[]) {
    retainWorkspaceTabViewSummaries(new Set());
    for (const id of ids) setWorkspaceTabViewSummary({ id, viewLabel: id, kind: "terminal", label: id, stateLabel: "" });
    vi.mocked(snapshotWorkspaceTabs).mockResolvedValue({ owned: ids.map((id) => ({ id, kind: "terminal", payload: {} })), others: [] } as
      unknown as Awaited<ReturnType<typeof snapshotWorkspaceTabs>>);
    stopShell = await startWorkspaceTabViewShell(router);
    setActiveWorkspaceTabView(ids[0]!);
  }

  beforeEach(() => {
    native.getView.mockReset().mockImplementation(async (id: string) => ({ id, label: id, ownerWindow: "main" }));
    native.visible.mockReset().mockImplementation(async () => undefined);
    native.focus.mockReset().mockImplementation(async () => undefined);
    native.handlers.clear();
    native.emitted.length = 0;
    native.autoAcknowledge = true;
    native.autoDeactivate = true;
    setActiveWorkspaceTabView(null);
  });
  afterEach(() => {
    stopShell();
    vi.mocked(snapshotWorkspaceTabs).mockReset().mockResolvedValue({ owned: [], others: [] } as
      unknown as Awaited<ReturnType<typeof snapshotWorkspaceTabs>>);
  });

  const closing = (id: string, operationId = `close-${id}`) =>
    native.handlers.get("workspace-tab-view-closing")?.({ payload: { id, viewLabel: id, operationId } });
  const ready = (id: string) => native.emitted.find((item) => item.event === "workspace-tab-view-closing-ready"
    && item.payload.id === id);

  it("shows the neighbouring Tab and hides the closing view before the view tears down", async () => {
    await startWith(["A", "B"]);
    const order: string[] = [];
    native.visible.mockImplementation(async (id: string, visible: boolean) => { order.push(`${id}:${visible}`); });
    closing("A");
    await vi.waitFor(() => expect(ready("A")).toBeTruthy());
    expect(ready("A")!.payload.operationId).toBe("close-A");
    expect(activeWorkspaceTabViewId.value).toBe("B");
    // Input of the closing view is released before the next view is activated.
    const events = native.emitted.map((item) => `${item.event}:${String(item.payload.id)}`);
    expect(events.indexOf("workspace-tab-view-deactivate:A")).toBeLessThan(events.indexOf("workspace-tab-view-activate:B"));
    // New content first, then the closing view is hidden.
    expect(order.indexOf("B:true")).toBeGreaterThanOrEqual(0);
    expect(order.indexOf("B:true")).toBeLessThan(order.indexOf("A:false"));
    expect(native.focus).toHaveBeenLastCalledWith("B");
  });

  it("releases and hides the last Tab so the shell page shows", async () => {
    await startWith(["A"]);
    closing("A");
    await vi.waitFor(() => expect(ready("A")).toBeTruthy());
    expect(activeWorkspaceTabViewId.value).toBeNull();
    expect(native.emitted.some((item) => item.event === "workspace-tab-view-deactivate" && item.payload.id === "A")).toBe(true);
    expect(native.visible).toHaveBeenCalledWith("A", false);
  });

  it("shows a Tab again when its handed-off close is rolled back", async () => {
    await startWith(["A", "B"]);
    closing("A");
    await vi.waitFor(() => expect(ready("A")).toBeTruthy());
    expect(activeWorkspaceTabViewId.value).toBe("B");
    native.handlers.get("workspace-tab-view-close-restored")?.({ payload: { id: "A", viewLabel: "A" } });
    await vi.waitFor(() => expect(activeWorkspaceTabViewId.value).toBe("A"));
    expect(native.focus).toHaveBeenLastCalledWith("A");
  });

  it("does not reactivate a Tab whose close was cancelled before any handoff", async () => {
    await startWith(["A", "B"]);
    setActiveWorkspaceTabView("B");
    native.handlers.get("workspace-tab-view-close-cancelled")?.({ payload: { id: "A", viewLabel: "A" } });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(activeWorkspaceTabViewId.value).toBe("B");
  });
});
