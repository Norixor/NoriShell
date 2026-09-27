import { parseTerminalLayout, type TerminalLayoutNode } from "./components/terminal/terminalLayout";
import type { PersistedTerminalWorkspaceLayout } from "./components/terminal/terminalWorkspaceLayout";
import type { SshSessionTarget } from "./core-api/generated/core-api";
import type { TerminalOutputGeometryMarker } from "./terminal-output-geometry";

export const MAX_TERMINAL_TAB_HANDOFF_BYTES = 256 * 1024;

export type TerminalHandoffPane =
  | { kind: "launcher"; paneId: string; label: string }
  | { kind: "ssh" | "local" | "telnet" | "plugin"; paneId: string; label: string;
      sessionId: string; generation: string; initialDimensions?: { rows: number; cols: number };
      outputGeometry?: TerminalOutputGeometryMarker[] }
  | { kind: "sshDeferred"; paneId: string; label: string; target: SshSessionTarget }
  | { kind: "localDeferred"; paneId: string; label: string }
  | { kind: "telnetDeferred"; paneId: string; label: string; endpoint: { address: string; port: number } }
  | { kind: "pluginDeferred"; paneId: string; label: string; pluginId: string; providerId: string; schemaHash: string };

export interface TerminalTabHandoffSnapshot {
  schemaVersion: 1;
  tabId: string;
  layout: TerminalLayoutNode;
  activePaneId: string;
  panes: TerminalHandoffPane[];
}

