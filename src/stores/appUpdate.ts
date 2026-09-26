import { invoke } from "@tauri-apps/api/core";
import { relaunch } from "@tauri-apps/plugin-process";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { defineStore } from "pinia";
import { computed, ref } from "vue";

import type { ExitReadiness } from "../core-api/generated/core-api";
import { flushTerminalWorkspaceBeforeExit } from "../terminal-workspace-persistence";

type ReleaseStatus = "idle" | "checking" | "upToDate" | "updateAvailable" | "noRelease" | "failed";
type InstallStatus = "idle" | "checking" | "downloading" | "installing" | "resourcesActive" | "cancelled" | "failed" | "restartNeeded" | "restartRequired";
type ReleaseFailureCode = "windowNotAllowed" | "requestFailed" | "httpRejected" | "responseTooLarge" | "invalidResponse" | "invalidCurrentVersion" | "internal";
type InstallFailureCode = "packageUnavailable" | "versionChanged" | "downloadFailed" | "readinessFailed" | "installFailed" | "restartFailed";

interface ReleaseCheckResponse {
  currentVersion: string;
  status: Exclude<ReleaseStatus, "idle" | "checking" | "failed">;
  latestVersion: string | null;
  supportsAutoInstall: boolean;
}

export const useAppUpdateStore = defineStore("appUpdate", () => {
  const status = ref<ReleaseStatus>("idle");
  const installStatus = ref<InstallStatus>("idle");
  const checkFailureCode = ref<ReleaseFailureCode | null>(null);
  const installFailureCode = ref<InstallFailureCode | null>(null);
  const checkDiagnosticId = ref<string | null>(null);
  const installDiagnosticId = ref<string | null>(null);
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
    checkFailureCode.value = null;
    checkDiagnosticId.value = null;
    checkInFlight = invoke<ReleaseCheckResponse>("release_check")
      .then((result) => {
        currentVersion.value = result.currentVersion;
        status.value = result.status;
        latestVersion.value = result.latestVersion;
        supportsAutoInstall.value = result.supportsAutoInstall;
      })
      .catch((error: unknown) => {
        let failure: unknown = error;
        if (typeof error === "string") {
          try { failure = JSON.parse(error) as unknown; } catch { failure = null; }
        }
        const code = typeof failure === "object" && failure !== null && "code" in failure ? failure.code : null;
        checkFailureCode.value = typeof code === "string" && ["windowNotAllowed", "requestFailed", "httpRejected", "responseTooLarge", "invalidResponse", "invalidCurrentVersion", "internal"].includes(code)
          ? code as ReleaseFailureCode : "internal";
        const diagnosticId = typeof failure === "object" && failure !== null && "diagnosticId" in failure ? failure.diagnosticId : null;
        checkDiagnosticId.value = typeof diagnosticId === "string" ? diagnosticId : checkFailureCode.value === "internal" ? crypto.randomUUID() : null;
        if (checkDiagnosticId.value && diagnosticId !== checkDiagnosticId.value) console.warn(`Release check failed; diagnosticId=${checkDiagnosticId.value}`, error);
        status.value = "failed";
      })
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
    } catch (error) {
      installFailureCode.value = "restartFailed";
      installDiagnosticId.value = crypto.randomUUID();
      console.warn(`App restart failed; diagnosticId=${installDiagnosticId.value}`, error);
      installStatus.value = "restartRequired";
    }
  }

  async function performInstall(disconnectActiveResources: boolean) {
    let update: Update | null = null;
    let prepared = false;
    let stage: "package" | "download" | "readiness" | "install" = "package";
    installStatus.value = "checking";
    installFailureCode.value = null;
    installDiagnosticId.value = null;
    progressPercent.value = null;
    try {
      update = await check({ timeout: 15_000 });
      if (!update) {
        installFailureCode.value = "packageUnavailable";
        installStatus.value = "failed";
        return;
      }
      if (update.version !== latestVersion.value) {
        installFailureCode.value = "versionChanged";
        installStatus.value = "failed";
        return;
      }
      stage = "download";
      installStatus.value = "downloading";
      let downloaded = 0;
      let total: number | undefined;
      await update.download((event) => {
        if (event.event === "Started") total = event.data.contentLength;
        if (event.event === "Progress") downloaded += event.data.chunkLength;
        if (total && total > 0) progressPercent.value = Math.min(100, Math.floor((downloaded / total) * 100));
      });
      stage = "readiness";

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
      stage = "install";
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
        if (prepared) {
          installFailureCode.value = "installFailed";
          installStatus.value = "restartNeeded";
        }
        else if (typeof error === "object" && error !== null && "code" in error
          && error.code === "app.tool_window_exit_cancelled") installStatus.value = "cancelled";
        else {
          installFailureCode.value = stage === "download" ? "downloadFailed" : stage === "readiness" ? "readinessFailed" : "packageUnavailable";
          installStatus.value = "failed";
        }
        if (installStatus.value !== "cancelled") {
          installDiagnosticId.value = crypto.randomUUID();
          console.warn(`App update failed during ${stage}; diagnosticId=${installDiagnosticId.value}`, error);
        }
      }
    } finally {
      if (installStatus.value !== "installing" && installStatus.value !== "restartRequired") {
        await update?.close().catch(() => undefined);
      }
    }
  }

  return { status, installStatus, checkFailureCode, installFailureCode, checkDiagnosticId, installDiagnosticId, currentVersion, latestVersion, supportsAutoInstall, progressPercent, hasUpdate, checkForUpdates, installUpdate, restartApp };
});
