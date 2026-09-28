import { flushPromises, mount } from "@vue/test-utils";
import { defineComponent, h } from "vue";
import { createI18n } from "vue-i18n";
import { createMemoryHistory, createRouter } from "vue-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import NvxDesktopDisplaySettings from "../components/desktop/NvxDesktopDisplaySettings.vue";
import DesktopView from "./DesktopView.vue";
import { desktopEn } from "../locales/desktop";
import { DESKTOP_LOCAL_PREFERENCES_KEY, reloadDesktopLocalPreferences } from "../components/desktop/localPreferences";
import type { DesktopProfile, DesktopSessionSummary } from "../core-api/generated/core-api";
import type { DesktopHeaderController } from "../stores/workspaceTabs";

const mocks = vi.hoisted(() => ({
  save: vi.fn(), snapshot: vi.fn(), profiles: vi.fn(), availability: vi.fn(), focus: vi.fn(),
  openOwned: vi.fn(),
  closeOwned: vi.fn(), disconnect: vi.fn(), invalidate: vi.fn(), remoteKey: vi.fn(), sendKeys: vi.fn(),
  register: vi.fn(), sync: vi.fn(), tips: vi.fn(), windowAction: vi.fn(), ownership: vi.fn(),
  projectionUpdate: vi.fn(),
}));
const savedConnections = vi.hoisted(() => ({ changed: undefined as (() => void) | undefined }));
vi.mock("../core-api/desktop-client", () => ({ desktopClient: mocks }));
vi.mock("../workspace-tab-windows", () => ({ snapshotWorkspaceTabs: mocks.ownership,
  updateOwnWorkspaceTabProjection: mocks.projectionUpdate, workspaceWindowLabel: () => "main" }));
