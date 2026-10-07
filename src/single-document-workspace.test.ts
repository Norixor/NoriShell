import { createPinia, setActivePinia } from "pinia";
import { createMemoryHistory, createRouter } from "vue-router";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { useUiStore } from "./stores/ui";
import { useWorkspaceTabsStore, workspacePageTabForRoute, type TerminalHeaderController } from "./stores/workspaceTabs";
import {
  activateSingleDocumentTab, activeSingleDocumentTabId, closeSingleDocumentNewPage, closeSingleDocumentTab,
  createSingleDocumentTab, focusSingleDocumentTerminalSession, openSingleDocumentApprovedPluginChannel,
  openSingleDocumentPluginProtocolLaunches, performSingleDocumentShellAction, resetSingleDocumentFilePersistenceForTests,
  restoreSingleDocumentResources, setSingleDocumentRouter, startSingleDocumentFilePersistence, useSingleDocumentTabs,
} from "./single-document-workspace";

const core = vi.hoisted(() => ({
  sftp: vi.fn(async () => ({ snapshotRevision: "0", sessions: [] as unknown[] })),
  hosts: vi.fn(async () => [] as unknown[]),
  desktops: vi.fn(async () => [] as unknown[]),
  ownership: vi.fn(async () => ({ owned: [] as unknown[], others: [] as unknown[] })),
  project: vi.fn<(payload: unknown) => Promise<void>>(async () => undefined),
  register: vi.fn(async (path: string) => ({ directoryRef: `ref:${path}`, revision: "9", displayName: path, rememberablePath: path })),
  release: vi.fn(async () => undefined),
}));
vi.mock("./core-api/client", () => ({ fetchSftpSessionSnapshot: core.sftp, listHosts: core.hosts,
  registerSftpLocalDirectory: core.register, releaseSftpLocalDirectory: core.release, parseCoreApiError: () => null }));
vi.mock("./core-api/desktop-client", () => ({ desktopClient: { snapshot: core.desktops } }));
vi.mock("./workspace-tab-windows", () => ({ snapshotWorkspaceTabs: core.ownership, updateOwnWorkspaceTabProjection: core.project }));

const pluginTerminal = vi.hoisted(() => ({ launches: vi.fn() }));
vi.mock("./core-api/plugin-terminal", () => ({ listPluginProtocolLaunches: pluginTerminal.launches }));

function makeRouter() {
  return createRouter({ history: createMemoryHistory(), routes: [{ path: "/:pathMatch(.*)*", component: { template: "<div />" } }] });
}

function terminalController(overrides: Partial<TerminalHeaderController> = {}): TerminalHeaderController {
  return {
    activate: vi.fn(), close: vi.fn(() => true), closeMany: vi.fn(() => true), quickConnect: vi.fn(() => true),
    openTelnet: vi.fn(() => true), deactivate: vi.fn(), toggleQuickCommands: vi.fn(), createTab: vi.fn(),
    focusNativeSession: vi.fn(() => false), focusSshSession: vi.fn(() => false), focusLocalSession: vi.fn(() => false),
    focusTelnetSession: vi.fn(() => false), ...overrides,
  };
}