/** Replace only this window's Tabs; owned IDs absent locally are closed tombstones. */
export function mergeOwnedTerminalWorkspaceLayout(
  latest: PersistedTerminalWorkspaceLayout,
  local: PersistedTerminalWorkspaceLayout,
  ownedTabIds: ReadonlySet<string>,
): PersistedTerminalWorkspaceLayout {
  const localById = new Map(local.tabs.filter((tab) => ownedTabIds.has(tab.tabId)).map((tab) => [tab.tabId, tab]));
  const mergedTabs = latest.tabs.flatMap((tab) => {
    if (!ownedTabIds.has(tab.tabId)) return [tab];
    const replacement = localById.get(tab.tabId);
    localById.delete(tab.tabId);
    return replacement ? [replacement] : [];
  });
  mergedTabs.push(...localById.values());
  const mergedIds = new Set(mergedTabs.map((tab) => tab.tabId));
  return {
    schemaVersion: 1,
    activeTabId: latest.activeTabId && mergedIds.has(latest.activeTabId)
      ? latest.activeTabId
      : local.activeTabId && mergedIds.has(local.activeTabId)
        ? local.activeTabId
        : mergedTabs[0]?.tabId ?? null,
    tabs: mergedTabs,
  };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isSafeText(value: unknown, maxLength = 128): value is string {
  return typeof value === "string" && value.length > 0 && value.length <= maxLength
    && Array.from(value).every((character) => character.charCodeAt(0) > 31 && character.charCodeAt(0) !== 127);
}

function parseOutputGeometry(value: unknown): TerminalOutputGeometryMarker[] | null {
  if (!Array.isArray(value) || !value.length || value.length > 4096) return null;
  const markers: TerminalOutputGeometryMarker[] = [];
  let previous = -1n;
  for (const raw of value) {
    if (!isRecord(raw) || Object.keys(raw).some((key) => !["afterOutputSeq", "rows", "cols"].includes(key))
      || typeof raw.afterOutputSeq !== "string" || raw.afterOutputSeq.length > 20
      || !/^(0|[1-9][0-9]*)$/.test(raw.afterOutputSeq)
      || !Number.isInteger(raw.rows) || !Number.isInteger(raw.cols)
      || (raw.rows as number) < 1 || (raw.rows as number) > 1000
      || (raw.cols as number) < 1 || (raw.cols as number) > 1000) return null;
    const sequence = BigInt(raw.afterOutputSeq);
    if (sequence <= previous || sequence > 18446744073709551615n) return null;
    previous = sequence;
    markers.push({ afterOutputSeq: raw.afterOutputSeq, rows: raw.rows as number, cols: raw.cols as number });
  }
  return markers[0]?.afterOutputSeq === "0" ? markers : null;
}

function layoutPaneIds(node: TerminalLayoutNode): string[] {
  return node.kind === "pane" ? [node.paneId] : [...layoutPaneIds(node.first), ...layoutPaneIds(node.second)];
}

function parseEndpoint(value: unknown): { address: string; port: number; username?: string | null } | null {
  if (!isRecord(value) || !isSafeText(value.address, 512) || !Number.isInteger(value.port)
    || (value.port as number) < 1 || (value.port as number) > 65535
    || !(value.username === undefined || value.username === null || isSafeText(value.username, 256))
    || Object.keys(value).some((key) => !["address", "port", "username"].includes(key))) return null;
  return { address: value.address, port: value.port as number,
    ...(value.username === undefined ? {} : { username: value.username as string | null }) };
}

function parseSshTarget(value: unknown): SshSessionTarget | null {
  if (!isRecord(value)) return null;
  if (value.kind === "host" && isSafeText(value.hostId)
    && isSafeText(value.expectedHostStateVersion)
    && Object.keys(value).every((key) => ["kind", "hostId", "expectedHostStateVersion"].includes(key))) {
    return { kind: "host", hostId: value.hostId, expectedHostStateVersion: value.expectedHostStateVersion };
  }
  if (value.kind === "quickConnect" && Object.keys(value).every((key) => ["kind", "endpoint"].includes(key))) {
    const endpoint = parseEndpoint(value.endpoint);
    if (endpoint) return { kind: "quickConnect", endpoint: { ...endpoint, username: endpoint.username ?? null } };
  }
  return null;
}

/** Parse the IPC payload without admitting credentials, plugin configuration, or extra fields. */
export function parseTerminalTabHandoff(value: unknown): TerminalTabHandoffSnapshot | null {
  let encoded: string;
  try { encoded = JSON.stringify(value); } catch { return null; }
  if (new TextEncoder().encode(encoded).byteLength > MAX_TERMINAL_TAB_HANDOFF_BYTES || !isRecord(value)
    || value.schemaVersion !== 1 || !isSafeText(value.tabId) || !isSafeText(value.activePaneId)
    || !Array.isArray(value.panes)
    || Object.keys(value).some((key) => !["schemaVersion", "tabId", "layout", "activePaneId", "panes"].includes(key))) return null;
  const layout = parseTerminalLayout(value.layout);
  if (!layout) return null;
  const expectedPaneIds = new Set(layoutPaneIds(layout));
  if (value.panes.length !== expectedPaneIds.size || !expectedPaneIds.has(value.activePaneId)) return null;
  const panes: TerminalHandoffPane[] = [];
  for (const raw of value.panes) {
    if (!isRecord(raw) || !isSafeText(raw.paneId) || !isSafeText(raw.label, 512)
      || !expectedPaneIds.delete(raw.paneId)) return null;
    if (raw.kind === "launcher") {
      if (Object.keys(raw).some((key) => !["kind", "paneId", "label"].includes(key))) return null;
      panes.push({ kind: "launcher", paneId: raw.paneId, label: raw.label });
      continue;
    }
    if (raw.kind === "localDeferred") {
      if (Object.keys(raw).some((key) => !["kind", "paneId", "label"].includes(key))) return null;
      panes.push({ kind: "localDeferred", paneId: raw.paneId, label: raw.label });
      continue;
    }
    if (raw.kind === "sshDeferred") {
      const target = parseSshTarget(raw.target);
      if (!target || Object.keys(raw).some((key) => !["kind", "paneId", "label", "target"].includes(key))) return null;
      panes.push({ kind: "sshDeferred", paneId: raw.paneId, label: raw.label, target });
      continue;
    }
    if (raw.kind === "telnetDeferred") {
      const endpoint = parseEndpoint(raw.endpoint);
      if (!endpoint || endpoint.username !== undefined
        || Object.keys(raw).some((key) => !["kind", "paneId", "label", "endpoint"].includes(key))) return null;
      panes.push({ kind: "telnetDeferred", paneId: raw.paneId, label: raw.label,
        endpoint: { address: endpoint.address, port: endpoint.port } });
      continue;
    }
    if (raw.kind === "pluginDeferred") {
      if (!isSafeText(raw.pluginId) || !isSafeText(raw.providerId) || !isSafeText(raw.schemaHash)
        || Object.keys(raw).some((key) => !["kind", "paneId", "label", "pluginId", "providerId", "schemaHash"].includes(key))) return null;
      panes.push({ kind: "pluginDeferred", paneId: raw.paneId, label: raw.label,
        pluginId: raw.pluginId, providerId: raw.providerId, schemaHash: raw.schemaHash });
      continue;
    }
    if (!["ssh", "local", "telnet", "plugin"].includes(String(raw.kind))
      || !isSafeText(raw.sessionId) || !isSafeText(raw.generation)
      || Object.keys(raw).some((key) => !["kind", "paneId", "label", "sessionId", "generation", "initialDimensions", "outputGeometry"].includes(key))) return null;
    const dimensions = raw.initialDimensions;
    if (dimensions !== undefined && (!isRecord(dimensions)
      || !Number.isInteger(dimensions.rows) || !Number.isInteger(dimensions.cols)
      || (dimensions.rows as number) < 1 || (dimensions.rows as number) > 1000
      || (dimensions.cols as number) < 1 || (dimensions.cols as number) > 1000
      || Object.keys(dimensions).some((key) => !["rows", "cols"].includes(key)))) return null;
    const outputGeometry = raw.outputGeometry === undefined ? undefined : parseOutputGeometry(raw.outputGeometry);
    if (outputGeometry === null) return null;
    panes.push({ kind: raw.kind as "ssh" | "local" | "telnet" | "plugin", paneId: raw.paneId,
      label: raw.label, sessionId: raw.sessionId, generation: raw.generation,
      ...(dimensions === undefined ? {} : { initialDimensions: dimensions as { rows: number; cols: number } }),
      ...(outputGeometry === undefined ? {} : { outputGeometry }) });
  }
  return expectedPaneIds.size === 0
    ? { schemaVersion: 1, tabId: value.tabId, layout, activePaneId: value.activePaneId, panes }
    : null;
}
