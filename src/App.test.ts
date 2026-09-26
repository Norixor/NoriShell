import { enableAutoUnmount, flushPromises, mount } from "@vue/test-utils";
import { createPinia } from "pinia";
import { defineComponent, h, onActivated, onMounted } from "vue";
import { createMemoryHistory, createRouter, useRoute } from "vue-router";
import { afterEach, describe, expect, it, vi } from "vitest";

import App from "./App.vue";
import { useRouteReveal } from "./routeReveal";
import { usePluginsStore } from "./stores/plugins";
import type { InstalledPluginSummary } from "./core-api/generated/core-api";

enableAutoUnmount(afterEach);

const hooks = vi.hoisted(() => ({
  applicationExit: undefined as undefined | (() => Promise<void>),
  flushAndExit: vi.fn(),
  listInstalledPlugins: vi.fn(),
  listPluginNavigation: vi.fn().mockResolvedValue([]),
  runtimeInvalidated: undefined as undefined | ((event: { payload: { pluginId: string } }) => void),
  runtimeReady: undefined as undefined | ((event: { payload: InstalledPluginSummary }) => void),
}));

vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (event: string, callback: never) => {
    if (event === "application-exit-requested") hooks.applicationExit = callback;
    if (event === "plugin-runtime-invalidated") hooks.runtimeInvalidated = callback;
    if (event === "plugin-runtime-ready") hooks.runtimeReady = callback;
    return vi.fn();
  }),
}));
vi.mock("vue-i18n", async (importOriginal) => ({
  ...await importOriginal<typeof import("vue-i18n")>(),
  useI18n: () => ({ t: (key: string) => key }),
}));
vi.mock("./stores/ui", () => ({
  useUiStore: () => ({ applyPreferences: vi.fn(), locale: "zh-CN", uiZoom: 100, appliedUiZoom: 100, setUiZoom: vi.fn().mockResolvedValue(true) }),
}));
vi.mock("./stores/appUpdate", () => ({ useAppUpdateStore: () => ({ checkForUpdates: vi.fn().mockResolvedValue(undefined) }) }));
vi.mock("./core-api/native-tray", () => ({ readyNativeTrayActions: vi.fn().mockResolvedValue([]), takeNativeTrayAction: vi.fn() }));
vi.mock("./core-api/native-notifications", () => ({ setNativeNotificationContext: vi.fn().mockResolvedValue(undefined) }));
vi.mock("./core-api/client", () => ({
  fetchVaultStatus: vi.fn().mockResolvedValue({ state: "missing" }),
  completePluginSafeModeStartup: vi.fn().mockResolvedValue({}),
  listInstalledPlugins: hooks.listInstalledPlugins,
  listPluginNavigation: hooks.listPluginNavigation,
  requestApplicationExit: vi.fn(),
  setPluginLocale: vi.fn().mockResolvedValue(undefined),
}));
vi.mock("./terminal-workspace-persistence", () => ({
  requestExitAfterTerminalWorkspaceFlush: hooks.flushAndExit,
}));
const PluginTargetStub = defineComponent({
  name: "PluginTargetStub",
  props: {
    targetId: { type: String, required: true },
    instanceKey: { type: String, required: true },
    displayLabel: { type: String, required: true },
  },
  emits: ["availability"],
  setup: (props) => () => h("section", { class: "fixture-target" }, props.targetId),
});

