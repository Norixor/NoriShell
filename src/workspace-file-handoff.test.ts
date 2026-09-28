import { createPinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { createMemoryHistory, createRouter } from "vue-router";

import { fetchSftpSessionSnapshot, listSftpLocalDirectory, registerSftpLocalDirectory, releaseSftpLocalDirectory } from "./core-api/client";
import { useWorkspaceTabsStore } from "./stores/workspaceTabs";
import { createFileRecovery } from "./workspace-file-handoff";
import { isFileTabHandoffSnapshot, type FileTabHandoffSnapshot } from "./views/fileTabHandoffSnapshot";
import type { TerminalLayoutNode } from "./components/terminal/terminalLayout";

vi.mock("./core-api/client", () => ({
  fetchSftpSessionSnapshot: vi.fn(),
  listSftpLocalDirectory: vi.fn(),
  registerSftpLocalDirectory: vi.fn(),
  releaseSftpLocalDirectory: vi.fn(),
}));

function router() {
  return createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: "/sftp", component: { template: "<div />" } },
      { path: "/terminal", component: { template: "<div />" } },
    ],
  });
}

function snapshot(id: string): FileTabHandoffSnapshot {
  return {
    version: 1,
    tab: { groupId: id, kind: "remote", hostId: "host-1", label: "Files", paneCount: 2 },
    layout: {
      kind: "split", splitId: "split-1", direction: "horizontal", ratio: 0.5,
      first: { kind: "pane", paneId: "local-1", terminalId: "local" },
      second: { kind: "pane", paneId: "remote-1", terminalId: "remote" },
    },
    activePaneId: "remote-1",
    panes: [
      {
        paneId: "local-1",
        endpoint: { kind: "local", directoryRef: "cap-1", revision: "4", displayPath: "Home" },
        directory: "Home", remoteDirectoryPathBytes: null, localTrail: [], localRememberedPath: null,
        search: "", sort: "name", showHidden: false, foldersFirst: true,
      },
      {
        paneId: "remote-1",
        endpoint: { kind: "remote", hostId: "host-1", sessionId: "session-1", generation: "8" },
        directory: "/var/log", remoteDirectoryPathBytes: [47, 118, 97, 114, 47, 108, 111, 103],
        localTrail: [], localRememberedPath: null,
        search: "", sort: "name", showHidden: false, foldersFirst: true,
      },
    ],
  };
}