vi.mock("../saved-connections", () => ({ onSavedConnectionsChanged: vi.fn(async (callback: () => void) => {
  savedConnections.changed = callback;
  return () => { savedConnections.changed = undefined; };
}) }));
vi.mock("../stores/workspaceTabs", () => ({ useWorkspaceTabsStore: () => ({ registerDesktopController: mocks.register, syncDesktopState: mocks.sync }) }));
vi.mock("../stores/tips", () => ({ useTipsStore: () => ({ show: mocks.tips }) }));
vi.mock("../platform-window", () => ({ performWindowAction: mocks.windowAction }));
vi.mock("../tool-windows", () => ({ openToolWindow: vi.fn(), onToolWindowChanged: vi.fn().mockResolvedValue(() => undefined) }));
vi.mock("../platform", () => ({ detectDesktopPlatform: () => "macos" }));
const canvas = defineComponent({
  props: { session: { type: Object, default: undefined }, active: Boolean, fit: Boolean, panning: Boolean, commandAsControl: Boolean, hiDpi: Boolean },
  setup(_, { expose }) {
    expose({ invalidate: mocks.invalidate, sendKeys: mocks.sendKeys });
    return () => h("canvas", { tabindex: 0, onKeydown: mocks.remoteKey, onKeyup: mocks.remoteKey });
  },
});
const session = (id = "one") => ({
  id, generation: "1", state: "running", profile: { id, protocol: "rdp", label: `Desktop ${id}`, address: "test.invalid", port: 3389, width: 1280, height: 800, rdpTransportMode: "auto", rdpGraphicsMode: "auto", revision: "1" },
}) as DesktopSessionSummary;
let fullscreenElement: Element | null = null;
let controller: DesktopHeaderController;
const exit = vi.fn();
const wrappers: ReturnType<typeof mount>[] = [];
/** Mounts a Desktop Tab WebView; by default its Tab owns and has admitted the first snapshot session. */
async function fixture(settings = false, admit = true) {
  if (admit) window.history.replaceState({}, "", "/workspace-tab.html?tabId=desktop:one");
  const router = createRouter({ history: createMemoryHistory(), routes: [{ path: "/desktop", component: { template: "<div/>" } }] });
  await router.push("/desktop");
  const wrapper = mount(DesktopView, {
    attachTo: document.body,
    global: {
      plugins: [router, createI18n({ legacy: false, locale: "en", messages: { en: { desktop: desktopEn } } })],
      stubs: { NvxDesktopCanvas: canvas, NvxDialog: !settings, Teleport: true, NvxDesktopProfileMenu: true },
    },
  });
  wrappers.push(wrapper);
  await flushPromises();
  const [first] = await mocks.snapshot() as DesktopSessionSummary[];
  if (admit && first) {
    await controller.importHandoff?.(first);
    await controller.admitHandoff?.("desktop:one");
    await flushPromises();
  }
  return wrapper;
}
function changed(element: Element | null, event = "fullscreenchange") {
  fullscreenElement = element;
  document.dispatchEvent(new Event(event));
}
function mockEntry(wrapper: Awaited<ReturnType<typeof fixture>>) {
  const target = wrapper.get(".desktop-screen").element;
  const request = vi.fn().mockImplementation(async () => changed(target));
  Object.defineProperty(target, "requestFullscreen", { configurable: true, value: request });
  return { target, request };
}
beforeEach(() => {
  vi.useFakeTimers();
  vi.clearAllMocks();
  savedConnections.changed = undefined;
  fullscreenElement = null;
  mocks.save.mockImplementation(async profile => ({ ...profile, revision: "2" }));
  mocks.disconnect.mockResolvedValue(undefined);
  mocks.focus.mockResolvedValue("1");
  mocks.openOwned.mockResolvedValue(session("new"));
  mocks.snapshot.mockResolvedValue([session()]);
  mocks.ownership.mockResolvedValue({ owned: [{ id: "desktop:one", kind: "desktop", owner: "main",
    payload: { schemaVersion: 1, tabId: "desktop:one", sessionId: "one", generation: "1" } }], others: [] });
  mocks.profiles.mockResolvedValue([session().profile]);
  mocks.availability.mockResolvedValue([{ protocol: "rdp", available: true }]);
  mocks.invalidate.mockResolvedValue(undefined);
  mocks.sendKeys.mockResolvedValue(true);
  try { localStorage.removeItem(DESKTOP_LOCAL_PREFERENCES_KEY); } catch { /* storage optional */ }
  reloadDesktopLocalPreferences();
  mocks.closeOwned.mockResolvedValue(undefined);
  mocks.projectionUpdate.mockResolvedValue(undefined);
  mocks.register.mockImplementation((value: DesktopHeaderController) => { controller = value; return () => undefined; });
  Object.defineProperty(document, "fullscreenElement", { configurable: true, get: () => fullscreenElement });
  Object.defineProperty(document, "exitFullscreen", { configurable: true, value: exit });
  exit.mockImplementation(async () => changed(null));
});
afterEach(() => {
  for (const wrapper of wrappers.splice(0)) wrapper.unmount();
  vi.restoreAllMocks();
  for (const key of ["fullscreenElement", "exitFullscreen", "webkitFullscreenElement", "webkitExitFullscreen"]) Reflect.deleteProperty(document, key);
  vi.useRealTimers();
  window.history.replaceState({}, "", "/");
});
describe("desktop workspace layout", () => {
  it("does not render a Core-owned native desktop session in the shell", async () => {
    const wrapper = await fixture(false, false);
    expect(wrapper.find("canvas").exists()).toBe(false);
    expect(mocks.sync.mock.lastCall?.[0].tabs).toEqual([]);
    expect(mocks.closeOwned).not.toHaveBeenCalled();
  });

  it("clears a session snapshot error after the next successful snapshot", async () => {
    const wrapper = await fixture();
    mocks.snapshot.mockRejectedValueOnce(new Error("snapshot unavailable"));
    await vi.advanceTimersByTimeAsync(750);
    await flushPromises();
    expect(wrapper.find(".desktop-profiles .nvx-inline-notice--error").exists()).toBe(true);

    await vi.advanceTimersByTimeAsync(750);
    await flushPromises();
    expect(wrapper.find(".desktop-profiles .nvx-inline-notice--error").exists()).toBe(false);
    expect(wrapper.get(".desktop-toolbar__identity").text()).toContain("Desktop one");
  });
  it.each(["profiles", "availability"] as const)("keeps a %s load failure visible when session snapshots succeed", async (source) => {
    mocks[source].mockRejectedValue(new Error(`${source} unavailable`));
    const wrapper = await fixture();
    expect(wrapper.find(".desktop-profiles .nvx-inline-notice--error").exists()).toBe(true);
    await vi.advanceTimersByTimeAsync(750);
    await flushPromises();
    expect(wrapper.find(".desktop-profiles .nvx-inline-notice--error").exists()).toBe(true);
  });
  it.each(["connecting", "needsInteraction", "disconnecting"] as const)("polls %s sooner and returns to the normal interval when running", async (state) => {
    mocks.snapshot.mockResolvedValue([{ ...session(), state }]);
    await fixture();
    // The poll before the Tab admitted its session used the normal interval.
    await vi.advanceTimersByTimeAsync(750);
    const calls = mocks.snapshot.mock.calls.length;
    await vi.advanceTimersByTimeAsync(249);
    expect(mocks.snapshot).toHaveBeenCalledTimes(calls);
    mocks.snapshot.mockResolvedValue([session()]);
    await vi.advanceTimersByTimeAsync(1);
    expect(mocks.snapshot).toHaveBeenCalledTimes(calls + 1);
    await vi.advanceTimersByTimeAsync(749);
    expect(mocks.snapshot).toHaveBeenCalledTimes(calls + 1);
    await vi.advanceTimersByTimeAsync(1);
    expect(mocks.snapshot).toHaveBeenCalledTimes(calls + 2);
  });
  it("stages the same Core session when ownership projected it before bootstrap", async () => {
    window.history.replaceState({}, "", "/workspace-tab.html?tabId=desktop:one");
    const wrapper = await fixture(false, false);
    await controller.importHandoff?.(session());
    await flushPromises();
    expect(wrapper.find("canvas").exists()).toBe(false);
    expect(mocks.sync.mock.lastCall?.[0].tabs).toEqual([]);
    await controller.admitHandoff?.("desktop:one");
    await flushPromises();
    expect((wrapper.getComponent(canvas).props("session") as DesktopSessionSummary).id).toBe("one");
    expect(mocks.sync.mock.lastCall?.[0].tabs).toEqual([expect.objectContaining({ groupId: "desktop:one" })]);
    expect(mocks.openOwned).not.toHaveBeenCalled();
    expect(mocks.disconnect).not.toHaveBeenCalled();
  });

  it("rejects a different generation instead of accepting a stale desktop handoff", async () => {
    await fixture();
    await expect(controller.importHandoff?.({ ...session(), generation: "2" }))
      .rejects.toThrow("workspace_tab.desktop_session_stale");
  });

  it("refreshes saved profile names without remounting or reconnecting the active desktop", async () => {
    const wrapper = await fixture();
    const originalCanvas = wrapper.get("canvas").element;
    const renamed = { ...session().profile, label: "Synced desktop" };
    mocks.profiles.mockResolvedValue([renamed, { ...session("two").profile, label: "Added desktop" }]);
    savedConnections.changed?.();
    await flushPromises();
    expect(wrapper.get("#desktop-profiles").text()).toContain("Synced desktop");
    expect(wrapper.get("#desktop-profiles").text()).toContain("Added desktop");
    expect(wrapper.get("canvas").element).toBe(originalCanvas);
    expect(mocks.openOwned).not.toHaveBeenCalled();
    expect(mocks.disconnect).not.toHaveBeenCalled();
    mocks.profiles.mockResolvedValue([]);
    savedConnections.changed?.();
    await flushPromises();
    expect(wrapper.get("#desktop-profiles").text()).not.toContain("Synced desktop");
  });

  it("keeps the latest desktop profile read when an older one finishes late", async () => {
    const wrapper = await fixture();
    let finishOld!: (profiles: DesktopProfile[]) => void;
    mocks.profiles.mockReturnValueOnce(new Promise((resolve) => { finishOld = resolve; }));
    savedConnections.changed?.();
    mocks.profiles.mockResolvedValueOnce([{ ...session().profile, label: "Latest" }]);
    savedConnections.changed?.();
    await flushPromises();
    finishOld([{ ...session().profile, label: "Stale" }]);
    await flushPromises();
    expect(wrapper.get("#desktop-profiles").text()).toContain("Latest");
    expect(wrapper.get("#desktop-profiles").text()).not.toContain("Stale");
  });
  it("collapses and restores the saved desktops without replacing the display or changing the session", async () => {
    const wrapper = await fixture();
    const originalCanvas = wrapper.get("canvas").element;
    await wrapper.get('[aria-label="Collapse saved desktops"]').trigger("click");
    expect(wrapper.get("#desktop-profiles").isVisible()).toBe(false);
    expect(wrapper.get('[aria-label="Expand saved desktops"]').attributes("aria-expanded")).toBe("false");
    await wrapper.get('[aria-label="Expand saved desktops"]').trigger("click");
    expect(wrapper.get("#desktop-profiles").isVisible()).toBe(true);
    expect(wrapper.get("canvas").element).toBe(originalCanvas);
    expect(mocks.openOwned).not.toHaveBeenCalled();
    expect(mocks.disconnect).not.toHaveBeenCalled();
  });
  it("keeps the sidebar toggle available without an active session", async () => {
    mocks.snapshot.mockResolvedValue([]);
    const wrapper = await fixture();
    await wrapper.get('[aria-label="Collapse saved desktops"]').trigger("click");
    expect(wrapper.find('[aria-label="Expand saved desktops"]').exists()).toBe(true);
    expect(wrapper.find('[aria-label="Fullscreen desktop"]').exists()).toBe(false);
  });
  it("requests only the display fullscreen within the click and restores focus and sidebar state on exit", async () => {
    const wrapper = await fixture();
    await wrapper.get('[aria-label="Collapse saved desktops"]').trigger("click");
    const { target, request } = mockEntry(wrapper);
    mocks.invalidate.mockReturnValue(new Promise(() => undefined));
    await wrapper.get('[aria-label="Fullscreen desktop"]').trigger("click");
    await flushPromises();
    expect(request).toHaveBeenCalledOnce();
    expect(mocks.invalidate).toHaveBeenCalled();
    expect(mocks.windowAction).not.toHaveBeenCalled();
    expect(target.querySelector(".desktop-toolbar")).toBeNull();
    expect(target.querySelector("#desktop-profiles")).toBeNull();
    expect(document.activeElement).toBe(wrapper.get('[aria-label="Exit desktop fullscreen"]').element);
    await wrapper.get('[aria-label="Exit desktop fullscreen"]').trigger("click");
    await flushPromises();
    expect(fullscreenElement).toBeNull();
    expect(wrapper.get("#desktop-profiles").isVisible()).toBe(false);
    expect(document.activeElement).toBe(wrapper.get('[aria-label="Fullscreen desktop"]').element);
    expect(mocks.openOwned).not.toHaveBeenCalled();
    expect(mocks.disconnect).not.toHaveBeenCalled();
  });
  it("consumes both Escape key edges before they reach remote desktop input", async () => {
    const wrapper = await fixture();
    mockEntry(wrapper);
    await wrapper.get('[aria-label="Fullscreen desktop"]').trigger("click");
    const display = wrapper.get("canvas");
    await display.trigger("keydown", { key: "Escape" });
    await flushPromises();
    await display.trigger("keyup", { key: "Escape" });
    expect(exit).toHaveBeenCalledOnce();
    expect(mocks.remoteKey).not.toHaveBeenCalled();
    expect(wrapper.find('[aria-label="Exit desktop fullscreen"]').exists()).toBe(false);
    await display.trigger("keydown", { key: "Escape" });
    expect(mocks.remoteKey).toHaveBeenCalledOnce();
  });
  it("updates controls after an external fullscreen exit and reports a rejected entry", async () => {
    const wrapper = await fixture();
    const { request } = mockEntry(wrapper);
    await wrapper.get('[aria-label="Fullscreen desktop"]').trigger("click");
    changed(null);
    await flushPromises();
    expect(wrapper.find('[aria-label="Exit desktop fullscreen"]').exists()).toBe(false);
    request.mockRejectedValue(new Error("denied"));
    await wrapper.get('[aria-label="Fullscreen desktop"]').trigger("click");
    await flushPromises();
    expect(mocks.tips).toHaveBeenCalledWith(expect.objectContaining({ title: desktopEn.fullscreenFailed }));
    expect(wrapper.get('[aria-label="Fullscreen desktop"]').attributes("disabled")).toBeUndefined();
    expect(mocks.windowAction).not.toHaveBeenCalled();
  });
  it("leaves fullscreen on workspace deactivation, including a late entry completion", async () => {
    const wrapper = await fixture();
    const { target, request } = mockEntry(wrapper);
    let finish!: () => void;
    request.mockImplementation(() => new Promise<void>((resolve) => { finish = resolve; }));
    await wrapper.get('[aria-label="Fullscreen desktop"]').trigger("click");
    controller.deactivate();
    await flushPromises();
    changed(target);
    finish();
    await flushPromises();
    expect(fullscreenElement).toBeNull();
    expect(wrapper.find('[aria-label="Exit desktop fullscreen"]').exists()).toBe(false);
    expect(mocks.disconnect).not.toHaveBeenCalled();
  });
  it("supports the prefixed WebKit element fullscreen entry and exit", async () => {
    const wrapper = await fixture();
    const target = wrapper.get(".desktop-screen").element;
    Object.defineProperty(document, "fullscreenElement", { configurable: true, value: undefined });
    Object.defineProperty(document, "exitFullscreen", { configurable: true, value: undefined });
    Object.defineProperty(document, "webkitFullscreenElement", { configurable: true, get: () => fullscreenElement });
    const webkitExit = vi.fn().mockImplementation(() => changed(null, "webkitfullscreenchange"));
    Object.defineProperty(document, "webkitExitFullscreen", { configurable: true, value: webkitExit });
    Object.defineProperty(target, "requestFullscreen", { configurable: true, value: undefined });
    const request = vi.fn().mockImplementation(() => changed(target, "webkitfullscreenchange"));
    Object.defineProperty(target, "webkitRequestFullscreen", { configurable: true, value: request });
    await wrapper.get('[aria-label="Fullscreen desktop"]').trigger("click");
    await flushPromises();
    expect(request).toHaveBeenCalledOnce();
    await wrapper.get('[aria-label="Exit desktop fullscreen"]').trigger("click");
    expect(webkitExit).toHaveBeenCalledOnce();
    expect(fullscreenElement).toBeNull();
  });
});


