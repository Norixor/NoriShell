import { mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { createMemoryHistory, createRouter } from "vue-router";
import { flushPromises } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { i18n } from "../../locales";
import { useAppUpdateStore } from "../../stores/appUpdate";
import { usePluginExtensionsStore } from "../../stores/pluginExtensions";
import NvxNavigationRail from "./NvxNavigationRail.vue";

const native = vi.hoisted(() => ({
  tauri: false,
  showShellRoute: vi.fn(async (navigate?: () => unknown) => { await navigate?.(); }),
  openPluginPage: vi.fn(async () => undefined),
}));
vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => native.tauri }));
vi.mock("../../workspace-tab-view-shell", () => ({
  showWorkspaceShellRoute: native.showShellRoute,
  openManagedPluginPage: native.openPluginPage,
}));

import { retainWorkspaceTabViewSummaries, setActiveWorkspaceTabView, setWorkspaceTabViewSummary } from "../../workspace-tab-view-state";

const pluginItem = {
  pluginId: "org.norixor",
  pluginName: "Norixor",
  artifactFingerprintSha256: "a".repeat(64),
  packageSha256: "b".repeat(64),
  instanceGeneration: "1",
  stateVersion: "1",
  contributionRevision: "1",
  navigation: { navigationId: "norixorSync", label: "同步", icon: "shield", pageId: "account", order: 100 },
};

async function mountRail(path = "/") {
  const pinia = createPinia();
  setActivePinia(pinia);
  usePluginExtensionsStore().navigation = [pluginItem];
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [{ path: "/:pathMatch(.*)*", component: { template: "<div />" } }],
  });
  await router.push(path);
  await router.isReady();
  return { router, wrapper: mount(NvxNavigationRail, { global: { plugins: [pinia, router, i18n] } }) };
}

const selected = (wrapper: Awaited<ReturnType<typeof mountRail>>["wrapper"]) =>
  wrapper.findAll("a.router-link-active").map((link) => link.attributes("href"));

