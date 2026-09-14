import { Channel, invoke } from "@tauri-apps/api/core";
import { createUuidV7 } from "./ids";
import { coreApiCommands } from "./generated/core-api";
import type { PluginTerminalInputLease } from "./generated/core-api";

export type {
  PluginTerminalProfile,
  PluginTerminalSessionState,
  PluginProtocolLaunchSummary,
  PluginTerminalSummary as PluginTerminalSessionSummary,
  PluginTerminalAttachment as PluginTerminalSessionAttachment,
  PluginTerminalFrame as PluginTerminalOutputFrame,
  PluginTerminalGap as PluginTerminalOutputGap,
  PluginTerminalOutputItem as PluginTerminalSessionOutputItem,
  PluginTerminalEvent as PluginTerminalSessionEvent,
  PluginTerminalSnapshot as PluginTerminalSessionSnapshot,
} from "./generated/core-api";
import type {
  PluginTerminalProfile,
  PluginProtocolLaunchSummary,
  PluginTerminalSummary as PluginTerminalSessionSummary,
  PluginTerminalAttachment as PluginTerminalSessionAttachment,
  PluginTerminalOutputItem as PluginTerminalSessionOutputItem,
  PluginTerminalEvent as PluginTerminalSessionEvent,
  PluginTerminalSnapshot as PluginTerminalSessionSnapshot,
} from "./generated/core-api";
export type PluginTerminalConfiguration = PluginTerminalProfile["configuration"];
export type PluginInputLease = PluginTerminalInputLease;
interface SessionFence { sessionId: string; expectedGeneration: string; expectedStateRevision: string }
interface InputFence extends SessionFence {
  streamId: string; attachmentId: string; viewId: string;
  leaseId: string; focusEpoch: string; inputEpoch: string;
}
function meta() { return { requestId: crypto.randomUUID() }; }
function operation(name: string, operationId = createUuidV7()) {
  return { meta: meta(), operationId, idempotencyKey: `plugin-terminal-${name}-${operationId}` };
}
export function listPluginProtocolLaunches(): Promise<PluginProtocolLaunchSummary[]> {
  return invoke(coreApiCommands.pluginProtocolLaunchList, { request: { meta: meta() } });
}
export function claimPluginProtocolLaunch(launch: PluginProtocolLaunchSummary): Promise<{ session: PluginTerminalSessionSummary }> {
  return invoke(coreApiCommands.pluginProtocolLaunchClaim, {
    request: { meta: meta(), launchId: launch.launchId, expectedRevision: launch.revision },
  });
}
export function fetchPluginTerminalSessionSnapshot(): Promise<PluginTerminalSessionSnapshot> {
  return invoke(coreApiCommands.pluginTerminalSnapshot, { request: { meta: meta() } });
}
export function openPluginTerminalSession(input: PluginTerminalProfile & { label: string; tabId: string; paneId: string; rows: number; cols: number }, operationId?: string): Promise<{ session: PluginTerminalSessionSummary }> {
  return invoke(coreApiCommands.pluginTerminalOpen, { request: { ...operation("open", operationId), ...input } });
}
export function attachPluginTerminalSession(input: SessionFence & { streamId: string; viewId: string; afterOutputSeq: string | null }, onEvent: (event: PluginTerminalSessionEvent) => void): Promise<{ session: PluginTerminalSessionSummary; attachment: PluginTerminalSessionAttachment; replay: PluginTerminalSessionOutputItem[] }> {
  const channel = new Channel<PluginTerminalSessionEvent>();
  channel.onmessage = onEvent;
  return invoke(coreApiCommands.pluginTerminalAttach, { request: { ...operation("attach"), attachAttemptId: createUuidV7(), ...input }, onEvent: channel });
}
export function heartbeatPluginAttachment(input: { sessionId: string; expectedGeneration: string; expectedAttachmentRevision: string; attachmentId: string; viewId: string }): Promise<PluginTerminalSessionAttachment> {
  return invoke(coreApiCommands.pluginTerminalAttachmentHeartbeat, { request: { meta: meta(), ...input } });
}
export function detachPluginTerminalSession(input: SessionFence & { expectedAttachmentRevision: string; attachmentId: string; viewId: string; intent: "userClose" | "rendererUnavailable"; disconnectIfLast: boolean }): Promise<{ session: PluginTerminalSessionSummary; remainingAttachmentCount: number }> {
  return invoke(coreApiCommands.pluginTerminalDetach, { request: { ...operation("detach"), ...input } });
}
export function renewPluginInputLease(input: InputFence): Promise<PluginInputLease> {
  return invoke(coreApiCommands.pluginTerminalInputLeaseRenew, { request: { meta: meta(), ...input } });
}
export function sendPluginInput(input: InputFence & { clientSeq: string; bytes: number[] }): Promise<void> {
  return invoke(coreApiCommands.pluginTerminalInput, { request: { meta: meta(), ...input } });
}
export function resizePluginTerminal(input: InputFence & { resizeSeq: string; rows: number; cols: number }): Promise<void> {
  return invoke(coreApiCommands.pluginTerminalResize, { request: { meta: meta(), ...input } });
}
export function disconnectPluginTerminalSession(input: SessionFence): Promise<PluginTerminalSessionSummary> {
  return invoke(coreApiCommands.pluginTerminalDisconnect, { request: { ...operation("disconnect"), ...input } });
}
export function reconnectPluginTerminalSession(input: SessionFence & { rows: number; cols: number }): Promise<PluginTerminalSessionSummary> {
  return invoke(coreApiCommands.pluginTerminalReconnect, { request: { ...operation("reconnect"), ...input } });
}
