import type {
  ForwardRulePreflightResponse,
  ForwardRuleSummary,
  ForwardSessionSummary,
  HostSummary,
  PortForwardRule,
} from "../core-api/generated/core-api";

const hostId = "019d0000-0000-7000-8000-00000000a001";
const webRule = {
  kind: "local" as const,
  hostId,
  localBindAddress: "127.0.0.1",
  localListenPort: 8080,
  remoteTargetHost: "127.0.0.1",
  remoteTargetPort: 80,
};

export const tunnelVisualFixture: {
  hosts: HostSummary[];
  rules: ForwardRuleSummary[];
  sessions: ForwardSessionSummary[];
} = {
  hosts: [
    {
      hostId,
      label: "生产网关",
      address: "gateway.example.test",
      normalizedAddress: "gateway.example.test",
      port: 22,
      username: "deploy",
      identityId: null,
      favorite: true,
      hasReadyCredential: true,
      stateVersion: "4",
    },
    {
      hostId: "019d0000-0000-7000-8000-00000000a002",
      label: "开发环境",
      address: "dev.example.test",
      normalizedAddress: "dev.example.test",
      port: 22,
      username: "dev",
      identityId: null,
      favorite: false,
      hasReadyCredential: true,
      stateVersion: "2",
    },
  ],
  rules: [
    {
      ruleId: "019d0000-0000-7000-8000-00000000b001",
      label: "生产 Web 预览",
      hostId,
      rule: webRule,
      stateVersion: "1",
      createdAtUnixMs: 1n,
      updatedAtUnixMs: 1n,
    },
    {
      ruleId: "019d0000-0000-7000-8000-00000000b002",
      label: "远端运维入口",
      hostId,
      rule: {
        kind: "remote",
        hostId,
        remoteBindAddress: "127.0.0.1",
        remoteListenPort: 9000,
        localTargetHost: "127.0.0.1",
        localTargetPort: 3000,
      },
      stateVersion: "2",
      createdAtUnixMs: 1n,
      updatedAtUnixMs: 1n,
    },
    {
      ruleId: "019d0000-0000-7000-8000-00000000b003",
      label: "私有 SOCKS5",
      hostId: "019d0000-0000-7000-8000-00000000a002",
      rule: {
        kind: "dynamic",
        hostId: "019d0000-0000-7000-8000-00000000a002",
        localBindAddress: "127.0.0.1",
        localListenPort: 1080,
      },
      stateVersion: "1",
      createdAtUnixMs: 1n,
      updatedAtUnixMs: 1n,
    },
    {
      ruleId: "019d0000-0000-7000-8000-00000000b004",
      label: "远端数据库管理",
      hostId: "019d0000-0000-7000-8000-00000000a002",
      rule: {
        kind: "remote",
        hostId: "019d0000-0000-7000-8000-00000000a002",
        remoteBindAddress: "127.0.0.1",
        remoteListenPort: 15433,
        localTargetHost: "127.0.0.1",
        localTargetPort: 5432,
      },
      stateVersion: "1",
      createdAtUnixMs: 1n,
      updatedAtUnixMs: 1n,
    },
  ],
  sessions: [
    {
      sessionId: "019d0000-0000-7000-8000-00000000c001",
      hostId,
      generation: "1",
      stateRevision: "8",
      state: "running",
      ruleId: "019d0000-0000-7000-8000-00000000b001",
      ruleRevision: "1",
      ruleSnapshot: webRule,
      startedAtUnixMs: BigInt(Date.now() - 38 * 60_000),
      actualBind: { kind: "local", address: "127.0.0.1", port: 8080 },
      childCount: 3,
      listenerToTargetBytes: "2841640",
      targetToListenerBytes: "8421360",
      failure: null,
      lastChildFailure: null,
      cleanup: { listenerClosedOrRemoteCancelled: "notRequired", childrenCleared: "notRequired", transportDisconnected: "notRequired", abandonedChildCount: 0, uncertain: false },
    },
    {
      sessionId: "019d0000-0000-7000-8000-00000000c002",
      hostId,
      generation: "1",
      stateRevision: "4",
      state: "running",
      ruleId: "019d0000-0000-7000-8000-00000000b002",
      ruleRevision: "2",
      ruleSnapshot: {
        kind: "remote",
        hostId,
        remoteBindAddress: "127.0.0.1",
        remoteListenPort: 9000,
        localTargetHost: "127.0.0.1",
        localTargetPort: 3000,
      },
      startedAtUnixMs: BigInt(Date.now() - 14 * 60_000),
      actualBind: { kind: "remote", address: "127.0.0.1", port: 9000 },
      childCount: 1,
      listenerToTargetBytes: "524288",
      targetToListenerBytes: "1258291",
      failure: null,
      lastChildFailure: null,
      cleanup: { listenerClosedOrRemoteCancelled: "notRequired", childrenCleared: "notRequired", transportDisconnected: "notRequired", abandonedChildCount: 0, uncertain: false },
    },
    {
      sessionId: "019d0000-0000-7000-8000-00000000c003",
      hostId: "019d0000-0000-7000-8000-00000000a002",
      generation: "1",
      stateRevision: "5",
      state: "failed",
      ruleId: null,
      ruleRevision: null,
      ruleSnapshot: {
        kind: "local",
        hostId: "019d0000-0000-7000-8000-00000000a002",
        localBindAddress: "127.0.0.1",
        localListenPort: 16379,
        remoteTargetHost: "redis.internal",
        remoteTargetPort: 6379,
      },
      startedAtUnixMs: BigInt(Date.now() - 5_000),
      actualBind: null,
      childCount: 0,
      listenerToTargetBytes: "0",
      targetToListenerBytes: "0",
      failure: { code: "hostKeyMismatch", stage: "host-key", messageKey: "errors.forward.runtime" },
      lastChildFailure: null,
      cleanup: { listenerClosedOrRemoteCancelled: "notRequired", childrenCleared: "complete", transportDisconnected: "complete", abandonedChildCount: 0, uncertain: false },
    },
  ],
};

