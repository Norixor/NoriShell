import { invoke } from "@tauri-apps/api/core";
import { relaunch } from "@tauri-apps/plugin-process";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { defineStore } from "pinia";
import { computed, ref } from "vue";

import type { ExitReadiness } from "../core-api/generated/core-api";
import { flushTerminalWorkspaceBeforeExit } from "../terminal-workspace-persistence";

type ReleaseStatus = "idle" | "checking" | "upToDate" | "updateAvailable" | "noRelease" | "failed";
type InstallStatus = "idle" | "checking" | "downloading" | "installing" | "resourcesActive" | "cancelled" | "failed" | "restartNeeded" | "restartRequired";

interface ReleaseCheckResponse {
  currentVersion: string;
  status: Exclude<ReleaseStatus, "idle" | "checking" | "failed">;
  latestVersion: string | null;
  supportsAutoInstall: boolean;
}

export const useAppUpdateStore = defineStore("appUpdate", () => {
  const status = ref<ReleaseStatus>("idle");
  const installStatus = ref<InstallStatus>("idle");
  const currentVersion = ref<string | null>(null);
  const latestVersion = ref<string | null>(null);
  const supportsAutoInstall = ref(false);
  const progressPercent = ref<number | null>(null);
  const hasUpdate = computed(() => status.value === "updateAvailable" && !!latestVersion.value);
  let checkInFlight: Promise<void> | null = null;
  let installInFlight: Promise<void> | null = null;

  function checkForUpdates(): Promise<void> {
    if (checkInFlight) return checkInFlight;
    if (installInFlight) return installInFlight;
    status.value = "checking";
    checkInFlight = invoke<ReleaseCheckResponse>("release_check")
      .then((result) => {
        currentVersion.value = result.currentVersion;
        status.value = result.status;
        latestVersion.value = result.latestVersion;
        supportsAutoInstall.value = result.supportsAutoInstall;
      })
      .catch(() => { status.value = "failed"; })
      .finally(() => { checkInFlight = null; });
    return checkInFlight;
  }

  function installUpdate(disconnectActiveResources = false): Promise<void> {
    if (installInFlight) return installInFlight;
    if (!hasUpdate.value || !supportsAutoInstall.value) return Promise.resolve();
    if (installStatus.value === "restartNeeded" || installStatus.value === "restartRequired") return Promise.resolve();
    installInFlight = performInstall(disconnectActiveResources).finally(() => { installInFlight = null; });
    return installInFlight;
  }

  async function restartApp(): Promise<void> {
    if (installStatus.value !== "restartNeeded" && installStatus.value !== "restartRequired") return;
    try {
      await invoke("release_update_allow_relaunch");
      await relaunch();
    } catch {
      // The restart action remains available if the platform could not relaunch.
    }
  }

  async function performInstall(disconnectActiveResources: boolean) {
    let update: Update | null = null;
    let prepared = false;
    installStatus.value = "checking";
    progressPercent.value = null;
    try {
      update = await check({ timeout: 15_000 });
      if (!update || update.version !== latestVersion.value) {
        installStatus.value = "failed";
        return;
      }
      installStatus.value = "downloading";
      let downloaded = 0;
      let total: number | undefined;
      await update.download((event) => {
        if (event.event === "Started") total = event.data.contentLength;
        if (event.event === "Progress") downloaded += event.data.chunkLength;
        if (total && total > 0) progressPercent.value = Math.min(100, Math.floor((downloaded / total) * 100));
      });

      // Both checks happen after the signed package is downloaded, so failed
      // downloads never interrupt a live terminal or transfer.
      const ready = await invoke<ExitReadiness>("release_update_readiness");
      if (!ready.canExit && !disconnectActiveResources) {
        installStatus.value = "resourcesActive";
        return;
      }
      await flushTerminalWorkspaceBeforeExit();
      const stillReady = await invoke<ExitReadiness>("release_update_prepare_install", { disconnectActiveResources });
      if (!stillReady.canExit) {
        installStatus.value = "resourcesActive";
        return;
      }
      prepared = true;
      installStatus.value = "installing";
      await update.install();
      // Windows exits after starting its updater; macOS needs an explicit relaunch.
      if (navigator.userAgent.includes("Mac")) {
        await invoke("release_update_allow_relaunch");
        installStatus.value = "restartRequired";
        await relaunch();
      }
    } catch (error) {
      if (installStatus.value !== "restartRequired") {
        if (prepared) installStatus.value = "restartNeeded";
        else if (typeof error === "object" && error !== null && "code" in error
          && error.code === "app.tool_window_exit_cancelled") installStatus.value = "cancelled";
        else installStatus.value = "failed";
      }
    } finally {
      if (installStatus.value !== "installing" && installStatus.value !== "restartRequired") {
        await update?.close().catch(() => undefined);
      }
    }
  }

  return { status, installStatus, currentVersion, latestVersion, supportsAutoInstall, progressPercent, hasUpdate, checkForUpdates, installUpdate, restartApp };
});
