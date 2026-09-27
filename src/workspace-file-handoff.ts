import { watch } from "vue";
import type { Router } from "vue-router";

import { fetchSftpSessionSnapshot, listSftpLocalDirectory } from "./core-api/client";
import type { useWorkspaceTabsStore } from "./stores/workspaceTabs";
import type { WorkspaceTabHandoff } from "./workspace-tab-transfer";
import { isFileTabHandoffSnapshot, type FileTabHandoffSnapshot } from "./views/fileTabHandoffSnapshot";

type WorkspaceTabsStore = ReturnType<typeof useWorkspaceTabsStore>;

async function validateLiveResources(snapshot: FileTabHandoffSnapshot) {
  const remote = snapshot.panes.flatMap((pane) => pane.endpoint.kind === "remote" && pane.endpoint.sessionId
    ? [{ sessionId: pane.endpoint.sessionId, generation: pane.endpoint.generation, hostId: pane.endpoint.hostId }]
    : []);
  if (remote.length) {
    const live = (await fetchSftpSessionSnapshot()).sessions;
    for (const expected of remote) {
      if (!live.some((session) => session.sessionId === expected.sessionId
        && session.generation === expected.generation && session.hostId === expected.hostId
        && session.state !== "closed")) throw new Error("workspace_tab.file_session_stale");
    }
  }
  const local = snapshot.panes.flatMap((pane) => pane.endpoint.kind === "local" ? [
    ...(pane.endpoint.directoryRef && pane.endpoint.revision
      ? [{ directoryRef: pane.endpoint.directoryRef, revision: pane.endpoint.revision }] : []),
    ...pane.localTrail.map((item) => ({ directoryRef: item.capability.directoryRef, revision: item.capability.revision })),
  ] : []);
  for (let index = 0; index < local.length; index += 8) {
    await Promise.all(local.slice(index, index + 8).map(async (capability) => {
      const listing = await listSftpLocalDirectory({
        directoryRef: capability.directoryRef,
        expectedRevision: capability.revision,
        cursor: null,
        pageSize: 1,
      });
      if (listing.directoryRef !== capability.directoryRef || listing.revision !== capability.revision) {
        throw new Error("workspace_tab.file_capability_stale");
      }
    }));
  }
}

/** A File Tab transfers opaque Core handles and a bounded view DTO; it never opens a new SFTP transport. */
export function createFileHandoff(store: WorkspaceTabsStore, router: Router): WorkspaceTabHandoff<FileTabHandoffSnapshot> & {
  observe(id: string, listener: (snapshot: FileTabHandoffSnapshot) => void): () => void;
} {
  const frozen = new Map<string, { wasActive: boolean }>();
  const staged = new Set<string>();

  return {
    kind: "file",
    owns: (id) => store.fileTabs.some((tab) => tab.groupId === id),
    observe(id, listener) {
      return watch(() => store.fileController(id), (controller, _previous, onCleanup) => {
        if (controller) onCleanup(controller.observeHandoffSnapshot(listener));
      }, { immediate: true });
    },
    async snapshot(id) {
      const controller = store.fileController(id);
      if (!controller) throw new Error("workspace_tab.file_view_unavailable");
      return controller.snapshotHandoff();
    },
    async freeze(id) {
      const controller = store.fileController(id);
      if (!controller) throw new Error("workspace_tab.file_view_unavailable");
      controller.freezeHandoff();
      const wasActive = store.activeFileTabId === id;
      frozen.set(id, { wasActive });
      if (wasActive) {
        store.showFileWelcome();
      }
    },
    async import(id, value) {
      if (!isFileTabHandoffSnapshot(value, id) || store.fileTabs.some((tab) => tab.groupId === id)) {
        throw new Error("workspace_tab.invalid_file");
      }
      await validateLiveResources(value);
      if (!store.stageImportedFileTab(value)) throw new Error("workspace_tab.file_duplicate");
      staged.add(id);
    },
    commit(id) {
      store.fileController(id)?.commitHandoff();
      store.finishCloseFileTab(id);
      frozen.delete(id);
    },
    async rollback(id) {
      const previous = frozen.get(id);
      if (!previous) return;
      frozen.delete(id);
      store.fileController(id)?.rollbackHandoff();
      if (previous.wasActive) {
        store.activateFileTab(id);
        await router.push("/sftp");
      }
    },
    async discard(id) {
      if (!staged.delete(id)) return;
      store.discardImportedFileTab(id);
    },
    async activate(id) {
      if (!staged.has(id)) return;
      if (!store.admitImportedFileTab(id)) throw new Error("workspace_tab.file_admission_failed");
      await router.push("/sftp");
      staged.delete(id);
    },
    async activateExisting(id) {
      if (!store.fileTabs.some((tab) => tab.groupId === id)) throw new Error("workspace_tab.not_found");
      store.activateFileTab(id);
      await router.push("/sftp");
    },
  };
}