function nextWireSequence(value: string) {
  return String(Number(value) + 1);
}

export function createTunnelVisualFixtureSession(
  rule: PortForwardRule,
  savedRule: ForwardRuleSummary | null,
): ForwardSessionSummary {
  const actualBind = rule.kind === "remote"
    ? { kind: "remote" as const, address: rule.remoteBindAddress, port: rule.remoteListenPort }
    : { kind: "local" as const, address: rule.localBindAddress, port: rule.localListenPort };
  return {
    sessionId: crypto.randomUUID(),
    hostId: rule.hostId,
    generation: "1",
    stateRevision: "1",
    state: "running",
    ruleId: savedRule?.ruleId ?? null,
    ruleRevision: savedRule?.stateVersion ?? null,
    ruleSnapshot: rule,
    startedAtUnixMs: BigInt(Date.now()),
    actualBind,
    childCount: 0,
    listenerToTargetBytes: "0",
    targetToListenerBytes: "0",
    failure: null,
    lastChildFailure: null,
    cleanup: {
      listenerClosedOrRemoteCancelled: "notRequired",
      childrenCleared: "notRequired",
      transportDisconnected: "notRequired",
      abandonedChildCount: 0,
      uncertain: false,
    },
  };
}

export function stopTunnelVisualFixtureSession(
  session: ForwardSessionSummary,
): ForwardSessionSummary {
  return {
    ...session,
    stateRevision: nextWireSequence(session.stateRevision),
    state: "stopped",
    childCount: 0,
    cleanup: {
      listenerClosedOrRemoteCancelled: "complete",
      childrenCleared: "complete",
      transportDisconnected: "complete",
      abandonedChildCount: 0,
      uncertain: false,
    },
  };
}

export function saveTunnelVisualFixtureRule(
  label: string,
  rule: PortForwardRule,
  existing: ForwardRuleSummary | null,
): ForwardRuleSummary {
  const now = BigInt(Date.now());
  return {
    ruleId: existing?.ruleId ?? crypto.randomUUID(),
    label,
    hostId: rule.hostId,
    rule,
    stateVersion: existing ? nextWireSequence(existing.stateVersion) : "1",
    createdAtUnixMs: existing?.createdAtUnixMs ?? now,
    updatedAtUnixMs: now,
  };
}

export function preflightTunnelVisualFixtureRule(
  rule: PortForwardRule,
): ForwardRulePreflightResponse {
  if (rule.kind === "remote") {
    return {
      localBindAvailableAtCheck: null,
      checkedAddress: null,
      checkedPort: null,
      advisoryOnly: true,
    };
  }
  return {
    localBindAvailableAtCheck: true,
    checkedAddress: rule.localBindAddress,
    checkedPort: rule.localListenPort,
    advisoryOnly: true,
  };
}