describe("NvxNavigationRail", () => {
  beforeEach(() => {
    native.tauri = false;
    native.showShellRoute.mockReset().mockImplementation(async (navigate?: () => unknown) => { await navigate?.(); });
    native.openPluginPage.mockClear();
    retainWorkspaceTabViewSummaries(new Set());
    setActiveWorkspaceTabView(null);
  });

  it("selects the page of the active Tab instead of the shell route it covers", async () => {
    const { wrapper } = await mountRail("/terminal");
    expect(selected(wrapper)).toEqual(["/terminal"]);
    for (const [id, route] of [["file-1", "/sftp"], ["plugin-1", "/plugin/org.norixor/account"], ["new-1", "/new"], ["keys-1", "/known-hosts"]] as const) {
      setWorkspaceTabViewSummary({ id, viewLabel: id, kind: id === "file-1" ? "file" : "page", label: id, stateLabel: "", route });
    }
    for (const [id, href] of [["file-1", "/sftp"], ["plugin-1", "/plugin/org.norixor/account"], ["new-1", "/terminal"], ["keys-1", "/settings"]] as const) {
      setActiveWorkspaceTabView(id);
      await flushPromises();
      expect(selected(wrapper)).toEqual([href]);
      expect(wrapper.get(`a[href="${href}"]`).attributes("aria-current")).toBe("page");
    }
    setActiveWorkspaceTabView(null);
    await flushPromises();
    expect(selected(wrapper)).toEqual(["/terminal"]);
  });

  it("opens a plugin page directly as its Page Tab without rendering it in the shell", async () => {
    native.tauri = true;
    const { wrapper, router } = await mountRail("/terminal");
    await wrapper.get('a[href="/plugin/org.norixor/account"]').trigger("click");
    await flushPromises();
    expect(native.openPluginPage).toHaveBeenCalledWith(pluginItem);
    expect(router.currentRoute.value.path).toBe("/terminal");
  });

  it("releases the active Tab before showing a shell page, even on the same route", async () => {
    native.tauri = true;
    const { wrapper, router } = await mountRail("/hosts");
    setWorkspaceTabViewSummary({ id: "terminal-1", viewLabel: "terminal-1", kind: "terminal", label: "ops", stateLabel: "", route: "/terminal" });
    setActiveWorkspaceTabView("terminal-1");
    await wrapper.get('a[href="/hosts"]').trigger("click");
    await flushPromises();
    expect(native.showShellRoute).toHaveBeenCalledOnce();
    await wrapper.get('a[href="/tunnels"]').trigger("click");
    await flushPromises();
    expect(router.currentRoute.value.path).toBe("/tunnels");
  });

  it("hands navigation to the shell so the Tab covers the page until it painted", async () => {
    native.tauri = true;
    const { wrapper, router } = await mountRail("/hosts");
    let navigate: (() => unknown) | undefined;
    native.showShellRoute.mockImplementationOnce(async (callback?: () => unknown) => { navigate = callback; });
    await wrapper.get('a[href="/tunnels"]').trigger("click");
    await flushPromises();
    // The Rail does not navigate on its own; the shell decides when.
    expect(router.currentRoute.value.path).toBe("/hosts");
    await navigate?.();
    await flushPromises();
    expect(router.currentRoute.value.path).toBe("/tunnels");
  });

  it("still navigates when the active Tab cannot release input", async () => {
    native.tauri = true;
    const { wrapper, router } = await mountRail("/hosts");
    native.showShellRoute.mockRejectedValueOnce(new Error("workspace_tab.input_focus_release_failed"));
    await wrapper.get('a[href="/tunnels"]').trigger("click");
    await flushPromises();
    expect(router.currentRoute.value.path).toBe("/tunnels");
  });

  it("uses the plugin's localized navigation label", async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const extensions = usePluginExtensionsStore();
    extensions.navigation = [{
      pluginId: "org.norixor",
      pluginName: "Norixor",
      artifactFingerprintSha256: "a".repeat(64),
      packageSha256: "b".repeat(64),
      instanceGeneration: "1",
      stateVersion: "1",
      contributionRevision: "1",
      navigation: {
        navigationId: "norixorSync",
        label: "同步",
        icon: "shield",
        pageId: "account",
        order: 100,
      },
    }];
    const router = createRouter({
      history: createMemoryHistory(),
      routes: [{ path: "/:pathMatch(.*)*", component: { template: "<div />" } }],
    });
    await router.push("/");
    await router.isReady();

    const wrapper = mount(NvxNavigationRail, {
      global: { plugins: [pinia, router, i18n] },
    });
    const pluginLink = wrapper.get('a[href="/plugin/org.norixor/account"]');
    expect(pluginLink.text()).toBe("同步");
  });

  it("shows check status beside the version and opens the update confirmation from a new release", async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const updates = useAppUpdateStore();
    updates.checkedVersion = "0.1.5";
    const router = createRouter({
      history: createMemoryHistory(),
      routes: [{ path: "/:pathMatch(.*)*", component: { template: "<div />" } }],
    });
    await router.push("/");
    await router.isReady();

    const wrapper = mount(NvxNavigationRail, {
      global: { plugins: [pinia, router, i18n] },
    });
    expect(wrapper.find(".nvx-navigation-rail__main a[href='/terminal']").exists()).toBe(true);
    const footer = wrapper.get(".nvx-navigation-rail__footer");
    expect(footer.find("a[href='/settings']").exists()).toBe(true);
    expect(footer.text()).toContain("v0.1.5");
    expect(footer.find(".nvx-navigation-rail__status-dot--neutral").exists()).toBe(true);

    updates.status = "upToDate";
    await wrapper.vm.$nextTick();
    expect(footer.find(".nvx-navigation-rail__status-dot--success").exists()).toBe(true);

    updates.latestVersion = "0.1.6";
    updates.supportsAutoInstall = true;
    updates.status = "updateAvailable";
    await wrapper.vm.$nextTick();
    expect(footer.text()).toContain("v0.1.5");
    expect(footer.find(".nvx-navigation-rail__status-dot--warning").exists()).toBe(true);
    const updateLink = footer.get("a[href='/settings?section=about']");
    expect(updateLink.attributes("aria-label")).toContain(i18n.global.t("releases.versionUpdateAction"));
    expect(updates.installConfirmPending).toBe(false);
    await updateLink.trigger("click");
    expect(updates.installConfirmPending).toBe(true);

    updates.supportsAutoInstall = false;
    await wrapper.vm.$nextTick();
    expect(footer.get("a[href='/settings?section=about']").attributes("aria-label")).toContain(i18n.global.t("releases.versionUpdateManualAction"));

    updates.status = "failed";
    updates.checkFailureCode = "requestFailed";
    await wrapper.vm.$nextTick();
    expect(footer.find(".nvx-navigation-rail__status-dot--danger").exists()).toBe(true);
    expect(footer.find("a[href='/settings?section=about']").exists()).toBe(false);
    expect(footer.get("[role='status']").attributes("aria-label")).toContain(i18n.global.t("releases.checkErrors.requestFailed"));
  });
});
