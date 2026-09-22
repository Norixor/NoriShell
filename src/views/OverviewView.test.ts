import { flushPromises, mount } from "@vue/test-utils";
import { createPinia } from "pinia";
import { createMemoryHistory, createRouter } from "vue-router";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { ServerOverviewSnapshot } from "../core-api/generated/core-api";
import { i18n } from "../locales";

const client = vi.hoisted(() => ({
  secureVault: vi.fn(),
  secureChallenge: vi.fn(),
  decideMetricsHostKey: vi.fn(),
  fetchServerOverview: vi.fn(),
  prepareMetricsKeyboardInteractiveAnswer: vi.fn(),
  reconcileMetrics: vi.fn(),
  respondMetricsKeyboardInteractive: vi.fn(),
  retryMetrics: vi.fn(),
}));

vi.mock("../core-api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../core-api/client")>();
  return {
    ...actual,
    canUseDesktopCore: () => true,
    decideMetricsHostKey: client.decideMetricsHostKey,
    fetchServerOverview: client.fetchServerOverview,
    prepareMetricsKeyboardInteractiveAnswer: client.prepareMetricsKeyboardInteractiveAnswer,
    reconcileMetrics: client.reconcileMetrics,
    respondMetricsKeyboardInteractive: client.respondMetricsKeyboardInteractive,
    retryMetrics: client.retryMetrics,
  };
});

vi.mock("../core-api/secure-ssh-challenge-client", () => ({ requestSecureSshChallenge: client.secureChallenge }));

vi.mock("../core-api/secure-vault-client", () => ({ requestSecureVault: client.secureVault }));

import OverviewView from "./OverviewView.vue";

const hostId = "019d0000-0000-7000-8000-000000000001";
const metricsSessionId = "019d0000-0000-7000-8000-000000000002";
const challengeId = "019d0000-0000-7000-8000-000000000003";

function snapshot(withChallenge = false): ServerOverviewSnapshot {
  return {
    snapshotRevision: "1",
    cards: [{
      catalogEntry: {
        host: {
          hostId,
          label: "Production",
          address: "server.example",
          normalizedAddress: "server.example",
          port: 22,
          username: "deploy",
          identityId: null,
          favorite: false,
          hasReadyCredential: true,
          stateVersion: "1",
        },
        group: null,
        tags: [],
        recentConnection: null,
      },
      monitoringPolicy: {
        hostId,
        revision: "1",
        policy: {
          enabled: withChallenge,
          sampleIntervalSeconds: 15,
          sampleTimeoutSeconds: 5,
          diskMountIds: ["root"],
          networkInterfaceIds: ["aggregateNonLoopback"],
        },
      },
      connectionState: "disconnected",
      terminalCounts: { connecting: 0, running: 0, lost: 0, failed: 0 },
      sftpCounts: { connecting: 0, running: 0, lost: 0, failed: 0 },
      forwardCounts: { connecting: 0, running: 0, lost: 0, failed: 0 },
      metricsCounts: { connecting: withChallenge ? 1 : 0, running: 0, lost: 0, failed: 0 },
      terminalSessionIds: [],
      metricsSession: withChallenge ? {
        metricsSessionId,
        hostId,
        generation: "2",
        stateRevision: "3",
        state: "needsHostKeyReview",
        authenticationReason: null,
        failureCode: null,
        nextRetryAtUnixMs: null,
        latestSnapshot: null,
        hostKeyChallenge: {
          challengeId,
          metricsSessionId,
          hostId,
          generation: "2",
          routeStage: { kind: "target" },
          algorithm: "ssh-ed25519",
          fingerprintSha256: "SHA256:test-fingerprint",
          trustedFingerprintSha256: null,
        },
        keyboardInteractiveChallenge: null,
      } : null,
    }],
  };
}

