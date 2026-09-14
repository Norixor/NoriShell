import { flushPromises, mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { createMemoryHistory, createRouter } from "vue-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { i18n } from "../../locales";
import { useShortcutsStore } from "../../stores/shortcuts";
import { type TerminalHeaderController, useWorkspaceTabsStore } from "../../stores/workspaceTabs";
import { NvxPluginExtensionTarget } from "../plugins";
import NvxWorkspaceTabBar from "./NvxWorkspaceTabBar.vue";

class ResizeObserverStub {
  observe = vi.fn();
  disconnect = vi.fn();
}

function routerForTests() {
  return createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: "/terminal", component: { template: "<div />" } },
      { path: "/desktop", component: { template: "<div />" } },
      { path: "/sftp", component: { template: "<div />" } },
      { path: "/settings", component: { template: "<div />" } },
      { path: "/settings/identities", component: { template: "<div />" } },
      { path: "/known-hosts", component: { template: "<div />" } },
    ],
  });
}

async function mountMixedWorkspaceTabs() {
  const pinia = createPinia();
  setActivePinia(pinia);
  const router = routerForTests();
  await router.push("/terminal");
  await router.isReady();
  const workspaceTabs = useWorkspaceTabsStore();
  const terminal = terminalControllerStub({
    activate: vi.fn(() => { void router.push("/terminal"); }),
  });
  const desktop = {
    activate: vi.fn(() => { void router.push("/desktop"); }),
    close: vi.fn(), closeMany: vi.fn(), deactivate: vi.fn(),
  };
  workspaceTabs.registerTerminalController(terminal);
  workspaceTabs.registerDesktopController(desktop);
  workspaceTabs.syncTerminalState({
    tabs: [{ groupId: "terminal-1", label: "Terminal fixture", stateLabel: "Running" }],
    activeTabId: "terminal-1", busy: false, activationBlocked: false, quickCommandsOpen: false,
  });
  workspaceTabs.ensurePageTabForRoute("/settings/identities");
  workspaceTabs.syncDesktopState({
    tabs: [{ groupId: "desktop:session-1", label: "Desktop fixture", stateLabel: "Running" }],
    activeTabId: "desktop:session-1", busy: false,
  });
  const wrapper = mount(NvxWorkspaceTabBar, {
    attachTo: document.body,
    global: { plugins: [pinia, router, i18n] },
  });
  await flushPromises();
  return { wrapper, router, terminal, desktop };
}

function terminalControllerStub(overrides: Partial<TerminalHeaderController> = {}) {
  return {
    activate: vi.fn(),
    close: vi.fn(),
    closeMany: vi.fn(),
    create: vi.fn(),
    createLocal: vi.fn(() => true),
    quickConnect: vi.fn(() => true),
    focusNativeSession: vi.fn(() => true),
    focusSshSession: vi.fn(() => true),
    focusLocalSession: vi.fn(() => true),
    deactivate: vi.fn(),
    toggleQuickCommands: vi.fn(),
    focusTelnetSession: vi.fn(() => true),
    ...overrides,
  } satisfies TerminalHeaderController;
}

function expectNoSessionLifecycleChanges(terminal: Awaited<ReturnType<typeof mountMixedWorkspaceTabs>>["terminal"], desktop: Awaited<ReturnType<typeof mountMixedWorkspaceTabs>>["desktop"]) {
  expect(terminal.create).not.toHaveBeenCalled();
  expect(terminal.close).not.toHaveBeenCalled();
  expect(terminal.closeMany).not.toHaveBeenCalled();
  expect(desktop.close).not.toHaveBeenCalled();
  expect(desktop.closeMany).not.toHaveBeenCalled();
}

