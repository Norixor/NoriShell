import { DEFAULT_SFTP_PREFERENCES, type SftpBrowserPreferences } from "../stores/sftpPreferences";

export type SftpPaneKind = "local" | "remote";
export type SftpPaneSort = "name" | "size" | "modified";
export type SftpPaneEntryKind = "file" | "directory" | "symlink" | "other";

export interface SftpPaneObjectPrecondition {
  kind: SftpPaneEntryKind;
  size: number | null;
  modifiedAtUnixMs: number | null;
}

export type SftpPaneEndpoint =
  | {
    kind: "local";
    directoryRef: string | null;
    revision: string | null;
    displayPath: string;
  }
  | {
    kind: "remote";
    hostId: string | null;
    sessionId: string | null;
    generation: string | null;
  };

export interface SftpPaneEntry {
  key: string;
  displayName: string;
  kind: SftpPaneEntryKind;
  entryRef: string;
  nameBytes: number[];
  size: number | null;
  modifiedAtUnixMs: number | null;
  permissionBits?: number | null;
  remotePathBytes: number[] | null;
  localRelativePath: string | null;
  precondition: SftpPaneObjectPrecondition;
}

export interface SftpPaneState {
  paneId: string;
  endpoint: SftpPaneEndpoint;
  endpointRevision: number;
  directory: string;
  directoryRef: string | null;
  remoteDirectoryPathBytes: number[] | null;
  directoryRevision: number;
  search: string;
  sort: SftpPaneSort;
  showHidden: boolean;
  foldersFirst: boolean;
  selectedEntryKey: string | null;
  selectedEntryKeys: string[];
  selectionAnchorKey: string | null;
  selectionRevision: number;
  entries: SftpPaneEntry[];
  loading: boolean;
  error: string | null;
}

export interface SftpPaneDragIntent {
  nonce: string;
  sourcePaneId: string;
  sourceEndpointRevision: number;
  sourceDirectoryRevision: number;
  sourceSelectionRevision: number;
  entryKey: string;
}

export interface SftpPaneDropFence {
  source: SftpPaneState;
  target: SftpPaneState;
  entry: SftpPaneEntry;
  sourceEndpointRevision: number;
  targetEndpointRevision: number;
  sourceDirectoryRevision: number;
  targetDirectoryRevision: number;
}

function defaultEndpoint(kind: SftpPaneKind): SftpPaneEndpoint {
  return kind === "local"
    ? {
      kind: "local",
      directoryRef: null,
      revision: null,
      displayPath: "",
    }
    : {
      kind: "remote",
      hostId: null,
      sessionId: null,
      generation: null,
    };
}

export function createSftpPaneState(paneId: string, kind: SftpPaneKind, defaults: SftpBrowserPreferences = DEFAULT_SFTP_PREFERENCES): SftpPaneState {
  return {
    paneId,
    endpoint: defaultEndpoint(kind),
    endpointRevision: 1,
    directory: kind === "remote" ? "/" : ".",
    directoryRef: null,
    remoteDirectoryPathBytes: kind === "remote" ? [47] : null,
    directoryRevision: 1,
    search: "",
    sort: defaults.sort,
    showHidden: defaults.showHidden,
    foldersFirst: defaults.foldersFirst,
    selectedEntryKey: null,
    selectedEntryKeys: [],
    selectionAnchorKey: null,
    selectionRevision: 1,
    entries: [],
    loading: false,
    error: null,
  };
}

export function cloneSftpPaneState(source: SftpPaneState, paneId: string): SftpPaneState {
  const endpoint: SftpPaneEndpoint = source.endpoint.kind === "remote"
    ? {
      kind: "remote",
      hostId: source.endpoint.hostId,
      sessionId: null,
      generation: null,
    }
    : defaultEndpoint("local");
  return {
    ...source,
    paneId,
    endpoint,
    endpointRevision: 1,
    directory: endpoint.kind === "remote" ? "/" : ".",
    directoryRef: null,
    remoteDirectoryPathBytes: endpoint.kind === "remote" ? [47] : null,
    directoryRevision: 1,
    selectedEntryKey: null,
    selectedEntryKeys: [],
    selectionAnchorKey: null,
    selectionRevision: 1,
    entries: [],
    loading: false,
    error: null,
  };
}

