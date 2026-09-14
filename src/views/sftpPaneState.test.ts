import { describe, expect, it } from "vitest";

import {
  beginSftpPaneDirectoryLoad,
  captureSftpPaneDragIntent,
  cloneSftpPaneState,
  completeSftpPaneDirectoryLoad,
  createSftpPaneState,
  reconcileSftpPaneRemoteGeneration,
  replaceSftpPaneEndpoint,
  resolveSftpPaneDropFence,
  selectSftpPaneEntry,
  selectSftpPaneEntryRange,
  toggleSftpPaneEntry,
  type SftpPaneEntry,
} from "./sftpPaneState";

const fileEntry: SftpPaneEntry = {
  key: "file:release.bin",
  displayName: "release.bin",
  kind: "file",
  entryRef: "entry-release",
  nameBytes: Array.from(new TextEncoder().encode("release.bin")),
  size: 8,
  modifiedAtUnixMs: 1_788_000_000_000,
  remotePathBytes: [47, 114],
  localRelativePath: null,
  precondition: {
    kind: "file",
    size: 8,
    modifiedAtUnixMs: 1_788_000_000_000,
  },
};

describe("SFTP pane state fences", () => {
  it("creates a disconnected split endpoint instead of reusing one remote transport", () => {
    const source = createSftpPaneState("source", "remote");
    replaceSftpPaneEndpoint(source, {
      kind: "remote",
      hostId: "host-1",
      sessionId: "session-1",
      generation: "7",
    }, "/srv", [47, 115, 114, 118]);
    source.entries = [fileEntry];
    selectSftpPaneEntry(source, fileEntry.key);

    const clone = cloneSftpPaneState(source, "clone");

    expect(clone.endpoint).toEqual({
      kind: "remote",
      hostId: "host-1",
      sessionId: null,
      generation: null,
    });
    expect(clone.entries).toEqual([]);
    expect(clone.selectedEntryKey).toBeNull();
    expect(clone.loading).toBe(false);
  });

  it("rejects a directory result after the pane endpoint changed", () => {
    const pane = createSftpPaneState("pane", "remote");
    const fence = beginSftpPaneDirectoryLoad(pane);
    replaceSftpPaneEndpoint(pane, {
      kind: "remote",
      hostId: "host-2",
      sessionId: "session-2",
      generation: "2",
    }, "/");

    expect(completeSftpPaneDirectoryLoad(pane, fence, "/stale", [fileEntry])).toBe(false);
    expect(pane.entries).toEqual([]);
  });

  it("clears stale directory identity and selection when a remote generation changes", () => {
    const pane = createSftpPaneState("pane", "remote");
    replaceSftpPaneEndpoint(pane, {
      kind: "remote",
      hostId: "host-1",
      sessionId: "session-1",
      generation: "1",
    }, "/srv/releases", [47, 115, 114, 118], "directory-generation-1");
    pane.entries = [fileEntry];
    selectSftpPaneEntry(pane, fileEntry.key);
    pane.loading = true;

    expect(reconcileSftpPaneRemoteGeneration(
      pane,
      "host-1",
      "session-1",
      "2",
    )).toBe(true);
    expect(pane.endpoint).toEqual({
      kind: "remote",
      hostId: "host-1",
      sessionId: "session-1",
      generation: "2",
    });
    expect(pane.directory).toBe("/");
    expect(pane.directoryRef).toBeNull();
    expect(pane.remoteDirectoryPathBytes).toEqual([47]);
    expect(pane.entries).toEqual([]);
    expect(pane.selectedEntryKey).toBeNull();
    expect(pane.loading).toBe(false);

    expect(reconcileSftpPaneRemoteGeneration(
      pane,
      "host-1",
      "session-1",
      "2",
    )).toBe(false);
  });

  it("rejects a drop when selection, directory or endpoint changed after drag start", () => {
    const source = createSftpPaneState("source", "remote");
    const target = createSftpPaneState("target", "local");
    replaceSftpPaneEndpoint(source, {
      kind: "remote",
      hostId: "host-1",
      sessionId: "session-1",
      generation: "1",
    }, "/", [47], "remote-directory-1");
    replaceSftpPaneEndpoint(target, {
      kind: "local",
      directoryRef: "local-directory-1",
      revision: "1",
      displayPath: "Downloads",
    }, "Downloads", null, "local-directory-1");
    source.entries = [fileEntry];
    selectSftpPaneEntry(source, fileEntry.key);
    const intent = captureSftpPaneDragIntent(source, fileEntry)!;
    const panes = { source, target };

    expect(resolveSftpPaneDropFence(intent, panes, target.paneId)).not.toBeNull();

    selectSftpPaneEntry(source, null);
    expect(resolveSftpPaneDropFence(intent, panes, target.paneId)).toBeNull();
  });

  it("supports additive and range selection with one monotonic selection revision", () => {
    const pane = createSftpPaneState("pane", "remote");
    const second = { ...fileEntry, key: "file:second", displayName: "second" };
    const third = { ...fileEntry, key: "file:third", displayName: "third" };
    pane.entries = [fileEntry, second, third];

    selectSftpPaneEntry(pane, fileEntry.key);
    toggleSftpPaneEntry(pane, third.key);
    expect(pane.selectedEntryKeys).toEqual([fileEntry.key, third.key]);
    expect(pane.selectedEntryKey).toBe(third.key);

    selectSftpPaneEntryRange(
      pane,
      second.key,
      pane.entries.map((entry) => entry.key),
    );
    expect(pane.selectedEntryKeys).toEqual([second.key, third.key]);
    expect(pane.selectedEntryKey).toBe(second.key);

    toggleSftpPaneEntry(pane, second.key);
    expect(pane.selectedEntryKeys).toEqual([third.key]);
    expect(pane.selectedEntryKey).toBe(third.key);
  });

  it("rejects server copy when both Panes reference the same SFTP session", () => {
    const source = createSftpPaneState("source", "remote");
    const target = createSftpPaneState("target", "remote");
    const endpoint = {
      kind: "remote" as const,
      hostId: "host-1",
      sessionId: "session-1",
      generation: "1",
    };
    replaceSftpPaneEndpoint(source, endpoint, "/source", [47, 115], "source-directory");
    replaceSftpPaneEndpoint(target, endpoint, "/target", [47, 116], "target-directory");
    source.entries = [fileEntry];
    selectSftpPaneEntry(source, fileEntry.key);
    const intent = captureSftpPaneDragIntent(source, fileEntry)!;

    expect(resolveSftpPaneDropFence(intent, { source, target }, target.paneId)).toBeNull();
  });

  it("allows bounded directory copy intents while keeping symlinks failed closed", () => {
    const pane = createSftpPaneState("source", "remote");
    const directory = {
      ...fileEntry,
      key: "directory:releases",
      displayName: "releases",
      kind: "directory" as const,
      size: null,
      precondition: { kind: "directory" as const, size: null, modifiedAtUnixMs: null },
    };
    const symlink = {
      ...fileEntry,
      key: "symlink:latest",
      displayName: "latest",
      kind: "symlink" as const,
      precondition: { kind: "symlink" as const, size: 8, modifiedAtUnixMs: null },
    };

    expect(captureSftpPaneDragIntent(pane, directory)).toMatchObject({
      sourcePaneId: "source",
      entryKey: directory.key,
    });
    expect(captureSftpPaneDragIntent(pane, symlink)).toBeNull();
  });
});
