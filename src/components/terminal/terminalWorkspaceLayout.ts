import type { PluginTerminalProfile } from "../../core-api/plugin-terminal";
import type {
  SshSessionEndpoint,
  SshSessionTarget,
} from "../../core-api/generated/core-api";
import {
  countTerminalPanes,
  parseTerminalLayout,
  terminalLayoutDepth,
  type TerminalLayoutNode,
} from "./terminalLayout";

export const TERMINAL_WORKSPACE_LAYOUT_SCHEMA_VERSION = 1;
export const MAX_TERMINAL_WORKSPACE_TABS = 32;
export const MAX_TERMINAL_WORKSPACE_PANES_PER_TAB = 2_048;
export const MAX_TERMINAL_WORKSPACE_LAYOUT_DEPTH = 64;

export interface PersistedLauncherPane {
  kind: "launcher";
  paneId: string;
  label: string;
}

export interface PersistedHostSshPane {
  kind: "sshHost";
  paneId: string;
  label: string;
  hostId: string;
}

export interface PersistedQuickConnectSshPane {
  kind: "sshQuickConnect";
  paneId: string;
  label: string;
  endpoint: SshSessionEndpoint;
}

export interface PersistedLocalPane {
  kind: "local";
  paneId: string;
  label: string;
}

export interface PersistedTelnetPane {
  kind: "telnet";
  paneId: string;
  label: string;
  address: string;
  port: number;
}

export interface PersistedPluginPane extends PluginTerminalProfile {
  kind: "plugin"; paneId: string; label: string;
}

export type PersistedTerminalWorkspacePane =
  | PersistedLauncherPane
  | PersistedHostSshPane
  | PersistedQuickConnectSshPane
  | PersistedLocalPane
  | PersistedTelnetPane
  | PersistedPluginPane;

export interface PersistedTerminalWorkspaceTab {
  tabId: string;
  layout: TerminalLayoutNode;
  activePaneId: string;
  panes: PersistedTerminalWorkspacePane[];
}

export interface PersistedTerminalWorkspaceLayout {
  schemaVersion: typeof TERMINAL_WORKSPACE_LAYOUT_SCHEMA_VERSION;
  activeTabId: string | null;
  tabs: PersistedTerminalWorkspaceTab[];
}

export type TerminalWorkspacePaneForPersistence =
  | { kind: "launcher"; paneId: string; label: string }
  | { kind: "session"; paneId: string; label: string; target: SshSessionTarget }
  | { kind: "local"; paneId: string; label: string }
  | { kind: "plugin"; paneId: string; label: string; profile: PluginTerminalProfile | null }
  | { kind: "telnet"; paneId: string; label: string; endpoint: { address: string; port: number } };

export interface TerminalWorkspaceTabForPersistence {
  tabId: string;
  layout: TerminalLayoutNode;
  activePaneId: string;
  panes: TerminalWorkspacePaneForPersistence[];
}

/** Builds the explicit allow-listed projection written to SQLite. */
export function projectTerminalWorkspaceLayout(
  tabs: readonly TerminalWorkspaceTabForPersistence[],
  activeTabId: string,
): PersistedTerminalWorkspaceLayout {
  return {
    schemaVersion: TERMINAL_WORKSPACE_LAYOUT_SCHEMA_VERSION,
    activeTabId: tabs.length > 0 ? activeTabId : null,
    tabs: tabs.map((tab) => ({
      tabId: tab.tabId,
      layout: projectLayoutNode(tab.layout),
      activePaneId: tab.activePaneId,
      panes: tab.panes.map((pane): PersistedTerminalWorkspacePane => {
        const base = { paneId: pane.paneId, label: pane.label };
        if (pane.kind === "launcher" || pane.kind === "local") {
          return { kind: pane.kind, ...base };
        }
        if (pane.kind === "plugin") {
          return pane.profile ? { kind: "plugin", ...base, pluginId: pane.profile.pluginId, providerId: pane.profile.providerId, schemaHash: pane.profile.schemaHash, configuration: { ...pane.profile.configuration } } : { kind: "launcher", ...base };
        }
        if (pane.kind === "telnet") {
          return {
            kind: "telnet",
            ...base,
            address: pane.endpoint.address,
            port: pane.endpoint.port,
          };
        }
        return pane.target.kind === "host"
          ? { kind: "sshHost", ...base, hostId: pane.target.hostId }
          : { kind: "sshQuickConnect", ...base, endpoint: { ...pane.target.endpoint } };
      }),
    })),
  };
}

function projectLayoutNode(node: TerminalLayoutNode): TerminalLayoutNode {
  if (node.kind === "pane") {
    // terminalId is a renderer binding slot, not a durable Session identity.
    // Normalize it to the already public paneId at the persistence boundary.
    return { kind: "pane", paneId: node.paneId, terminalId: node.paneId };
  }
  return {
    ...node,
    first: projectLayoutNode(node.first),
    second: projectLayoutNode(node.second),
  };
}

/**
 * Parses the untrusted application-layout projection returned by persistence.
 * Session, credential, attachment and output state are intentionally not part of this format.
 */
