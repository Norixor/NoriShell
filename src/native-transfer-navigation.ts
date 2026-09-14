import type { SftpTransferEndpointFence, SftpTransferIntentSummary, SftpTransferSummary } from "./core-api/generated/core-api";

type Target = { transferId: string; minimumRevision: string } & (
  | { kind: "intent"; source: SftpTransferEndpointFence; target: SftpTransferEndpointFence }
  | { kind: "legacy"; sessionId: string; generation: string }
);
const pending = new Map<string, { expires: number; target: Target }>();
export function registerNativeTransferNavigation(operation: string, target: Target) {
  pending.set(operation, { expires: Date.now() + 60_000, target: structuredClone(target) });
  while (pending.size > 32) pending.delete(pending.keys().next().value!);
}
export function takeNativeTransferNavigation(operation: string): Target | null {
  const entry = pending.get(operation); pending.delete(operation);
  return entry && entry.expires > Date.now() ? entry.target : null;
}
function sameFence(a: SftpTransferEndpointFence, b: SftpTransferEndpointFence) {
  return a.kind === "remoteSession" && b.kind === "remoteSession"
    ? a.sessionId === b.sessionId && a.generation === b.generation
    : a.kind === "localCapability" && b.kind === "localCapability" && a.directoryRef === b.directoryRef && a.revision === b.revision;
}
export function matchesNativeTransferNavigation(target: Target, intents: SftpTransferIntentSummary[], legacy: SftpTransferSummary[]): boolean {
  const revisionMatches = (current: string) => /^\d+$/.test(current) && /^\d+$/.test(target.minimumRevision) && BigInt(current) >= BigInt(target.minimumRevision);
  if (target.kind === "intent") return intents.some((item) => item.transferId === target.transferId && revisionMatches(item.stateRevision) && sameFence(item.sourceFence, target.source) && sameFence(item.targetFence, target.target));
  return legacy.some((item) => item.transferId === target.transferId && revisionMatches(item.stateRevision) && item.sessionId === target.sessionId && item.generation === target.generation);
}
