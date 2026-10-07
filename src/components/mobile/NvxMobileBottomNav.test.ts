import { flushPromises, mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { createMemoryHistory, createRouter } from "vue-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { i18n } from "../../locales";
import { resetMobileOverlaysForTests } from "../../mobile-overlays";
import { usePluginExtensionsStore } from "../../stores/pluginExtensions";
import { useWorkspaceTabsStore } from "../../stores/workspaceTabs";
import NvxMobileBottomNav from "./NvxMobileBottomNav.vue";

vi.mock("../../platform-host", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../platform-host")>()),
  isMobileLayout: () => true,
}));

async function mountNav(path = "/terminal") {
  const router = createRouter({ history: createMemoryHistory(), routes: [{ path: "/:pathMatch(.*)*", component: { template: "<div />" } }] });
  await router.push(path);
  const wrapper = mount(NvxMobileBottomNav, {
    attachTo: document.body,
    global: { plugins: [router, i18n], stubs: { NvxMobileQuickCommandsSheet: true, Teleport: true } },
  });
  await flushPromises();
  return { wrapper, router };
}

describe("NvxMobileBottomNav", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    i18n.global.locale.value = "en";
    resetMobileOverlaysForTests();
    // happy-dom reports its 1024x768 window as landscape; the tests start in portrait.
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      value: () => ({ matches: false, addEventListener() { /* portrait stub */ }, removeEventListener() { /* portrait stub */ } }),
    });
  });
  afterEach(() => {
    Object.defineProperty(window, "matchMedia", { configurable: true, value: undefined });
    document.body.innerHTML = "";
    document.documentElement.style.removeProperty("--nvx-mobile-bottom-inset");
  });

  it("offers the primary routes plus More, highlights the current one with a pill and navigates on tap", async () => {
    const { wrapper, router } = await mountNav("/terminal");
    const labels = wrapper.findAll("nav button").map((button) => button.text());
    expect(labels).toEqual(["Overview", "Terminal", "Hosts", "SFTP", "More"]);
    const active = wrapper.find('nav button[aria-current="page"]');
    expect(active.text()).toBe("Terminal");
    expect(active.classes()).toContain("nvx-mobile-nav__item--active");
    expect(active.find(".nvx-mobile-nav__pill").exists()).toBe(true);
    await wrapper.findAll("nav button")[2]!.trigger("click");
    await flushPromises();
    expect(router.currentRoute.value.path).toBe("/hosts");
    expect(wrapper.find('nav button[aria-current="page"]').text()).toBe("Hosts");
    expect(document.documentElement.style.getPropertyValue("--nvx-mobile-bottom-inset")).toMatch(/px$/);
  });

  it("highlights More only for a route it lists, never as a fallback", async () => {
    const { wrapper, router } = await mountNav("/overview");
    expect(wrapper.find('nav button[aria-current="page"]').text()).toBe("Overview");
    await router.push("/settings");
    await flushPromises();
    expect(wrapper.find('nav button[aria-current="page"]').text()).toBe("More");
    await router.push("/plugin/org.example/page");
    await flushPromises();
    expect(wrapper.find('nav button[aria-current="page"]').text()).toBe("More");
    await router.push("/");
    await flushPromises();
    expect(wrapper.find('nav button[aria-current="page"]').exists()).toBe(false);
  });

  it("lists only existing secondary routes in the More sheet, including plugin pages", async () => {
    usePluginExtensionsStore().navigation = [{
      pluginId: "org.norixor", pluginName: "Norixor", artifactFingerprintSha256: "a".repeat(64), packageSha256: "b".repeat(64),
      instanceGeneration: "1", stateVersion: "1", contributionRevision: "1",
      navigation: { navigationId: "sync", label: "Sync", icon: "shield", pageId: "account", order: 100 },
    }];
    const { wrapper, router } = await mountNav("/settings");
    await wrapper.find('nav button[aria-label="More"]').trigger("click");
    await flushPromises();
    const items = wrapper.findAll(".nvx-mobile-more__item");
    expect(items.map((item) => item.text())).toEqual(["Remote", "Tunnels", "Plugins", "Sync", "Settings"]);
    expect(wrapper.find(".nvx-dialog--sheet").exists()).toBe(true);
    await items[1]!.trigger("click");
    await flushPromises();
    expect(router.currentRoute.value.path).toBe("/tunnels");
    expect(wrapper.find(".nvx-mobile-more__item").exists()).toBe(false);
  });

  it("shows the helper keys with the Quick Commands entry only on the Terminal page with Terminal Tabs, and speaks Chinese", async () => {
    i18n.global.locale.value = "zh-CN";
    const { wrapper, router } = await mountNav("/terminal");
    expect(wrapper.find(".nvx-terminal-extra-keys").exists()).toBe(false);
    useWorkspaceTabsStore().syncTerminalState({ tabs: [{ groupId: "t1", label: "web", stateLabel: "运行中" }],
      activeTabId: "t1", busy: false, activationBlocked: false, quickCommandsOpen: false });
    await flushPromises();
    expect(wrapper.find(".nvx-terminal-extra-keys").exists()).toBe(true);
    expect(wrapper.find('.nvx-terminal-extra-keys__trailing button[aria-label="显示快捷命令"]').exists()).toBe(true);
    expect(wrapper.findAll("nav button").map((button) => button.text())).toEqual(["概览", "终端", "主机", "SFTP", "更多"]);
    await router.push("/hosts");
    await flushPromises();
    expect(wrapper.find(".nvx-terminal-extra-keys").exists()).toBe(false);
  });

  it("hides the navigation row on every page while the Activity reports the soft keyboard open", async () => {
    const ime = (visible: boolean) => {
      if (visible) document.documentElement.setAttribute("data-nvx-ime", "visible");
      else document.documentElement.removeAttribute("data-nvx-ime");
      window.dispatchEvent(new CustomEvent("norishell:ime-visibility", { detail: { visible } }));
    };
    try {
      // A form page: only the navigation goes, nothing else is rendered.
      const { wrapper, router } = await mountNav("/hosts");
      expect(wrapper.find("nav").exists()).toBe(true);
      ime(true);
      await flushPromises();
      expect(wrapper.find("nav").exists()).toBe(false);
      ime(false);
      await flushPromises();
      expect(wrapper.find("nav").exists()).toBe(true);
      // The Terminal page in portrait keeps the helper keys directly above the keyboard.
      await router.push("/terminal");
      useWorkspaceTabsStore().syncTerminalState({ tabs: [{ groupId: "t1", label: "web", stateLabel: "Running" }],
        activeTabId: "t1", busy: false, activationBlocked: false, quickCommandsOpen: false });
      await flushPromises();
      ime(true);
      await flushPromises();
      expect(wrapper.find("nav").exists()).toBe(false);
      expect(wrapper.find(".nvx-mobile-nav__handle").exists()).toBe(false);
      expect(wrapper.find(".nvx-terminal-extra-keys").exists()).toBe(true);
      wrapper.unmount();
      // A remount while the keyboard is already open reads the attribute.
      const { wrapper: reopened } = await mountNav("/settings");
      expect(reopened.find("nav").exists()).toBe(false);
      reopened.unmount();
    } finally {
      ime(false);
    }
  });

  it("drops even the handle in landscape while the soft keyboard is open, keeping only the helper keys", async () => {
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      value: () => ({ matches: true, addEventListener() { /* landscape stub */ }, removeEventListener() { /* landscape stub */ } }),
    });
    const listeners = new Set<() => void>();
    const viewport = { height: 364, addEventListener: (_: string, fn: () => void) => listeners.add(fn), removeEventListener: (_: string, fn: () => void) => listeners.delete(fn) };
    Object.defineProperty(window, "visualViewport", { configurable: true, value: viewport });
    try {
      const { wrapper } = await mountNav("/terminal");
      useWorkspaceTabsStore().syncTerminalState({ tabs: [{ groupId: "t1", label: "web", stateLabel: "Running" }],
        activeTabId: "t1", busy: false, activationBlocked: false, quickCommandsOpen: false });
      await flushPromises();
      expect(wrapper.find(".nvx-mobile-nav__handle").exists()).toBe(true);
      await wrapper.find(".nvx-mobile-nav__handle").trigger("click");
      expect(wrapper.find("nav").exists()).toBe(true);
      viewport.height = 175;
      for (const fn of listeners) fn();
      await flushPromises();
      expect(wrapper.find(".nvx-mobile-nav__handle").exists()).toBe(false);
      expect(wrapper.find("nav").exists()).toBe(false);
      expect(wrapper.find(".nvx-terminal-extra-keys").exists()).toBe(true);
      viewport.height = 364;
      for (const fn of listeners) fn();
      await flushPromises();
      expect(wrapper.find(".nvx-mobile-nav__handle").exists()).toBe(true);
      wrapper.unmount();
    } finally {
      Object.defineProperty(window, "visualViewport", { configurable: true, value: undefined });
    }
  });

  it("collapses the navigation behind a handle in landscape with a Terminal Tab and keeps the helper keys", async () => {
    const listeners = new Set<() => void>();
    let landscape = false;
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      value: () => ({
        get matches() { return landscape; },
        addEventListener: (_: string, fn: () => void) => listeners.add(fn),
        removeEventListener: (_: string, fn: () => void) => listeners.delete(fn),
      }),
    });
    const rotate = (next: boolean) => { landscape = next; for (const fn of listeners) fn(); };
    try {
      const { wrapper, router } = await mountNav("/terminal");
      useWorkspaceTabsStore().syncTerminalState({ tabs: [{ groupId: "t1", label: "web", stateLabel: "Running" }],
        activeTabId: "t1", busy: false, activationBlocked: false, quickCommandsOpen: false });
      await flushPromises();
      // Portrait: unchanged.
      expect(wrapper.find("nav").exists()).toBe(true);
      expect(wrapper.find(".nvx-mobile-nav__handle").exists()).toBe(false);
      rotate(true);
      await flushPromises();
      expect(wrapper.find("nav").exists()).toBe(false);
      expect(wrapper.find(".nvx-terminal-extra-keys").exists()).toBe(true);
      const handle = wrapper.find(".nvx-mobile-nav__handle");
      expect(handle.attributes("aria-label")).toBe("Show navigation");
      expect(handle.attributes("aria-expanded")).toBe("false");
      // The handle reveals the navigation temporarily; a tap on an item collapses it again.
      await handle.trigger("click");
      expect(wrapper.find("nav").exists()).toBe(true);
      expect(wrapper.find(".nvx-mobile-nav__handle").attributes("aria-label")).toBe("Hide navigation");
      await wrapper.findAll("nav button")[1]!.trigger("click");
      await flushPromises();
      expect(router.currentRoute.value.path).toBe("/terminal");
      expect(wrapper.find("nav").exists()).toBe(false);
      // Other pages in landscape keep the full navigation without a handle.
      await wrapper.find(".nvx-mobile-nav__handle").trigger("click");
      await wrapper.findAll("nav button")[2]!.trigger("click");
      await flushPromises();
      expect(router.currentRoute.value.path).toBe("/hosts");
      expect(wrapper.find("nav").exists()).toBe(true);
      expect(wrapper.find(".nvx-mobile-nav__handle").exists()).toBe(false);
      await router.push("/terminal");
      await flushPromises();
      expect(wrapper.find("nav").exists()).toBe(false);
      rotate(false);
      await flushPromises();
      expect(wrapper.find("nav").exists()).toBe(true);
      expect(wrapper.find(".nvx-mobile-nav__handle").exists()).toBe(false);
      wrapper.unmount();
      expect(listeners.size).toBe(0);
    } finally {
      Object.defineProperty(window, "matchMedia", { configurable: true, value: undefined });
    }
  });
});
