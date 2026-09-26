import { flushPromises, mount } from "@vue/test-utils";
import { defineComponent, h } from "vue";
import { createI18n } from "vue-i18n";
import { createMemoryHistory, createRouter } from "vue-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import NvxDesktopDisplaySettings from "../components/desktop/NvxDesktopDisplaySettings.vue";
import DesktopView from "./DesktopView.vue";
import { desktopEn } from "../locales/desktop";
import type { DesktopProfile, DesktopSessionSummary } from "../core-api/generated/core-api";
import type { DesktopHeaderController } from "../stores/workspaceTabs";

const mocks = vi.hoisted(() => ({
  save: vi.fn(), snapshot: vi.fn(), profiles: vi.fn(), availability: vi.fn(), focus: vi.fn(), open: vi.fn(),
  close: vi.fn(), disconnect: vi.fn(), invalidate: vi.fn(), remoteKey: vi.fn(),
  register: vi.fn(), sync: vi.fn(), tips: vi.fn(), windowAction: vi.fn(),
}));
const savedConnections = vi.hoisted(() => ({ changed: undefined as (() => void) | undefined }));
vi.mock("../core-api/desktop-client", () => ({ desktopClient: mocks }));
vi.mock("../saved-connections", () => ({ onSavedConnectionsChanged: vi.fn(async (callback: () => void) => {
  savedConnections.changed = callback;
  return () => { savedConnections.changed = undefined; };
}) }));
vi.mock("../stores/workspaceTabs", () => ({ useWorkspaceTabsStore: () => ({ registerDesktopController: mocks.register, syncDesktopState: mocks.sync }) }));
vi.mock("../stores/tips", () => ({ useTipsStore: () => ({ show: mocks.tips }) }));
vi.mock("../platform-window", () => ({ performWindowAction: mocks.windowAction }));
vi.mock("../tool-windows", () => ({ openToolWindow: vi.fn(), onToolWindowChanged: vi.fn().mockResolvedValue(() => undefined) }));
const canvas = defineComponent({
  props: { session: { type: Object, default: undefined }, active: Boolean, fit: Boolean, panning: Boolean },
  setup(_, { expose }) {
    expose({ invalidate: mocks.invalidate });
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
async function fixture(settings = false) {
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
  mocks.open.mockResolvedValue(session("new"));
  mocks.snapshot.mockResolvedValue([session()]);
  mocks.profiles.mockResolvedValue([session().profile]);
  mocks.availability.mockResolvedValue([{ protocol: "rdp", available: true }]);
  mocks.invalidate.mockResolvedValue(undefined);
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
});
describe("desktop workspace layout", () => {
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
    expect(mocks.open).not.toHaveBeenCalled();
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
    expect(mocks.open).not.toHaveBeenCalled();
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
    expect(mocks.open).not.toHaveBeenCalled();
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
  it("leaves fullscreen when another session is selected", async () => {
    mocks.snapshot.mockResolvedValue([session(), session("two")]);
    const wrapper = await fixture();
    mockEntry(wrapper);
    await wrapper.get('[aria-label="Fullscreen desktop"]').trigger("click");
    controller.activate("desktop:two");
    await flushPromises();
    expect(exit).toHaveBeenCalledOnce();
    expect(wrapper.get(".desktop-toolbar__identity").text()).toContain("Desktop two");
    expect(mocks.open).not.toHaveBeenCalled();
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
  it("shows actual negotiated modes separately and saves before reconnecting", async () => {
    mocks.snapshot.mockResolvedValue([{ ...session(), rdpTransportActual: "tcp", rdpGraphicsActual: "remoteFxProgressive" }]);
    const wrapper = await settingsFixture();
    expect(wrapper.get(".desktop-settings-current").text()).toContain("RemoteFX Progressive");
    const fields = wrapper.getComponent(NvxDesktopDisplaySettings);
    fields.vm.$emit("update:modelValue", { ...fields.props("modelValue"), width: 1920, height: 1080, rdpTransportMode: "tcpOnly" });
    await apply(wrapper);
    expect(mocks.save).toHaveBeenCalledWith(expect.objectContaining({ width: 1920, height: 1080, rdpTransportMode: "tcpOnly", revision: "1" }));
    expect(mocks.open).toHaveBeenCalledWith(expect.objectContaining({ revision: "2", width: 1920 }));
    expect(mocks.save.mock.invocationCallOrder[0]).toBeLessThan(mocks.disconnect.mock.invocationCallOrder[0]!);
    expect(mocks.disconnect.mock.invocationCallOrder[0]).toBeLessThan(mocks.open.mock.invocationCallOrder[0]!);
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
    expect(mocks.open).toHaveBeenCalledWith(expect.objectContaining({ protocol: "vnc", vncResolutionMode: "fixed" }));
  });
  it("does not disconnect when saving fails", async () => {
    mocks.save.mockRejectedValue(new Error("failed"));
    const wrapper = await settingsFixture();
    await apply(wrapper);
    expect(mocks.disconnect).not.toHaveBeenCalled();
    expect(mocks.open).not.toHaveBeenCalled();
    expect(wrapper.text()).toContain(desktopEn.displaySettingsSaveFailed);
  });
  it("does not reconnect when disconnecting fails and keeps the saved revision for retry", async () => {
    mocks.disconnect.mockRejectedValue(new Error("failed"));
    const wrapper = await settingsFixture();
    await apply(wrapper);
    expect(mocks.open).not.toHaveBeenCalled();
    expect(wrapper.text()).toContain(desktopEn.displaySettingsDisconnectFailed);
    expect(wrapper.getComponent(NvxDesktopDisplaySettings).props("modelValue").revision).toBe("2");
  });
  it("does not stop or reconnect a different session after a delayed save", async () => {
    mocks.snapshot.mockResolvedValue([session(), session("two")]);
    let finish!: (value: unknown) => void;
    mocks.save.mockImplementation(() => new Promise(resolve => { finish = resolve; }));
    const wrapper = await settingsFixture();
    await apply(wrapper);
    controller.activate("desktop:two");
    await flushPromises();
    finish({ ...session().profile, revision: "2" });
    await flushPromises();
    expect(mocks.disconnect).not.toHaveBeenCalled();
    expect(mocks.open).not.toHaveBeenCalled();
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
