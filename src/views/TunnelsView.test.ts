import { flushPromises, mount } from "@vue/test-utils";
import { createPinia } from "pinia";
import { createMemoryHistory, createRouter } from "vue-router";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { i18n } from "../locales";
import { useTipsStore } from "../stores/tips";

const client = vi.hoisted(() => ({
  createForwardRule: vi.fn(),
  deleteForwardRule: vi.fn(),
  fetchForwardSessionSnapshot: vi.fn(),
  listForwardRules: vi.fn(),
  listHosts: vi.fn(),
  preflightForwardRule: vi.fn(),
  retainForwardCleanupForExitOnce: vi.fn(),
  startForwardSession: vi.fn(),
  stopForwardSession: vi.fn(),
  updateForwardRule: vi.fn(),
}));

vi.mock("../core-api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../core-api/client")>();
  return { ...actual, canUseDesktopCore: () => true, ...client };
});

import TunnelsView from "./TunnelsView.vue";

const host = {
  hostId: "019d0000-0000-7000-8000-000000000401",
  label: "Network",
  address: "network.example.test",
  normalizedAddress: "network.example.test",
  port: 22,
  username: "deploy",
  identityId: null,
  favorite: false,
  hasReadyCredential: true,
  stateVersion: "3",
};
const localRule = {
  kind: "local" as const,
  hostId: host.hostId,
  localBindAddress: "127.0.0.1",
  localListenPort: 8022,
  remoteTargetHost: "db.internal",
  remoteTargetPort: 5432,
};
const savedRule = {
  ruleId: "019d0000-0000-7000-8000-000000000410",
  label: "Development database",
  hostId: host.hostId,
  rule: localRule,
  stateVersion: "1",
  createdAtUnixMs: 1n,
  updatedAtUnixMs: 1n,
};

function session(overrides: Record<string, unknown> = {}) {
  return {
    sessionId: "019d0000-0000-7000-8000-000000000402",
    hostId: host.hostId,
    generation: "1",
    stateRevision: "2",
    state: "running",
    ruleId: savedRule.ruleId,
    ruleRevision: savedRule.stateVersion,
    ruleSnapshot: localRule,
    startedAtUnixMs: BigInt(Date.now() - 60_000),
    actualBind: { address: "127.0.0.1", port: 8022 },
    childCount: 2,
    listenerToTargetBytes: "2048",
    targetToListenerBytes: "4096",
    failure: null,
    lastChildFailure: null,
    cleanup: {
      listenerClosedOrRemoteCancelled: "notRequired",
      childrenCleared: "notRequired",
      transportDisconnected: "notRequired",
      abandonedChildCount: 0,
      uncertain: false,
    },
    ...overrides,
  };
}

async function mountView() {
  const pinia = createPinia();
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: "/tunnels", component: TunnelsView },
      { path: "/terminal", component: { template: "<div>Terminal</div>" } },
    ],
  });
  await router.push("/tunnels");
  await router.isReady();
  const wrapper = mount(TunnelsView, { global: { plugins: [pinia, router, i18n] } });
  await flushPromises();
  return { router, tips: useTipsStore(pinia), wrapper };
}

