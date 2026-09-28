import { flushPromises, mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { createMemoryHistory, createRouter } from "vue-router";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { i18n } from "../locales";
import { useWorkspaceTabsStore } from "../stores/workspaceTabs";
import FileWorkspaceView from "./FileWorkspaceView.vue";

const listHosts = vi.hoisted(() => vi.fn());
const managed = vi.hoisted(() => ({ run: vi.fn(), createFile: vi.fn(), createForFocus: vi.fn() }));
vi.mock("../core-api/client", async (importOriginal) => ({
  ...await importOriginal<typeof import("../core-api/client")>(),
  canUseDesktopCore: () => true,
  listHosts,
}));
vi.mock("../saved-connections", () => ({ onSavedConnectionsChanged: vi.fn(async () => () => undefined) }));
vi.mock("../workspace-tab-shell-action", () => ({ runWorkspaceTabShellAction: managed.run }));
vi.mock("../workspace-tab-view-shell", () => ({
  createManagedFileTab: managed.createFile,
  createManagedFileForFocus: managed.createForFocus,
}));
vi.mock("./SftpView.vue", () => ({
  default: {
    name: "SftpView",
    props: ["workspaceTabId", "initialKind", "initialHostId", "active"],
    template: '<div class="file-tab-fixture" :data-kind="initialKind" :data-host="initialHostId" :data-active="active" />',
  },
}));

const host = {
  hostId: "019d0000-0000-7000-8000-000000000301",
  label: "QA server",
  normalizedAddress: "qa.example.test",
  port: 22,
};

async function mountWorkspace(path = "/sftp") {
  const pinia = createPinia();
  setActivePinia(pinia);
  const router = createRouter({ history: createMemoryHistory(), routes: [{ path: "/sftp", component: FileWorkspaceView }] });
  await router.push(path);
  await router.isReady();
  const wrapper = mount(FileWorkspaceView, { global: { plugins: [pinia, router, i18n] } });
  await flushPromises();
  return { wrapper, router, tabs: useWorkspaceTabsStore() };
}

describe("FileWorkspaceView", () => {
  beforeEach(() => {
    listHosts.mockReset().mockResolvedValue([host]);
    managed.run.mockReset().mockResolvedValue(undefined);
    managed.createFile.mockReset().mockResolvedValue("file:new");
    managed.createForFocus.mockReset().mockResolvedValue("file:focus");
    i18n.global.locale.value = "en";
  });

  it("asks the native manager for local and saved-server tabs", async () => {
    const { wrapper, tabs } = await mountWorkspace();
    try {
      expect(tabs.fileTabs).toHaveLength(0);
      expect(wrapper.find(".file-tab-fixture").exists()).toBe(false);
      await wrapper.find(".file-welcome__choice").trigger("click");
      await flushPromises();
      expect(tabs.fileTabs).toHaveLength(0);
      expect(managed.run).toHaveBeenCalledWith({ type: "new-file", kind: "local" });

      tabs.showFileWelcome();
      await flushPromises();
      await wrapper.find(".file-welcome__host").trigger("click");
      await flushPromises();
      expect(tabs.fileTabs).toHaveLength(0);
      expect(managed.run).toHaveBeenCalledWith({ type: "new-file", kind: "remote", hostId: host.hostId, label: host.label });
    } finally { wrapper.unmount(); }
  });

  it("opens a dedicated File tab for an explicit Host navigation", async () => {
    const { wrapper, tabs } = await mountWorkspace(`/sftp?hostId=${host.hostId}`);
    try {
      expect(tabs.fileTabs).toHaveLength(0);
      expect(managed.run).toHaveBeenCalledWith({ type: "new-file", kind: "remote", hostId: host.hostId, label: host.label });
    } finally { wrapper.unmount(); }
  });

  it("starts with local and empty server panes without requiring a saved Host", async () => {
    listHosts.mockResolvedValueOnce([]);
    const { wrapper, tabs } = await mountWorkspace();
    try {
      await wrapper.find(".file-welcome__choice--remote").trigger("click");
      await flushPromises();
      expect(tabs.fileTabs).toHaveLength(0);
      expect(managed.run).toHaveBeenCalledWith({ type: "new-file", kind: "remote" });
    } finally { wrapper.unmount(); }
  });

  it("focuses an existing owner for a tray session and accepts a later operation for the same Host", async () => {
    const { wrapper, router, tabs } = await mountWorkspace();
    try {
      const ownedTab = "file:019d0000-0000-7000-8000-000000000990";
      expect(tabs.createInitialFileTab(ownedTab, "remote", host.hostId, host.label)).toBe(true);
      expect(tabs.claimFileSession("019d0000-0000-7000-8000-000000000991", ownedTab)).toBe(true);
      tabs.showFileWelcome();
      await router.push("/sftp?focusOperation=first&focusSessionId=019d0000-0000-7000-8000-000000000991&focusGeneration=1");
      await flushPromises();
      expect(tabs.fileTabs).toHaveLength(1);
      expect(tabs.activeFileTabId).toBe(ownedTab);

      await router.push(`/sftp?hostId=${host.hostId}&fileOperationId=second`);
      await flushPromises();
      expect(tabs.fileTabs).toHaveLength(1);
      expect(managed.run).toHaveBeenCalledWith({ type: "new-file", kind: "remote", hostId: host.hostId, label: host.label });
      await router.push(`/sftp?hostId=${host.hostId}&fileOperationId=third`);
      await flushPromises();
      expect(tabs.fileTabs).toHaveLength(1);
      expect(managed.run).toHaveBeenCalledTimes(2);
    } finally { wrapper.unmount(); }
  });
});
