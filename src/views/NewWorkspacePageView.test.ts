import { flushPromises, mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { createMemoryHistory, createRouter } from "vue-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { i18n } from "../locales";
import { useTipsStore } from "../stores/tips";
import NewWorkspacePageView from "./NewWorkspacePageView.vue";

const client = vi.hoisted(() => ({
  canUseDesktopCore: vi.fn(() => true),
  listHostCatalog: vi.fn(),
  parseCoreApiError: vi.fn(() => null),
}));
const shell = vi.hoisted(() => ({ request: vi.fn() }));
vi.mock("../core-api/client", () => client);
vi.mock("../workspace-tab-shell-action", () => ({ requestWorkspaceTabShellAction: shell.request }));
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
  return { wrapper, router };
}

describe("NewWorkspacePageView", () => {
  beforeEach(() => {
    localStorage.clear();
    i18n.global.locale.value = "zh-CN";
    client.canUseDesktopCore.mockReturnValue(true);
    client.listHostCatalog.mockResolvedValue([{
      host, group: null, tags: [],
      recentConnection: { hostId: host.hostId, connectedAtUnixMs: Date.now() - 60_000, recencySequence: "1", successfulConnectionCount: "1" },
    }]);
    shell.request.mockResolvedValue(undefined);
  });
  afterEach(() => {
    document.body.innerHTML = "";
    vi.clearAllMocks();
  });

  const replaceSource = { closeSourceOnSuccess: true };

  it("uses real recent Host data and asks the owner shell to replace this Page with the target", async () => {
    const { wrapper, router } = await mountPage();
    try {
      expect(wrapper.text()).toContain("QA server");
      expect(wrapper.text()).toContain("上次使用 · 终端");
      expect(wrapper.text()).toContain("尚未使用");

      await wrapper.get(".new-workspace__recent-row .new-workspace__row-actions button:last-child").trigger("click");
      await flushPromises();
      expect(router.currentRoute.value.path).toBe("/new");
      expect(shell.request).toHaveBeenLastCalledWith({ type: "navigate", path: "/sftp",
        query: { hostId: host.hostId, fileOperationId: expect.stringMatching(/^[0-9a-f-]{36}$/) } }, replaceSource);

      await wrapper.get(".new-workspace__tile:nth-child(2) .new-workspace__tile-link").trigger("click");
      await flushPromises();
      expect(shell.request).toHaveBeenLastCalledWith({ type: "new-file", kind: "local" }, replaceSource);

      await wrapper.get(".new-workspace__tile:first-child .new-workspace__tile-link").trigger("click");
      await flushPromises();
      expect(shell.request).toHaveBeenLastCalledWith({ type: "new-terminal", behavior: "local" }, replaceSource);

      await wrapper.get(".new-workspace__recent-row .new-workspace__row-actions button:first-child").trigger("click");
      await flushPromises();
      expect(shell.request).toHaveBeenLastCalledWith({ type: "navigate", path: "/terminal", query: { hostId: host.hostId,
        source: "overview", connectOperationId: expect.stringMatching(/^[0-9a-f-]{36}$/) } }, replaceSource);
    } finally { wrapper.unmount(); }
  });

  it("opens a Host picker from the terminal card without inventing a connection", async () => {
    const { wrapper } = await mountPage();
    try {
      await wrapper.get(".new-workspace__tile:first-child .new-workspace__tile-actions button:last-child").trigger("click");
      expect(shell.request).not.toHaveBeenCalled();
      expect(document.body.querySelector('[role="dialog"]')?.textContent).toContain("QA server");
      document.body.querySelector<HTMLButtonElement>(".new-workspace__picker-list button")?.click();
      await flushPromises();
      expect(shell.request).toHaveBeenCalledWith(expect.objectContaining({ type: "navigate", path: "/terminal" }), replaceSource);
    } finally { wrapper.unmount(); }
  });

  it("keeps the Page usable and reports a localized failure when the target is not created", async () => {
    shell.request.mockRejectedValueOnce(new Error("workspace_tab.view_create_failed"));
    const { wrapper } = await mountPage();
    try {
      const localFiles = wrapper.get(".new-workspace__tile:nth-child(2) .new-workspace__tile-link");
      await localFiles.trigger("click");
      await flushPromises();
      expect(useTipsStore().items.at(-1)).toMatchObject({ tone: "error", message: i18n.global.t("workspaceTabError.action") });
      expect(localFiles.attributes("disabled")).toBeUndefined();
      await localFiles.trigger("click");
      await flushPromises();
      expect(shell.request).toHaveBeenCalledTimes(2);
    } finally { wrapper.unmount(); }
  });
});