export function parseTerminalWorkspaceLayout(
  value: unknown,
): PersistedTerminalWorkspaceLayout | null {
  if (!isRecord(value)
    || value.schemaVersion !== TERMINAL_WORKSPACE_LAYOUT_SCHEMA_VERSION
    || !Array.isArray(value.tabs)
    || value.tabs.length > MAX_TERMINAL_WORKSPACE_TABS
    || !(value.activeTabId === null || isSafeText(value.activeTabId, 128))) {
    return null;
  }

  const tabIds = new Set<string>();
  const paneIds = new Set<string>();
  const tabs: PersistedTerminalWorkspaceTab[] = [];
  for (const candidate of value.tabs) {
    const parsed = parseWorkspaceTab(candidate, tabIds, paneIds);
    if (!parsed) return null;
    tabs.push(parsed);
  }

  if ((tabs.length === 0 && value.activeTabId !== null)
    || (tabs.length > 0
      && (value.activeTabId === null || !tabIds.has(value.activeTabId)))) {
    return null;
  }

  return {
    schemaVersion: TERMINAL_WORKSPACE_LAYOUT_SCHEMA_VERSION,
    activeTabId: value.activeTabId,
    tabs,
  };
}

function parseWorkspaceTab(
  value: unknown,
  tabIds: Set<string>,
  globalPaneIds: Set<string>,
): PersistedTerminalWorkspaceTab | null {
  if (!isRecord(value)
    || !isSafeText(value.tabId, 128)
    || tabIds.has(value.tabId)
    || !isSafeText(value.activePaneId, 128)
    || !Array.isArray(value.panes)) {
    return null;
  }

  const layout = parseTerminalLayout(value.layout);
  if (!layout
    || countTerminalPanes(layout) > MAX_TERMINAL_WORKSPACE_PANES_PER_TAB
    || terminalLayoutDepth(layout) > MAX_TERMINAL_WORKSPACE_LAYOUT_DEPTH) return null;
  const layoutPaneIds = collectLayoutPaneIds(layout);
  if (value.panes.length !== layoutPaneIds.size || !layoutPaneIds.has(value.activePaneId)) return null;

  const panes: PersistedTerminalWorkspacePane[] = [];
  const tabPaneIds = new Set<string>();
  for (const candidate of value.panes) {
    const pane = parseWorkspacePane(candidate);
    if (!pane
      || tabPaneIds.has(pane.paneId)
      || globalPaneIds.has(pane.paneId)
      || !layoutPaneIds.has(pane.paneId)) {
      return null;
    }
    tabPaneIds.add(pane.paneId);
    globalPaneIds.add(pane.paneId);
    panes.push(pane);
  }
  if ([...layoutPaneIds].some((paneId) => !tabPaneIds.has(paneId))) return null;

  tabIds.add(value.tabId);
  return {
    tabId: value.tabId,
    layout,
    activePaneId: value.activePaneId,
    panes,
  };
}

function parseWorkspacePane(value: unknown): PersistedTerminalWorkspacePane | null {
  if (!isRecord(value)
    || !isSafeText(value.paneId, 128)
    || !isSafeText(value.label, 256)) {
    return null;
  }

  const base = { paneId: value.paneId, label: value.label };
  switch (value.kind) {
    case "launcher":
      return { kind: value.kind, ...base };
    case "local":
      return { kind: value.kind, ...base };
    case "plugin": {
      if (!isSafeText(value.pluginId, 256) || !isSafeText(value.providerId, 128)
        || !isSafeText(value.schemaHash, 128) || !isRecord(value.configuration)
        || Object.keys(value.configuration).length > 64) return null;
      const configuration: Record<string, boolean | number | string> = {};
      for (const [key, item] of Object.entries(value.configuration)) {
        if (!isSafeText(key, 128) || !((typeof item === "string" && isSafeOptionalText(item, 4096))
          || typeof item === "boolean" || (typeof item === "number" && Number.isFinite(item)))) return null;
        Object.defineProperty(configuration, key, { value: item, enumerable: true, configurable: true, writable: true });
      }
      return { kind: "plugin", ...base, pluginId: value.pluginId, providerId: value.providerId, schemaHash: value.schemaHash, configuration };
    }
    case "telnet":
      return isSafeText(value.address, 255)
        && Number.isInteger(value.port)
        && (value.port as number) >= 1
        && (value.port as number) <= 65_535
        ? {
            kind: value.kind,
            ...base,
            address: value.address,
            port: value.port as number,
          }
        : null;
    case "sshHost":
      return isSafeText(value.hostId, 128)
        ? { kind: value.kind, ...base, hostId: value.hostId }
        : null;
    case "sshQuickConnect": {
      const endpoint = parseEndpoint(value.endpoint);
      return endpoint ? { kind: value.kind, ...base, endpoint } : null;
    }
    default:
      return null;
  }
}

function parseEndpoint(value: unknown): SshSessionEndpoint | null {
  if (!isRecord(value)
    || !isSafeText(value.address, 255)
    || !Number.isInteger(value.port)
    || (value.port as number) < 1
    || (value.port as number) > 65_535
    || !(value.username === null || isSafeOptionalText(value.username, 255))) {
    return null;
  }
  return {
    address: value.address,
    port: value.port as number,
    username: value.username,
  };
}

function collectLayoutPaneIds(node: TerminalLayoutNode): Set<string> {
  if (node.kind === "pane") return new Set([node.paneId]);
  return new Set([
    ...collectLayoutPaneIds(node.first),
    ...collectLayoutPaneIds(node.second),
  ]);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isSafeText(value: unknown, maxLength: number): value is string {
  return typeof value === "string"
    && value.length > 0
    && value.length <= maxLength
    && !hasControlCharacter(value);
}

function isSafeOptionalText(value: unknown, maxLength: number): value is string {
  return typeof value === "string"
    && value.length <= maxLength
    && !hasControlCharacter(value);
}

function hasControlCharacter(value: string): boolean {
  return [...value].some((character) => {
    const codePoint = character.codePointAt(0) ?? 0;
    return codePoint <= 31 || codePoint === 127;
  });
}