describe("File Tab recovery", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    vi.mocked(releaseSftpLocalDirectory).mockResolvedValue(undefined);
    vi.mocked(listSftpLocalDirectory).mockImplementation(async ({ directoryRef, expectedRevision }) => ({
      directoryRef, revision: expectedRevision, entries: [], nextCursor: null,
    }));
  });
  it("accepts a Pane layout beyond the old 32 Pane handoff limit", () => {
    const id = "file:019d0000-0000-7000-8000-000000000003";
    const value = snapshot(id);
    value.panes = Array.from({ length: 33 }, (_, index) => ({
      paneId: `pane-${index}`,
      endpoint: { kind: "local", directoryRef: null, revision: null, displayPath: "" },
      directory: "", remoteDirectoryPathBytes: null, localTrail: [], localRememberedPath: null,
      search: "", sort: "name", showHidden: false, foldersFirst: true,
    }));
    value.tab.paneCount = value.panes.length;
    value.activePaneId = "pane-0";
    let layout: TerminalLayoutNode = { kind: "pane", paneId: "pane-0", terminalId: "local" };
    for (let index = 1; index < value.panes.length; index += 1) {
      layout = {
        kind: "split", splitId: `split-${index}`, direction: "horizontal", ratio: 0.5,
        first: layout, second: { kind: "pane", paneId: `pane-${index}`, terminalId: "local" },
      };
    }
    value.layout = layout;

    expect(isFileTabHandoffSnapshot(value, id)).toBe(true);
  });

  it("accepts a plugin SFTP Tab bound to a shared session without a saved Host", () => {
    const id = "file:019d0000-0000-7000-8000-000000000009";
    const value = snapshot(id);
    value.tab.hostId = null;
    value.tab.initialSessionId = "019d0000-0000-7000-8000-000000000010";
    value.tab.initialGeneration = "7";
    value.panes[1]!.endpoint = {
      kind: "remote", hostId: null,
      sessionId: value.tab.initialSessionId, generation: value.tab.initialGeneration,
    };

    expect(isFileTabHandoffSnapshot(value, id)).toBe(true);
  });

  it("rejects a recovered remote Pane whose Session generation changed", async () => {
    const store = useWorkspaceTabsStore(createPinia());
    const recovery = createFileRecovery(store, router());
    vi.mocked(fetchSftpSessionSnapshot).mockResolvedValue({
      snapshotRevision: "1", sessions: [{
        sessionId: "session-1", hostId: "host-1", parentSshSession: null,
        generation: "8", stateRevision: "1", state: "ready", transferCount: 1,
        activeTransferCount: 1, failure: null,
      }], transfers: [],
    });
    const stale = snapshot("file:019d0000-0000-7000-8000-000000000002");
    stale.panes = stale.panes.slice(1);
    stale.layout = { kind: "pane", paneId: "remote-1", terminalId: "remote" };
    stale.tab.paneCount = 1;
    stale.panes[0]!.endpoint = { kind: "remote", hostId: "host-1", sessionId: "session-1", generation: "9" };
    await expect(recovery.import(stale.tab.groupId, stale)).rejects.toThrow("workspace_tab.file_session_stale");
    expect(store.importedFileSnapshots.has(stale.tab.groupId)).toBe(false);
  });

  it("recovers local Pane and trail with fresh capabilities while preserving the remote Session", async () => {
    const store = useWorkspaceTabsStore(createPinia());
    const recovery = createFileRecovery(store, router());
    const id = "file:019d0000-0000-7000-8000-000000000005";
    const value = snapshot(id);
    value.panes[0]!.localRememberedPath = "/Users/test/Documents";
    value.panes[0]!.localTrail = [{
      capability: { directoryRef: "old-parent", revision: "3", displayName: "test", rememberablePath: "/Users/test" },
      displayPath: "Home", rememberedPath: "/Users/test",
    }];
    vi.mocked(registerSftpLocalDirectory).mockImplementation(async (path) => ({
      directoryRef: path === "/Users/test" ? "new-parent" : "new-current",
      revision: "1", displayName: "new", rememberablePath: path,
    }));
    vi.mocked(fetchSftpSessionSnapshot).mockResolvedValue({
      snapshotRevision: "1", sessions: [{
        sessionId: "session-1", hostId: "host-1", parentSshSession: null,
        generation: "8", stateRevision: "1", state: "ready", transferCount: 0,
        activeTransferCount: 0, failure: null,
      }], transfers: [],
    });

    await recovery.import(id, value);
    const recovered = store.importedFileSnapshots.get(id)!;
    expect(registerSftpLocalDirectory).toHaveBeenCalledTimes(2);
    expect(registerSftpLocalDirectory).toHaveBeenNthCalledWith(1, "/Users/test");
    expect(registerSftpLocalDirectory).toHaveBeenNthCalledWith(2, "/Users/test/Documents");
    expect(recovered.panes[0]?.endpoint).toEqual({ kind: "local", directoryRef: "new-current", revision: "1", displayPath: "Home" });
    expect(recovered.panes[0]?.localTrail[0]?.capability.directoryRef).toBe("new-parent");
    expect(recovered.panes[1]?.endpoint).toEqual(value.panes[1]?.endpoint);
    expect(listSftpLocalDirectory).not.toHaveBeenCalled();
    await recovery.activate(id);
    expect(store.activeFileTabId).toBe(id);
    expect(releaseSftpLocalDirectory).not.toHaveBeenCalled();
  });

  it("fails recovery when a remembered path is absent or resolves elsewhere, releasing new capabilities", async () => {
    const store = useWorkspaceTabsStore(createPinia());
    const recovery = createFileRecovery(store, router());
    const missing = snapshot("file:019d0000-0000-7000-8000-000000000006");
    missing.panes = missing.panes.slice(0, 1);
    missing.layout = { kind: "pane", paneId: "local-1", terminalId: "local" };
    missing.tab = { ...missing.tab, kind: "local", hostId: null, paneCount: 1 };
    missing.activePaneId = "local-1";
    await expect(recovery.import(missing.tab.groupId, missing)).rejects.toThrow("workspace_tab.file_path_unavailable");
    expect(registerSftpLocalDirectory).not.toHaveBeenCalled();
    expect(store.fileTabs).toHaveLength(0);

    const changed = snapshot("file:019d0000-0000-7000-8000-000000000007");
    changed.panes[0]!.localRememberedPath = "/Users/test/Documents";
    changed.panes[0]!.localTrail = [{
      capability: { directoryRef: "old-parent", revision: "3", displayName: "test", rememberablePath: "/Users/test" },
      displayPath: "Home", rememberedPath: "/Users/test",
    }];
    vi.mocked(registerSftpLocalDirectory).mockImplementation(async (path) => ({
      directoryRef: path === "/Users/test" ? "new-parent" : "new-current",
      revision: "1", displayName: "new", rememberablePath: path === "/Users/test" ? path : "/Users/other/Documents",
    }));
    await expect(recovery.import(changed.tab.groupId, changed)).rejects.toThrow("workspace_tab.file_path_changed");
    expect(releaseSftpLocalDirectory).toHaveBeenCalledWith({ directoryRef: "new-parent", expectedRevision: "1" });
    expect(releaseSftpLocalDirectory).toHaveBeenCalledWith({ directoryRef: "new-current", expectedRevision: "1" });
    expect(store.importedFileSnapshots.has(changed.tab.groupId)).toBe(false);

    const inaccessible = snapshot("file:019d0000-0000-7000-8000-000000000008");
    inaccessible.panes[0]!.localRememberedPath = "/Users/test/Gone";
    vi.mocked(registerSftpLocalDirectory).mockRejectedValueOnce(new Error("sftp.invalid_input"));
    await expect(recovery.import(inaccessible.tab.groupId, inaccessible)).rejects.toThrow("workspace_tab.file_path_unavailable");
    expect(store.importedFileSnapshots.has(inaccessible.tab.groupId)).toBe(false);
  });
});
