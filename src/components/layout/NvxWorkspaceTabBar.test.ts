import { flushPromises, mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { createMemoryHistory, createRouter } from "vue-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { i18n } from "../../locales";
import { useShortcutsStore } from "../../stores/shortcuts";
import { useUiStore } from "../../stores/ui";
import { useWorkspaceTabsStore } from "../../stores/workspaceTabs";
import {
  retainWorkspaceTabViewSummaries,
  setActiveWorkspaceTabView,
  setPendingWorkspaceTabView,
  setWorkspaceTabViewSummary,
  type WorkspaceTabViewSummary,
} from "../../workspace-tab-view-state";
import { NvxPluginExtensionTarget } from "../plugins";
import NvxWorkspaceTabBar from "./NvxWorkspaceTabBar.vue";

const shell = vi.hoisted(() => ({
  activate: vi.fn(async () => undefined),
  close: vi.fn(async () => true),
  page: vi.fn(async () => true),
  file: vi.fn(async () => "file:new"),
  terminal: vi.fn(async () => "terminal:new"),
  emitTo: vi.fn(async () => undefined),
}));
vi.mock("@tauri-apps/api/event", () => ({ emitTo: shell.emitTo, listen: vi.fn(async () => () => undefined) }));
vi.mock("../../workspace-tab-view-shell", () => ({
  activateWorkspaceTabView: shell.activate,
  requestCloseWorkspaceTabView: shell.close,
  createManagedPageForRoute: shell.page,
  createManagedFileTab: shell.file,
  createManagedTerminalTab: shell.terminal,
}));
vi.mock("../../workspace-tab-windows", () => ({
  workspaceWindowLabel: () => "main",
  beginWorkspaceTabDrag: vi.fn(),
  cancelWorkspaceTabDrag: vi.fn(async () => undefined),
  finishWorkspaceTabDrag: vi.fn(async () => undefined),
}));
vi.mock("../../workspace-tab-transfer", () => ({}));

class ResizeObserverStub {
  observe = vi.fn();
  disconnect = vi.fn();
}

function summary(id: string, kind: WorkspaceTabViewSummary["kind"] = "terminal"): WorkspaceTabViewSummary {
  return { id, viewLabel: `view-${id}`, kind, label: id, stateLabel: "" };
}

async function mountBar(path = "/terminal", before?: () => Promise<void>) {
  const pinia = createPinia();
  setActivePinia(pinia);
  await before?.();
  const router = createRouter({
    history: createMemoryHistory(),
    routes: ["/terminal", "/new", "/sftp", "/settings"].map((route) => ({ path: route, component: { template: "<div />" } })),
  });
  await router.push(path);
  await router.isReady();
  const wrapper = mount(NvxWorkspaceTabBar, { attachTo: document.body, global: { plugins: [pinia, router, i18n] } });
  await flushPromises();
  return { wrapper, router };
}

function press(init: KeyboardEventInit, target: EventTarget = window) {
  const event = new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init });
  target.dispatchEvent(event);
  return event;
}

