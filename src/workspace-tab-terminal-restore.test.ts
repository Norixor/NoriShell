import { beforeEach, describe, expect, it, vi } from "vitest";

const core = vi.hoisted(() => ({
  ssh: vi.fn(), local: vi.fn(), telnet: vi.fn(), layout: vi.fn(), plugin: vi.fn(), registry: vi.fn(),
  sshDetails: vi.fn(), localDetails: vi.fn(),
}));

vi.mock("./core-api/client", () => ({
  fetchSshSessionSnapshot: core.ssh,
  fetchLocalSessionSnapshot: core.local,
  fetchTelnetSessionSnapshot: core.telnet,
  fetchTerminalWorkspaceLayout: core.layout,
  getSshSession: core.sshDetails,
  getLocalSession: core.localDetails,
}));
vi.mock("./core-api/plugin-terminal", () => ({ fetchPluginTerminalSessionSnapshot: core.plugin }));
vi.mock("./workspace-tab-windows", () => ({ snapshotWorkspaceTabs: core.registry }));
vi.mock("./workspace-tab-view-shell", () => ({ activateWorkspaceTabView: vi.fn(), createManagedWorkspaceTab: vi.fn() }));
vi.mock("./workspace-tab-errors", () => ({ showWorkspaceTabFailure: vi.fn() }));

import { planTerminalWorkspaceRestore } from "./workspace-tab-terminal-restore";

const tab = (tabId: string, paneId: string, kind: "sshHost" | "local" = "sshHost") => ({
  tabId, activePaneId: paneId, layout: { kind: "pane", paneId, terminalId: paneId },
  panes: [kind === "sshHost" ? { kind, paneId, label: tabId, hostId: "host-1" } : { kind, paneId, label: tabId }],
});

function ssh(sessionId: string, viewIds: string[] = []) {
  core.ssh.mockResolvedValue({ sessions: [{ sessionId, generation: "3", state: "running" }] });
  core.sshDetails.mockResolvedValue({ attachments: viewIds.map((viewId) => ({ viewId })) });
}

describe("Terminal workspace startup plan", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    core.ssh.mockResolvedValue({ sessions: [] });
    core.local.mockResolvedValue({ sessions: [] });
    core.telnet.mockResolvedValue({ sessions: [] });
    core.plugin.mockResolvedValue({ sessions: [] });
    core.registry.mockResolvedValue({ owned: [], others: [] });
    core.layout.mockResolvedValue({ revision: "1", layout: {
      schemaVersion: 1, activeTabId: "tab-b", tabs: [tab("tab-a", "pane-a"), tab("tab-b", "pane-b", "local")],
    } });
  });

  it("restarts history only when chosen and no Session is live", async () => {
    const history = await planTerminalWorkspaceRestore(true);
    expect(history.tabs.map((seed) => [seed.tab.tabId, seed.autoReconnect, seed.live])).toEqual([
      ["tab-a", true, []], ["tab-b", true, []],
    ]);
    expect(history.activeTabId).toBe("tab-b");
    expect((await planTerminalWorkspaceRestore(false)).tabs).toEqual([]);
  });

  it("attaches a live Session to its persisted Pane before any restart, even for the Welcome start", async () => {
    ssh("session-1", ["pane-a"]);
    const plan = await planTerminalWorkspaceRestore(false);
    expect(plan.tabs.map((seed) => [seed.tab.tabId, seed.autoReconnect, seed.live])).toEqual([
      ["tab-a", false, [{ kind: "ssh", sessionId: "session-1", generation: "3", paneId: "pane-a" }]],
      ["tab-b", false, []],
    ]);
  });

  it("leaves Tabs and Sessions that a Tab WebView already owns", async () => {
    ssh("session-1", ["pane-a"]);
    core.registry.mockResolvedValue({ owned: [], others: [{ id: "tab-a", kind: "terminal", owner: "workspace-1",
      terminalPanes: [{ paneId: "pane-a", kind: "ssh", sessionId: "session-1", generation: "3" }] }] });
    const plan = await planTerminalWorkspaceRestore(true);
    expect(plan.tabs.map((seed) => [seed.tab.tabId, seed.autoReconnect])).toEqual([["tab-b", true]]);
  });

  it("adopts an unbound live Session into its own Tab unless an unprojected Tab may own it", async () => {
    ssh("orphan");
    const plan = await planTerminalWorkspaceRestore(true);
    const adopted = plan.tabs.at(-1)!;
    expect(adopted.live).toEqual([{ kind: "ssh", sessionId: "orphan", generation: "3", paneId: adopted.tab.activePaneId }]);
    expect(plan.tabs.slice(0, 2).every((seed) => !seed.autoReconnect)).toBe(true);

    core.registry.mockResolvedValue({ owned: [], others: [{ id: "tab-x", kind: "terminal", owner: "workspace-1", terminalPanes: [] }] });
    const uncertain = await planTerminalWorkspaceRestore(true);
    expect(uncertain.tabs.map((seed) => seed.tab.tabId)).toEqual(["tab-a", "tab-b"]);
    expect(uncertain.tabs.every((seed) => seed.autoReconnect)).toBe(true);
  });

  it("adopts several orphan plugin Sessions bound to one Tab into that single Tab", async () => {
    const pluginSession = (paneId: string) => ({ sessionId: `session-${paneId}`, generation: "1", tabId: "plugin-tab",
      paneId, state: "running", cleanupBlocked: false });
    core.plugin.mockResolvedValue({ sessions: [pluginSession("pane-a1"), pluginSession("pane-a2")] });
    const plan = await planTerminalWorkspaceRestore(false);
    const pluginTabs = plan.tabs.filter((seed) => seed.tab.tabId === "plugin-tab");
    expect(pluginTabs).toHaveLength(1);
    expect(pluginTabs[0]!.live.map((pane) => pane.paneId)).toEqual(["pane-a1", "pane-a2"]);
    expect(pluginTabs[0]!.tab.layout).toMatchObject({ kind: "split",
      first: { paneId: "pane-a1" }, second: { paneId: "pane-a2" } });
  });
});
