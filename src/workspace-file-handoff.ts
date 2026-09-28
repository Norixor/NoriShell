import type { Router } from "vue-router";

import { fetchSftpSessionSnapshot, registerSftpLocalDirectory, releaseSftpLocalDirectory } from "./core-api/client";
import type { SftpLocalDirectoryCapability } from "./core-api/generated/core-api";
import type { useWorkspaceTabsStore } from "./stores/workspaceTabs";
import type { WorkspaceTabRecovery } from "./workspace-tab-recovery";
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
  // Core validates each local capability when the target loads it. A directory
  // listing can fail or expire without invalidating ownership of the File Tab.
}

async function restoreLocalCapabilities(snapshot: FileTabHandoffSnapshot): Promise<{
  snapshot: FileTabHandoffSnapshot;
  capabilities: SftpLocalDirectoryCapability[];
}> {
  const capabilities: SftpLocalDirectoryCapability[] = [];
  const registerExact = async (path: string | null) => {
    if (!path || path.length > 4096 || path.includes("\0")) throw new Error("workspace_tab.file_path_unavailable");
    let capability: SftpLocalDirectoryCapability;
    try {
      capability = await registerSftpLocalDirectory(path);
    } catch (cause) {
      throw new Error("workspace_tab.file_path_unavailable", { cause });
    }
    capabilities.push(capability);
    // A changed symlink or directory must not silently turn a recovered Pane
    // into a different location. The Core returns its canonical path.
    if (capability.rememberablePath !== path) throw new Error("workspace_tab.file_path_changed");
    return capability;
  };
  try {
    const panes = [] as FileTabHandoffSnapshot["panes"];
    for (const pane of snapshot.panes) {
      if (pane.endpoint.kind === "remote") {
        panes.push(pane);
        continue;
      }
      const localTrail = [] as typeof pane.localTrail;
      for (const item of pane.localTrail) {
        if (item.rememberedPath !== item.capability.rememberablePath) {
          throw new Error("workspace_tab.file_path_unavailable");
        }
        localTrail.push({ ...item, capability: await registerExact(item.rememberedPath) });
      }
      if (!pane.endpoint.directoryRef) {
        panes.push({ ...pane, localTrail });
        continue;
      }
      const current = await registerExact(pane.localRememberedPath);
      panes.push({
        ...pane,
        endpoint: { ...pane.endpoint, directoryRef: current.directoryRef, revision: current.revision },
        localTrail,
      });
    }
    return { snapshot: { ...snapshot, panes }, capabilities };
  } catch (error) {
    await Promise.all(capabilities.map((capability) => releaseSftpLocalDirectory({
      directoryRef: capability.directoryRef, expectedRevision: capability.revision,
    }).catch(() => undefined)));
    throw error;
  }
}

/** Crash recovery reopens recorded local directories and verifies SFTP sessions; it never reopens a transport. */
export function createFileRecovery(store: WorkspaceTabsStore, router: Router): WorkspaceTabRecovery<FileTabHandoffSnapshot> {
  const staged = new Set<string>();
  const recoveredCapabilities = new Map<string, SftpLocalDirectoryCapability[]>();
  const release = (capabilities: readonly SftpLocalDirectoryCapability[]) => Promise.all(capabilities.map(
    (capability) => releaseSftpLocalDirectory({
      directoryRef: capability.directoryRef, expectedRevision: capability.revision,
    }).catch(() => undefined)));

  return {
    async import(id, value) {
      if (!isFileTabHandoffSnapshot(value, id) || store.fileTabs.some((tab) => tab.groupId === id)) {
        throw new Error("workspace_tab.invalid_file");
      }
      const restored = await restoreLocalCapabilities(value);
      try {
        await validateLiveResources(restored.snapshot);
        if (!store.stageImportedFileTab(restored.snapshot)) throw new Error("workspace_tab.file_duplicate");
      } catch (error) {
        await release(restored.capabilities);
        throw error;
      }
      recoveredCapabilities.set(id, restored.capabilities);
      staged.add(id);
    },
    async discard(id) {
      if (!staged.delete(id)) return;
      store.discardImportedFileTab(id);
      const capabilities = recoveredCapabilities.get(id) ?? [];
      recoveredCapabilities.delete(id);
      await release(capabilities);
    },
    async activate(id) {
      if (!staged.has(id)) return;
      if (!store.admitImportedFileTab(id)) throw new Error("workspace_tab.file_admission_failed");
      await router.push("/sftp");
      staged.delete(id);
      recoveredCapabilities.delete(id);
    },
    async activateExisting(id) {
      if (!store.fileTabs.some((tab) => tab.groupId === id)) throw new Error("workspace_tab.not_found");
      store.activateFileTab(id);
      await router.push("/sftp");
    },
  };
}