describe("desktop display settings", () => {
  async function settingsFixture() {
    const wrapper = await fixture(true);
    await wrapper.get('[aria-label="Display settings"]').trigger("click");
    await flushPromises();
    return wrapper;
  }
  async function apply(wrapper: Awaited<ReturnType<typeof fixture>>) {
    await wrapper.findAll("button").find(button => button.text() === desktopEn.applyDisplaySettings)!.trigger("click");
    await flushPromises();
  }
  it("replaces the session inside the same native desktop Tab", async () => {
    window.history.replaceState({}, "", "/workspace-tab.html?tabId=desktop:one");
    mocks.ownership.mockResolvedValue({
      owned: [{ id: "desktop:one", kind: "desktop", owner: "main", 
        payload: { schemaVersion: 1, tabId: "desktop:one", sessionId: "one", generation: "1" } }],
      others: [],
    });
    const wrapper = await fixture(true, false);
    await controller.importHandoff?.(session());
    await controller.admitHandoff?.("desktop:one");
    await flushPromises();
    await wrapper.get('[aria-label="Display settings"]').trigger("click");
    await flushPromises();
    await apply(wrapper);
    expect(mocks.projectionUpdate).toHaveBeenNthCalledWith(1, { schemaVersion: 1,
      tabId: "desktop:one", profileId: "one" });
    expect(mocks.projectionUpdate).toHaveBeenCalledTimes(1);
    expect(mocks.closeOwned).toHaveBeenCalledWith(expect.objectContaining({ id: "one" }));
    expect(mocks.closeOwned.mock.invocationCallOrder[0]).toBeLessThan(mocks.openOwned.mock.invocationCallOrder[0]!);
    expect(mocks.sync.mock.lastCall?.[0].tabs).toEqual([expect.objectContaining({ groupId: "desktop:one" })]);
    expect((wrapper.getComponent(canvas).props("session") as DesktopSessionSummary).id).toBe("new");
  });
  it("waits for Core to create a session before acknowledging a new Desktop Tab", async () => {
    window.history.replaceState({}, "", "/workspace-tab.html?tabId=desktop:new");
    await fixture(false, false);
    await controller.importIdle?.("desktop:new", "one");
    await controller.admitHandoff?.("desktop:new");
    let rejectOpen!: (error: Error) => void;
    mocks.openOwned.mockImplementationOnce(() => new Promise((_resolve, reject) => { rejectOpen = reject; }));
    const pending = controller.beginOpenProfile("one");
    await flushPromises();
    expect(mocks.projectionUpdate).toHaveBeenCalledWith({ schemaVersion: 1,
      tabId: "desktop:new", profileId: "one" });
    expect(mocks.openOwned).toHaveBeenCalledOnce();
    expect(controller.isBusy()).toBe(true);
    rejectOpen(new Error("desktop.connection_failed"));
    await expect(pending).rejects.toThrow("desktop.connection_failed");
    expect(controller.isBusy()).toBe(false);
  });
  it("keeps the old native session bound if Core cleanup fails", async () => {
    window.history.replaceState({}, "", "/workspace-tab.html?tabId=desktop:one");
    mocks.ownership.mockResolvedValue({
      owned: [{ id: "desktop:one", kind: "desktop", owner: "main", 
        payload: { schemaVersion: 1, tabId: "desktop:one", sessionId: "one", generation: "1" } }],
      others: [],
    });
    mocks.closeOwned.mockRejectedValueOnce(new Error("desktop.cleanup_failed"));
    const wrapper = await fixture(true, false);
    await controller.importHandoff?.(session());
    await controller.admitHandoff?.("desktop:one");
    await flushPromises();
    await wrapper.get('[aria-label="Display settings"]').trigger("click");
    await flushPromises();
    await apply(wrapper);
    expect(mocks.closeOwned).toHaveBeenCalledWith(expect.objectContaining({ id: "one" }));
    expect(mocks.openOwned).not.toHaveBeenCalled();
    expect(mocks.projectionUpdate).not.toHaveBeenCalled();
    expect(mocks.sync.mock.lastCall?.[0].tabs).toEqual([expect.objectContaining({ groupId: "desktop:one" })]);
    expect(wrapper.text()).toContain(desktopEn.displaySettingsConnectFailed);
  });
  it("keeps a reconnect action in the same Tab when opening a replacement fails", async () => {
    window.history.replaceState({}, "", "/workspace-tab.html?tabId=desktop:one");
    mocks.ownership.mockResolvedValue({
      owned: [{ id: "desktop:one", kind: "desktop", owner: "main", 
        payload: { schemaVersion: 1, tabId: "desktop:one", sessionId: "one", generation: "1" } }],
      others: [],
    });
    mocks.openOwned.mockRejectedValueOnce(new Error("desktop.connection_failed"));
    const wrapper = await fixture(true, false);
    await controller.importHandoff?.(session());
    await controller.admitHandoff?.("desktop:one");
    await flushPromises();
    await wrapper.get('[aria-label="Display settings"]').trigger("click");
    await flushPromises();
    await apply(wrapper);
    expect(mocks.projectionUpdate).toHaveBeenCalledWith({ schemaVersion: 1,
      tabId: "desktop:one", profileId: "one" });
    expect(wrapper.find('[aria-label="Reconnect"]').exists()).toBe(true);
    mocks.openOwned.mockResolvedValue(session("new"));
    await wrapper.get('[aria-label="Reconnect"]').trigger("click");
    await flushPromises();
    expect(mocks.openOwned).toHaveBeenCalledTimes(2);
    expect(mocks.projectionUpdate).toHaveBeenCalledWith({ schemaVersion: 1,
      tabId: "desktop:one", profileId: "one" });
  });
  it("does not open a replacement if the idle projection fails", async () => {
    window.history.replaceState({}, "", "/workspace-tab.html?tabId=desktop:one");
    mocks.ownership.mockResolvedValue({
      owned: [{ id: "desktop:one", kind: "desktop", owner: "main", 
        payload: { schemaVersion: 1, tabId: "desktop:one", sessionId: "one", generation: "1" } }],
      others: [],
    });
    mocks.projectionUpdate.mockRejectedValueOnce(new Error("workspace_tab.projection_failed"));
    const wrapper = await fixture(true, false);
    await controller.importHandoff?.(session());
    await controller.admitHandoff?.("desktop:one");
    await flushPromises();
    await wrapper.get('[aria-label="Display settings"]').trigger("click");
    await flushPromises();
    await apply(wrapper);
    expect(mocks.openOwned).not.toHaveBeenCalled();
    expect(mocks.closeOwned).toHaveBeenCalledTimes(1);
    expect(wrapper.find('[aria-label="Reconnect"]').exists()).toBe(true);
  });
  it("rejects closing the Tab while a replacement is still opening", async () => {
    window.history.replaceState({}, "", "/workspace-tab.html?tabId=desktop:one");
    mocks.ownership.mockResolvedValue({
      owned: [{ id: "desktop:one", kind: "desktop", owner: "main", 
        payload: { schemaVersion: 1, tabId: "desktop:one", sessionId: "one", generation: "1" } }],
      others: [],
    });
    let finishOpen!: (value: DesktopSessionSummary) => void;
    mocks.openOwned.mockImplementationOnce(() => new Promise(resolve => { finishOpen = resolve; }));
    const wrapper = await fixture(true, false);
    await controller.importHandoff?.(session());
    await controller.admitHandoff?.("desktop:one");
    await flushPromises();
    await wrapper.get('[aria-label="Display settings"]').trigger("click");
    await flushPromises();
    const pending = wrapper.findAll("button").find(button => button.text() === desktopEn.applyDisplaySettings)!.trigger("click");
    await flushPromises();
    expect(controller.isBusy()).toBe(true);
    await expect(controller.close("desktop:one")).rejects.toThrow("workspace_tab.busy");
    finishOpen(session("new"));
    await pending;
    await flushPromises();
  });
  it("does not reconnect while closing the previous native session", async () => {
    window.history.replaceState({}, "", "/workspace-tab.html?tabId=desktop:one");
    const failed = { ...session(), state: "failed" } as DesktopSessionSummary;
    mocks.snapshot.mockResolvedValue([failed]);
    mocks.ownership.mockResolvedValue({
      owned: [{ id: "desktop:one", kind: "desktop", owner: "main", 
        payload: { schemaVersion: 1, tabId: "desktop:one", sessionId: "one", generation: "1" } }],
      others: [],
    });
    let finishClose!: () => void;
    mocks.closeOwned.mockImplementationOnce(() => new Promise<void>(resolve => { finishClose = resolve; }));
    const wrapper = await fixture(false, false);
    await controller.importHandoff?.(failed);
    await controller.admitHandoff?.("desktop:one");
    await flushPromises();
    const pending = controller.close("desktop:one");
    await flushPromises();
    expect(controller.isBusy()).toBe(true);
    await wrapper.get('[aria-label="Reconnect"]').trigger("click");
    await flushPromises();
    expect(mocks.openOwned).not.toHaveBeenCalled();
    finishClose();
    await pending;
    await flushPromises();
    expect(controller.isBusy()).toBe(false);
  });
  it("shows actual negotiated modes separately and saves before reconnecting", async () => {
    mocks.snapshot.mockResolvedValue([{ ...session(), rdpTransportActual: "tcp", rdpGraphicsActual: "remoteFxProgressive" }]);
    const wrapper = await settingsFixture();
    expect(wrapper.get(".desktop-settings-current").text()).toContain("RemoteFX Progressive");
    expect(wrapper.text()).toContain(desktopEn.displaySettingsHint);
    const fields = wrapper.getComponent(NvxDesktopDisplaySettings);
    fields.vm.$emit("update:modelValue", { ...fields.props("modelValue"), width: 1920, height: 1080, rdpTransportMode: "tcpOnly" });
    await apply(wrapper);
    expect(mocks.save).toHaveBeenCalledWith(expect.objectContaining({ width: 1920, height: 1080, rdpTransportMode: "tcpOnly", revision: "1" }));
    expect(mocks.openOwned).toHaveBeenCalledWith(expect.objectContaining({ revision: "2", width: 1920 }));
    expect(mocks.save.mock.invocationCallOrder[0]).toBeLessThan(mocks.disconnect.mock.invocationCallOrder[0]!);
    expect(mocks.disconnect.mock.invocationCallOrder[0]).toBeLessThan(mocks.openOwned.mock.invocationCallOrder[0]!);
  });
  it("offers VNC protocol and remote resolution in the session toolbar", async () => {
    const vnc = { ...session(), profile: { ...session().profile, protocol: "vnc", vncProtocolVersion: "auto", vncResolutionMode: "server", width: 1280, height: 800 } };
    mocks.snapshot.mockResolvedValue([vnc]);
    mocks.profiles.mockResolvedValue([vnc.profile]);
    const wrapper = await settingsFixture();
    expect(wrapper.text()).toContain(desktopEn.vncProtocolVersion);
    expect(wrapper.text()).toContain(desktopEn.vncResolutionMode);
    const fields = wrapper.getComponent(NvxDesktopDisplaySettings);
    fields.vm.$emit("update:modelValue", { ...fields.props("modelValue"), vncResolutionMode: "fixed", width: 1600, height: 900 });
    await apply(wrapper);
    expect(mocks.save).toHaveBeenCalledWith(expect.objectContaining({ protocol: "vnc", vncResolutionMode: "fixed", width: 1600, height: 900 }));
    expect(mocks.openOwned).toHaveBeenCalledWith(expect.objectContaining({ protocol: "vnc", vncResolutionMode: "fixed" }));
  });
  it("does not disconnect when saving fails", async () => {
    mocks.save.mockRejectedValue(new Error("failed"));
    const wrapper = await settingsFixture();
    await apply(wrapper);
    expect(mocks.disconnect).not.toHaveBeenCalled();
    expect(mocks.openOwned).not.toHaveBeenCalled();
    expect(wrapper.text()).toContain(desktopEn.displaySettingsSaveFailed);
  });
  it("does not reconnect when disconnecting fails and keeps the saved revision for retry", async () => {
    mocks.disconnect.mockRejectedValue(new Error("failed"));
    const wrapper = await settingsFixture();
    await apply(wrapper);
    expect(mocks.openOwned).not.toHaveBeenCalled();
    expect(wrapper.text()).toContain(desktopEn.displaySettingsDisconnectFailed);
    expect(wrapper.getComponent(NvxDesktopDisplaySettings).props("modelValue").revision).toBe("2");
  });
  it("does not stop or reconnect after the Tab was deactivated during a delayed save", async () => {
    let finish!: (value: unknown) => void;
    mocks.save.mockImplementation(() => new Promise(resolve => { finish = resolve; }));
    const wrapper = await settingsFixture();
    await apply(wrapper);
    controller.deactivate();
    await flushPromises();
    finish({ ...session().profile, revision: "2" });
    await flushPromises();
    expect(mocks.disconnect).not.toHaveBeenCalled();
    expect(mocks.openOwned).not.toHaveBeenCalled();
    expect(wrapper.text()).toContain(desktopEn.displaySettingsSessionChanged);
  });
  it("reports resize failures with their concrete code and ignores stale resize requests", async () => {
    const wrapper = await fixture();
    wrapper.getComponent(canvas).vm.$emit("resolutionError", { code: "desktop.rdpResolutionUnavailable", messageKey: "desktop.errors.rdpResolutionUnavailable", params: {}, diagnosticId: "resize-test" });
    expect(mocks.tips).toHaveBeenCalledWith(expect.objectContaining({ title: desktopEn.errors.rdpResolutionUnavailable, message: "desktop.rdpResolutionUnavailable · resize-test" }));
    mocks.tips.mockClear();
    wrapper.getComponent(canvas).vm.$emit("resolutionError", { code: "desktop.staleInput", messageKey: "desktop.errors.staleInput", params: {} });
    expect(mocks.tips).not.toHaveBeenCalled();
  });
  it("validates the resolution before persisting or disconnecting", async () => {
    const wrapper = await settingsFixture();
    const fields = wrapper.getComponent(NvxDesktopDisplaySettings);
    fields.vm.$emit("update:modelValue", { ...fields.props("modelValue"), width: 8192, height: 8192 });
    await apply(wrapper);
    expect(mocks.save).not.toHaveBeenCalled();
    expect(mocks.disconnect).not.toHaveBeenCalled();
    expect(wrapper.text()).toContain(desktopEn.displaySettingsInvalid);
  });
});

