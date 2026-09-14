import { readonly, shallowRef } from "vue";

import type { PluginHostNavigationEvent, SftpDirectoryListing, SftpRemoteDirectoryEntry } from "../core-api/generated/core-api";
import type { cancelSftpDirectoryListing, listSftpDirectory } from "../core-api/client";

export type SftpPluginNavigation = PluginHostNavigationEvent & {
  request: { kind: "sftp"; path: string; edit: boolean };
};

const pending = shallowRef<readonly SftpPluginNavigation[]>([]);
const recentOperations = new Set<string>();
const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const encoder = new TextEncoder();

export const pendingSftpPluginNavigations = readonly(pending);

export function sftpNavigationPath(path: string) {
  if (!path.startsWith("/") || encoder.encode(path).length > 4096
    || Array.from(path).some((character) => {
      const code = character.codePointAt(0)!;
      return code < 32 || (code >= 127 && code <= 159) || (code >= 0x202a && code <= 0x202e) || (code >= 0x2066 && code <= 0x2069);
    })
    || path.split("/").some((part) => part === "." || part === "..")) return null;
  const normalized = path.replace(/\/+/gu, "/").replace(/\/+$/u, "") || "/";
  const lastSlash = normalized.lastIndexOf("/");
  return {
    path: normalized,
    parentPath: normalized === "/" ? "/" : normalized.slice(0, lastSlash) || "/",
    name: normalized === "/" ? null : normalized.slice(lastSlash + 1),
  };
}

// Only App's main-window Core event listener may enqueue these intents. Route
// query strings and DOM events never authorize a connection or a file editor.
export function acceptSftpPluginNavigation(value: unknown): boolean {
  if (!value || typeof value !== "object") return false;
  const candidate = value as Partial<SftpPluginNavigation>;
  const request = candidate.request;
  const session = candidate.sftpSession;
  if (typeof candidate.operationId !== "string" || !uuid.test(candidate.operationId)
    || typeof candidate.pluginId !== "string" || !candidate.pluginId || candidate.pluginId.length > 256
    || !session || !uuid.test(session.sessionId) || !/^[0-9]{1,20}$/u.test(session.generation)
    || !session.parentSshSession || !uuid.test(session.parentSshSession.sessionId)
    || !/^[0-9]{1,20}$/u.test(session.parentSshSession.generation)
    || !request || request.kind !== "sftp" || typeof request.edit !== "boolean"
    || typeof request.path !== "string" || !sftpNavigationPath(request.path)
    || recentOperations.has(candidate.operationId) || pending.value.length >= 16) return false;
  recentOperations.add(candidate.operationId);
  if (recentOperations.size > 256) recentOperations.delete(recentOperations.values().next().value!);
  pending.value = [...pending.value, {
    operationId: candidate.operationId,
    pluginId: candidate.pluginId,
    sftpSession: { ...session, parentSshSession: { ...session.parentSshSession } },
    request: { kind: "sftp", path: request.path, edit: request.edit },
  }];
  return true;
}

export function takeSftpPluginNavigation(): SftpPluginNavigation | null {
  const next = pending.value[0] ?? null;
  pending.value = pending.value.slice(1);
  return next;
}

export function discardSftpPluginNavigations(pluginId: string) {
  pending.value = pending.value.filter((intent) => intent.pluginId !== pluginId);
}

/** Locates only protocol-listed objects on the already-created child session. */
export async function resolveSftpPluginNavigation(
  intent: SftpPluginNavigation,
  api: { list: typeof listSftpDirectory; cancel: typeof cancelSftpDirectoryListing },
  current: () => boolean,
): Promise<{ listing: SftpDirectoryListing; entry: SftpRemoteDirectoryEntry | null }> {
  const path = sftpNavigationPath(intent.request.path);
  if (!path) throw new Error("invalidPath");
  const { sessionId, generation } = intent.sftpSession;
  let cursorOwner: SftpDirectoryListing | null = null;
  let handedOff = false;
  const releaseCursor = async () => {
    if (!cursorOwner?.nextCursor) return;
    const owner = cursorOwner;
    cursorOwner = null;
    await api.cancel({ sessionId: owner.sessionId, expectedGeneration: owner.generation, path: owner.path, cursor: owner.nextCursor! }).catch(() => undefined);
  };
  const read = async (directory: string, cursor: number[] | null) => {
    if (!current()) throw new Error("staleNavigation");
    const listing = await api.list({ sessionId, expectedGeneration: generation, path: { bytes: [...encoder.encode(directory)] }, cursor, pageSize: 256 });
    cursorOwner = listing;
    if (!current() || listing.sessionId !== sessionId || listing.generation !== generation) throw new Error("staleNavigation");
    return listing;
  };
  try {
    let listing = await read(path.parentPath, null);
    if (path.name === null) { handedOff = true; return { listing, entry: null }; }
    const targetBytes = [...encoder.encode(path.path)];
    const entries = [...listing.entries];
    for (let page = 0; page < 40; page += 1) {
      const entry = listing.entries.find((item) => item.path.bytes.length === targetBytes.length
        && item.path.bytes.every((value, index) => value === targetBytes[index]));
      if (entry) {
        if (entry.kind === "directory" && !intent.request.edit) {
          await releaseCursor();
          const directory = await read(path.path, null);
          handedOff = true;
          return { listing: directory, entry: null };
        }
        handedOff = true;
        return { listing: { ...listing, entries }, entry };
      }
      if (!listing.nextCursor || page === 39) break;
      listing = await read(path.parentPath, listing.nextCursor);
      entries.push(...listing.entries);
    }
    throw new Error("entryUnavailable");
  } finally {
    if (!handedOff) await releaseCursor();
  }
}
