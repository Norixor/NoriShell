<script setup lang="ts">
import { storeToRefs } from "pinia";
import { computed, onActivated, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useRoute } from "vue-router";

import { canUseDesktopCore, listHosts } from "../core-api/client";
import type { HostSummary } from "../core-api/generated/core-api";
import { useRouteReveal } from "../routeReveal";
import { onSavedConnectionsChanged } from "../saved-connections";
import { useWorkspaceTabsStore } from "../stores/workspaceTabs";
import { isWorkspaceTabView } from "../workspace-window-context";
import { showWorkspaceTabFailure } from "../workspace-tab-errors";
import { runWorkspaceTabShellAction, type WorkspaceTabShellAction } from "../workspace-tab-shell-action";
import { createManagedFileForFocus, createManagedFileTab } from "../workspace-tab-view-shell";
import { pendingSftpPluginNavigations, takeSftpPluginNavigation } from "./sftpPluginNavigation";
import FileWelcomeView from "./FileWelcomeView.vue";
import SftpView from "./SftpView.vue";

const route = useRoute();
const revealRoute = useRouteReveal();
const workspaceTabs = useWorkspaceTabsStore();
const { fileTabs, activeFileTabId } = storeToRefs(workspaceTabs);
const hosts = ref<HostSummary[]>([]);
const loading = ref(true);
let mounted = false;
let stopSavedConnections: (() => void) | undefined;
let lastFocusOperation = "";
let lastHostRequest = "";

const currentTab = computed(() => fileTabs.value.find((tab) => tab.groupId === activeFileTabId.value) ?? null);

function showFileCreateFailure(error: unknown) {
  showWorkspaceTabFailure(error, "file-create", "workspace_tab.action_failed", "errors.tray.actionUnavailable");
}

async function refreshHosts() {
  if (!canUseDesktopCore()) { loading.value = false; revealRoute(); return; }
  try { hosts.value = await listHosts(); }
  finally { loading.value = false; revealRoute(); }
}

function createFileTabFromShell(action: Extract<WorkspaceTabShellAction, { type: "new-file" }>) {
  void runWorkspaceTabShellAction(action).catch(showFileCreateFailure);
}

const createLocal = () => createFileTabFromShell({ type: "new-file", kind: "local" });
const createEmptyRemote = () => createFileTabFromShell({ type: "new-file", kind: "remote" });

function createRemote(hostId: string) {
  const host = hosts.value.find((item) => item.hostId === hostId);
  if (host) createFileTabFromShell({ type: "new-file", kind: "remote", hostId, label: host.label });
}

function acceptExternalNavigation() {
  if (!mounted || route.path !== "/sftp") return;
  const focusOperation = route.query.focusOperation;
  if (typeof focusOperation === "string" && focusOperation !== lastFocusOperation) {
    lastFocusOperation = focusOperation;
    const sessionId = route.query.focusSessionId;
    if (typeof sessionId === "string") {
      const owner = workspaceTabs.fileSessionOwner(sessionId);
      if (owner) workspaceTabs.activateFileTab(owner);
      else if (isWorkspaceTabView() && currentTab.value) workspaceTabs.claimFileSession(sessionId, currentTab.value.groupId);
      else if (!isWorkspaceTabView()) void createManagedFileForFocus(Object.fromEntries(
        Object.entries(route.query).filter((entry): entry is [string, string] => typeof entry[1] === "string"),
      )).catch(showFileCreateFailure);
    } else if (!currentTab.value && !isWorkspaceTabView()) {
      void createManagedFileForFocus({ focusOperation }).catch(showFileCreateFailure);
    }
    return;
  }
  const pendingPluginSession = pendingSftpPluginNavigations.value[0]?.sftpSession.sessionId;
  if (pendingPluginSession) {
    const owner = workspaceTabs.fileSessionOwner(pendingPluginSession);
    if (owner) workspaceTabs.activateFileTab(owner);
    else if (isWorkspaceTabView() && currentTab.value) workspaceTabs.claimFileSession(pendingPluginSession, currentTab.value.groupId);
    else if (!isWorkspaceTabView()) {
      const intent = takeSftpPluginNavigation();
      if (intent) void createManagedFileTab("remote", null, "", intent).catch(showFileCreateFailure);
    }
    return;
  }
  const hostId = route.query.hostId;
  const hostOperation = route.query.fileOperationId;
  const hostRequestKey = typeof hostOperation === "string" ? hostOperation : typeof hostId === "string" ? hostId : "";
  if (typeof hostId === "string" && hostRequestKey !== lastHostRequest) {
    lastHostRequest = hostRequestKey;
    createRemote(hostId);
    return;
  }
}

watch([() => route.query.focusOperation, () => route.query.hostId, () => route.query.fileOperationId, pendingSftpPluginNavigations], acceptExternalNavigation);

onMounted(async () => {
  mounted = true;
  try {
    await refreshHosts();
    if (canUseDesktopCore()) {
      stopSavedConnections = await onSavedConnectionsChanged(() => { void refreshHosts(); });
    }
  } catch {
    loading.value = false;
    revealRoute();
  }
  acceptExternalNavigation();
});
onActivated(() => {
  if (!mounted) return;
  revealRoute();
  void refreshHosts().catch(() => undefined);
  acceptExternalNavigation();
});
onBeforeUnmount(() => {
  mounted = false;
  stopSavedConnections?.();
});
</script>

<template>
  <div class="file-workspace-view">
    <FileWelcomeView
      v-if="!currentTab"
      :hosts="hosts"
      :loading="loading"
      @create-local="createLocal"
      @create-empty-remote="createEmptyRemote"
      @create-remote="createRemote"
    />
    <template
      v-for="tab in fileTabs"
      :key="tab.groupId"
    >
      <SftpView
        v-show="tab.groupId === activeFileTabId"
        :workspace-tab-id="tab.groupId"
        :initial-kind="tab.kind"
        :initial-host-id="tab.hostId"
        :initial-session-id="tab.initialSessionId"
        :initial-generation="tab.initialGeneration"
        :handoff-snapshot="workspaceTabs.importedFileSnapshots.get(tab.groupId) ?? null"
        :active="tab.groupId === activeFileTabId"
      />
    </template>
  </div>
</template>

<style scoped>
.file-workspace-view { width:100%; height:100%; min-width:0; min-height:0; overflow:hidden; }
</style>
