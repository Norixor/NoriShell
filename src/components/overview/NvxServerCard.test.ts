import { mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it } from "vitest";

import type {
  MetricFieldState,
  MetricSnapshot,
  MetricsSessionFailureCode,
  ServerOverviewCard,
} from "../../core-api/generated/core-api";
import { i18n } from "../../locales";
import NvxServerCard from "./NvxServerCard.vue";

const card: ServerOverviewCard = {
  catalogEntry: {
    host: {
      hostId: "019d0000-0000-7000-8000-000000000001",
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
    hostId: "019d0000-0000-7000-8000-000000000001",
    revision: "1",
    policy: {
      enabled: false,
      sampleIntervalSeconds: 15,
      sampleTimeoutSeconds: 5,
      diskMountIds: ["root"],
      networkInterfaceIds: ["aggregateNonLoopback"],
    },
  },
  connectionState: "connected",
  terminalCounts: { connecting: 0, running: 1, lost: 0, failed: 0 },
  sftpCounts: { connecting: 0, running: 0, lost: 0, failed: 0 },
  forwardCounts: { connecting: 0, running: 0, lost: 0, failed: 0 },
  metricsCounts: { connecting: 0, running: 0, lost: 0, failed: 0 },
  terminalSessionIds: ["019d0000-0000-7000-8000-000000000002"],
  metricsSession: null,
};

function metricSnapshot(
  networkState: MetricFieldState,
  stale = false,
): MetricSnapshot {
  return {
    hostId: card.catalogEntry.host.hostId,
    metricsSessionId: "019d0000-0000-7000-8000-000000000003",
    generation: "1",
    sampleSequence: "1",
    providerId: "linux-procfs",
    providerVersion: 1,
    platform: "linux",
    sampleStartedAtUnixMs: 1_700_000_000_000,
    sampleCompletedAtUnixMs: 1_700_000_001_000,
    stale,
    cpu: { state: "available", basisPoints: 0 },
    memory: {
      state: "available",
      usedBytes: "0",
      availableBytes: "1024",
      totalBytes: "1024",
    },
    network: {
      resourceId: "aggregateNonLoopback",
      state: networkState,
      receiveBytesPerSecond: "0",
      transmitBytesPerSecond: "0",
    },
    disks: [{
      resourceId: "root",
      state: "available",
      filesystemId: "root",
      mount: "/",
      usedBytes: "0",
      availableBytes: "1024",
      totalBytes: "1024",
    }],
  };
}

function monitoredCard(
  latestSnapshot: MetricSnapshot | null,
  failureCode: MetricsSessionFailureCode | null = null,
): ServerOverviewCard {
  return {
    ...card,
    monitoringPolicy: {
      ...card.monitoringPolicy,
      policy: { ...card.monitoringPolicy.policy, enabled: true },
    },
    metricsCounts: { connecting: 0, running: 1, lost: 0, failed: 0 },
    metricsSession: {
      metricsSessionId: "019d0000-0000-7000-8000-000000000003",
      hostId: card.catalogEntry.host.hostId,
      generation: "1",
      stateRevision: "1",
      state: failureCode ? "backoff" : "ready",
      authenticationReason: null,
      failureCode,
      nextRetryAtUnixMs: failureCode ? 1_700_000_010_000 : null,
      latestSnapshot,
      hostKeyChallenge: null,
      keyboardInteractiveChallenge: null,
    },
  };
}

describe("NvxServerCard", () => {
  beforeEach(() => {
    i18n.global.locale.value = "zh-CN";
  });

  it("keeps a disabled metric distinct and creates a new terminal for a connected host", async () => {
    const wrapper = mount(NvxServerCard, {
      props: { card },
      global: { plugins: [i18n] },
    });

    expect(wrapper.text()).not.toContain("已连接");
    expect(wrapper.get(".nvx-server-card__connection-dot").attributes("aria-label"))
      .toBe("已连接");
    expect(wrapper.text()).toContain("监控未启用");
    expect(wrapper.text()).toContain("新建终端");
    expect(wrapper.get(".nvx-server-card").classes()).toContain("nvx-card");
    expect(wrapper.text()).not.toContain("终端 1/1");
    expect(wrapper.text()).not.toContain("SFTP 0/0");
    expect(wrapper.findAll(".nvx-server-card__metric")).toHaveLength(4);
    expect(wrapper.text()).toContain("终端 1");
    const network = wrapper.get(".nvx-server-card__network");
    expect(network.attributes("data-status")).toBe("disabled");
    expect(network.text()).not.toContain("B/s");
    const primary = wrapper.get(".nvx-server-card__new-terminal");
    await primary.trigger("click");
    expect(wrapper.emitted("connect")).toEqual([[card.catalogEntry.host.hostId]]);

    const sessionButton = wrapper.get('[aria-label="打开终端会话 1"]');
    await sessionButton.trigger("click");
    expect(wrapper.emitted("focusTerminal")).toEqual([[card.terminalSessionIds[0]]]);
  });

  it("shows the group with the endpoint below the Host name", () => {
    const wrapper = mount(NvxServerCard, {
      props: {
        card: {
          ...card,
          catalogEntry: {
            ...card.catalogEntry,
            group: {
              groupId: "019d0000-0000-7000-8000-000000000101",
              label: "Production Group",
              stateVersion: "1",
            },
          },
        },
      },
      global: { plugins: [i18n] },
    });

    expect(wrapper.get(".nvx-server-card__metadata").text()).toContain("Production Group");
    expect(wrapper.get(".nvx-server-card__group").text()).toBe("Production Group");
  });

  it("always offers New Terminal when no terminal is running", () => {
    const wrapper = mount(NvxServerCard, {
      props: {
        card: {
          ...card,
          connectionState: "disconnected",
          terminalCounts: { connecting: 0, running: 0, lost: 0, failed: 0 },
          terminalSessionIds: [],
        },
      },
      global: { plugins: [i18n] },
    });
    expect(wrapper.text()).toContain("新建终端");
    expect(wrapper.text()).not.toContain("一键连接");
    expect(wrapper.text()).not.toContain("打开终端会话");
  });

  it("explains every contributing issue when the aggregate state is degraded", () => {
    const wrapper = mount(NvxServerCard, {
      props: {
        card: {
          ...monitoredCard(metricSnapshot("available"), "authenticationRejected"),
          connectionState: "degraded",
          terminalCounts: { connecting: 0, running: 1, lost: 1, failed: 0 },
          metricsCounts: { connecting: 0, running: 0, lost: 0, failed: 1 },
        },
      },
      global: { plugins: [i18n] },
    });

    const status = wrapper.get(".nvx-server-card__connection-dot");
    expect(wrapper.text()).not.toContain("部分异常");
    expect(status.attributes("title")).toContain("终端连接已关闭或正在断开（1）");
    expect(status.attributes("title")).toContain("监控连接失败（1）");
    expect(status.attributes("title")).toContain("监控认证被服务器拒绝");
    expect(status.attributes("aria-label")).toContain("部分异常");
  });

  it.each<{
    fieldState: MetricFieldState;
    expectedStatus: string;
    expectedLabel: string;
  }>([
    { fieldState: "initialBaseline", expectedStatus: "loading", expectedLabel: "等待下一次采样" },
    { fieldState: "counterReset", expectedStatus: "loading", expectedLabel: "等待下一次采样" },
    { fieldState: "unsupported", expectedStatus: "unsupported", expectedLabel: "暂不支持" },
    { fieldState: "permissionDenied", expectedStatus: "permissionDenied", expectedLabel: "权限不足" },
    { fieldState: "error", expectedStatus: "error", expectedLabel: "采样失败" },
  ])("renders Network $fieldState as a field state instead of a zero rate", ({
    fieldState,
    expectedStatus,
    expectedLabel,
  }) => {
    const wrapper = mount(NvxServerCard, {
      props: { card: monitoredCard(metricSnapshot(fieldState)) },
      global: { plugins: [i18n] },
    });
    const network = wrapper.get(".nvx-server-card__network");

    expect(network.attributes("data-status")).toBe(expectedStatus);
    expect(network.attributes("aria-label")).toContain(expectedLabel);
    expect(network.text()).not.toContain("B/s");
  });

  it("renders Network loading without inventing a zero rate before the first snapshot", () => {
    const wrapper = mount(NvxServerCard, {
      props: { card: monitoredCard(null) },
      global: { plugins: [i18n] },
    });
    const network = wrapper.get(".nvx-server-card__network");

    expect(network.attributes("data-status")).toBe("loading");
    expect(network.attributes("aria-label")).toContain("正在采样");
    expect(network.text()).not.toContain("B/s");
  });

  it("ends the sampling busy state while monitoring waits for Vault unlock", () => {
    const waitingForVault: ServerOverviewCard = {
      ...monitoredCard(null),
      connectionState: "disconnected",
      metricsCounts: { connecting: 0, running: 0, lost: 1, failed: 0 },
      metricsSession: {
        ...monitoredCard(null).metricsSession!,
        state: "needsAuthentication",
        authenticationReason: "vaultLocked",
      },
    };
    const wrapper = mount(NvxServerCard, {
      props: { card: waitingForVault },
      global: { plugins: [i18n] },
    });

    expect(wrapper.text()).not.toContain("未连接");
    expect(wrapper.get(".nvx-server-card__connection-dot").attributes("aria-label"))
      .toBe("未连接");
    expect(wrapper.text()).toContain("等待解锁 Vault");
    expect(wrapper.text()).not.toContain("前往解锁 Vault");
    expect(wrapper.find('button[aria-label="解锁并继续监控"]').exists()).toBe(true);
    expect(wrapper.text()).not.toContain("正在采样");
    expect(wrapper.get(".nvx-server-card__network").attributes("data-status")).toBe("disabled");
  });

  it("keeps stale zero rates visible but labels them separately from fresh zero rates", () => {
    const staleWrapper = mount(NvxServerCard, {
      props: {
        card: monitoredCard(metricSnapshot("available", true), "sampleTimedOut"),
      },
      global: { plugins: [i18n] },
    });
    const staleNetwork = staleWrapper.get(".nvx-server-card__network");

    expect(staleNetwork.attributes("data-status")).toBe("stale");
    expect(staleWrapper.text()).toContain("数据已过期");
    expect(staleNetwork.findAll(".nvx-server-card__network-values > span")).toHaveLength(2);
    expect(staleNetwork.text().match(/0B/g)).toHaveLength(2);
    expect(staleNetwork.attributes("aria-label")?.match(/0\.0 B\/s/g)).toHaveLength(2);

    const freshWrapper = mount(NvxServerCard, {
      props: { card: monitoredCard(metricSnapshot("available")) },
      global: { plugins: [i18n] },
    });
    const freshNetwork = freshWrapper.get(".nvx-server-card__network");

    expect(freshNetwork.attributes("data-status")).toBe("available");
    expect(freshNetwork.text()).not.toContain("数据已过期");
    expect(freshNetwork.findAll(".nvx-server-card__network-values > span")).toHaveLength(2);
    expect(freshNetwork.text().match(/0B/g)).toHaveLength(2);
    expect(freshNetwork.attributes("aria-label")?.match(/0\.0 B\/s/g)).toHaveLength(2);
    expect(freshWrapper.text()).not.toContain("linux-procfs v1 · Linux");
  });
});