async function mountApplication(path = "/terminal", holdReadyOn: string[] = []) {
  const routeMounted = vi.fn();
  const pendingReveals = new Map<string, () => void>();
  const routeComponent = defineComponent({
    name: "SshTerminalView",
    setup() {
      const routePath = useRoute().path;
      const reveal = useRouteReveal();
      onMounted(() => {
        routeMounted();
        if (holdReadyOn.includes(routePath)) pendingReveals.set(routePath, reveal);
        else reveal();
      });
      onActivated(() => { if (!holdReadyOn.includes(routePath)) reveal(); });
      return () => h("div", { class: "fixture-route" }, "Ordinary route");
    },
  });
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [{ path: "/:pathMatch(.*)*", component: routeComponent }],
  });
  await router.push(path);
  const pinia = createPinia();
  const wrapper = mount(App, {
    attachTo: document.body,
    global: {
      plugins: [pinia, router],
      stubs: {
        NvxWindowFrame: { template: '<div><slot platform="macos" /></div>' },
        NvxAppHeader: true,
        NvxNavigationRail: true,
        NvxTips: true,
        NvxPluginCommandPalette: true,
        NvxPluginExtensionTarget: PluginTargetStub,
        NvxDialog: {
          props: ["modelValue", "title", "description"],
          template:
            '<section v-if="modelValue"><h1>{{ title }}</h1><p>{{ description }}</p><slot /><slot name="actions" /></section>',
        },
        NvxButton: { props: ["disabled"], template: '<button :disabled="disabled"><slot /></button>' },
      },
    },
  });
  await flushPromises();
  return { wrapper, router, routeMounted, pendingReveals, pinia };
}

const enabledPlugin: InstalledPluginSummary = {
  pluginId: "com.norishell.utility-demo",
  name: "Utility Demo",
  publisher: "NoriShell",
  packageKind: "wasm",
  artifactFingerprintSha256: "a".repeat(64),
  activeVersion: "1.0.2",
  packageSha256: "b".repeat(64),
  capabilities: ["uiPanel"],
  grants: [{ capability: "uiPanel", granted: true }],
  hasSettings: false,
  state: "enabled",
  stateVersion: "7",
  installedAtUnixMs: 1n,
  updatedAtUnixMs: 1n,
};

describe("application exit failure", () => {
  it("opens a recoverable retry dialog when an ordinary exit cleanup rejects", async () => {
    hooks.flushAndExit.mockRejectedValueOnce(new Error("cleanup incomplete"));
    const { wrapper } = await mountApplication();

    await hooks.applicationExit?.();
    await flushPromises();

    expect(wrapper.text()).toContain("lifecycle.cleanupFailedTitle");
    expect(wrapper.text()).toContain("lifecycle.cleanupFailed");
    expect(wrapper.text()).toContain("lifecycle.retryCleanupAndQuit");
  });
});