describe("NvxWorkspaceTabBar", () => {
  beforeEach(() => {
    vi.stubGlobal("ResizeObserver", ResizeObserverStub);
    i18n.global.locale.value = "zh-CN";
    for (const mock of Object.values(shell)) mock.mockClear();
    retainWorkspaceTabViewSummaries(new Set());
    setActiveWorkspaceTabView(null);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    retainWorkspaceTabViewSummaries(new Set());
    document.body.innerHTML = "";
  });

  it("shows one Header item per native Tab and routes selection and close to its view", async () => {
    for (const item of [summary("terminal-1"), summary("file-1", "file"), summary("page-1", "page")]) setWorkspaceTabViewSummary(item);
    setActiveWorkspaceTabView("file-1");
    const { wrapper } = await mountBar();
    const tabs = wrapper.findAll('[role="tab"]');
    expect(tabs.map((tab) => tab.text())).toEqual([
      expect.stringContaining("terminal-1"), expect.stringContaining("file-1"), expect.stringContaining("page-1"),
    ]);
    expect(tabs[1]!.attributes("aria-selected")).toBe("true");
    await tabs[0]!.trigger("click");
    expect(shell.activate).toHaveBeenCalledWith("terminal-1");
    await wrapper.findAll(".nvx-terminal-tab-bar__close")[2]!.trigger("click");
    expect(shell.close).toHaveBeenCalledWith("page-1");
    wrapper.unmount();
  });

  it.each([
    ["welcome", () => expect(shell.page).toHaveBeenCalledWith("/new")],
    ["sftpWelcome", () => expect(shell.file).toHaveBeenCalledWith("remote")],
    ["localTerminal", () => expect(shell.terminal).toHaveBeenCalledWith("local")],
    ["terminalWelcome", () => expect(shell.terminal).toHaveBeenCalledWith("welcome")],
  ] as const)("creates the %s preference as one managed Tab", async (behavior, expectCreated) => {
    const { wrapper } = await mountBar();
    useUiStore().newTerminalBehavior = behavior;
    await wrapper.get(".nvx-terminal-tab-bar__create").trigger("click");
    expectCreated();
    wrapper.unmount();
  });

  it("turns a /new shell route into a Page Tab without leaving a shell page behind", async () => {
    const { wrapper, router } = await mountBar("/new");
    await flushPromises();
    expect(shell.page).toHaveBeenCalledOnce();
    expect(router.currentRoute.value.path).toBe("/terminal");
    wrapper.unmount();
  });

  it.each([
    { platform: "MacIntel", modifier: { metaKey: true }, wrongModifier: { ctrlKey: true } },
    { platform: "Win32", modifier: { ctrlKey: true }, wrongModifier: { metaKey: true } },
  ])("uses platform-standard new, close and numbered Tab shortcuts on $platform", async ({ platform, modifier, wrongModifier }) => {
    Object.defineProperty(navigator, "platform", { configurable: true, value: platform });
    for (const item of [summary("terminal-1"), summary("terminal-2")]) setWorkspaceTabViewSummary(item);
    setActiveWorkspaceTabView("terminal-1");
    const { wrapper } = await mountBar();
    useUiStore().newTerminalBehavior = "terminalWelcome";

    expect(press({ key: "t", ...wrongModifier }).defaultPrevented).toBe(false);
    expect(shell.terminal).not.toHaveBeenCalled();
    expect(press({ key: "t", ...modifier }).defaultPrevented).toBe(true);
    expect(shell.terminal).toHaveBeenCalledWith("welcome");
    expect(press({ key: "w", ...modifier }).defaultPrevented).toBe(true);
    expect(shell.close).toHaveBeenCalledWith("terminal-1");
    expect(press({ key: "2", code: "Digit2", ...modifier }).defaultPrevented).toBe(true);
    expect(shell.activate).toHaveBeenCalledWith("terminal-2");
    wrapper.unmount();
  });

  it("consumes reserved Tab shortcuts without acting behind a blocking dialog", async () => {
    Object.defineProperty(navigator, "platform", { configurable: true, value: "MacIntel" });
    setWorkspaceTabViewSummary(summary("terminal-1"));
    setActiveWorkspaceTabView("terminal-1");
    const { wrapper } = await mountBar();
    useWorkspaceTabsStore().syncTerminalState({ tabs: [], activeTabId: "", busy: false, activationBlocked: true, quickCommandsOpen: false });
    for (const key of ["t", "w"]) expect(press({ key, metaKey: true }).defaultPrevented).toBe(true);
    expect(shell.terminal).not.toHaveBeenCalled();
    expect(shell.close).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("uses custom bindings at window capture while leaving ordinary fields alone", async () => {
    Object.defineProperty(navigator, "platform", { configurable: true, value: "MacIntel" });
    const { wrapper, router } = await mountBar("/terminal", async () => {
      const shortcuts = useShortcutsStore();
      expect(await shortcuts.setBinding("macos", "navigation.sftp", "Meta+Shift+KeyX")).toEqual({ ok: true });
      expect(await shortcuts.setBinding("macos", "workspace.new-local", "Meta+Shift+KeyN")).toEqual({ ok: true });
    });

    const field = document.createElement("input");
    document.body.append(field);
    expect(press({ code: "KeyX", metaKey: true, shiftKey: true }, field).defaultPrevented).toBe(false);
    expect(router.currentRoute.value.path).toBe("/terminal");

    expect(press({ code: "KeyX", metaKey: true, shiftKey: true }).defaultPrevented).toBe(true);
    await flushPromises();
    expect(router.currentRoute.value.path).toBe("/sftp");
    expect(press({ code: "KeyN", metaKey: true, shiftKey: true }).defaultPrevented).toBe(true);
    expect(shell.terminal).toHaveBeenCalledWith("local");
    wrapper.unmount();
  });

  it("mounts the controlled plugin action target immediately to the right of the create button", async () => {
    const { wrapper } = await mountBar();
    const target = wrapper.getComponent(NvxPluginExtensionTarget);
    expect(target.props("targetId")).toBe("app.header.actions");
    expect(target.props("showIdentity")).toBe(false);
    expect(wrapper.get(".nvx-terminal-tab-bar__create").element.compareDocumentPosition(
      target.element,
    ) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    wrapper.unmount();
  });

  it("toggles Quick Commands in the active Terminal Tab instead of the covered shell page", async () => {
    const { wrapper } = await mountBar();
    expect(wrapper.find('button[aria-label="显示快捷命令"]').exists()).toBe(false);
    setWorkspaceTabViewSummary({ ...summary("terminal-1"), quickCommandsOpen: false });
    setActiveWorkspaceTabView("terminal-1");
    await flushPromises();
    await wrapper.get('button[aria-label="显示快捷命令"]').trigger("click");
    expect(shell.emitTo).toHaveBeenCalledWith("view-terminal-1", "workspace-tab-view-toggle-quick-commands", { id: "terminal-1" });
    setWorkspaceTabViewSummary({ ...summary("terminal-1"), quickCommandsOpen: true });
    await flushPromises();
    expect(wrapper.find('button[aria-label="收起快捷命令"]').exists()).toBe(true);
    // No writable terminal exists while the Tab is still rendering.
    setPendingWorkspaceTabView("terminal-1");
    await flushPromises();
    expect(wrapper.find('button[aria-label="收起快捷命令"]').exists()).toBe(false);
    setPendingWorkspaceTabView(null);
    setWorkspaceTabViewSummary(summary("page-1", "page"));
    setActiveWorkspaceTabView("page-1");
    await flushPromises();
    expect(wrapper.find('button[aria-label="显示快捷命令"]').exists()).toBe(false);
    wrapper.unmount();
  });
});