async function mountView(initialSnapshot: ServerOverviewSnapshot) {
  const pinia = createPinia();
  client.fetchServerOverview.mockResolvedValue(initialSnapshot);
  client.reconcileMetrics.mockResolvedValue([]);
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: "/overview", component: OverviewView },
      { path: "/terminal", component: { template: "<div>Terminal</div>" } },
      { path: "/hosts", component: { template: "<div>Hosts</div>" } },
      { path: "/settings", component: { template: "<div>Settings</div>" } },
    ],
  });
  await router.push("/overview");
  await router.isReady();
  const wrapper = mount(OverviewView, { global: { plugins: [pinia, router, i18n] } });
  await flushPromises();
  return { router, wrapper };
}

describe("OverviewView Core operation boundaries", () => {
  beforeEach(() => {
    client.secureVault.mockReset().mockResolvedValue(true);
    client.secureChallenge.mockReset().mockReturnValue(new Promise(() => undefined));
    vi.clearAllMocks();
    i18n.global.locale.value = "en";
  });

  it("reconciles monitoring and creates one explicit terminal operation", async () => {
    const { router, wrapper } = await mountView(snapshot());
    expect(client.reconcileMetrics).toHaveBeenCalledTimes(1);

    await wrapper.get(".nvx-server-card__new-terminal").trigger("click");
    await flushPromises();
    expect(router.currentRoute.value.path).toBe("/terminal");
    expect(router.currentRoute.value.query.hostId).toBe(hostId);
    expect(router.currentRoute.value.query.source).toBe("overview");
    expect(router.currentRoute.value.query.connectOperationId).toMatch(/^[0-9a-f-]{36}$/);
    wrapper.unmount();
  });

  it("requires an explicit decision for an unknown Metrics host key", async () => {
    client.decideMetricsHostKey.mockResolvedValue({});
    const { wrapper } = await mountView(snapshot(true));

    const review = wrapper.findAll("button")
      .find((candidate) => candidate.attributes("aria-label") === "Review server fingerprint");
    await review?.trigger("click");
    await flushPromises();
    expect(document.body.textContent).not.toContain("SHA256:test-fingerprint");
    expect(client.secureChallenge).toHaveBeenCalledWith({ kind: "metricsHostKey", request: {
      metricsSessionId,
      hostId,
      expectedGeneration: "2",
      challengeId,
    } });
    expect(client.decideMetricsHostKey).not.toHaveBeenCalled();
    wrapper.unmount();
  });
  it.each(["approve", "cancel", "host", "generation", "unmount"])("handles an explicit Vault recovery with %s outcome", async (outcome) => {
    const initial = snapshot(true);
    const metrics = initial.cards[0]!.metricsSession!;
    metrics.hostKeyChallenge = null;
    metrics.state = "needsAuthentication";
    metrics.authenticationReason = "vaultLocked";
    let approve!: (value: boolean) => void;
    client.secureVault.mockReturnValue(new Promise<boolean>((resolve) => { approve = resolve; }));
    const { wrapper, router } = await mountView(initial);
    expect(client.secureVault).not.toHaveBeenCalled();
    await wrapper.findAll("button").find((button) => [i18n.global.t("overview.unlockVault"), i18n.global.t("sshSettings.vault.title")].includes(button.attributes("aria-label") ?? ""))!.trigger("click");
    await flushPromises();
    const fresh = structuredClone(initial);
    if (outcome === "host") fresh.cards[0]!.catalogEntry.host.stateVersion = "99";
    if (outcome === "generation") fresh.cards[0]!.metricsSession!.generation = "99";
    client.fetchServerOverview.mockResolvedValue(fresh);
    if (outcome === "unmount") wrapper.unmount();
    approve(outcome !== "cancel"); await flushPromises();
    expect(client.secureVault).toHaveBeenCalledWith("ensureUnlocked");
    if (outcome === "approve") expect(client.retryMetrics).toHaveBeenCalledWith({ hostId, expectedGeneration: "2" });
    else expect(client.retryMetrics).not.toHaveBeenCalled();
    if (outcome !== "unmount") expect(router.currentRoute.value.path).toBe("/overview");
    if (outcome !== "unmount") wrapper.unmount();
  });

});
