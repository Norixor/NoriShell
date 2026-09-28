import { flushPromises, mount } from "@vue/test-utils";
import { createPinia } from "pinia";
import { createMemoryHistory, createRouter } from "vue-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { i18n } from "../../locales";
import { useTipsStore } from "../../stores/tips";
import { retainWorkspaceTabViewSummaries, setWorkspaceTabViewSummary } from "../../workspace-tab-view-state";
import NvxWorkspaceTabBar from "./NvxWorkspaceTabBar.vue";

const events = vi.hoisted(() => ({
  callbacks: new Map<string, (event: { payload: unknown }) => void>(),
  stops: new Map<string, ReturnType<typeof vi.fn>>(),
  releaseGate: null as Promise<void> | null,
  rejectRelease: false,
  beginDrag: vi.fn(async () => {}),
  cancelDrag: vi.fn(async () => {}),
  ask: vi.fn(async () => false),
  closeTab: vi.fn<(id: string, batch?: boolean) => Promise<boolean>>(async () => true),
  menuOptions: null as { items: { action?: () => void }[] } | null,
}));
vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true, invoke: vi.fn() }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({ label: "main" }) }));
vi.mock("@tauri-apps/api/menu", () => ({ Menu: { new: async (options: { items: { action?: () => void }[] }) => {
  events.menuOptions = options;
  return { popup: async () => undefined, close: async () => undefined };
} } }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ ask: events.ask }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (name: string, callback: (event: { payload: unknown }) => void) => {
    if (name === "workspace-tab-drag-released") {
      await events.releaseGate;
      if (events.rejectRelease) throw new Error("listener unavailable");
    }
    events.callbacks.set(name, callback);
    const stop = vi.fn();
    events.stops.set(name, stop);
    return stop;
  }),
}));
vi.mock("../../workspace-tab-windows", () => ({
  workspaceWindowLabel: () => "main",
  beginWorkspaceTabDrag: events.beginDrag,
  cancelWorkspaceTabDrag: events.cancelDrag,
}));
vi.mock("../../workspace-tab-view-shell", () => ({ requestCloseWorkspaceTabView: events.closeTab }));
vi.mock("../../workspace-tab-transfer", () => ({}));

async function mountBar() {
  const router = createRouter({ history: createMemoryHistory(), routes: [
    { path: "/terminal", component: { template: "<div />" } },
  ] });
  await router.push("/terminal");
  const wrapper = mount(NvxWorkspaceTabBar, { global: {
    plugins: [createPinia(), router, i18n],
    stubs: { NvxPluginExtensionTarget: true },
  } });
  await flushPromises();
  return wrapper;
}
function hover(nonce: string, active: boolean, id = "incoming") {
  events.callbacks.get("workspace-tab-drag-hover")?.({ payload: { id, nonce, active } });
}
function pointer(type: string, x: number) {
  const event = new Event(type, { bubbles: true, cancelable: true });
  Object.defineProperties(event, {
    button: { value: 0 }, buttons: { value: 1 }, pointerId: { value: 7 },
    screenX: { value: x }, screenY: { value: 20 },
  });
  return event;
}
function attemptDrag(wrapper: Awaited<ReturnType<typeof mountBar>>) {
  wrapper.find('[role="tab"]').element.dispatchEvent(pointer("pointerdown", 100));
  window.dispatchEvent(pointer("pointermove", 120));
  window.dispatchEvent(pointer("pointerup", 120));
}

