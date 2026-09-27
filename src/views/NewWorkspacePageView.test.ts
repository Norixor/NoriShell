import { flushPromises, mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { createMemoryHistory, createRouter } from "vue-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { i18n } from "../locales";
import { useWorkspaceTabsStore } from "../stores/workspaceTabs";
import NewWorkspacePageView from "./NewWorkspacePageView.vue";

const client = vi.hoisted(() => ({
  canUseDesktopCore: vi.fn(() => true),
  listHostCatalog: vi.fn(),
}));
vi.mock("../core-api/client", () => client);
vi.mock("../saved-connections", () => ({
  onSavedConnectionsChanged: vi.fn(async () => vi.fn()),
}));

const host = {
  hostId: "019d0000-0000-7000-8000-000000000701",
  label: "QA server",
  address: "example.test",
  normalizedAddress: "example.test",
  port: 22,
  username: "user",
  identityId: null,
  favorite: false,
  hasReadyCredential: false,
  stateVersion: "1",
};

async function mountPage() {
  const pinia = createPinia();
  setActivePinia(pinia);
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: "/new", component: NewWorkspacePageView },
      { path: "/hosts", component: { template: "<div />" } },
      { path: "/terminal", component: { template: "<div />" } },
      { path: "/sftp", component: { template: "<div />" } },
    ],
  });
  await router.push("/new");
  await router.isReady();
  const wrapper = mount(NewWorkspacePageView, {
    attachTo: document.body,
    global: { plugins: [pinia, router, i18n] },
  });
  await flushPromises();
  return { wrapper, router, workspaceTabs: useWorkspaceTabsStore(pinia) };
}

describe("NewWorkspacePageView", () => {
  beforeEach(() => {
    localStorage.clear();
    i18n.global.locale.value = "zh-CN";
    client.listHostCatalog.mockResolvedValue([{
      host, group: null, tags: [],
      recentConnection: { hostId: host.hostId, connectedAtUnixMs: Date.now() - 60_000, recencySequence: "1", successfulConnectionCount: "1" },
    }]);
  });
  afterEach(() => {
    document.body.innerHTML = "";
    vi.clearAllMocks();
  });

  it("uses real recent Host data and opens distinct Terminal and File tabs", async () => {
    const { wrapper, router, workspaceTabs } = await mountPage();
    try {
      expect(wrapper.text()).toContain("QA server");
      expect(wrapper.text()).toContain("上次使用 · 终端");
      expect(wrapper.text()).toContain("尚未使用");

      await wrapper.get(".new-workspace__recent-row .new-workspace__row-actions button:last-child").trigger("click");
      await flushPromises();
      expect(router.currentRoute.value.path).toBe("/sftp");
      expect(router.currentRoute.value.query.hostId).toBe(host.hostId);

      await wrapper.get(".new-workspace__card:nth-child(2) .new-workspace__actions button:last-child").trigger("click");
      await flushPromises();
      expect(workspaceTabs.fileTabs).toHaveLength(1);
      expect(workspaceTabs.fileTabs[0]?.kind).toBe("local");

      await wrapper.get(".new-workspace__recent-row .new-workspace__row-actions button:first-child").trigger("click");
      await flushPromises();
      expect(router.currentRoute.value.path).toBe("/terminal");
      expect(router.currentRoute.value.query.hostId).toBe(host.hostId);
      expect(router.currentRoute.value.query.source).toBe("overview");
    } finally { wrapper.unmount(); }
  });

  it("opens a Host picker from the terminal card without inventing a connection", async () => {
    const { wrapper, router } = await mountPage();
    try {
      await wrapper.get(".new-workspace__actions button:first-child").trigger("click");
      expect(document.body.querySelector('[role="dialog"]')?.textContent).toContain("QA server");
      document.body.querySelector<HTMLButtonElement>(".new-workspace__picker-list button")?.click();
      await flushPromises();
      expect(router.currentRoute.value.path).toBe("/terminal");
      expect(router.currentRoute.value.query.hostId).toBe(host.hostId);
    } finally { wrapper.unmount(); }
  });
});
