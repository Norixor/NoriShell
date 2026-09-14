import type { PortForwardRule } from "../core-api/generated/core-api";

export type ForwardKind = PortForwardRule["kind"];

export interface ForwardRuleDraft {
  hostId: string;
  kind: ForwardKind;
  bindAddress: string;
  listenPort: string;
  targetHost: string;
  targetPort: string;
}

export function buildForwardRule(draft: ForwardRuleDraft): PortForwardRule | null {
  const listenPort = Number(draft.listenPort);
  if (
    !draft.hostId
    || !draft.bindAddress.trim()
    || !Number.isInteger(listenPort)
    || listenPort < 1
    || listenPort > 65_535
  ) return null;

  if (draft.kind === "dynamic") {
    return {
      kind: "dynamic",
      hostId: draft.hostId,
      localBindAddress: draft.bindAddress.trim(),
      localListenPort: listenPort,
    };
  }

  const targetPort = Number(draft.targetPort);
  if (
    !draft.targetHost.trim()
    || !Number.isInteger(targetPort)
    || targetPort < 1
    || targetPort > 65_535
  ) return null;

  if (draft.kind === "local") {
    return {
      kind: "local",
      hostId: draft.hostId,
      localBindAddress: draft.bindAddress.trim(),
      localListenPort: listenPort,
      remoteTargetHost: draft.targetHost.trim(),
      remoteTargetPort: targetPort,
    };
  }

  return {
    kind: "remote",
    hostId: draft.hostId,
    remoteBindAddress: draft.bindAddress.trim(),
    remoteListenPort: listenPort,
    localTargetHost: draft.targetHost.trim(),
    localTargetPort: targetPort,
  };
}

export function ruleHostId(rule: PortForwardRule) {
  return rule.hostId;
}

export function ruleListener(rule: PortForwardRule) {
  if (rule.kind === "remote") return `${rule.remoteBindAddress}:${rule.remoteListenPort}`;
  return `${rule.localBindAddress}:${rule.localListenPort}`;
}

export function ruleTarget(rule: PortForwardRule) {
  if (rule.kind === "local") return `${rule.remoteTargetHost}:${rule.remoteTargetPort}`;
  if (rule.kind === "remote") return `${rule.localTargetHost}:${rule.localTargetPort}`;
  return null;
}

export function ruleToDraft(rule: PortForwardRule): ForwardRuleDraft {
  if (rule.kind === "local") {
    return {
      hostId: rule.hostId,
      kind: rule.kind,
      bindAddress: rule.localBindAddress,
      listenPort: String(rule.localListenPort),
      targetHost: rule.remoteTargetHost,
      targetPort: String(rule.remoteTargetPort),
    };
  }
  if (rule.kind === "remote") {
    return {
      hostId: rule.hostId,
      kind: rule.kind,
      bindAddress: rule.remoteBindAddress,
      listenPort: String(rule.remoteListenPort),
      targetHost: rule.localTargetHost,
      targetPort: String(rule.localTargetPort),
    };
  }
  return {
    hostId: rule.hostId,
    kind: rule.kind,
    bindAddress: rule.localBindAddress,
    listenPort: String(rule.localListenPort),
    targetHost: "",
    targetPort: "",
  };
}