describe("native drag destination placeholder", () => {
  beforeEach(() => {
    vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
    events.callbacks.clear();
    events.stops.clear();
    events.releaseGate = null;
    events.rejectRelease = false;
    events.beginDrag.mockClear();
    events.cancelDrag.mockClear();
    events.ask.mockReset().mockResolvedValue(false);
    events.closeTab.mockReset().mockResolvedValue(true);
    events.menuOptions = null;
    retainWorkspaceTabViewSummaries(new Set());
  });
  afterEach(() => { vi.unstubAllGlobals(); retainWorkspaceTabViewSummaries(new Set()); });

  it("enters without creating a tab and removes the placeholder on leave or cancellation", async () => {
    const wrapper = await mountBar();
    hover("first", true);
    await flushPromises();
    expect(wrapper.find(".nvx-terminal-tab-bar__incoming").exists()).toBe(true);
    expect(wrapper.findAll('[role="tab"]')).toHaveLength(0);
    hover("first", false);
    await flushPromises();
    expect(wrapper.find(".nvx-terminal-tab-bar__incoming").exists()).toBe(false);
    wrapper.unmount();
    expect(events.stops.get("workspace-tab-drag-hover")).toHaveBeenCalledOnce();
  });

  it("ignores the previous drag's cleanup and replaces a current placeholder with its real tab", async () => {
    const wrapper = await mountBar();
    hover("first", true);
    hover("second", true);
    hover("first", false);
    await flushPromises();
    expect(wrapper.find(".nvx-terminal-tab-bar__incoming").exists()).toBe(true);
    setWorkspaceTabViewSummary({ id: "incoming", viewLabel: "tab-view", kind: "terminal", label: "Shell", stateLabel: "Running" });
    await flushPromises();
    expect(wrapper.find(".nvx-terminal-tab-bar__incoming").exists()).toBe(false);
    expect(wrapper.findAll('[role="tab"]')).toHaveLength(1);
    wrapper.unmount();
  });

  it("does not start a drag until the release listener is registered", async () => {
    let release!: () => void;
    events.releaseGate = new Promise<void>((resolve) => { release = resolve; });
    setWorkspaceTabViewSummary({ id: "drag-tab", viewLabel: "drag-view", kind: "terminal", label: "Shell", stateLabel: "Running" });
    const wrapper = await mountBar();
    attemptDrag(wrapper);
    expect(events.beginDrag).not.toHaveBeenCalled();

    release();
    await flushPromises();
    attemptDrag(wrapper);
    expect(events.beginDrag).toHaveBeenCalledOnce();
    wrapper.unmount();
  });

  it("keeps dragging disabled and reports a failed release-listener registration", async () => {
    events.rejectRelease = true;
    setWorkspaceTabViewSummary({ id: "drag-tab", viewLabel: "drag-view", kind: "terminal", label: "Shell", stateLabel: "Running" });
    const wrapper = await mountBar();
    attemptDrag(wrapper);
    expect(events.beginDrag).not.toHaveBeenCalled();
    expect(useTipsStore().items[0]?.message).toBe(i18n.global.t("workspaceTabError.action"));
    useTipsStore().clearAll();
    wrapper.unmount();
  });

  it("confirms a native mixed batch before closing any Tab", async () => {
    setWorkspaceTabViewSummary({ id: "page-one", viewLabel: "page-view", kind: "page", label: "Page", stateLabel: "" });
    setWorkspaceTabViewSummary({ id: "terminal-one", viewLabel: "terminal-view", kind: "terminal", label: "Shell", stateLabel: "Running" });
    const wrapper = await mountBar();
    const requestCloseAll = async () => {
      await wrapper.findAll(".nvx-terminal-tab-bar__item")[0]!.trigger("contextmenu", { clientX: 100, clientY: 50 });
      events.menuOptions?.items.at(-1)?.action?.();
      await flushPromises();
    };

    await requestCloseAll();
    expect(events.ask).toHaveBeenCalledWith(expect.stringContaining("2"), expect.objectContaining({ kind: "warning" }));
    expect(events.closeTab).not.toHaveBeenCalled();

    events.ask.mockResolvedValueOnce(true);
    await requestCloseAll();
    expect(events.closeTab).toHaveBeenNthCalledWith(1, "terminal-one", true);
    expect(events.closeTab).toHaveBeenNthCalledWith(2, "page-one", true);
    wrapper.unmount();
  });


  it("stops a confirmed batch at the first Tab whose resources refuse to close", async () => {
    setWorkspaceTabViewSummary({ id: "terminal-one", viewLabel: "terminal-view", kind: "terminal", label: "Shell", stateLabel: "Starting" });
    setWorkspaceTabViewSummary({ id: "file-one", viewLabel: "file-view", kind: "file", label: "Files", stateLabel: "" });
    events.ask.mockResolvedValueOnce(true);
    events.closeTab.mockRejectedValueOnce(new Error("workspace_tab.initial_resource_pending"));
    const wrapper = await mountBar();
    await wrapper.find(".nvx-terminal-tab-bar__item").trigger("contextmenu", { clientX: 100, clientY: 50 });
    events.menuOptions?.items.at(-1)?.action?.();
    await flushPromises();
    expect(events.ask).toHaveBeenCalledOnce();
    expect(events.closeTab).toHaveBeenCalledOnce();
    expect(events.closeTab).toHaveBeenCalledWith("terminal-one", true);
    expect(useTipsStore().items.at(-1)?.message).toBe(i18n.global.t("workspaceTabError.action"));
    useTipsStore().clearAll();
    wrapper.unmount();
  });

  it("closes Page Tabs without a resource confirmation", async () => {
    setWorkspaceTabViewSummary({ id: "page-one", viewLabel: "page-view", kind: "page", label: "Page", stateLabel: "" });
    const wrapper = await mountBar();
    await wrapper.find(".nvx-terminal-tab-bar__item").trigger("contextmenu", { clientX: 100, clientY: 50 });
    events.menuOptions?.items.at(-1)?.action?.();
    await flushPromises();
    expect(events.ask).not.toHaveBeenCalled();
    expect(events.closeTab).toHaveBeenCalledWith("page-one", true);
    wrapper.unmount();
  });
});
