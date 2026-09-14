import type {
  ServerOverviewCard,
  ServerOverviewConnectionState,
  ServerOverviewSnapshot,
} from "../core-api/generated/core-api";

function card(
  index: number,
  label: string,
  address: string,
  groupId: string,
  groupLabel: string,
  connectionState: ServerOverviewConnectionState,
  cpuBasisPoints: number,
  memoryPercent: number,
  diskPercent: number,
): ServerOverviewCard {
  const suffix = String(index).padStart(12, "0");
  const hostId = `019d0000-0000-7000-8000-${suffix}`;
  const metricsSessionId = `019d0000-0000-7000-9000-${suffix}`;
  const totalBytes = 100_000n;
  return {
    catalogEntry: {
      host: {
        hostId,
        label,
        address,
        normalizedAddress: address,
        port: 22,
        username: "root",
        identityId: null,
        favorite: index === 1,
        hasReadyCredential: true,
        stateVersion: "1",
      },
      group: { groupId, label: groupLabel, stateVersion: "1" },
      tags: [],
      recentConnection: null,
    },
    monitoringPolicy: {
      hostId,
      revision: "1",
      policy: {
        enabled: true,
        sampleIntervalSeconds: 15,
        sampleTimeoutSeconds: 5,
        diskMountIds: ["root"],
        networkInterfaceIds: ["aggregateNonLoopback"],
      },
    },
    connectionState,
    terminalCounts: {
      connecting: 0,
      running: connectionState === "connected" ? 1 : 0,
      lost: 0,
      failed: 0,
    },
    sftpCounts: { connecting: 0, running: 0, lost: 0, failed: 0 },
    forwardCounts: { connecting: 0, running: 0, lost: 0, failed: 0 },
    metricsCounts: { connecting: 0, running: 1, lost: 0, failed: 0 },
    terminalSessionIds: connectionState === "connected"
      ? [`019d0000-0000-7000-a000-${suffix}`]
      : [],
    metricsSession: {
      metricsSessionId,
      hostId,
      generation: "1",
      stateRevision: "1",
      state: "ready",
      authenticationReason: null,
      failureCode: null,
      nextRetryAtUnixMs: null,
      latestSnapshot: {
        hostId,
        metricsSessionId,
        generation: "1",
        sampleSequence: "1",
        providerId: "linux-procfs",
        providerVersion: 1,
        platform: "linux",
        sampleStartedAtUnixMs: Date.now() - 16_000,
        sampleCompletedAtUnixMs: Date.now() - 15_000,
        stale: false,
        cpu: { state: "available", basisPoints: cpuBasisPoints },
        memory: {
          state: "available",
          usedBytes: String((totalBytes * BigInt(memoryPercent)) / 100n),
          availableBytes: String((totalBytes * BigInt(100 - memoryPercent)) / 100n),
          totalBytes: String(totalBytes),
        },
        network: {
          resourceId: "aggregateNonLoopback",
          state: "available",
          receiveBytesPerSecond: String(3_200_000 + index * 120_000),
          transmitBytesPerSecond: String(800_000 + index * 40_000),
        },
        disks: [{
          resourceId: "root",
          state: "available",
          filesystemId: "root",
          mount: "/",
          usedBytes: String((totalBytes * BigInt(diskPercent)) / 100n),
          availableBytes: String((totalBytes * BigInt(100 - diskPercent)) / 100n),
          totalBytes: String(totalBytes),
        }],
      },
      hostKeyChallenge: null,
      keyboardInteractiveChallenge: null,
    },
  };
}

export const overviewVisualFixture: ServerOverviewSnapshot = {
  snapshotRevision: "1",
  cards: [
    card(1, "优优科技公司生产环境核心服务器", "47.93.189.24", "production", "生产环境", "connected", 1_400, 48, 40),
    card(2, "生产应用 01", "10.0.1.11", "production", "生产环境", "connected", 4_200, 43, 59),
    card(3, "生产数据库", "10.0.1.21", "production", "生产环境", "degraded", 6_800, 72, 66),
    card(4, "预发布网关", "10.0.2.10", "staging", "预发布", "connected", 2_600, 38, 31),
    card(5, "预发布应用", "10.0.2.11", "staging", "预发布", "disconnected", 0, 0, 0),
    card(6, "备份节点", "10.0.3.18", "infrastructure", "基础设施", "connected", 1_800, 51, 76),
  ],
};
