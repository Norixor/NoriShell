import { createPinia } from "pinia";
import { flushPromises, mount } from "@vue/test-utils";
import { createMemoryHistory, createRouter } from "vue-router";
import { defineComponent, h } from "vue";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { i18n } from "../locales";
import { useWorkspaceTabsStore } from "../stores/workspaceTabs";

const tabId = "pane-close";
const viewLabel = "workspace-tab-cGFuZS1jbG9zZQ";
const native = vi.hoisted(() => ({
  handlers: new Map<string, (event: { payload: unknown }) => void>(),
  closeOwn: vi.fn(),
  flush: vi.fn(),
  emit: vi.fn(),
  take: vi.fn(async () => null as unknown),
  autoHandoff: true,
}));

vi.mock("@tauri-apps/api/webview", () => ({ getCurrentWebview: () => ({ label: viewLabel }) }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({ label: "main" }) }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (name: string, handler: (event: { payload: unknown }) => void) => {
    native.handlers.set(name, handler);
    return () => native.handlers.delete(name);
  }),
  emitTo: native.emit,
}));
vi.mock("../workspace-tab-windows", async (importOriginal) => ({
  ...await importOriginal<typeof import("../workspace-tab-windows")>(),
  getWorkspaceTabContext: vi.fn(async () => ({ id: tabId, ownerWindow: "main" })),
  takeWorkspaceTabBootstrap: native.take,
  closeOwnWorkspaceTabView: native.closeOwn,
  updateOwnWorkspaceTabProjection: vi.fn(async () => undefined),
}));
vi.mock("../terminal-workspace-persistence", async (importOriginal) => ({
  ...await importOriginal<typeof import("../terminal-workspace-persistence")>(),
  flushTerminalWorkspaceBeforeExit: native.flush,
}));
vi.mock("../stores/pluginAppIntegrations", async (importOriginal) => ({
  ...await importOriginal<typeof import("../stores/pluginAppIntegrations")>(),
  startPluginAppShortcuts: vi.fn(async () => () => undefined),
}));

import WorkspaceTabApp from "./WorkspaceTabApp.vue";

function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>((done) => { resolve = done; });
  return { promise, resolve };
}

async function mountTabApp() {
  const router = createRouter({ history: createMemoryHistory(), routes: [
    { path: "/workspace-tab-idle", component: { template: "<div />" } },
  ] });
  await router.push("/workspace-tab-idle");
  await router.isReady();
  const pinia = createPinia();
  const wrapper = mount(WorkspaceTabApp, { global: { plugins: [pinia, router, i18n] } });
  await flushPromises();
  native.handlers.get("workspace-tab-view-bootstrap")?.({ payload: {
    id: tabId, kind: "terminal", mode: "unavailable", route: "/terminal", ownerWindow: "main",
  } });
  await flushPromises();
  return { wrapper, tabs: useWorkspaceTabsStore(pinia) };
}

