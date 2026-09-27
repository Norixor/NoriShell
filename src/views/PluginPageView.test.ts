import { flushPromises, mount } from "@vue/test-utils";
import { reactive } from "vue";
import { createMemoryHistory, createRouter } from "vue-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { i18n } from "../locales";

const mocks = vi.hoisted(() => ({
  loadNavigation: vi.fn(),
  listInstalled: vi.fn(),
  ensure: vi.fn(),
  snapshot: vi.fn(),
  register: vi.fn(),
  unregister: vi.fn(),
  focus: vi.fn(),
}));
const extensions = reactive({
  navigation: [] as { pluginId: string; pluginName: string; navigation: { pageId: string; label: string; icon: string } }[],
  loadNavigation: mocks.loadNavigation,
});

vi.mock("@tauri-apps/api/core", async (importOriginal) => ({
  ...await importOriginal<typeof import("@tauri-apps/api/core")>(),
  isTauri: () => true,
}));
vi.mock("../core-api/client", async (importOriginal) => ({
  ...await importOriginal<typeof import("../core-api/client")>(),
  listInstalledPlugins: mocks.listInstalled,
}));
vi.mock("../stores/pluginExtensions", () => ({ usePluginExtensionsStore: () => extensions }));
vi.mock("../stores/workspaceTabs", () => ({ useWorkspaceTabsStore: () => ({ ensurePluginPageTab: mocks.ensure }) }));
vi.mock("../workspace-tab-windows", () => ({
  snapshotWorkspaceTabs: mocks.snapshot,
  registerWorkspaceTab: mocks.register,
  unregisterWorkspaceTab: mocks.unregister,
  focusWorkspaceWindowTarget: mocks.focus,
  workspaceWindowLabel: () => "main",
}));
vi.mock("../routeReveal", () => ({ useRouteReveal: () => () => undefined }));

import PluginPageView from "./PluginPageView.vue";

const item = {
  pluginId: "org.example.sync",
  pluginName: "Sync",
  navigation: { pageId: "home", label: "Sync", icon: "cloud" },
};
const tabId = "page:plugin:org.example.sync:home";

async function mountPage() {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: "/plugin/:pluginId/:pageId", component: PluginPageView },
      { path: "/terminal", component: { template: "<div />" } },
    ],
  });
  await router.push("/plugin/org.example.sync/home");
  await router.isReady();
  const wrapper = mount(PluginPageView, {
    global: { plugins: [router, i18n], stubs: { NvxPluginExtensionTarget: true, NvxPluginSettingsDialog: true } },
  });
  return { wrapper, router };
}

describe("plugin Page Tab ownership", () => {
  beforeEach(() => {
    extensions.navigation = [item];
    mocks.loadNavigation.mockReset().mockResolvedValue(undefined);
    mocks.listInstalled.mockReset().mockResolvedValue([]);
    mocks.ensure.mockReset();
    mocks.snapshot.mockReset().mockResolvedValue({ owned: [], incoming: [], outgoing: [], others: [] });
    mocks.register.mockReset().mockResolvedValue({ id: tabId, kind: "page", owner: "main", revision: 1, payload: {} });
    mocks.unregister.mockReset().mockResolvedValue(undefined);
    mocks.focus.mockReset().mockResolvedValue(undefined);
  });
  afterEach(() => { extensions.navigation = []; });

  it("does not recreate the source Tab when navigation loading finishes after unmount", async () => {
    let finishLoad!: () => void;
    mocks.loadNavigation.mockReturnValue(new Promise<void>((resolve) => { finishLoad = resolve; }));
    const { wrapper } = await mountPage();
    wrapper.unmount();
    finishLoad();
    await flushPromises();
    expect(mocks.ensure).not.toHaveBeenCalled();
    expect(mocks.register).not.toHaveBeenCalled();
  });

  it("does not show a Page Tab while Core reports an outgoing transfer", async () => {
    mocks.snapshot.mockResolvedValue({
      owned: [{ id: tabId, kind: "page", owner: "main", revision: 1, payload: {} }],
      incoming: [], outgoing: [{ tab: { id: tabId }, target: "workspace-a" }], others: [],
    });
    const { wrapper, router } = await mountPage();
    await flushPromises();
    expect(mocks.ensure).not.toHaveBeenCalled();
    expect(mocks.register).not.toHaveBeenCalled();
    expect(router.currentRoute.value.path).toBe("/terminal");
    wrapper.unmount();
  });

  it("releases a newly claimed Core record if the page unmounts before the claim returns", async () => {
    let finishClaim!: (value: unknown) => void;
    mocks.register.mockReturnValue(new Promise((resolve) => { finishClaim = resolve; }));
    const { wrapper } = await mountPage();
    await vi.waitFor(() => expect(mocks.register).toHaveBeenCalledOnce());
    wrapper.unmount();
    finishClaim({ id: tabId, kind: "page", owner: "main", revision: 4, payload: {} });
    await flushPromises();
    expect(mocks.ensure).not.toHaveBeenCalled();
    expect(mocks.unregister).toHaveBeenCalledWith(tabId, 4);
  });

  it("claims Core ownership before showing a newly opened Page Tab", async () => {
    const { wrapper } = await mountPage();
    await flushPromises();
    expect(mocks.register).toHaveBeenCalledWith(expect.objectContaining({ id: tabId, kind: "page" }));
    expect(mocks.register.mock.invocationCallOrder[0]).toBeLessThan(mocks.ensure.mock.invocationCallOrder[0]!);
    expect(mocks.ensure).toHaveBeenCalledWith(item);
    wrapper.unmount();
  });

  it("leaves the local Tab absent when another window wins the Core claim", async () => {
    mocks.register.mockRejectedValue("workspace_tab.conflict");
    const { wrapper } = await mountPage();
    await flushPromises();
    expect(mocks.ensure).not.toHaveBeenCalled();
    expect(wrapper.text()).toContain("workspace_tab.conflict");
    wrapper.unmount();
  });

  it("focuses the winner if ownership changes between snapshot and registration", async () => {
    mocks.snapshot.mockResolvedValueOnce({ owned: [], incoming: [], outgoing: [], others: [] })
      .mockResolvedValueOnce({
        owned: [], incoming: [], outgoing: [],
        others: [{ id: tabId, kind: "page", owner: "workspace-a", terminalPanes: [] }],
      });
    mocks.register.mockRejectedValue("workspace_tab.conflict");
    const { wrapper, router } = await mountPage();
    await flushPromises();
    expect(mocks.ensure).not.toHaveBeenCalled();
    expect(mocks.focus).toHaveBeenCalledWith("workspace-a");
    expect(router.currentRoute.value.path).toBe("/terminal");
    wrapper.unmount();
  });
});
