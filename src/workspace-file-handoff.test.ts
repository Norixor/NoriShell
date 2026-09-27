import { createPinia } from "pinia";
import { describe, expect, it, vi } from "vitest";
import { createMemoryHistory, createRouter } from "vue-router";

import { fetchSftpSessionSnapshot, listSftpLocalDirectory } from "./core-api/client";
import { useWorkspaceTabsStore } from "./stores/workspaceTabs";
import { createFileHandoff } from "./workspace-file-handoff";
import { isFileTabHandoffSnapshot, type FileTabHandoffSnapshot } from "./views/fileTabHandoffSnapshot";
import type { TerminalLayoutNode } from "./components/terminal/terminalLayout";

vi.mock("./core-api/client", () => ({
  fetchSftpSessionSnapshot: vi.fn(),
  listSftpLocalDirectory: vi.fn(),
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

describe("File Tab handoff", () => {
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

  it("freezes without disconnecting and restores the source on rollback", async () => {
    const store = useWorkspaceTabsStore(createPinia());
    const id = store.createFileTab("remote", "host-1", "Files");
    const value = snapshot(id);
    const controller = {
      requestClose: vi.fn(), runShortcut: vi.fn(), snapshotHandoff: vi.fn(() => value),
      freezeHandoff: vi.fn(), rollbackHandoff: vi.fn(), commitHandoff: vi.fn(),
      observeHandoffSnapshot: vi.fn(() => () => undefined),
    };
    store.registerFileController(id, controller);
    const handoff = createFileHandoff(store, router());

    expect(await handoff.snapshot(id)).toEqual(value);
    await handoff.freeze(id);
    expect(store.fileTabs).toHaveLength(1);
    expect(store.activeFileTabId).toBe("");
    await handoff.rollback(id);
    expect(controller.rollbackHandoff).toHaveBeenCalledOnce();
    expect(store.activeFileTabId).toBe(id);

    await handoff.freeze(id);
    handoff.commit(id);
    expect(controller.commitHandoff).toHaveBeenCalledOnce();
    expect(store.fileTabs).toHaveLength(0);
  });

  it("validates live handles before staging and admits the same IDs after commit", async () => {
    const store = useWorkspaceTabsStore(createPinia());
    const handoff = createFileHandoff(store, router());
    const id = "file:019d0000-0000-7000-8000-000000000001";
    const value = snapshot(id);
    vi.mocked(fetchSftpSessionSnapshot).mockResolvedValue({
      snapshotRevision: "1", sessions: [{
        sessionId: "session-1", hostId: "host-1", parentSshSession: null,
        generation: "8", stateRevision: "1", state: "ready", transferCount: 1,
        activeTransferCount: 1, failure: null,
      }], transfers: [],
    });
    vi.mocked(listSftpLocalDirectory).mockResolvedValue({
      directoryRef: "cap-1", revision: "4", entries: [], nextCursor: null,
    });

    await handoff.import(id, value);
    expect(store.fileTabs).toHaveLength(0);
    expect(store.importedFileSnapshots.get(id)).toEqual(value);
    await handoff.activate(id);
    expect(store.fileTabs.map((tab) => tab.groupId)).toEqual([id]);
    expect(store.activeFileTabId).toBe(id);
    expect(store.importedFileSnapshots.get(id)?.panes[1]?.endpoint).toEqual(value.panes[1]?.endpoint);
    expect(listSftpLocalDirectory).toHaveBeenCalledWith({ directoryRef: "cap-1", expectedRevision: "4", cursor: null, pageSize: 1 });

    const stale = snapshot("file:019d0000-0000-7000-8000-000000000002");
    stale.panes[1]!.endpoint = { kind: "remote", hostId: "host-1", sessionId: "session-1", generation: "9" };
    await expect(handoff.import(stale.tab.groupId, stale)).rejects.toThrow("workspace_tab.file_session_stale");
  });
});