describe("single-document workspace tabs", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    setSingleDocumentRouter(null);
    resetSingleDocumentFilePersistenceForTests();
    core.ownership.mockResolvedValue({ owned: [], others: [] });
  });

  it("orders Terminal, File, Desktop and Page Tabs and highlights the active Tab of the current route", () => {
    const store = useWorkspaceTabsStore();
    store.syncTerminalState({ tabs: [{ groupId: "t1", label: "web", stateLabel: "Running", hostId: "h1", bellAttention: true }],
      activeTabId: "t1", busy: false, activationBlocked: false, quickCommandsOpen: false });
    expect(store.createInitialFileTab("file:11111111-1111-1111-1111-111111111111", "local", null, "Local")).toBe(true);
    store.importPageTab(workspacePageTabForRoute("/new")!);
    const { tabs } = useSingleDocumentTabs();
    expect(tabs.value.map((tab) => [tab.kind, tab.id])).toEqual([
      ["terminal", "t1"], ["file", "file:11111111-1111-1111-1111-111111111111"], ["page", "page:newPage"],
    ]);
    expect(tabs.value[0]).toMatchObject({ hostId: "h1", bellAttention: true, compact: false });
    expect(activeSingleDocumentTabId(store, "/terminal")).toBe("t1");
    expect(activeSingleDocumentTabId(store, "/sftp")).toBe("file:11111111-1111-1111-1111-111111111111");
    expect(activeSingleDocumentTabId(store, "/new")).toBe("page:newPage");
    expect(activeSingleDocumentTabId(store, "/hosts")).toBe("");
  });

  it("activates through the owning controller after navigating and revokes terminal focus when leaving it", async () => {
    const store = useWorkspaceTabsStore();
    const router = makeRouter();
    await router.push("/terminal");
    const controller = terminalController();
    store.registerTerminalController(controller);
    store.syncTerminalState({ tabs: [{ groupId: "t1", label: "web", stateLabel: "Running" }], activeTabId: "t1",
      busy: false, activationBlocked: false, quickCommandsOpen: false });
    store.createInitialFileTab("file:11111111-1111-1111-1111-111111111111", "local", null, "Local");
    const { tabs } = useSingleDocumentTabs();
    await activateSingleDocumentTab(store, router, tabs.value[1]!);
    expect(controller.deactivate).toHaveBeenCalledTimes(1);
    expect(router.currentRoute.value.path).toBe("/sftp");
    await activateSingleDocumentTab(store, router, tabs.value[0]!);
    expect(router.currentRoute.value.path).toBe("/terminal");
    expect(controller.activate).toHaveBeenCalledWith("t1");
  });

  it("closes a Terminal Tab only through its controller and drops a Page Tab with its route", async () => {
    const store = useWorkspaceTabsStore();
    const router = makeRouter();
    const controller = terminalController({ close: vi.fn(() => false) });
    store.registerTerminalController(controller);
    store.syncTerminalState({ tabs: [{ groupId: "t1", label: "web", stateLabel: "Running" }], activeTabId: "t1",
      busy: false, activationBlocked: false, quickCommandsOpen: false });
    store.importPageTab(workspacePageTabForRoute("/new")!);
    await router.push("/new");
    const { tabs } = useSingleDocumentTabs();
    expect(await closeSingleDocumentTab(store, router, tabs.value[0]!)).toBe(false);
    expect(controller.close).toHaveBeenCalledWith("t1");
    expect(store.terminalTabs).toHaveLength(1);
    expect(await closeSingleDocumentTab(store, router, tabs.value[1]!)).toBe(true);
    expect(store.pageTabs).toHaveLength(0);
    expect(router.currentRoute.value.path).toBe("/terminal");
  });

  it("creates according to the new-page preference", async () => {
    const store = useWorkspaceTabsStore();
    const ui = useUiStore();
    const router = makeRouter();
    await router.push("/hosts");
    const controller = terminalController();
    store.registerTerminalController(controller);
    ui.newTerminalBehavior = "localTerminal";
    await createSingleDocumentTab(store, router);
    expect(router.currentRoute.value.path).toBe("/terminal");
    expect(controller.createTab).toHaveBeenCalledWith("local");
    ui.newTerminalBehavior = "welcome";
    await createSingleDocumentTab(store, router);
    expect(router.currentRoute.value.path).toBe("/new");
    expect(store.pageTabs.map((tab) => tab.pageType)).toEqual(["newPage"]);
    closeSingleDocumentNewPage();
    expect(store.pageTabs).toHaveLength(0);
  });

  it("performs shell actions in this document, including Desktop Tabs owned by this document", async () => {
    const store = useWorkspaceTabsStore();
    const router = makeRouter();
    await router.push("/hosts");
    await expect(performSingleDocumentShellAction({ type: "navigate", path: "/terminal", query: { hostId: "h1" } }))
      .rejects.toThrow("workspace_tab.manager_unavailable");
    setSingleDocumentRouter(router);
    await performSingleDocumentShellAction({ type: "navigate", path: "/terminal", query: { hostId: "h1", source: "overview" } });
    expect(router.currentRoute.value.path).toBe("/terminal");
    expect(router.currentRoute.value.query.hostId).toBe("h1");
    expect(typeof router.currentRoute.value.query.connectOperationId).toBe("string");
    await performSingleDocumentShellAction({ type: "new-file", kind: "remote", hostId: "h1", label: "web" });
    expect(router.currentRoute.value.path).toBe("/sftp");
    expect(store.fileTabs).toHaveLength(1);
    expect(store.fileTabs[0]).toMatchObject({ kind: "remote", hostId: "h1", label: "web" });
    expect(store.fileTabs[0]!.initialSessionId).toMatch(/^[0-9a-f-]{36}$/);
    const openProfile = vi.fn(async () => undefined);
    store.registerDesktopController({ activate: vi.fn(), beginOpenProfile: vi.fn(), openProfile, close: vi.fn(),
      deactivate: vi.fn(), isBusy: () => false });
    await performSingleDocumentShellAction({ type: "new-desktop", profileId: "p" });
    expect(router.currentRoute.value.path).toBe("/desktop");
    expect(openProfile).toHaveBeenCalledWith("p");
    vi.useFakeTimers();
    try {
      const pending = expect(performSingleDocumentShellAction({ type: "new-terminal" })).rejects.toThrow("workspace_tab.manager_unavailable");
      await vi.advanceTimersByTimeAsync(2_100);
      await pending;
    } finally { vi.useRealTimers(); }
  });

  it("opens pending plugin protocol launches and approved channels as Tabs of this document", async () => {
    const store = useWorkspaceTabsStore();
    const router = makeRouter();
    await router.push("/hosts");
    setSingleDocumentRouter(router);
    const reconcilePluginProtocolLaunches = vi.fn(async () => undefined);
    const openApprovedPluginChannel = vi.fn(async () => undefined);
    store.registerTerminalController(terminalController({ reconcilePluginProtocolLaunches, openApprovedPluginChannel }));
    pluginTerminal.launches.mockResolvedValueOnce([]);
    await openSingleDocumentPluginProtocolLaunches();
    // Nothing pending: no navigation and no Tab.
    expect(router.currentRoute.value.path).toBe("/hosts");
    expect(reconcilePluginProtocolLaunches).not.toHaveBeenCalled();
    pluginTerminal.launches.mockResolvedValueOnce([
      { launchId: "l1", revision: "1", tabId: "t1", paneId: "p1", claimed: false, expiresAtUnixMs: Date.now() + 60_000 },
      { launchId: "l2", revision: "1", tabId: "t2", paneId: "p2", claimed: true, expiresAtUnixMs: Date.now() + 60_000 },
    ]);
    await openSingleDocumentPluginProtocolLaunches();
    expect(router.currentRoute.value.path).toBe("/terminal");
    expect(reconcilePluginProtocolLaunches).toHaveBeenCalledTimes(1);
    const payload = { operationId: "op", authorizationToken: "token", label: "pod" } as never;
    await openSingleDocumentApprovedPluginChannel(payload);
    expect(openApprovedPluginChannel).toHaveBeenCalledWith(payload);
  });

  it("focuses only the exact existing Terminal Pane of a Session and never creates one", async () => {
    const store = useWorkspaceTabsStore();
    const router = makeRouter();
    await router.push("/overview");
    setSingleDocumentRouter(router);
    const focusSshSession = vi.fn(() => true);
    store.registerTerminalController(terminalController({ focusSshSession }));
    await focusSingleDocumentTerminalSession({ kind: "focusSshSession", sessionId: "s1", generation: "3" });
    expect(router.currentRoute.value.path).toBe("/terminal");
    expect(focusSshSession).toHaveBeenCalledWith("s1", "3");
    focusSshSession.mockReturnValue(false);
    await expect(focusSingleDocumentTerminalSession({ kind: "focusSshSession", sessionId: "s1", generation: "4" }))
      .rejects.toThrow("workspace_tab.session_unavailable");
  });

  describe("recovery after this document reloads", () => {
    const sftp = (sessionId: string, state: string, hostId: string | null = "h1", generation = "2") => ({
      sessionId, hostId, parentSshSession: null, generation, stateRevision: "1", state,
      transferCount: 0, activeTransferCount: 0, failure: null,
    });
    const uuid = (n: number) => `019d0000-0000-7000-8000-00000000000${n}`;

    it("binds every live unowned SftpSession to its own remote File Tab and never reopens a closed one", async () => {
      const store = useWorkspaceTabsStore();
      store.createInitialFileTab("file:11111111-1111-1111-1111-111111111111", "local", null, "Local");
      store.createInitialFileTab("file:22222222-2222-2222-2222-222222222222", "remote", "h1", "Owned", uuid(4), "1");
      store.claimFileSession(uuid(4), "file:22222222-2222-2222-2222-222222222222");
      store.activateFileTab("file:11111111-1111-1111-1111-111111111111");
      core.hosts.mockResolvedValue([{ hostId: "h1", label: "web" }]);
      core.sftp.mockResolvedValue({ snapshotRevision: "3", sessions: [
        sftp(uuid(1), "ready"), sftp(uuid(2), "failed", null, "5"), sftp(uuid(3), "closed"), sftp(uuid(4), "ready", "h1", "1"),
      ] });
      await restoreSingleDocumentResources(store);
      const recovered = store.fileTabs.slice(2);
      expect(recovered.map((tab) => [tab.kind, tab.hostId, tab.label, tab.initialSessionId, tab.initialGeneration])).toEqual([
        ["remote", "h1", "web", uuid(1), "2"],
        ["remote", null, "SFTP files", uuid(2), "5"],
      ]);
      expect(store.fileSessionOwner(uuid(1))).toBe(recovered[0]!.groupId);
      expect(store.fileSessionOwner(uuid(2))).toBe(recovered[1]!.groupId);
      expect(store.fileSessionOwner(uuid(3))).toBeNull();
      // The Tab the user had stays current.
      expect(store.activeFileTabId).toBe("file:11111111-1111-1111-1111-111111111111");
      await restoreSingleDocumentResources(store);
      expect(store.fileTabs).toHaveLength(4);
    });

    it("shows main-document Desktop Tabs again before the Desktop view mounts", async () => {
      const store = useWorkspaceTabsStore();
      core.sftp.mockResolvedValue({ snapshotRevision: "0", sessions: [] });
      const desktop = (id: string, generation = "1") => ({ id, generation, state: "running", profile: { label: `Desktop ${id}` } });
      core.desktops.mockResolvedValue([desktop("one"), desktop("two", "3")]);
      core.ownership.mockResolvedValue({ owned: [
        { id: "desktop:one", kind: "desktop", owner: "main", payload: { schemaVersion: 1, tabId: "desktop:one", sessionId: "one", generation: "1" } },
        { id: "desktop:two", kind: "desktop", owner: "main", payload: { schemaVersion: 1, tabId: "desktop:two", sessionId: "two", generation: "1" } },
      ], others: [] });
      await restoreSingleDocumentResources(store);
      expect(store.desktopTabs.map((tab) => tab.groupId)).toEqual(["desktop:one"]);
      expect(store.activeDesktopTabId).toBe("desktop:one");

      // Activation waits for the Desktop view that mounts after the route change.
      const router = makeRouter();
      await router.push("/terminal");
      const activate = vi.fn();
      const pending = activateSingleDocumentTab(store, router,
        useSingleDocumentTabs().tabs.value.find((tab) => tab.kind === "desktop")!);
      store.registerDesktopController({ activate, beginOpenProfile: vi.fn(), close: vi.fn(), deactivate: vi.fn(), isBusy: () => false });
      await pending;
      expect(router.currentRoute.value.path).toBe("/desktop");
      expect(activate).toHaveBeenCalledWith("desktop:one");

      // Once mounted, the Desktop view owns its Tab projection.
      core.desktops.mockClear();
      await restoreSingleDocumentResources(store);
      expect(core.desktops).not.toHaveBeenCalled();
    });

    const fileId = "file:019d0000-0000-7000-8000-000000000701";
    const recordedTab = (sessionId: string, generation = "2") => ({
      version: 1,
      tab: { groupId: fileId, kind: "remote", hostId: "h1", label: "web", paneCount: 2 },
      layout: { kind: "split", splitId: "s", direction: "horizontal", ratio: 0.3,
        first: { kind: "pane", paneId: "local-pane", terminalId: "local" },
        second: { kind: "pane", paneId: "remote-pane", terminalId: "remote" } },
      activePaneId: "remote-pane",
      panes: [
        { paneId: "local-pane", endpoint: { kind: "local", directoryRef: "ref:old", revision: "1", displayPath: "home/docs" },
          directory: "home/docs", remoteDirectoryPathBytes: null, localTrail: [], localRememberedPath: "/data/home/docs",
          search: "", sort: "name", showHidden: false, foldersFirst: true },
        { paneId: "remote-pane", endpoint: { kind: "remote", hostId: "h1", sessionId, generation },
          directory: "/var/log", remoteDirectoryPathBytes: [47, 118, 97, 114, 47, 108, 111, 103], localTrail: [],
          localRememberedPath: null, search: "err", sort: "modified", showHidden: true, foldersFirst: false },
      ],
    });

    it("restores a recorded File Tab with its split, Pane endpoints and directories before adopting other sessions", async () => {
      const store = useWorkspaceTabsStore();
      core.ownership.mockResolvedValue({ owned: [{ id: fileId, kind: "file", owner: "main", payload: recordedTab(uuid(1)) }], others: [] });
      core.sftp.mockResolvedValue({ snapshotRevision: "4", sessions: [sftp(uuid(1), "ready"), sftp(uuid(2), "ready")] });
      core.hosts.mockResolvedValue([{ hostId: "h1", label: "web" }]);
      await restoreSingleDocumentResources(store);
      expect(store.fileTabs.map((tab) => tab.groupId)[0]).toBe(fileId);
      const restored = store.importedFileSnapshots.get(fileId)!;
      expect(restored.layout).toEqual(recordedTab(uuid(1)).layout);
      expect(restored.panes[1]).toMatchObject({ directory: "/var/log", search: "err", sort: "modified",
        endpoint: { sessionId: uuid(1), generation: "2" } });
      // The local directory is reopened at its recorded path with a fresh capability.
      expect(core.register).toHaveBeenCalledWith("/data/home/docs");
      expect(restored.panes[0]!.endpoint).toMatchObject({ directoryRef: "ref:/data/home/docs", revision: "9" });
      expect(store.fileSessionOwner(uuid(1))).toBe(fileId);
      // Only the session without a record gets a default Tab; the recorded one is not duplicated.
      expect(store.fileTabs).toHaveLength(2);
      expect(store.fileTabs[1]).toMatchObject({ initialSessionId: uuid(2) });
      expect(store.activeFileTabId).toBe(fileId);
    });

    it("drops a recorded Tab whose session ended instead of reconnecting it", async () => {
      vi.useFakeTimers();
      try {
        const store = useWorkspaceTabsStore();
        core.ownership.mockResolvedValue({ owned: [{ id: fileId, kind: "file", owner: "main", payload: recordedTab(uuid(1)) }], others: [] });
        core.sftp.mockResolvedValue({ snapshotRevision: "4", sessions: [sftp(uuid(1), "closed")] });
        core.project.mockClear();
        await restoreSingleDocumentResources(store);
        expect(store.fileTabs).toHaveLength(0);
        // The opened local capability is released again.
        expect(core.release).toHaveBeenCalled();
        await vi.advanceTimersByTimeAsync(400);
        expect(core.project).toHaveBeenLastCalledWith({ schemaVersion: 1, kind: "mainDocumentFileTabs", tabs: [] });
      } finally { vi.useRealTimers(); }
    });

    it("records every File Tab's latest projection as one complete set", async () => {
      vi.useFakeTimers();
      try {
        const store = useWorkspaceTabsStore();
        core.project.mockClear();
        const kept = { ...recordedTab(uuid(1)), tab: { ...recordedTab(uuid(1)).tab, groupId: fileId } };
        store.createInitialFileTab(fileId, "remote", "h1", "web");
        const otherId = "file:019d0000-0000-7000-8000-000000000702";
        store.createInitialFileTab(otherId, "local", null, "Local");
        const stop = startSingleDocumentFilePersistence(store, new Map([[fileId, kept as never]]));
        await vi.advanceTimersByTimeAsync(400);
        // The not yet mounted Tab keeps its recovered projection; a Tab with no projection is not recorded.
        expect(core.project).toHaveBeenLastCalledWith({ schemaVersion: 1, kind: "mainDocumentFileTabs", tabs: [kept] });
        let publish: (snapshot: unknown) => void = () => undefined;
        const local = { version: 1, tab: { groupId: otherId } };
        store.registerFileController(otherId, { requestClose: vi.fn(), runShortcut: vi.fn(), snapshotHandoff: vi.fn(),
          observeHandoffSnapshot: (listener) => { publish = listener as never; listener(local as never); return () => undefined; } });
        await vi.advanceTimersByTimeAsync(400);
        expect(core.project).toHaveBeenLastCalledWith({ schemaVersion: 1, kind: "mainDocumentFileTabs", tabs: [kept, local] });
        const calls = core.project.mock.calls.length;
        publish(local);
        await vi.advanceTimersByTimeAsync(400);
        expect(core.project.mock.calls.length).toBe(calls);
        store.finishCloseFileTab(fileId);
        await vi.advanceTimersByTimeAsync(400);
        expect(core.project).toHaveBeenLastCalledWith({ schemaVersion: 1, kind: "mainDocumentFileTabs", tabs: [local] });
        stop();
      } finally { vi.useRealTimers(); }
    });
  });
});