export function replaceSftpPaneEndpoint(
  pane: SftpPaneState,
  endpoint: SftpPaneEndpoint,
  directory: string,
  remoteDirectoryPathBytes: number[] | null = endpoint.kind === "remote" ? [47] : null,
  directoryRef: string | null = endpoint.kind === "local" ? endpoint.directoryRef : null,
) {
  pane.endpoint = endpoint;
  pane.endpointRevision += 1;
  pane.directory = directory;
  pane.directoryRef = directoryRef;
  pane.remoteDirectoryPathBytes = remoteDirectoryPathBytes
    ? [...remoteDirectoryPathBytes]
    : null;
  pane.directoryRevision += 1;
  pane.selectedEntryKey = null;
  pane.selectedEntryKeys = [];
  pane.selectionAnchorKey = null;
  pane.selectionRevision += 1;
  pane.entries = [];
  pane.error = null;
  pane.loading = false;
}

export function reconcileSftpPaneRemoteGeneration(
  pane: SftpPaneState,
  hostId: string | null,
  sessionId: string,
  generation: string,
) {
  if (pane.endpoint.kind !== "remote"
    || pane.endpoint.sessionId !== sessionId
    || pane.endpoint.generation === generation) {
    return false;
  }
  replaceSftpPaneEndpoint(pane, {
    kind: "remote",
    hostId,
    sessionId,
    generation,
  }, "/", [47]);
  return true;
}

export function beginSftpPaneDirectoryLoad(pane: SftpPaneState) {
  pane.loading = true;
  pane.error = null;
  return {
    endpointRevision: pane.endpointRevision,
    directoryRevision: pane.directoryRevision,
  };
}

export function completeSftpPaneDirectoryLoad(
  pane: SftpPaneState,
  fence: { endpointRevision: number; directoryRevision: number },
  directory: string,
  entries: SftpPaneEntry[],
  remoteDirectoryPathBytes: number[] | null = null,
  directoryRef: string | null = null,
) {
  if (pane.endpointRevision !== fence.endpointRevision
    || pane.directoryRevision !== fence.directoryRevision) {
    return false;
  }
  pane.directory = directory;
  pane.directoryRef = directoryRef;
  pane.remoteDirectoryPathBytes = remoteDirectoryPathBytes
    ? [...remoteDirectoryPathBytes]
    : null;
  pane.directoryRevision += 1;
  pane.entries = entries;
  pane.selectedEntryKey = null;
  pane.selectedEntryKeys = [];
  pane.selectionAnchorKey = null;
  pane.selectionRevision += 1;
  pane.loading = false;
  pane.error = null;
  return true;
}

export function failSftpPaneDirectoryLoad(
  pane: SftpPaneState,
  fence: { endpointRevision: number; directoryRevision: number },
  error: string,
) {
  if (pane.endpointRevision !== fence.endpointRevision
    || pane.directoryRevision !== fence.directoryRevision) {
    return false;
  }
  pane.loading = false;
  pane.error = error;
  return true;
}

export function selectSftpPaneEntry(pane: SftpPaneState, entryKey: string | null) {
  pane.selectedEntryKey = entryKey;
  pane.selectedEntryKeys = entryKey ? [entryKey] : [];
  pane.selectionAnchorKey = entryKey;
  pane.selectionRevision += 1;
}

export function toggleSftpPaneEntry(pane: SftpPaneState, entryKey: string) {
  const selected = new Set(pane.selectedEntryKeys);
  if (selected.has(entryKey)) selected.delete(entryKey);
  else selected.add(entryKey);
  pane.selectedEntryKeys = [...selected];
  pane.selectedEntryKey = selected.has(entryKey)
    ? entryKey
    : pane.selectedEntryKeys.at(-1) ?? null;
  pane.selectionAnchorKey = entryKey;
  pane.selectionRevision += 1;
}

