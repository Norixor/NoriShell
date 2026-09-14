import { mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { createMemoryHistory, createRouter } from "vue-router";
import { describe, expect, it, vi } from "vitest";

import { i18n } from "../../locales";
import { usePluginExtensionsStore } from "../../stores/pluginExtensions";
import NvxNavigationRail from "./NvxNavigationRail.vue";

vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => false }));

describe("NvxNavigationRail", () => {
  it("uses the plugin name instead of a long plugin page title", async () => {
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
        label: "Norixor 同步",
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
    expect(pluginLink.text()).toContain("Norixor");
    expect(pluginLink.text()).not.toContain("同步");
  });
});
