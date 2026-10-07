import { createPinia, setActivePinia } from "pinia";
import { createMemoryHistory, createRouter } from "vue-router";
import { beforeEach, describe, expect, it, vi } from "vitest";

// Android: Core IPC exists, but every Workspace Tab lives in the single main document.
const native = vi.hoisted(() => ({
  invoke: vi.fn(async (command: string) => { throw new Error(`unexpected native command ${command}`); }),
}));
vi.mock("@tauri-apps/api/core", async (importOriginal) => ({
  ...await importOriginal<typeof import("@tauri-apps/api/core")>(),
  isTauri: () => true,
  invoke: native.invoke,
}));
vi.mock("./platform", async (importOriginal) => ({
  ...await importOriginal<typeof import("./platform")>(),
  detectDesktopPlatform: () => "android",
}));
vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn(async () => undefined),
  emitTo: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}));

import { workspaceHostMode } from "./platform-host";
import { setSingleDocumentRouter } from "./single-document-workspace";
import { useWorkspaceTabsStore, type TerminalHeaderController } from "./stores/workspaceTabs";
import { startWorkspaceTabWindowUi } from "./workspace-tab-window-ui";
import {
  activateWorkspaceTabView, createManagedApprovedPluginChannel, createManagedDesktopForProfile, createManagedFileForFocus,
  createManagedFileTab, createManagedPageForRoute, createManagedPageTab, createManagedPluginProtocolLaunch,
  createManagedQuickConnect, createManagedTelnet, createManagedTerminalForDirectory, createManagedTerminalForHost,
  createManagedTerminalTab, createManagedWorkspaceTab, deactivateWorkspaceTabView, focusManagedDesktopSession,
  focusManagedFileSession, focusManagedTerminalSession, openManagedPluginPage, openManagedTransferTarget,
  performWorkspaceTabShellAction, requestCloseWorkspaceTabView, setWorkspaceTabViewContentBounds,
  startWorkspaceTabViewShell, suspendWorkspaceTabViews,
} from "./workspace-tab-view-shell";

/** Every command a native Tab WebView manager uses; none of them may run in a single document. */
const NATIVE_TAB_COMMANDS = [
  "create_tab_view", "workspace_tab_view_get", "workspace_tab_context_get", "workspace_tab_bootstrap_take",
  "set_tab_view_bounds", "set_tab_view_visible", "focus_tab_view", "move_tab_view", "close_tab_view",
  "close_own_tab_view", "workspace_window_open", "workspace_window_focus", "workspace_window_close",
  "workspace_window_close_empty", "workspace_tab_projection_update", "workspace_tab_drag_begin",
  "workspace_tab_drag_cancel", "workspace_tab_drag_finish", "workspace_tab_snapshot",
];

function nativeTabCommandCalls() {
  return native.invoke.mock.calls.map(([command]) => command).filter((command) => NATIVE_TAB_COMMANDS.includes(command));
}

const router = () => createRouter({ history: createMemoryHistory(), routes: [{ path: "/:pathMatch(.*)*", component: { template: "<div />" } }] });

describe("native Tab manager in a single document", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    native.invoke.mockClear();
    setSingleDocumentRouter(null);
  });

  it("is the Android host mode", () => {
    expect(workspaceHostMode()).toBe("singleDocument");
  });

  const pluginItem = { pluginId: "p", pageId: "home", label: "Plugin" } as never;
  const launch = { launchId: "l", revision: "1", tabId: "t", paneId: "p", claimed: false, expiresAtUnixMs: Date.now() + 60_000 } as never;
  const channel = { operationId: "op", authorizationToken: "token", label: "pod" } as never;
  it.each([
    ["activateWorkspaceTabView", () => activateWorkspaceTabView("t")],
    ["deactivateWorkspaceTabView", () => deactivateWorkspaceTabView()],
    ["requestCloseWorkspaceTabView", () => requestCloseWorkspaceTabView("t")],
    ["createManagedWorkspaceTab", () => createManagedWorkspaceTab("t", "terminal", "/terminal", {}, {})],
    ["createManagedTerminalTab", () => createManagedTerminalTab("local")],
    ["createManagedTerminalForHost", () => createManagedTerminalForHost({ hostId: "h" })],
    ["createManagedTerminalForDirectory", () => createManagedTerminalForDirectory("h", [47])],
    ["createManagedFileTab", () => createManagedFileTab("local")],
    ["createManagedFileForFocus", () => createManagedFileForFocus({ focusOperation: "o" })],
    ["openManagedPluginPage", () => openManagedPluginPage(pluginItem)],
    ["createManagedPageTab", () => createManagedPageTab({ groupId: "page:newPage", pageType: "newPage", route: "/new", labelKey: null, label: "", iconName: null } as never)],
    ["createManagedPageForRoute", () => createManagedPageForRoute("/new")],
    ["createManagedDesktopForProfile", () => createManagedDesktopForProfile({ id: "d", label: "Desktop" })],
    ["createManagedPluginProtocolLaunch", () => createManagedPluginProtocolLaunch(launch)],
    ["createManagedApprovedPluginChannel", () => createManagedApprovedPluginChannel(channel)],
    ["createManagedQuickConnect", () => createManagedQuickConnect("host")],
    ["createManagedTelnet", () => createManagedTelnet()],
    ["focusManagedDesktopSession", () => focusManagedDesktopSession("s", "1")],
    ["focusManagedFileSession", () => focusManagedFileSession("s", "1")],
    ["openManagedTransferTarget", () => openManagedTransferTarget({}, null)],
    ["startWorkspaceTabViewShell", () => startWorkspaceTabViewShell(router())],
  ] as const)("%s fails closed without a native Tab command", async (_name, run) => {
    await expect(run()).rejects.toThrow("workspace_tab.native_views_unsupported");
    expect(nativeTabCommandCalls()).toEqual([]);
  });

  it("keeps visibility, suspension and the window UI inert", async () => {
    setWorkspaceTabViewContentBounds({ x: 0, y: 0, width: 10, height: 10 });
    suspendWorkspaceTabViews()();
    const stop = await startWorkspaceTabWindowUi(router());
    stop();
    await Promise.resolve();
    expect(nativeTabCommandCalls()).toEqual([]);
  });

  it("routes shell actions and Overview session focus through this document", async () => {
    const shell = router();
    await shell.push("/overview");
    setSingleDocumentRouter(shell);
    const focusSshSession = vi.fn(() => true);
    useWorkspaceTabsStore().registerTerminalController({
      activate: vi.fn(), close: vi.fn(() => true), closeMany: vi.fn(() => true), quickConnect: vi.fn(() => true),
      openTelnet: vi.fn(() => true), deactivate: vi.fn(), toggleQuickCommands: vi.fn(),
      focusNativeSession: vi.fn(() => false), focusSshSession, focusLocalSession: vi.fn(() => false),
      focusTelnetSession: vi.fn(() => false),
    } satisfies TerminalHeaderController);
    await focusManagedTerminalSession({ kind: "focusSshSession", sessionId: "s", generation: "2" });
    expect(focusSshSession).toHaveBeenCalledWith("s", "2");
    expect(shell.currentRoute.value.path).toBe("/terminal");
    await expect(performWorkspaceTabShellAction({ type: "navigate", path: "/hosts" })).resolves.toBeNull();
    expect(shell.currentRoute.value.path).toBe("/hosts");
    expect(nativeTabCommandCalls()).toEqual([]);
  });
});
