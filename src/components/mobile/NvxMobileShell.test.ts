import { flushPromises, mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { createMemoryHistory, createRouter } from "vue-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { PluginExtensionTargetContext, PluginUiContribution } from "../../core-api/generated/core-api";
import { i18n } from "../../locales";
import { handleMobileBack, mobileOverlayDepth, MOBILE_OVERLAY_Z_PROPERTY, resetMobileOverlaysForTests } from "../../mobile-overlays";
import { NvxDialog } from "../ui";
import { usePluginExtensionsStore } from "../../stores/pluginExtensions";
import { useWorkspaceTabsStore, type TerminalHeaderController } from "../../stores/workspaceTabs";
import NvxMobileShell from "./NvxMobileShell.vue";

vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true }));
vi.mock("../../platform-host", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../platform-host")>()),
  isMobileLayout: () => true,
}));

// The desktop strip and the phone sheet render the same `app.header.actions` target; this
// fixture is the isolated-demo shape (one button that asks Core to open an owned surface).
function headerActionFixture(context: PluginExtensionTargetContext): PluginUiContribution[] {
  if (context.targetId !== "app.header.actions") return [];
  return [{
    pluginId: "com.norishell.isolated-demo", pluginName: "Isolated demo", icon: "plug",
    artifactFingerprintSha256: "a".repeat(64), packageSha256: "b".repeat(64), instanceGeneration: "1", stateVersion: "1", contributionRevision: "1", target: context,
    document: { schemaVersion: 1, rootNodeId: "root", nodes: [{ kind: "button", nodeId: "root", actionId: "open", label: "Open isolated demo", icon: "plug", variant: "secondary", disabled: false }] },
  }];
}

function mockHeaderActions(contribute: boolean) {
  const store = usePluginExtensionsStore();
  vi.spyOn(store, "acquireTarget").mockImplementation(async (targetId, instanceKey, displayLabel) => ({
    key: `${targetId}\0${instanceKey}`,
    context: { targetId, contextHandle: instanceKey, targetRevision: "1", surfaceKind: "toolbar", displayLabel: displayLabel ?? null },
  }));
  vi.spyOn(store, "releaseTarget").mockResolvedValue();
  vi.spyOn(store, "loadTargetContributions").mockImplementation(async (context) => contribute ? headerActionFixture(context) : []);
  vi.spyOn(store, "forContext").mockImplementation((context) => context && contribute ? headerActionFixture(context) : []);
  return store;
}

function installMatchMedia(landscape: boolean) {
  const listeners = new Set<() => void>();
  const query = {
    get matches() { return landscape; },
    addEventListener: (_: string, fn: () => void) => listeners.add(fn),
    removeEventListener: (_: string, fn: () => void) => listeners.delete(fn),
  };
  Object.defineProperty(window, "matchMedia", { configurable: true, value: () => query });
  return {
    rotate(next: boolean) { landscape = next; for (const fn of listeners) fn(); },
    listeners,
    uninstall() { Object.defineProperty(window, "matchMedia", { configurable: true, value: undefined }); },
  };
}

function registerController() {
  const store = useWorkspaceTabsStore();
  const controller: TerminalHeaderController = {
    activate: vi.fn(), close: vi.fn(() => true), closeMany: vi.fn(() => true), quickConnect: vi.fn(() => true),
    openTelnet: vi.fn(() => true), deactivate: vi.fn(), toggleQuickCommands: vi.fn(), createTab: vi.fn(),
    focusNativeSession: vi.fn(() => false), focusSshSession: vi.fn(() => false), focusLocalSession: vi.fn(() => false),
    focusTelnetSession: vi.fn(() => false),
  };
  store.registerTerminalController(controller);
  return { store, controller };
}

async function mountShell(path = "/terminal") {
  const router = createRouter({ history: createMemoryHistory(), routes: [{ path: "/:pathMatch(.*)*", component: { template: "<div />" } }] });
  await router.push(path);
  const wrapper = mount(NvxMobileShell, { attachTo: document.body, global: { plugins: [router, i18n] } });
  await flushPromises();
  return { wrapper, router };
}

function pointer(type: string, x: number) {
  return new MouseEvent(type, { bubbles: true, clientX: x, clientY: 20, button: 0 }) as PointerEvent;
}

