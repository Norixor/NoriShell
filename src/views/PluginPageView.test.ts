import { flushPromises, mount } from "@vue/test-utils";
import { defineComponent, h, reactive } from "vue";
import { createMemoryHistory, createRouter } from "vue-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { i18n } from "../locales";

const mocks = vi.hoisted(() => ({
  isChild: false,
  pageTabs: [] as { groupId: string }[],
  loadNavigation: vi.fn(),
  listInstalled: vi.fn(),
  ensure: vi.fn(),
  createManaged: vi.fn(),
  activateManaged: vi.fn(),
  snapshot: vi.fn(),
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
vi.mock("../stores/workspaceTabs", async (importOriginal) => ({
  pluginPageTab: (await importOriginal<typeof import("../stores/workspaceTabs")>()).pluginPageTab,
  useWorkspaceTabsStore: () => ({ pageTabs: mocks.pageTabs, ensurePluginPageTab: mocks.ensure }),
}));
vi.mock("../workspace-tab-view-shell", () => ({
  createManagedPageTab: mocks.createManaged,
  activateWorkspaceTabView: mocks.activateManaged,
}));
vi.mock("../workspace-window-context", () => ({ isWorkspaceTabView: () => mocks.isChild }));
vi.mock("../workspace-tab-windows", () => ({
  snapshotWorkspaceTabs: mocks.snapshot,
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
const tab = {
  groupId: tabId, pageType: "plugin", route: "/plugin/org.example.sync/home",
  labelKey: null, label: "Sync", iconName: "cloud",
};
const owned = { id: tabId, kind: "page", owner: "main", payload: tab };
const TargetStub = defineComponent({
  setup(_props, { expose }) {
    expose({ refreshAfterSettingsChange: vi.fn() });
    return () => h("div");
  },
});

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
    global: { plugins: [router, i18n], stubs: {
      NvxPluginExtensionTarget: TargetStub, NvxPluginSettingsDialog: true,
    } },
  });
  return { wrapper, router };
}

describe("plugin Page Tab ownership", () => {
  beforeEach(() => {
    mocks.isChild = false;
    mocks.pageTabs = [];
    extensions.navigation = [item];
    mocks.loadNavigation.mockReset().mockResolvedValue(undefined);
    mocks.listInstalled.mockReset().mockResolvedValue([]);
    mocks.ensure.mockReset();
    mocks.createManaged.mockReset().mockResolvedValue(undefined);
    mocks.activateManaged.mockReset().mockResolvedValue(undefined);
    mocks.snapshot.mockReset().mockResolvedValue({ owned: [], others: [] });
    mocks.focus.mockReset().mockResolvedValue(undefined);
  });
  afterEach(() => { extensions.navigation = []; });

  it("does not create a Tab when navigation loading finishes after unmount", async () => {
    let finishLoad!: () => void;
    mocks.loadNavigation.mockReturnValue(new Promise<void>((resolve) => { finishLoad = resolve; }));
    const { wrapper } = await mountPage();
    wrapper.unmount();
    finishLoad();
    await flushPromises();
    expect(mocks.createManaged).not.toHaveBeenCalled();
    expect(mocks.ensure).not.toHaveBeenCalled();
  });

  it("shows a child Page only when its Core record is owned", async () => {
    mocks.isChild = true;
    mocks.snapshot.mockResolvedValue({ owned: [owned], others: [] });
    const { wrapper } = await mountPage();
    await flushPromises();
    expect(mocks.ensure).toHaveBeenCalledWith(item);
    expect(mocks.createManaged).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("fails closed if a child loses its Core record", async () => {
    mocks.isChild = true;
    const { wrapper } = await mountPage();
    await flushPromises();
    expect(mocks.ensure).not.toHaveBeenCalled();
    expect(wrapper.text()).toContain("workspace_tab.not_found");
    wrapper.unmount();
  });

});
