import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({
  invoke: vi.fn(),
  check: vi.fn(),
  relaunch: vi.fn(),
  flush: vi.fn(),
  download: vi.fn(),
  install: vi.fn(),
  close: vi.fn(),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: api.invoke }));
vi.mock("@tauri-apps/plugin-updater", () => ({ check: api.check }));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch: api.relaunch }));
vi.mock("../terminal-workspace-persistence", () => ({ flushTerminalWorkspaceBeforeExit: api.flush }));

import { useAppUpdateStore } from "./appUpdate";

describe("app update", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    setActivePinia(createPinia());
    api.invoke.mockImplementation(async (command: string) => command === "release_check"
      ? { currentVersion: "0.1.2", status: "updateAvailable", latestVersion: "0.1.3", supportsAutoInstall: true }
      : { canExit: true, blockers: [] });
    api.check.mockResolvedValue({ version: "0.1.3", download: api.download, install: api.install, close: api.close });
    api.download.mockResolvedValue(undefined);
    api.install.mockResolvedValue(undefined);
    api.close.mockResolvedValue(undefined);
    api.flush.mockResolvedValue(undefined);
    api.relaunch.mockResolvedValue(undefined);
  });

  it("checks for updates without downloading or installing", async () => {
    const updates = useAppUpdateStore();
    await updates.checkForUpdates();
    expect(updates.hasUpdate).toBe(true);
    expect(api.check).not.toHaveBeenCalled();
    expect(api.download).not.toHaveBeenCalled();
  });

  it("stops before installation when a connection is still active", async () => {
    api.invoke.mockImplementation(async (command: string) => command === "release_check"
      ? { currentVersion: "0.1.2", status: "updateAvailable", latestVersion: "0.1.3", supportsAutoInstall: true }
      : { canExit: false, blockers: [{ kind: "sshSession", sessionId: "test" }] });
    const updates = useAppUpdateStore();
    await updates.checkForUpdates();
    await updates.installUpdate();
    expect(api.download).toHaveBeenCalledOnce();
    expect(api.install).not.toHaveBeenCalled();
    expect(api.flush).not.toHaveBeenCalled();
    expect(updates.installStatus).toBe("resourcesActive");
  });

  it("uses the verified package and flushes layout before installation", async () => {
    const updates = useAppUpdateStore();
    await updates.checkForUpdates();
    await updates.installUpdate();
    expect(api.check).toHaveBeenCalledWith({ timeout: 15_000 });
    expect(api.download).toHaveBeenCalledOnce();
    expect(api.invoke).toHaveBeenCalledWith("release_update_readiness");
    expect(api.invoke).toHaveBeenCalledWith("release_update_prepare_install", { disconnectActiveResources: false });
    expect(api.flush).toHaveBeenCalledOnce();
    expect(api.install).toHaveBeenCalledOnce();
    expect(api.invoke).not.toHaveBeenCalledWith("release_update_allow_relaunch");
  });

  it("requires an explicit force action before closing active resources for an update", async () => {
    api.invoke.mockImplementation(async (command: string) => {
      if (command === "release_check") {
        return { currentVersion: "0.1.2", status: "updateAvailable", latestVersion: "0.1.3", supportsAutoInstall: true };
      }
      if (command === "release_update_readiness") {
        return { canExit: false, blockers: [{ kind: "sshSession", sessionId: "test" }] };
      }
      return { canExit: true, blockers: [] };
    });
    const updates = useAppUpdateStore();
    await updates.checkForUpdates();
    await updates.installUpdate();
    expect(updates.installStatus).toBe("resourcesActive");
    expect(api.invoke).not.toHaveBeenCalledWith("release_update_prepare_install", expect.anything());
    expect(api.install).not.toHaveBeenCalled();

    await updates.installUpdate(true);
    expect(api.invoke).toHaveBeenCalledWith("release_update_prepare_install", { disconnectActiveResources: true });
    expect(api.install).toHaveBeenCalledOnce();
  });

  it("keeps the app running when a tool window cancels its draft approval", async () => {
    api.invoke.mockImplementation(async (command: string) => {
      if (command === "release_check") {
        return { currentVersion: "0.1.2", status: "updateAvailable", latestVersion: "0.1.3", supportsAutoInstall: true };
      }
      if (command === "release_update_prepare_install") throw { code: "app.tool_window_exit_cancelled" };
      return { canExit: true, blockers: [] };
    });
    const updates = useAppUpdateStore();
    await updates.checkForUpdates();
    await updates.installUpdate();
    expect(api.install).not.toHaveBeenCalled();
    expect(api.invoke).not.toHaveBeenCalledWith("release_update_allow_relaunch");
    expect(updates.installStatus).toBe("cancelled");
  });

  it("keeps resources fenced and offers a restart if installation fails", async () => {
    api.install.mockRejectedValue(new Error("installer unavailable"));
    const updates = useAppUpdateStore();
    await updates.checkForUpdates();
    await updates.installUpdate();
    expect(api.invoke).toHaveBeenCalledWith("release_update_prepare_install", { disconnectActiveResources: false });
    expect(updates.installStatus).toBe("restartNeeded");
    await updates.restartApp();
    expect(api.invoke).toHaveBeenCalledWith("release_update_allow_relaunch");
    expect(api.relaunch).toHaveBeenCalledOnce();
  });
});