describe("WorkspaceTabApp committed terminal close", () => {
  beforeEach(() => {
    window.history.replaceState({}, "", `/workspace-tab.html?tabId=${tabId}`);
    native.handlers.clear();
    native.closeOwn.mockReset().mockResolvedValue(undefined);
    native.flush.mockReset().mockResolvedValue(undefined);
    native.emit.mockReset().mockImplementation(async (_target: string, event: string, payload: Record<string, unknown>) => {
      // The owner shell hides this view and answers before the view tears down.
      if (event === "workspace-tab-view-closing" && native.autoHandoff) {
        native.handlers.get("workspace-tab-view-closing-ready")?.({ payload });
      }
    });
    native.autoHandoff = true;
    native.take.mockReset().mockResolvedValue(null);
  });

  it("starts from the bootstrap Core holds for a new Tab without waiting for the shell", async () => {
    native.take.mockResolvedValue({ id: tabId, kind: "terminal", mode: "unavailable", route: "/terminal", ownerWindow: "main" });
    const router = createRouter({ history: createMemoryHistory(), routes: [
      { path: "/workspace-tab-idle", component: { template: "<div />" } },
    ] });
    await router.push("/workspace-tab-idle");
    const wrapper = mount(WorkspaceTabApp, { global: { plugins: [createPinia(), router, i18n] } });
    await flushPromises();
    expect(native.take).toHaveBeenCalledOnce();
    expect(native.emit).toHaveBeenCalledWith("main", "workspace-tab-view-bootstrapped", { id: tabId, viewLabel });
    wrapper.unmount();
  });

  afterEach(() => { window.history.replaceState({}, "", "/"); });

  it("closes an internally completed last Pane only after the durable flush", async () => {
    const { wrapper } = await mountTabApp();
    const pending = deferred();
    native.flush.mockReturnValue(pending.promise);
    window.dispatchEvent(new CustomEvent("norishell:terminal-tab-close-committed", { detail: [tabId] }));
    await flushPromises();
    expect(native.flush).toHaveBeenCalledTimes(1);
    expect(native.closeOwn).not.toHaveBeenCalled();
    pending.resolve();
    await flushPromises();
    expect(native.closeOwn).toHaveBeenCalledTimes(1);
    wrapper.unmount();
  });

  it("lets the shell show the next content before destroying its native view", async () => {
    const { wrapper } = await mountTabApp();
    native.autoHandoff = false;
    window.dispatchEvent(new CustomEvent("norishell:terminal-tab-close-committed", { detail: [tabId] }));
    await flushPromises();
    const request = native.emit.mock.calls.find(([, event]) => event === "workspace-tab-view-closing");
    expect(request?.[0]).toBe("main");
    expect(request?.[2]).toMatchObject({ id: tabId, viewLabel });
    expect(native.closeOwn).not.toHaveBeenCalled();
    native.handlers.get("workspace-tab-view-closing-ready")?.({ payload: request![2] });
    await flushPromises();
    expect(native.closeOwn).toHaveBeenCalledTimes(1);
    wrapper.unmount();
  });

  it("still closes when the shell does not answer the handoff", async () => {
    vi.useFakeTimers();
    try {
      const { wrapper } = await mountTabApp();
      native.autoHandoff = false;
      window.dispatchEvent(new CustomEvent("norishell:terminal-tab-close-committed", { detail: [tabId] }));
      await vi.advanceTimersByTimeAsync(1_999);
      expect(native.closeOwn).not.toHaveBeenCalled();
      await vi.advanceTimersByTimeAsync(1);
      await flushPromises();
      expect(native.closeOwn).toHaveBeenCalledTimes(1);
      wrapper.unmount();
    } finally { vi.useRealTimers(); }
  });

  it("ignores a committed notice while the Tab still has a Pane", async () => {
    const { wrapper, tabs } = await mountTabApp();
    tabs.syncTerminalState({ tabs: [{ groupId: tabId, label: "zsh", stateLabel: "Running" }],
      activeTabId: tabId, busy: false, activationBlocked: false, quickCommandsOpen: false });
    window.dispatchEvent(new CustomEvent("norishell:terminal-tab-close-committed", { detail: [tabId] }));
    await flushPromises();
    expect(native.flush).not.toHaveBeenCalled();
    expect(native.closeOwn).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("keeps the native Tab when the layout flush fails", async () => {
    const { wrapper } = await mountTabApp();
    native.flush.mockRejectedValue(new Error("write failed"));
    window.dispatchEvent(new CustomEvent("norishell:terminal-tab-close-committed", { detail: [tabId] }));
    await flushPromises();
    expect(native.closeOwn).not.toHaveBeenCalled();
    expect(native.emit).toHaveBeenCalledWith("main", "workspace-tab-view-close-failed",
      expect.objectContaining({ id: tabId, code: "workspace_tab.layout_not_durable" }));
    wrapper.unmount();
  });
});

describe("WorkspaceTabApp new Terminal intent", () => {
  beforeEach(() => {
    window.history.replaceState({}, "", `/workspace-tab.html?tabId=${tabId}`);
    native.handlers.clear();
    native.emit.mockReset().mockResolvedValue(undefined);
    native.take.mockReset().mockResolvedValue(null);
  });
  afterEach(() => { window.history.replaceState({}, "", "/"); });

  async function mountWithTerminal(openHost: (request: unknown) => Promise<void>) {
    const pinia = createPinia();
    const store = useWorkspaceTabsStore(pinia);
    const rendered: string[] = [];
    const project = (label: string) => store.syncTerminalState({ tabs: [{ groupId: tabId, label, stateLabel: "" }],
      activeTabId: tabId, busy: false, activationBlocked: false, quickCommandsOpen: false });
    const TerminalStub = defineComponent({
      setup() {
        store.registerTerminalController({
          createInitialTab: async () => { rendered.push("welcome"); project("New connection"); },
          snapshotTabHandoff: async () => ({}) as never,
          openHost: async (request: unknown) => { await openHost(request); rendered.push("host"); project("prod-db"); },
        } as unknown as Parameters<typeof store.registerTerminalController>[0]);
        return () => h("div");
      },
    });
    const router = createRouter({ history: createMemoryHistory(), routes: [
      { path: "/workspace-tab-idle", component: { template: "<div />" } },
      { path: "/terminal", component: TerminalStub },
    ] });
    await router.push("/workspace-tab-idle");
    await router.isReady();
    const wrapper = mount(WorkspaceTabApp, { global: { plugins: [pinia, router, i18n] } });
    await flushPromises();
    return { wrapper, rendered };
  }

  const bootstrapWith = (intent: unknown) => native.handlers.get("workspace-tab-view-bootstrap")?.({ payload: {
    id: tabId, kind: "terminal", mode: "new", route: "/terminal", ownerWindow: "main", seed: { behavior: "welcome", intent },
  } });
  const events = () => native.emit.mock.calls.map(([, event, payload]) => ({ event, payload: payload as Record<string, unknown> }));

  it("opens the Host before reporting bootstrapped, and publishes the Host title first", async () => {
    const openHost = vi.fn(async () => undefined);
    const { wrapper, rendered } = await mountWithTerminal(openHost);
    bootstrapWith({ type: "open-host", hostId: "host-1", connectOperationId: "op-1", source: "overview" });
    await flushPromises();
    expect(openHost).toHaveBeenCalledWith({ hostId: "host-1", operationId: "op-1", source: "overview",
      pluginAuthorizationToken: undefined });
    const all = events();
    const summary = all.findIndex((item) => item.event === "workspace-tab-view-summary" && item.payload.label === "prod-db");
    const ready = all.findIndex((item) => item.event === "workspace-tab-view-bootstrapped");
    expect(summary).toBeGreaterThanOrEqual(0);
    expect(summary).toBeLessThan(ready);
    expect(all[ready]!.payload.intentCode).toBeUndefined();
    // The welcome Tab existed only inside the hidden view; the Host step replaced it before display.
    expect(rendered).toEqual(["welcome", "host"]);
    expect(all.some((item) => item.event === "workspace-tab-view-summary" && item.payload.label === "New connection")).toBe(false);
    wrapper.unmount();
  });

  it("reports an intent it cannot apply without failing the Tab bootstrap", async () => {
    const { wrapper } = await mountWithTerminal(vi.fn(async () => undefined));
    bootstrapWith({ type: "open-host", hostId: "host-1", connectOperationId: "op-2", source: "overview", directoryPathBytes: [47] });
    await flushPromises();
    const ready = events().find((item) => item.event === "workspace-tab-view-bootstrapped");
    expect(ready?.payload.intentCode).toBe("workspace_tab.invalid_directory");
    expect(events().some((item) => item.event === "workspace-tab-view-bootstrap-failed")).toBe(false);
    wrapper.unmount();
  });
});