describe("NvxMobileShell", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    i18n.global.locale.value = "en";
    resetMobileOverlaysForTests();
    // happy-dom reports its 1024x768 window as landscape; the tests start in portrait.
    installMatchMedia(false);
  });
  afterEach(() => {
    vi.restoreAllMocks();
    Object.defineProperty(window, "matchMedia", { configurable: true, value: undefined });
    document.body.innerHTML = "";
    document.documentElement.style.removeProperty("--nvx-mobile-viewport-height");
    document.documentElement.style.removeProperty("--nvx-mobile-header-height");
  });

  it("offers plugin header actions in a sheet only when a plugin contributes to app.header.actions", async () => {
    registerController();
    mockHeaderActions(false);
    const { wrapper: empty } = await mountShell("/hosts");
    expect(empty.find('button[aria-label="Plugin actions"]').exists()).toBe(false);
    expect(document.body.querySelector('[data-plugin-target="app.header.actions"]')).toBeNull();
    empty.unmount();
    vi.restoreAllMocks();

    const store = mockHeaderActions(true);
    const invoke = vi.spyOn(store, "invokeAction").mockResolvedValue(null);
    const { wrapper } = await mountShell("/hosts");
    // The target is mounted (and acquired) like the desktop strip, but hidden until opened. The
    // sheet lives under body so no ancestor containment can trap it beneath the scrim.
    const sheetElement = document.body.querySelector<HTMLElement>(".nvx-mobile-plugin-actions")!;
    expect(sheetElement.parentElement).toBe(document.body);
    expect(sheetElement.style.display).toBe("none");
    expect(sheetElement.getAttribute("role")).toBeNull();
    expect(document.body.querySelector('[data-plugin-target="app.header.actions"]')).not.toBeNull();
    const entry = wrapper.find('button[aria-label="Plugin actions"]');
    expect(entry.exists()).toBe(true);
    await entry.trigger("click");
    await flushPromises();
    expect(sheetElement.style.display).not.toBe("none");
    expect(sheetElement.getAttribute("role")).toBe("dialog");
    expect(sheetElement.textContent).toContain("Isolated demo");
    expect(sheetElement.textContent).toContain("Open isolated demo");
    const backdrop = document.body.querySelector<HTMLElement>(".nvx-mobile-sheet-backdrop")!;
    expect(backdrop).not.toBeNull();
    // Scrim and sheet share body's stacking context; mobile.css layers the sheet above the scrim.
    expect(backdrop.parentElement).toBe(document.body);
    expect(mobileOverlayDepth()).toBe(1);
    const action = Array.from(sheetElement.querySelectorAll("button")).find((button) => button.textContent?.trim() === "Open isolated demo")!;
    action.click();
    await flushPromises();
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(invoke.mock.calls[0]![0]).toMatchObject({ pluginId: "com.norishell.isolated-demo" });
    expect(invoke.mock.calls[0]!.slice(1, 3)).toEqual(["open", []]);
    // The Android back key closes the sheet and removes its scrim.
    expect(handleMobileBack()).toBe(true);
    await flushPromises();
    expect(sheetElement.style.display).toBe("none");
    expect(document.body.querySelector(".nvx-mobile-sheet-backdrop")).toBeNull();
    expect(mobileOverlayDepth()).toBe(0);
    wrapper.unmount();
  });

  it("hides the bar in landscape while the soft keyboard is open and brings it back afterwards", async () => {
    const media = installMatchMedia(true);
    const listeners = new Set<() => void>();
    const viewport = { height: 364, addEventListener: (_: string, fn: () => void) => listeners.add(fn), removeEventListener: (_: string, fn: () => void) => listeners.delete(fn) };
    Object.defineProperty(window, "visualViewport", { configurable: true, value: viewport });
    try {
      const { store } = registerController();
      store.syncTerminalState({
        tabs: [{ groupId: "t1", label: "web", stateLabel: "Running", stateTone: "success", terminalKind: "ssh", paneCount: 1 }],
        activeTabId: "t1", busy: false, activationBlocked: false, quickCommandsOpen: false,
      });
      const { wrapper } = await mountShell("/terminal");
      expect(wrapper.classes()).toContain("nvx-mobile-shell--compact");
      expect((wrapper.element as HTMLElement).style.display).not.toBe("none");
      viewport.height = 175;
      for (const fn of listeners) fn();
      await flushPromises();
      expect((wrapper.element as HTMLElement).style.display).toBe("none");
      expect(document.documentElement.style.getPropertyValue("--nvx-mobile-header-height")).toBe("0px");
      viewport.height = 364;
      for (const fn of listeners) fn();
      await flushPromises();
      expect((wrapper.element as HTMLElement).style.display).not.toBe("none");
      expect(document.documentElement.style.getPropertyValue("--nvx-mobile-header-height")).toBe("40px");
      // Portrait with the keyboard open keeps the bar: only landscape goes immersive.
      media.rotate(false);
      viewport.height = 500;
      for (const fn of listeners) fn();
      await flushPromises();
      expect((wrapper.element as HTMLElement).style.display).not.toBe("none");
      expect(wrapper.classes()).not.toContain("nvx-mobile-shell--compact");
      wrapper.unmount();
    } finally {
      media.uninstall();
      Object.defineProperty(window, "visualViewport", { configurable: true, value: undefined });
    }
  });

  it("lets a dialog opened from a plugin action stack above the plugin-actions sheet", async () => {
    registerController();
    mockHeaderActions(true);
    const { wrapper } = await mountShell("/hosts");
    await wrapper.find('button[aria-label="Plugin actions"]').trigger("click");
    await flushPromises();
    const sheet = document.body.querySelector<HTMLElement>(".nvx-mobile-plugin-actions")!;
    const scrim = document.body.querySelector<HTMLElement>(".nvx-mobile-sheet-backdrop")!;
    const sheetZ = Number(sheet.style.getPropertyValue(MOBILE_OVERLAY_Z_PROPERTY));
    expect(sheetZ).toBeGreaterThanOrEqual(40);
    expect(scrim.style.getPropertyValue(MOBILE_OVERLAY_Z_PROPERTY)).toBe(String(sheetZ));
    // A host approval or any other dialog the action opens registers later, so it lands on top.
    const dialog = mount(NvxDialog, { attachTo: document.body, props: { modelValue: true, title: "Isolated API demo", closeLabel: "Close" }, global: { plugins: [i18n] } });
    await flushPromises();
    const backdrop = document.body.querySelector<HTMLElement>(".nvx-dialog__backdrop")!;
    expect(Number(backdrop.style.getPropertyValue(MOBILE_OVERLAY_Z_PROPERTY))).toBeGreaterThan(sheetZ);
    expect(mobileOverlayDepth()).toBe(2);
    // The back key closes the dialog first, then the sheet.
    expect(handleMobileBack()).toBe(true);
    expect(dialog.emitted("update:modelValue")).toEqual([[false]]);
    dialog.unmount();
    await flushPromises();
    expect(mobileOverlayDepth()).toBe(1);
    expect(handleMobileBack()).toBe(true);
    await flushPromises();
    expect(sheet.style.display).toBe("none");
    expect(sheet.style.getPropertyValue(MOBILE_OVERLAY_Z_PROPERTY)).toBe("");
    wrapper.unmount();
  });

  it("compacts the bar in landscape only while the Terminal page shows a Terminal Tab", async () => {
    const media = installMatchMedia(false);
    try {
      const { store } = registerController();
      store.syncTerminalState({
        tabs: [{ groupId: "t1", label: "web", stateLabel: "Connected · 1 Pane", stateTone: "success", terminalKind: "ssh", paneCount: 1 }],
        activeTabId: "t1", busy: false, activationBlocked: false, quickCommandsOpen: false,
      });
      const { wrapper, router } = await mountShell("/terminal");
      expect(wrapper.classes()).not.toContain("nvx-mobile-shell--compact");
      media.rotate(true);
      await flushPromises();
      expect(wrapper.classes()).toContain("nvx-mobile-shell--compact");
      expect(document.documentElement.style.getPropertyValue("--nvx-mobile-header-height")).toBe("40px");
      // The pill still names the Tab; only the state line is dropped.
      expect(wrapper.find(".nvx-mobile-shell__title").text()).toBe("web");
      await router.push("/hosts");
      await flushPromises();
      expect(wrapper.classes()).not.toContain("nvx-mobile-shell--compact");
      expect(document.documentElement.style.getPropertyValue("--nvx-mobile-header-height")).toBe("");
      await router.push("/terminal");
      await flushPromises();
      expect(wrapper.classes()).toContain("nvx-mobile-shell--compact");
      media.rotate(false);
      await flushPromises();
      expect(wrapper.classes()).not.toContain("nvx-mobile-shell--compact");
      wrapper.unmount();
      expect(media.listeners.size).toBe(0);
    } finally {
      media.uninstall();
    }
  });

  it("shows the active Tab in the pill with its state, the Tab count and opens the switcher", async () => {
    const { store } = registerController();
    store.syncTerminalState({
      tabs: [
        { groupId: "t1", label: "web", stateLabel: "Connected · 1 Pane", stateTone: "success", terminalKind: "ssh", paneCount: 1 },
        { groupId: "t2", label: "db", stateLabel: "Connecting · 1 Pane", stateTone: "info", terminalKind: "ssh", paneCount: 1 },
      ],
      activeTabId: "t1", busy: false, activationBlocked: false, quickCommandsOpen: false,
    });
    const { wrapper } = await mountShell("/terminal");
    const pill = wrapper.find(".nvx-mobile-shell__pill");
    expect(pill.find(".nvx-mobile-shell__title").text()).toBe("web");
    expect(pill.find(".nvx-mobile-shell__subtitle").text()).toBe("Connected · 1 Pane");
    expect(pill.find(".nvx-mobile-shell__dot--success").exists()).toBe(true);
    expect(wrapper.find(".nvx-mobile-shell__count-badge").text()).toBe("2");
    expect(wrapper.find('header img').exists()).toBe(false);
    await pill.trigger("click");
    await flushPromises();
    const dialog = document.body.querySelector('[role="dialog"]');
    expect(dialog?.textContent).toContain("Tabs");
    expect(dialog?.querySelectorAll(".nvx-mobile-switcher__item")).toHaveLength(2);
  });

  it("names the current page when no Tab is active and keeps the New action", async () => {
    registerController();
    const { wrapper } = await mountShell("/hosts");
    expect(wrapper.find(".nvx-mobile-shell__title").text()).toBe("Hosts");
    expect(wrapper.find(".nvx-mobile-shell__subtitle").text()).toBe("No tabs");
    expect(wrapper.find(".nvx-mobile-shell__dot").exists()).toBe(false);
    expect(wrapper.find('button[aria-label="New tab"]').exists()).toBe(true);
  });

  it("switches to the next or previous Tab on a horizontal swipe without opening the switcher", async () => {
    const { store, controller } = registerController();
    store.syncTerminalState({
      tabs: [{ groupId: "t1", label: "web", stateLabel: "Running" }, { groupId: "t2", label: "db", stateLabel: "Running" }],
      activeTabId: "t1", busy: false, activationBlocked: false, quickCommandsOpen: false,
    });
    const { wrapper } = await mountShell("/terminal");
    const pill = wrapper.find(".nvx-mobile-shell__pill").element;
    pill.dispatchEvent(pointer("pointerdown", 200));
    pill.dispatchEvent(pointer("pointerup", 120));
    pill.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await flushPromises();
    expect(controller.activate).toHaveBeenCalledWith("t2");
    expect(document.body.querySelector('[role="dialog"]')).toBeNull();
    // A short drift is a tap, not a swipe.
    pill.dispatchEvent(pointer("pointerdown", 200));
    pill.dispatchEvent(pointer("pointerup", 190));
    pill.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await flushPromises();
    expect(document.body.querySelector('[role="dialog"]')).not.toBeNull();
  });

  it("speaks Chinese and mirrors the visual viewport height for the soft keyboard", async () => {
    i18n.global.locale.value = "zh-CN";
    const listeners = new Map<string, () => void>();
    Object.defineProperty(window, "visualViewport", {
      configurable: true,
      value: { height: 640, addEventListener: (name: string, fn: () => void) => listeners.set(name, fn), removeEventListener: (name: string) => listeners.delete(name) },
    });
    registerController();
    const { wrapper } = await mountShell("/settings");
    expect(wrapper.find(".nvx-mobile-shell__title").text()).toBe("设置");
    expect(document.documentElement.style.getPropertyValue("--nvx-mobile-viewport-height")).toBe("640px");
    (window.visualViewport as unknown as { height: number }).height = 380;
    listeners.get("resize")?.();
    expect(document.documentElement.style.getPropertyValue("--nvx-mobile-viewport-height")).toBe("380px");
    wrapper.unmount();
    expect(document.documentElement.style.getPropertyValue("--nvx-mobile-viewport-height")).toBe("");
    expect(listeners.size).toBe(0);
    Object.defineProperty(window, "visualViewport", { configurable: true, value: undefined });
  });
});
