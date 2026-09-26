import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";

const save = vi.hoisted(() => vi.fn());
vi.mock("../core-api/application-preferences", () => ({
  corePreferencesEnabled: () => true,
  saveApplicationPreferences: save,
}));

import { SFTP_PREFERENCES_KEY, useSftpPreferencesStore } from "./sftpPreferences";

beforeEach(() => {
  localStorage.clear();
  setActivePinia(createPinia());
  save.mockReset().mockResolvedValue(true);
});

describe("Core-owned file browsing preferences", () => {
  it("reports local directory memory failure after the Core preference commits", async () => {
    const store = useSftpPreferencesStore();
    const write = vi.spyOn(localStorage, "setItem").mockImplementation(() => { throw new Error("full"); });
    expect(await store.setRememberLastDirectory(true)).toBe(true);
    expect(save).toHaveBeenCalledOnce();
    expect(store.rememberLastDirectory).toBe(true);
    expect(store.directoryMemoryPersistenceFailed).toBe(true);
    write.mockRestore();
    expect(await store.setRememberLastDirectory(false)).toBe(true);
    expect(store.directoryMemoryPersistenceFailed).toBe(false);
  });

  it("discards stale device paths when Core says remembering is disabled", async () => {
    localStorage.setItem(SFTP_PREFERENCES_KEY, JSON.stringify({
      version: 1,
      browser: { showHidden: true, foldersFirst: true, sort: "name" },
      rememberLastDirectory: true,
      directories: { local: "/old", remote: [] },
    }));
    const store = useSftpPreferencesStore();
    store.hydrateCorePreferences({ browser: store.browser, rememberLastDirectory: false });
    expect(await store.setRememberLastDirectory(true)).toBe(true);
    expect(store.rememberedLocalDirectory()).toBeNull();
  });
});