export function selectSftpPaneEntryRange(
  pane: SftpPaneState,
  entryKey: string,
  orderedEntryKeys: readonly string[],
) {
  const anchor = pane.selectionAnchorKey;
  const anchorIndex = anchor ? orderedEntryKeys.indexOf(anchor) : -1;
  const entryIndex = orderedEntryKeys.indexOf(entryKey);
  if (anchorIndex < 0 || entryIndex < 0) {
    selectSftpPaneEntry(pane, entryKey);
    return;
  }
  const start = Math.min(anchorIndex, entryIndex);
  const end = Math.max(anchorIndex, entryIndex);
  pane.selectedEntryKeys = orderedEntryKeys.slice(start, end + 1);
  pane.selectedEntryKey = entryKey;
  pane.selectionRevision += 1;
}

export function visibleSftpPaneEntries(pane: SftpPaneState) {
  const query = pane.search.trim().toLocaleLowerCase();
  const filtered = pane.entries.filter((entry) =>
    (pane.showHidden || !entry.displayName.startsWith("."))
    && (!query || entry.displayName.toLocaleLowerCase().includes(query)),
  );
  return filtered.sort((left, right) => {
    if (pane.foldersFirst && left.kind === "directory" && right.kind !== "directory") return -1;
    if (pane.foldersFirst && left.kind !== "directory" && right.kind === "directory") return 1;
    if (pane.sort === "size") return (left.size ?? -1) - (right.size ?? -1);
    if (pane.sort === "modified") {
      return (left.modifiedAtUnixMs ?? -1) - (right.modifiedAtUnixMs ?? -1);
    }
    return left.displayName.localeCompare(right.displayName);
  });
}

export function captureSftpPaneDragIntent(
  pane: SftpPaneState,
  entry: SftpPaneEntry,
): SftpPaneDragIntent | null {
  if (entry.kind !== "file" && entry.kind !== "directory") return null;
  return {
    nonce: crypto.randomUUID(),
    sourcePaneId: pane.paneId,
    sourceEndpointRevision: pane.endpointRevision,
    sourceDirectoryRevision: pane.directoryRevision,
    sourceSelectionRevision: pane.selectionRevision,
    entryKey: entry.key,
  };
}

function paneEndpointReady(pane: SftpPaneState) {
  if (!pane.directoryRef) return false;
  if (pane.endpoint.kind === "local") {
    return pane.endpoint.directoryRef === pane.directoryRef
      && pane.endpoint.revision !== null;
  }
  return pane.endpoint.sessionId !== null && pane.endpoint.generation !== null;
}

export function resolveSftpPaneDropFence(
  intent: SftpPaneDragIntent,
  panes: Readonly<Record<string, SftpPaneState>>,
  targetPaneId: string,
): SftpPaneDropFence | null {
  const source = panes[intent.sourcePaneId];
  const target = panes[targetPaneId];
  if (!source || !target || source.paneId === target.paneId) return null;
  if (!paneEndpointReady(source) || !paneEndpointReady(target)) return null;
  if (source.endpoint.kind === "remote"
    && target.endpoint.kind === "remote"
    && source.endpoint.sessionId === target.endpoint.sessionId) {
    return null;
  }
  if (source.endpointRevision !== intent.sourceEndpointRevision
    || source.directoryRevision !== intent.sourceDirectoryRevision
    || source.selectionRevision !== intent.sourceSelectionRevision) {
    return null;
  }
  const entry = source.entries.find((candidate) => candidate.key === intent.entryKey);
  if (!entry || (entry.kind !== "file" && entry.kind !== "directory")) return null;
  if (source.selectedEntryKey !== entry.key) return null;
  return {
    source,
    target,
    entry,
    sourceEndpointRevision: source.endpointRevision,
    targetEndpointRevision: target.endpointRevision,
    sourceDirectoryRevision: source.directoryRevision,
    targetDirectoryRevision: target.directoryRevision,
  };
}
