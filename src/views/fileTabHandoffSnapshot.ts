import { parseTerminalLayout, type TerminalLayoutNode } from "../components/terminal/terminalLayout";
import type { SftpLocalDirectoryCapability } from "../core-api/generated/core-api";
import type { FileHeaderTab } from "../stores/workspaceTabs";
import type { SftpPaneEndpoint, SftpPaneSort } from "./sftpPaneState";

export interface FilePaneHandoffSnapshot {
  paneId: string;
  endpoint: SftpPaneEndpoint;
  directory: string;
  remoteDirectoryPathBytes: number[] | null;
  localTrail: Array<{ capability: SftpLocalDirectoryCapability; displayPath: string; rememberedPath: string | null }>;
  localRememberedPath: string | null;
  search: string;
  sort: SftpPaneSort;
  showHidden: boolean;
  foldersFirst: boolean;
}

export interface FileTabHandoffSnapshot {
  version: 1;
  tab: FileHeaderTab;
  layout: TerminalLayoutNode;
  activePaneId: string;
  panes: FilePaneHandoffSnapshot[];
}

const isRecord = (value: unknown): value is Record<string, unknown> =>
  !!value && typeof value === "object" && !Array.isArray(value);

function layoutPaneKinds(value: unknown): Map<string, "local" | "remote"> | null {
  const layout = parseTerminalLayout(value);
  if (!layout) return null;
  const kinds = new Map<string, "local" | "remote">();
  const pending = [layout];
  while (pending.length) {
    const node = pending.pop()!;
    if (node.kind === "split") {
      pending.push(node.first, node.second);
    } else if (node.terminalId === "local" || node.terminalId === "remote") {
      kinds.set(node.paneId, node.terminalId);
    } else return null;
  }
  return kinds;
}

function validCapability(value: unknown): value is SftpLocalDirectoryCapability {
  return isRecord(value) && typeof value.directoryRef === "string" && value.directoryRef.length > 0
    && typeof value.revision === "string" && value.revision.length > 0
    && typeof value.displayName === "string"
    && (value.rememberablePath === null || typeof value.rememberablePath === "string");
}

export function isFileTabHandoffSnapshot(value: unknown, id: string): value is FileTabHandoffSnapshot {
  if (!isRecord(value) || value.version !== 1 || !isRecord(value.tab) || !Array.isArray(value.panes)) return false;
  const tab = value.tab;
  if (tab.groupId !== id || !/^file:[0-9a-f-]{36}$/i.test(id)
    || (tab.kind !== "local" && tab.kind !== "remote")
    || (tab.hostId !== null && typeof tab.hostId !== "string") || typeof tab.label !== "string"
    || !Number.isSafeInteger(tab.paneCount) || tab.paneCount !== value.panes.length
    || (tab.initialSessionId != null && (tab.kind !== "remote" || (!tab.hostId && !tab.initialGeneration)
      || typeof tab.initialSessionId !== "string" || !/^[0-9a-f-]{36}$/i.test(tab.initialSessionId)))
    || (tab.initialGeneration != null && (!tab.initialSessionId || typeof tab.initialGeneration !== "string"
      || !/^[0-9]{1,20}$/u.test(tab.initialGeneration)))) return false;
  const kinds = layoutPaneKinds(value.layout);
  if (!kinds || kinds.size !== value.panes.length || !kinds.has(value.activePaneId as string)) return false;
  const seen = new Set<string>();
  const sessionIds = new Set<string>();
  const capabilityIds = new Set<string>();
  for (const candidate of value.panes) {
    if (!isRecord(candidate) || typeof candidate.paneId !== "string" || !kinds.has(candidate.paneId)
      || seen.has(candidate.paneId) || !isRecord(candidate.endpoint)
      || typeof candidate.directory !== "string" || candidate.directory.length > 4096
      || typeof candidate.search !== "string" || candidate.search.length > 4096
      || !["name", "size", "modified"].includes(candidate.sort as string)
      || typeof candidate.showHidden !== "boolean" || typeof candidate.foldersFirst !== "boolean"
      || (candidate.localRememberedPath !== null && typeof candidate.localRememberedPath !== "string")
      || !Array.isArray(candidate.localTrail)) return false;
    seen.add(candidate.paneId);
    const endpoint = candidate.endpoint;
    if (endpoint.kind !== kinds.get(candidate.paneId)) return false;
    if (endpoint.kind === "local") {
      if (typeof endpoint.displayPath !== "string" || endpoint.displayPath.length > 4096
        || (endpoint.directoryRef !== null && typeof endpoint.directoryRef !== "string")
        || (endpoint.revision !== null && typeof endpoint.revision !== "string")
        || Boolean(endpoint.directoryRef) !== Boolean(endpoint.revision)
        || candidate.remoteDirectoryPathBytes !== null) return false;
      if (endpoint.directoryRef) {
        if (capabilityIds.has(endpoint.directoryRef)) return false;
        capabilityIds.add(endpoint.directoryRef);
      }
    } else if (endpoint.kind === "remote") {
      if ((endpoint.hostId !== null && typeof endpoint.hostId !== "string")
        || (endpoint.sessionId !== null && typeof endpoint.sessionId !== "string")
        || (endpoint.generation !== null && typeof endpoint.generation !== "string")
        || Boolean(endpoint.sessionId) !== Boolean(endpoint.generation)
        || !Array.isArray(candidate.remoteDirectoryPathBytes)
        || candidate.remoteDirectoryPathBytes.length > 4096
        || !candidate.remoteDirectoryPathBytes.every((byte: unknown) => Number.isInteger(byte) && (byte as number) >= 0 && (byte as number) <= 255)
        || candidate.localTrail.length || candidate.localRememberedPath !== null) return false;
      if (endpoint.sessionId) {
        if (sessionIds.has(endpoint.sessionId)) return false;
        sessionIds.add(endpoint.sessionId);
      }
    } else return false;
    for (const trail of candidate.localTrail) {
      if (!isRecord(trail) || !validCapability(trail.capability)
        || typeof trail.displayPath !== "string"
        || (trail.rememberedPath !== null && typeof trail.rememberedPath !== "string")
        || capabilityIds.has(trail.capability.directoryRef)) return false;
      capabilityIds.add(trail.capability.directoryRef);
    }
  }
  return seen.size === kinds.size;
}