describe("desktop session state and local controls", () => {
  it("resumes the display and state polling when a deactivated Tab WebView is activated again", async () => {
    const wrapper = await fixture();
    const display = () => wrapper.findComponent(canvas);
    expect(display().props("active")).toBe(true);
    controller.deactivate();
    await flushPromises();
    expect(display().props("active")).toBe(false);
    // The Tab WebView never leaves /desktop, so only explicit activation can resume it.
    mocks.snapshot.mockClear();
    controller.activate("desktop:one");
    await flushPromises();
    expect(display().props("active")).toBe(true);
    expect(mocks.snapshot).toHaveBeenCalled();
  });
  it.each([
    ["connecting", desktopEn.phases.gatewayConnecting],
    ["needsInteraction", desktopEn.states.needsInteraction],
    ["disconnecting", desktopEn.states.disconnecting],
  ] as const)("covers the screen with a %s progress overlay without a reconnect action", async (state, text) => {
    mocks.snapshot.mockResolvedValue([{ ...session(), state, phase: "gatewayConnecting" }]);
    const wrapper = await fixture();
    const overlay = wrapper.get(".desktop-state--progress");
    expect(overlay.text()).toContain(text);
    expect(overlay.find("button").exists()).toBe(false);
    expect(wrapper.get(".desktop-screen").classes()).toContain("desktop-screen--dimmed");
    expect(wrapper.get('[aria-label="Send special keys"]').attributes("disabled")).toBeDefined();
  });
  it("explains a failed session in place of the stale image and reconnects from the overlay", async () => {
    mocks.snapshot.mockResolvedValue([{ ...session(), state: "failed", failure: "connectionLost" }]);
    const wrapper = await fixture();
    const overlay = wrapper.get(".desktop-state--failed");
    expect(overlay.text()).toContain(desktopEn.errors.connectionLost);
    expect(wrapper.get(".desktop-screen").classes()).toContain("desktop-screen--ended");
    expect(wrapper.find(".desktop-session .nvx-inline-notice--error").exists()).toBe(false);
    await overlay.get("button").trigger("click");
    await flushPromises();
    expect(mocks.closeOwned).toHaveBeenCalledWith(expect.objectContaining({ id: "one" }));
    expect(mocks.openOwned).toHaveBeenCalledOnce();
  });
  it("offers reconnect for a closed session and no overlay while running", async () => {
    mocks.snapshot.mockResolvedValue([{ ...session(), state: "closed" }]);
    const wrapper = await fixture();
    expect(wrapper.get(".desktop-state--closed").text()).toContain(desktopEn.reconnect);
    mocks.snapshot.mockResolvedValue([session()]);
    await vi.advanceTimersByTimeAsync(750);
    await flushPromises();
    expect(wrapper.find(".desktop-state").exists()).toBe(false);
    expect(wrapper.get(".desktop-screen").classes()).not.toContain("desktop-screen--ended");
  });
  it("sends special key chords through the display after choosing them from the toolbar menu", async () => {
    const wrapper = await fixture();
    const trigger = wrapper.get('[aria-label="Send special keys"]');
    expect(trigger.attributes("aria-haspopup")).toBe("menu");
    await trigger.trigger("click");
    await flushPromises();
    const items = wrapper.findAll('[role="menuitem"]');
    expect(items.map((item) => item.text())).toEqual(Object.values(desktopEn.specialKeyNames));
    await items[0]!.trigger("click");
    await flushPromises();
    expect(mocks.sendKeys).toHaveBeenCalledOnce();
    const sequence = mocks.sendKeys.mock.calls[0]?.[0] as { scanCode: number; down: boolean }[];
    expect(sequence.map((event) => [event.scanCode, event.down])).toEqual([[29, true], [56, true], [0x153, true], [0x153, false], [56, false], [29, false]]);
    expect(wrapper.find('[role="menu"]').exists()).toBe(false);
    mocks.sendKeys.mockResolvedValue(false);
    await trigger.trigger("click");
    await wrapper.findAll('[role="menuitem"]')[1]!.trigger("click");
    await flushPromises();
    expect(mocks.tips).toHaveBeenCalledWith(expect.objectContaining({ title: desktopEn.specialKeysFailed }));
  });
  it("applies device-local HiDPI and ⌘ preferences immediately and keeps them out of the profile", async () => {
    const wrapper = await fixture(true);
    await wrapper.get('[aria-label="Display settings"]').trigger("click");
    await flushPromises();
    const display = wrapper.getComponent(canvas);
    expect(display.props("hiDpi")).toBe(false);
    await wrapper.get("#desktop-local-hidpi").setValue(true);
    await wrapper.get("#desktop-local-command-control").setValue(true);
    expect(display.props("hiDpi")).toBe(true);
    expect(display.props("commandAsControl")).toBe(true);
    expect(JSON.parse(localStorage.getItem(DESKTOP_LOCAL_PREFERENCES_KEY) ?? "{}")).toEqual({ version: 1, commandAsControl: true, hiDpi: true });
    expect(mocks.save).not.toHaveBeenCalled();
    expect(mocks.disconnect).not.toHaveBeenCalled();
    expect(mocks.openOwned).not.toHaveBeenCalled();
  });
  it("names the unavailable protocol and the profile being deleted", async () => {
    mocks.availability.mockResolvedValue([{ protocol: "rdp", available: false, reasonKey: null }]);
    const wrapper = await fixture(true);
    expect(wrapper.get("#desktop-profiles .nvx-inline-notice--warning").text()).toContain("RDP is unavailable on this device.");
    expect(wrapper.find('[aria-label="Send special keys"]').exists()).toBe(true);
    wrapper.findComponent({ name: "NvxDesktopProfileMenu" }).vm.$emit("delete");
    await flushPromises();
    expect(wrapper.text()).toContain("Delete the saved desktop “Desktop one”?");
  });
});