describe("NvxWorkspaceTabBar", () => {
  beforeEach(() => {
    vi.stubGlobal("ResizeObserver", ResizeObserverStub);
    i18n.global.locale.value = "zh-CN";
  });

  afterEach(() => vi.unstubAllGlobals());

  it("tracks a settings child page alongside terminal tabs and reuses it", async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const router = routerForTests();
    await router.push("/settings/identities");
    await router.isReady();
    const workspaceTabs = useWorkspaceTabsStore();
    const controller = terminalControllerStub();
    workspaceTabs.registerTerminalController(controller);
    workspaceTabs.syncTerminalState({
      tabs: [{ groupId: "terminal-1", label: "ops@example.test", stateLabel: "运行中 · 1 个 Pane" }],
      activeTabId: "terminal-1",
      busy: false,
      activationBlocked: false,
      quickCommandsOpen: false,
    });

    const wrapper = mount(NvxWorkspaceTabBar, {
      global: { plugins: [pinia, router, i18n] },
    });
    await wrapper.vm.$nextTick();

    expect(wrapper.findAll("[role='tab']")).toHaveLength(2);
    expect(wrapper.text()).toContain("SSH 身份");
    expect(wrapper.text()).toContain("页面");

    await router.push("/known-hosts");
    await wrapper.vm.$nextTick();
    expect(wrapper.findAll("[role='tab']")).toHaveLength(3);
    expect(wrapper.text()).toContain("服务器主机密钥");

    await router.push("/settings/identities");
    await wrapper.vm.$nextTick();
    expect(wrapper.findAll("[role='tab']")).toHaveLength(3);

    await wrapper.findAll("[role='tab']")[0]!.trigger("click");
    expect(controller.activate).toHaveBeenCalledWith("terminal-1");
    wrapper.unmount();
  });

  it("switches Desktop, Terminal and Page tabs through their own controllers without changing session lifecycle", async () => {
    const { wrapper, router, terminal, desktop } = await mountMixedWorkspaceTabs();
    try {
      const tabs = wrapper.findAll("[role='tab']");
      expect(tabs).toHaveLength(3);
      await tabs[2]!.trigger("click");
      await flushPromises();
      expect(desktop.activate).toHaveBeenCalledWith("desktop:session-1");
      expect(terminal.deactivate).toHaveBeenCalledOnce();
      expect(terminal.activate).not.toHaveBeenCalled();
      expect(router.currentRoute.value.path).toBe("/desktop");
      expect(tabs[2]!.attributes("aria-selected")).toBe("true");

      await tabs[0]!.trigger("click");
      await flushPromises();
      expect(terminal.activate).toHaveBeenCalledWith("terminal-1");
      expect(desktop.deactivate).toHaveBeenCalledOnce();
      expect(router.currentRoute.value.path).toBe("/terminal");
      expect(tabs[0]!.attributes("aria-selected")).toBe("true");

      await tabs[2]!.trigger("click");
      await flushPromises();
      await tabs[1]!.trigger("click");
      await flushPromises();
      expect(router.currentRoute.value.path).toBe("/settings/identities");
      expect(tabs[1]!.attributes("aria-selected")).toBe("true");
      expect(desktop.deactivate).toHaveBeenCalledTimes(2);
      expect(terminal.deactivate).toHaveBeenCalledTimes(3);
      expect(terminal.activate).toHaveBeenCalledOnce();
      expect(desktop.activate).toHaveBeenCalledTimes(2);
      expectNoSessionLifecycleChanges(terminal, desktop);
    } finally { wrapper.unmount(); }
  });

  it.each([
    { platform: "MacIntel", modifier: { metaKey: true } },
    { platform: "Win32", modifier: { ctrlKey: true } },
  ])("reserves numbered tabs at the desktop input surface on $platform", async ({ platform, modifier }) => {
    Object.defineProperty(navigator, "platform", { configurable: true, value: platform });
    const { wrapper, router, terminal, desktop } = await mountMixedWorkspaceTabs();
    const input = document.createElement("textarea");
    input.dataset.desktopInput = "";
    const remoteInput = vi.fn();
    input.addEventListener("keydown", remoteInput);
    document.body.append(input);
    try {
      const dispatch = async (digit: number) => {
        const event = new KeyboardEvent("keydown", { code: `Digit${digit}`, ...modifier, bubbles: true, cancelable: true });
        input.dispatchEvent(event);
        await flushPromises();
        expect(event.defaultPrevented).toBe(true);
      };
      await dispatch(3);
      expect(desktop.activate).toHaveBeenCalledWith("desktop:session-1");
      expect(terminal.deactivate).toHaveBeenCalledOnce();
      expect(router.currentRoute.value.path).toBe("/desktop");
      await dispatch(9);
      expect(desktop.activate).toHaveBeenCalledOnce();
      expect(router.currentRoute.value.path).toBe("/desktop");
      await dispatch(1);
      expect(terminal.activate).toHaveBeenCalledWith("terminal-1");
      expect(desktop.deactivate).toHaveBeenCalledOnce();
      expect(router.currentRoute.value.path).toBe("/terminal");
      await dispatch(2);
      expect(router.currentRoute.value.path).toBe("/settings/identities");
      expect(remoteInput).not.toHaveBeenCalled();
      expectNoSessionLifecycleChanges(terminal, desktop);
    } finally { input.remove(); wrapper.unmount(); }
  });

  it("queues a new terminal until the terminal workspace registers", async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const router = routerForTests();
    await router.push("/settings");
    await router.isReady();
    const workspaceTabs = useWorkspaceTabsStore();
    const wrapper = mount(NvxWorkspaceTabBar, {
      global: { plugins: [pinia, router, i18n] },
    });

    await wrapper.get(".nvx-terminal-tab-bar__create").trigger("click");
    await flushPromises();
    expect(router.currentRoute.value.path).toBe("/terminal");

    const create = vi.fn();
    workspaceTabs.registerTerminalController(terminalControllerStub({ create }));
    expect(create).toHaveBeenCalledOnce();
    wrapper.unmount();
  });

  it.each([
    { platform: "MacIntel", modifier: { metaKey: true }, wrongModifier: { ctrlKey: true } },
    { platform: "Win32", modifier: { ctrlKey: true }, wrongModifier: { metaKey: true } },
  ])("uses platform-standard new and close Tab shortcuts on $platform", async ({
    platform,
    modifier,
    wrongModifier,
  }) => {
    Object.defineProperty(navigator, "platform", { configurable: true, value: platform });
    const pinia = createPinia();
    setActivePinia(pinia);
    const router = routerForTests();
    await router.push("/terminal");
    await router.isReady();
    const workspaceTabs = useWorkspaceTabsStore();
    const controller = terminalControllerStub();
    workspaceTabs.registerTerminalController(controller);
    workspaceTabs.syncTerminalState({
      tabs: [{ groupId: "terminal-1", label: "ops", stateLabel: "运行中" }],
      activeTabId: "terminal-1",
      busy: false,
      activationBlocked: false,
      quickCommandsOpen: false,
    });
    const wrapper = mount(NvxWorkspaceTabBar, {
      global: { plugins: [pinia, router, i18n] },
    });

    const wrongNewTab = new KeyboardEvent("keydown", {
      key: "t",
      ...wrongModifier,
      cancelable: true,
    });
    window.dispatchEvent(wrongNewTab);
    expect(wrongNewTab.defaultPrevented).toBe(false);
    expect(controller.create).not.toHaveBeenCalled();

    const newTab = new KeyboardEvent("keydown", {
      key: "t",
      ...modifier,
      cancelable: true,
    });
    window.dispatchEvent(newTab);
    expect(newTab.defaultPrevented).toBe(true);
    expect(controller.create).toHaveBeenCalledOnce();

    const closeTab = new KeyboardEvent("keydown", {
      key: "w",
      ...modifier,
      cancelable: true,
    });
    window.dispatchEvent(closeTab);
    expect(closeTab.defaultPrevented).toBe(true);
    expect(controller.close).toHaveBeenCalledWith("terminal-1");
    wrapper.unmount();
  });

  it("consumes reserved Tab shortcuts without acting behind a blocking dialog", async () => {
    Object.defineProperty(navigator, "platform", { configurable: true, value: "MacIntel" });
    const pinia = createPinia();
    setActivePinia(pinia);
    const router = routerForTests();
    await router.push("/terminal");
    await router.isReady();
    const workspaceTabs = useWorkspaceTabsStore();
    const controller = terminalControllerStub();
    workspaceTabs.registerTerminalController(controller);
    workspaceTabs.syncTerminalState({
      tabs: [{ groupId: "terminal-1", label: "ops", stateLabel: "运行中" }],
      activeTabId: "terminal-1",
      busy: false,
      activationBlocked: true,
      quickCommandsOpen: false,
    });
    const wrapper = mount(NvxWorkspaceTabBar, {
      global: { plugins: [pinia, router, i18n] },
    });

    for (const key of ["t", "w"]) {
      const event = new KeyboardEvent("keydown", {
        key,
        metaKey: true,
        cancelable: true,
      });
      window.dispatchEvent(event);
      expect(event.defaultPrevented).toBe(true);
    }
    expect(controller.create).not.toHaveBeenCalled();
    expect(controller.close).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("uses custom bindings at window capture, while leaving ordinary fields alone and routing terminal-only actions through the controller", async () => {
    Object.defineProperty(navigator, "platform", { configurable: true, value: "MacIntel" });
    const pinia = createPinia();
    setActivePinia(pinia);
    const router = routerForTests();
    await router.push("/terminal");
    await router.isReady();
    const workspaceTabs = useWorkspaceTabsStore();
    const controller = terminalControllerStub({ runShortcut: vi.fn() });
    workspaceTabs.registerTerminalController(controller);
    workspaceTabs.syncTerminalState({
      tabs: [{ groupId: "terminal-1", label: "ops", stateLabel: "running" }],
      activeTabId: "terminal-1",
      busy: false,
      activationBlocked: false,
      quickCommandsOpen: false,
    });
    const shortcuts = useShortcutsStore();
    expect(shortcuts.setBinding("macos", "navigation.sftp", "Meta+Shift+KeyX")).toEqual({ ok: true });
    expect(shortcuts.setBinding("macos", "workspace.new-local", "Meta+Shift+KeyN")).toEqual({ ok: true });
    const wrapper = mount(NvxWorkspaceTabBar, {
      attachTo: document.body,
      global: { plugins: [pinia, router, i18n] },
    });

    const field = document.createElement("input");
    document.body.append(field);
    const typed = new KeyboardEvent("keydown", {
      code: "KeyX", metaKey: true, shiftKey: true, bubbles: true, cancelable: true,
    });
    field.dispatchEvent(typed);
    expect(typed.defaultPrevented).toBe(false);
    expect(router.currentRoute.value.path).toBe("/terminal");

    const xterm = document.createElement("div");
    xterm.className = "xterm";
    const helper = document.createElement("textarea");
    helper.className = "xterm-helper-textarea";
    xterm.append(helper);
    document.body.append(xterm);
    const navigate = new KeyboardEvent("keydown", {
      code: "KeyX", metaKey: true, shiftKey: true, bubbles: true, cancelable: true,
    });
    helper.dispatchEvent(navigate);
    await flushPromises();
    expect(navigate.defaultPrevented).toBe(true);
    expect(router.currentRoute.value.path).toBe("/sftp");

    await router.push("/terminal");
    const newLocal = new KeyboardEvent("keydown", {
      code: "KeyN", metaKey: true, shiftKey: true, cancelable: true,
    });
    window.dispatchEvent(newLocal);
    expect(newLocal.defaultPrevented).toBe(true);
    expect(controller.runShortcut).toHaveBeenCalledWith("workspace.new-local");
    field.remove();
    xterm.remove();
    wrapper.unmount();
  });

  it("mounts the controlled plugin action target immediately to the right of the create button", async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const router = routerForTests();
    await router.push("/terminal");
    await router.isReady();
    const wrapper = mount(NvxWorkspaceTabBar, {
      global: { plugins: [pinia, router, i18n] },
    });

    const target = wrapper.getComponent(NvxPluginExtensionTarget);
    expect(target.props("targetId")).toBe("app.header.actions");
    expect(target.props("showIdentity")).toBe(false);
    expect(wrapper.get(".nvx-terminal-tab-bar__create").element.compareDocumentPosition(
      target.element,
    ) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    wrapper.unmount();
  });

  it("closes an active page tab without touching terminal lifecycle", async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const router = routerForTests();
    await router.push("/settings/identities");
    await router.isReady();
    const workspaceTabs = useWorkspaceTabsStore();
    const wrapper = mount(NvxWorkspaceTabBar, {
      global: { plugins: [pinia, router, i18n] },
    });
    await wrapper.vm.$nextTick();

    await wrapper.get(".nvx-terminal-tab-bar__close").trigger("click");
    await flushPromises();

    expect(workspaceTabs.pageTabs).toHaveLength(0);
    expect(router.currentRoute.value.path).toBe("/settings");
    wrapper.unmount();
  });

  it("sends bulk terminal closes to the controller and closes selected page tabs", async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const router = routerForTests();
    await router.push("/settings/identities");
    await router.isReady();
    const workspaceTabs = useWorkspaceTabsStore();
    const controller = terminalControllerStub();
    workspaceTabs.registerTerminalController(controller);
    workspaceTabs.syncTerminalState({
      tabs: [
        { groupId: "terminal-1", label: "one", stateLabel: "运行中" },
        { groupId: "terminal-2", label: "two", stateLabel: "未连接" },
      ],
      activeTabId: "terminal-1",
      busy: false,
      activationBlocked: false,
      quickCommandsOpen: false,
    });
    const wrapper = mount(NvxWorkspaceTabBar, {
      global: { plugins: [pinia, router, i18n] },
    });
    await wrapper.vm.$nextTick();

    await wrapper.findAll(".nvx-terminal-tab-bar__item")[1]?.trigger("contextmenu", {
      clientX: 120,
      clientY: 40,
    });
    await wrapper.get('[role="menu"] [role="menuitem"]:last-child').trigger("click");

    expect(controller.closeMany).toHaveBeenCalledWith(["terminal-1", "terminal-2"]);
    expect(workspaceTabs.pageTabs).toHaveLength(0);
    wrapper.unmount();
  });
});