describe("ordinary app content extension regions", () => {
  it("shows a new route only after it reports ready and ignores a stale route", async () => {
    const { wrapper, router, pendingReveals } = await mountApplication("/terminal", ["/hosts", "/sftp"]);
    expect(wrapper.get(".app-main").attributes("aria-busy")).toBe("false");

    await router.push("/hosts");
    await flushPromises();
    expect(wrapper.get(".app-main").attributes("aria-busy")).toBe("true");
    expect(wrapper.get(".app-route-placeholder").text()).toBe("navigation.loading");

    await router.push("/sftp");
    await flushPromises();
    pendingReveals.get("/hosts")?.();
    await flushPromises();
    expect(wrapper.get(".app-main").attributes("aria-busy")).toBe("true");

    pendingReveals.get("/sftp")?.();
    await flushPromises();
    expect(wrapper.get(".app-main").attributes("aria-busy")).toBe("false");
    expect(wrapper.find(".app-route-placeholder").exists()).toBe(false);
  });

  it("reveals a cached terminal when returning from another page", async () => {
    const { wrapper, router } = await mountApplication("/terminal");
    await router.push("/hosts");
    await flushPromises();
    await router.push("/terminal");
    await flushPromises();
    expect(wrapper.get(".app-main").attributes("aria-busy")).toBe("false");
  });

  it("mounts the four registered targets without allocating empty regions or remounting the route", async () => {
    const { wrapper, routeMounted } = await mountApplication();
    const targets = wrapper.findAllComponents(PluginTargetStub);
    expect(targets.map((target) => target.props("targetId"))).toEqual([
      "app.content.before", "app.content.sidebar", "app.content.after", "app.content.footer", "app.content.floating",
    ]);
    expect(new Set(targets.map((target) => target.props("instanceKey"))).size).toBe(1);
    expect(targets[0]?.props("displayLabel")).toBe("navigation.terminal");
    expect(wrapper.findAll("[data-plugin-region]").map((region) => region.attributes("style"))).toEqual(Array(4).fill("display: none;"));
    expect(routeMounted).toHaveBeenCalledOnce();

    targets[0]?.vm.$emit("availability", 2);
    await flushPromises();
    expect(wrapper.get('[data-plugin-region="before"]').isVisible()).toBe(true);
    expect(wrapper.get('[data-plugin-region="sidebar"]').isVisible()).toBe(false);
    targets[0]?.vm.$emit("availability", 0);
    await flushPromises();
    expect(wrapper.get('[data-plugin-region="before"]').isVisible()).toBe(false);
    expect(routeMounted).toHaveBeenCalledOnce();
  });

  it("uses a fresh route lease after navigation and ignores old region availability", async () => {
    const { wrapper, router, routeMounted } = await mountApplication("/hosts");
    const previous = wrapper.findAllComponents(PluginTargetStub)[0]!;
    const previousKey = previous.props("instanceKey");
    previous.vm.$emit("availability", 1);
    await flushPromises();
    await router.push("/sftp");
    await flushPromises();
    const next = wrapper.findAllComponents(PluginTargetStub)[0]!;
    expect(next.props("instanceKey")).not.toBe(previousKey);
    expect(next.props("displayLabel")).toBe("navigation.sftp");
    previous.vm.$emit("availability", 8);
    await flushPromises();
    expect(wrapper.get('[data-plugin-region="before"]').isVisible()).toBe(false);

    const currentKey = next.props("instanceKey");
    await router.push("/sftp?hostId=non-secret-selection");
    await flushPromises();
    expect(wrapper.findAllComponents(PluginTargetStub)[0]?.props("instanceKey")).toBe(currentKey);
    expect(routeMounted).toHaveBeenCalledTimes(2);
  });

  it("does not mount ordinary content targets on sensitive or unreviewed routes", async () => {
    const { wrapper, router } = await mountApplication();
    for (const path of ["/settings", "/settings/identities", "/known-hosts", "/vault", "/secure-plugin-permission", "/future-route"]) {
      await router.push(path);
      await flushPromises();
      expect(wrapper.findAllComponents(PluginTargetStub), path).toHaveLength(0);
      expect(wrapper.findAll("[data-plugin-region]"), path).toHaveLength(0);
    }
    await router.push("/plugin/org.example/inspector");
    await flushPromises();
    expect(wrapper.findAllComponents(PluginTargetStub)).toHaveLength(5);
  });
});

describe("plugin runtime status projection", () => {
  it("refreshes only installed state for runtime events and rejects a late older result", async () => {
    let resolveInvalidated: ((value: InstalledPluginSummary[]) => void) | undefined;
    const invalidatedResult = new Promise<InstalledPluginSummary[]>((resolve) => { resolveInvalidated = resolve; });
    hooks.listInstalledPlugins
      .mockReturnValueOnce(invalidatedResult)
      .mockResolvedValueOnce([{ ...enabledPlugin, state: "crashed", stateVersion: "8" }]);
    const { pinia } = await mountApplication();

    hooks.runtimeInvalidated?.({ payload: { pluginId: enabledPlugin.pluginId } });
    hooks.runtimeReady?.({ payload: enabledPlugin });
    await flushPromises();
    expect(usePluginsStore(pinia).installed).toEqual([{ ...enabledPlugin, state: "crashed", stateVersion: "8" }]);

    resolveInvalidated?.([enabledPlugin]);
    await flushPromises();
    expect(usePluginsStore(pinia).installed).toEqual([{ ...enabledPlugin, state: "crashed", stateVersion: "8" }]);
    expect(hooks.listInstalledPlugins).toHaveBeenCalledTimes(2);
  });

  it("keeps the prior installed projection when an event refresh fails", async () => {
    hooks.listInstalledPlugins.mockRejectedValueOnce(new Error("Core unavailable"));
    const { pinia } = await mountApplication();
    const plugins = usePluginsStore(pinia);
    plugins.installed = [enabledPlugin];

    hooks.runtimeInvalidated?.({ payload: { pluginId: enabledPlugin.pluginId } });
    await flushPromises();

    expect(plugins.installed).toEqual([enabledPlugin]);
    expect(plugins.errorCode).toBe("requestFailed");
    expect(plugins.success).toBeNull();
  });
});