describe("TunnelsView operations workspace", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    window.location.hash = "";
    i18n.global.locale.value = "en";
    client.listHosts.mockResolvedValue([host]);
    client.listForwardRules.mockResolvedValue({ rules: [] });
    client.fetchForwardSessionSnapshot.mockResolvedValue({ snapshotRevision: "0", sessions: [] });
    client.preflightForwardRule.mockResolvedValue({
      localBindAvailableAtCheck: true,
      checkedAddress: "127.0.0.1",
      checkedPort: 8022,
      advisoryOnly: true,
    });
  });

  it("runs a saved rule inside visual fixture mode without invoking Tauri Core", async () => {
    window.location.hash = "#/tunnels?visualFixture=tunnels";
    const { tips, wrapper } = await mountView();
    const row = wrapper.findAll("tbody tr").find((candidate) => candidate.text().includes("私有 SOCKS5"));
    expect(row).toBeDefined();
    await row?.find('button[aria-label="Start forward"]').trigger("click");
    await flushPromises();

    expect(client.startForwardSession).not.toHaveBeenCalled();
    const runningRow = wrapper.findAll("tbody tr").find((candidate) => candidate.text().includes("私有 SOCKS5"));
    expect(runningRow?.text()).toContain("Running");
    expect(wrapper.text()).not.toContain("The tunnel operation did not complete");
    expect(tips.items[0]?.title).toBe("A new independent tunnel session was created.");
    wrapper.unmount();
  });

  it("saves a typed rule and starts it with the saved revision fence", async () => {
    client.createForwardRule.mockResolvedValue(savedRule);
    client.startForwardSession.mockResolvedValue(session({ state: "starting", childCount: 0 }));
    const { wrapper } = await mountView();
    await wrapper.find("#forward-label").setValue("Development database");
    await wrapper.find("#forward-listen-port").setValue("8022");
    await wrapper.find("#forward-target-host").setValue("db.internal");
    await wrapper.find("#forward-target-port").setValue("5432");
    await wrapper.findAll("button").find((button) => button.text().includes("Save and start"))?.trigger("click");
    await flushPromises();

    expect(client.createForwardRule).toHaveBeenCalledWith({ label: "Development database", rule: localRule });
    expect(client.startForwardSession).toHaveBeenCalledWith({
      ruleId: savedRule.ruleId,
      ruleRevision: savedRule.stateVersion,
      rule: localRule,
    }, expect.any(Function));
    wrapper.unmount();
  });

  it("filters saved rules and keeps dynamic SOCKS targets absent", async () => {
    const dynamic = {
      ...savedRule,
      ruleId: "019d0000-0000-7000-8000-000000000411",
      label: "Private SOCKS",
      rule: { kind: "dynamic" as const, hostId: host.hostId, localBindAddress: "127.0.0.1", localListenPort: 1080 },
    };
    client.listForwardRules.mockResolvedValue({ rules: [savedRule, dynamic] });
    const { wrapper } = await mountView();
    const search = wrapper.find('input[placeholder="Search name, Host, listener, or target"]');
    await search.setValue("SOCKS");
    expect(wrapper.text()).toContain("Private SOCKS");
    expect(wrapper.text()).not.toContain("Development database");
    expect(wrapper.text()).toContain("Selected by each SOCKS5 request");
    wrapper.unmount();
  });

  it("shows real session traffic and routes host-key review through Terminal", async () => {
    client.listForwardRules.mockResolvedValue({ rules: [savedRule] });
    client.fetchForwardSessionSnapshot.mockResolvedValue({
      snapshotRevision: "3",
      sessions: [session({
        state: "failed",
        failure: { code: "hostKeyReviewRequired", stage: "host-key", messageKey: "errors.forward.hostKey" },
      })],
    });
    const { router, wrapper } = await mountView();
    expect(wrapper.text()).toContain("2.0 KB");
    expect(wrapper.text()).toContain("4.0 KB");
    await wrapper.findAll("button").find((button) => button.text().includes("Verify identity"))?.trigger("click");
    await flushPromises();
    expect(router.currentRoute.value.path).toBe("/terminal");
    expect(router.currentRoute.value.query.hostId).toBe(host.hostId);
    wrapper.unmount();
  });

  it("keeps uncertain cleanup visible and fences one explicitly confirmed exit attempt", async () => {
    const uncertain = session({
      state: "failed",
      stateRevision: "9",
      generation: "4",
      failure: { code: "cleanupUncertain", stage: "cleanup", messageKey: "errors.forward.cleanup" },
      cleanup: {
        listenerClosedOrRemoteCancelled: "uncertain",
        childrenCleared: "complete",
        transportDisconnected: "uncertain",
        abandonedChildCount: 1,
        uncertain: true,
      },
    });
    client.listForwardRules.mockResolvedValue({ rules: [savedRule] });
    client.fetchForwardSessionSnapshot.mockResolvedValue({ snapshotRevision: "9", sessions: [uncertain] });
    client.retainForwardCleanupForExitOnce.mockResolvedValue(uncertain);
    const { wrapper } = await mountView();
    await wrapper.findAll("button").find((button) => button.text().includes("Allow the next exit attempt"))?.trigger("click");
    await flushPromises();
    const confirm = Array.from(document.body.querySelectorAll<HTMLButtonElement>("button"))
      .find((button) => button.textContent?.includes("Accept risk for one attempt"));
    confirm?.click();
    await flushPromises();
    expect(client.retainForwardCleanupForExitOnce).toHaveBeenCalledWith({
      sessionId: uncertain.sessionId,
      expectedGeneration: uncertain.generation,
      expectedStateRevision: uncertain.stateRevision,
      retainUncertainCleanupForExitConfirmed: true,
    });
    wrapper.unmount();
  });
});
