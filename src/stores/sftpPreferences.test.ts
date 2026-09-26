import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { SFTP_PREFERENCES_KEY, useSftpPreferencesStore } from "./sftpPreferences";
import { createSftpPaneState, visibleSftpPaneEntries, type SftpPaneEntry } from "../views/sftpPaneState";

beforeEach(() => { localStorage.clear(); setActivePinia(createPinia()); });

describe("file browsing preferences", () => {
  it("keeps directory memory opt-in and reads earlier v1 browser-only preferences", () => {
    localStorage.setItem(SFTP_PREFERENCES_KEY, JSON.stringify({
      version: 1,
      browser: { showHidden: false, foldersFirst: false, sort: "size" },
    }));
    const store = useSftpPreferencesStore();
    expect(store.browser).toEqual({ showHidden: false, foldersFirst: false, sort: "size" });
    expect(store.rememberLastDirectory).toBe(false);
    expect(store.rememberedLocalDirectory()).toBeNull();
  });

  it("keeps the previous preference when persistence fails", async () => {
    const store = useSftpPreferencesStore();
    const write = vi.spyOn(localStorage, "setItem").mockImplementation(() => { throw new Error("full"); });
    expect(await store.replaceBrowser({ ...store.browser, showHidden: false })).toBe(false);
    expect(store.browser.showHidden).toBe(true);
    write.mockRestore();
  });

  it("restores saved defaults without changing an existing pane", async () => {
    const store = useSftpPreferencesStore();
    const oldPane = createSftpPaneState("old", "local", store.browser);
    expect(await store.replaceBrowser({ showHidden: false, foldersFirst: false, sort: "size" })).toBe(true);
    setActivePinia(createPinia());
    const restored = useSftpPreferencesStore();
    expect(createSftpPaneState("new", "remote", restored.browser).showHidden).toBe(false);
    expect(oldPane.showHidden).toBe(true);
  });

  it("rejects damaged or unknown data", () => {
    localStorage.setItem(SFTP_PREFERENCES_KEY, '{"version":1,"browser":{"showHidden":"false"}}');
    expect(useSftpPreferencesStore().browser).toEqual({ showHidden: true, foldersFirst: true, sort: "name" });
  });

  it("records only bounded, opaque directory values and clears them when disabled", async () => {
    const store = useSftpPreferencesStore();
    expect(await store.setRememberLastDirectory(true)).toBe(true);
    expect(store.rememberLocalDirectory("/Users/test/Projects")).toBe(true);
    expect(store.rememberRemoteDirectory("host-a", [47, 118, 97, 114])).toBe(true);
    expect(store.rememberedLocalDirectory()).toBe("/Users/test/Projects");
    expect(store.rememberedRemoteDirectory("host-a")).toEqual([47, 118, 97, 114]);

    for (let index = 0; index <= 100; index += 1) {
      expect(store.rememberRemoteDirectory(`host-${index}`, [47, index])).toBe(true);
    }
    expect(store.rememberedRemoteDirectory("host-a")).toBeNull();
    expect(store.rememberedRemoteDirectory("host-0")).toBeNull();
    expect(store.rememberedRemoteDirectory("host-100")).toEqual([47, 100]);
    expect(await store.setRememberLastDirectory(false)).toBe(true);
    expect(store.rememberedLocalDirectory()).toBeNull();
    expect(store.rememberedRemoteDirectory("host-100")).toBeNull();
    expect(JSON.parse(localStorage.getItem(SFTP_PREFERENCES_KEY) ?? "{}").directories).toEqual({ local: null, remote: [] });
  });

  it("does not change in-memory directory memory when its write fails", async () => {
    const store = useSftpPreferencesStore();
    expect(await store.setRememberLastDirectory(true)).toBe(true);
    expect(store.rememberLocalDirectory("/Users/test/first")).toBe(true);
    const write = vi.spyOn(localStorage, "setItem").mockImplementation(() => { throw new Error("full"); });
    expect(store.rememberLocalDirectory("/Users/test/second")).toBe(false);
    expect(store.rememberedLocalDirectory()).toBe("/Users/test/first");
    write.mockRestore();
  });

  it("keeps opaque non-UTF-8 remote bytes without accepting unknown stored fields", async () => {
    const store = useSftpPreferencesStore();
    expect(await store.setRememberLastDirectory(true)).toBe(true);
    expect(store.rememberRemoteDirectory("host-opaque", [47, 255, 128])).toBe(true);
    expect(store.rememberedRemoteDirectory("host-opaque")).toEqual([47, 255, 128]);

    setActivePinia(createPinia());
    localStorage.setItem(SFTP_PREFERENCES_KEY, JSON.stringify({
      version: 1,
      browser: { showHidden: true, foldersFirst: true, sort: "name" },
      rememberLastDirectory: true,
      directories: { local: null, remote: [] },
      unexpected: true,
    }));
    expect(useSftpPreferencesStore().rememberLastDirectory).toBe(false);
  });

  it("filters hidden entries before selection ordering without changing the listing", () => {
    const pane = createSftpPaneState("p", "remote", { showHidden: false, foldersFirst: false, sort: "name" });
    const entry = (name: string, kind: SftpPaneEntry["kind"]): SftpPaneEntry => ({
      key: name, displayName: name, kind, entryRef: name, nameBytes: [], size: 0,
      modifiedAtUnixMs: null, remotePathBytes: null, localRelativePath: null,
      precondition: { kind, size: 0, modifiedAtUnixMs: null },
    });
    pane.entries = [entry(".env", "file"), entry("z", "directory"), entry("a", "file")];
    expect(visibleSftpPaneEntries(pane).map((item) => item.key)).toEqual(["a", "z"]);
    pane.foldersFirst = true;
    expect(visibleSftpPaneEntries(pane).map((item) => item.key)).toEqual(["z", "a"]);
    expect(pane.entries).toHaveLength(3);
  });
});
