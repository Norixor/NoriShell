import { describe, expect, it, vi } from "vitest";
import type { SftpDirectoryListing } from "../core-api/generated/core-api";
import { acceptSftpPluginNavigation, resolveSftpPluginNavigation, sftpNavigationPath, type SftpPluginNavigation } from "./sftpPluginNavigation";

const bytes = (text: string) => [...new TextEncoder().encode(text)];
const intent: SftpPluginNavigation = {
  operationId: "019d0000-0000-7000-8000-000000000971", pluginId: "test.lookup",
  sftpSession: { sessionId: "019d0000-0000-7000-8000-000000000972", generation: "2", hostId: null, parentSshSession: { sessionId: "019d0000-0000-7000-8000-000000000973", generation: "1" }, stateRevision: "1", state: "ready", transferCount: 0, activeTransferCount: 0, failure: null },
  request: { kind: "sftp", path: "/etc/nginx.conf", edit: true },
};
function listing(entries: SftpDirectoryListing["entries"], nextCursor: number[] | null): SftpDirectoryListing {
  return { sessionId: intent.sftpSession.sessionId, generation: "2", directoryRef: "etc", path: { bytes: bytes("/etc") }, entries, nextCursor };
}
describe("trusted shared-session file navigation", () => {
  it("rejects Host-only connection intents and unsafe paths", () => {
    expect(acceptSftpPluginNavigation({ operationId: intent.operationId, pluginId: "test.lookup", hostId: "019d0000-0000-7000-8000-000000000973", expectedHostStateVersion: "1", request: intent.request })).toBe(false);
    for (const path of ["relative/file", "/etc/../secret", "/etc/\0config", "/etc/\u202econfig"]) expect(sftpNavigationPath(path)).toBeNull();
    expect(sftpNavigationPath("/etc//nginx.conf/")).toEqual({ path: "/etc/nginx.conf", parentPath: "/etc", name: "nginx.conf" });
  });
  it("uses paginated protocol entries and keeps their directory and object references", async () => {
    const entry = { entryRef: "config-entry", path: { bytes: bytes("/etc/nginx.conf") }, displayName: "nginx.conf", kind: "file" as const, size: 20, modifiedAtUnixMs: 1, permissionBits: null };
    const list = vi.fn().mockResolvedValueOnce(listing([], [1])).mockResolvedValueOnce(listing([entry], [2]));
    const cancel = vi.fn().mockResolvedValue(undefined);
    const result = await resolveSftpPluginNavigation(intent, { list, cancel }, () => true);
    expect(list).toHaveBeenLastCalledWith(expect.objectContaining({ sessionId: intent.sftpSession.sessionId, expectedGeneration: "2", cursor: [1] }));
    expect(result.entry).toEqual(entry);
    expect(result.listing.directoryRef).toBe("etc");
    expect(result.listing.nextCursor).toEqual([2]);
    expect(cancel).not.toHaveBeenCalled();
  });
  it("cancels returned listing cursors when the parent or pane changes before delivery", async () => {
    let current = true;
    const list = vi.fn().mockImplementation(async () => { current = false; return listing([], [9]); });
    const cancel = vi.fn().mockResolvedValue(undefined);
    await expect(resolveSftpPluginNavigation(intent, { list, cancel }, () => current)).rejects.toThrow("staleNavigation");
    expect(cancel).toHaveBeenCalledWith(expect.objectContaining({ sessionId: intent.sftpSession.sessionId, expectedGeneration: "2", cursor: [9] }));
  });
});
