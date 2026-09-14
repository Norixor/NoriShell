import { computed, ref } from "vue";

import { latestCompatiblePluginUpdate } from "./pluginCatalogVersion";
import type { PluginCatalogEntryDto } from "../../core-api/client";
import type { PluginCapabilityGrant, PluginLocalPackagePreview } from "../../core-api/generated/core-api";
import { usePluginsStore } from "../../stores/plugins";

export type PluginBatchUpdateResultState = "succeeded" | "failed" | "skipped";

export interface PluginBatchUpdateResult {
  pluginId: string;
  name: string;
  state: PluginBatchUpdateResultState;
  errorCode: string | null;
}

type PluginsStore = ReturnType<typeof usePluginsStore>;

interface PluginBatchUpdateCandidate {
  pluginId: string;
  name: string;
  entry: PluginCatalogEntryDto;
}

type PermissionDecision = {
  kind: "confirm";
  preview: PluginLocalPackagePreview;
  grants: PluginCapabilityGrant[];
} | {
  kind: "skip" | "stop";
};

export function usePluginBatchUpdate(plugins: PluginsStore) {
  const candidates = ref<PluginBatchUpdateCandidate[]>([]);
  const current = ref<PluginBatchUpdateCandidate | null>(null);
  const results = ref<PluginBatchUpdateResult[]>([]);
  const hasStarted = ref(false);
  const phase = ref<"idle" | "preparing" | "awaitingCapabilities" | "installing" | "stopping" | "complete">("idle");
  const stopRequested = ref(false);
  let active = true;
  let permissionDecision: ((decision: PermissionDecision) => void) | null = null;

  const total = computed(() => candidates.value.length);
  const processed = computed(() => results.value.length);
  const isActive = computed(() => ["preparing", "awaitingCapabilities", "installing", "stopping"].includes(phase.value));
  const isAwaitingCapabilities = computed(() => phase.value === "awaitingCapabilities");
  const hasUpdates = computed(() => candidates.value.length > 0);

  function canRetainAllCapabilities(preview: PluginLocalPackagePreview) {
    return preview.publisherVerified
      && preview.currentVersion !== null
      && preview.capabilities.every((capability) => preview.retainedCapabilityGrants.some((grant) => grant.capability === capability));
  }

  function record(candidate: PluginBatchUpdateCandidate, state: PluginBatchUpdateResultState, errorCode: string | null = null) {
    results.value = [...results.value, { pluginId: candidate.pluginId, name: candidate.name, state, errorCode }];
  }

  async function discardPrepared(preview: PluginLocalPackagePreview | null = plugins.preparedPackage) {
    if (preview && plugins.preparedPackage?.preparationId === preview.preparationId) await plugins.discardPrepared();
  }

  async function waitForPermission() {
    return new Promise<PermissionDecision>((resolve) => {
      permissionDecision = resolve;
    });
  }

  async function run() {
    for (const candidate of candidates.value) {
      if (!active || stopRequested.value) break;
      current.value = candidate;
      phase.value = "preparing";
      let preview = await plugins.prepareCatalog(candidate.entry);
      if (!active || stopRequested.value) {
        await discardPrepared(preview);
        break;
      }
      if (!preview) {
        record(candidate, "failed", plugins.errorCode ?? "requestFailed");
        continue;
      }

      let grants: PluginCapabilityGrant[];
      if (canRetainAllCapabilities(preview)) {
        grants = preview.retainedCapabilityGrants;
      } else {
        phase.value = "awaitingCapabilities";
        const decision = await waitForPermission();
        permissionDecision = null;
        if (!active || decision.kind === "stop" || stopRequested.value) {
          await discardPrepared(preview);
          break;
        }
        if (decision.kind === "skip") {
          await discardPrepared(preview);
          record(candidate, "skipped");
          continue;
        }
        if (decision.kind !== "confirm") break;
        if (plugins.preparedPackage?.preparationId !== decision.preview.preparationId
          || plugins.preparedPackage.pluginId !== decision.preview.pluginId
          || plugins.preparedPackage.packageSha256 !== decision.preview.packageSha256) {
          record(candidate, "failed", "requestFailed");
          stopRequested.value = true;
          break;
        }
        preview = decision.preview;
        grants = decision.grants;
      }

      phase.value = "installing";
      const operation = await plugins.installPrepared(preview, grants);
      if (!active) break;
      if (!operation) {
        const errorCode = plugins.errorCode ?? "requestFailed";
        record(candidate, "failed", errorCode);
        if (!["requestFailed", "pollTimedOut"].includes(errorCode)) {
          await discardPrepared(preview);
          continue;
        }
        stopRequested.value = true;
        break;
      }
      if (!["succeeded", "failed", "cancelled"].includes(operation.state)) {
        record(candidate, "failed", operation.errorCode ?? plugins.errorCode ?? "pollTimedOut");
        stopRequested.value = true;
        break;
      }
      if (operation.state === "succeeded" && !plugins.errorCode) record(candidate, "succeeded");
      else record(candidate, "failed", operation.errorCode ?? plugins.errorCode ?? "requestFailed");
      if (stopRequested.value) break;
    }
    if (stopRequested.value) {
      for (const candidate of candidates.value) {
        if (!results.value.some((result) => result.pluginId === candidate.pluginId)) record(candidate, "skipped");
      }
    }
    current.value = null;
    permissionDecision = null;
    phase.value = "complete";
  }

  function start() {
    if (phase.value !== "idle") return;
    const catalogEntries = plugins.catalog?.entries ?? [];
    candidates.value = plugins.installed.flatMap((plugin) => {
      const entry = latestCompatiblePluginUpdate(catalogEntries, plugin.pluginId, plugin.activeVersion);
      return entry ? [{ pluginId: plugin.pluginId, name: plugin.name, entry }] : [];
    });
    results.value = [];
    hasStarted.value = true;
    stopRequested.value = false;
    if (!candidates.value.length) {
      phase.value = "complete";
      return;
    }
    void run();
  }

  function confirmCapabilities(preview: PluginLocalPackagePreview, grants: PluginCapabilityGrant[]) {
    const currentPreview = plugins.preparedPackage;
    if (!isAwaitingCapabilities.value
      || !currentPreview
      || currentPreview.preparationId !== preview.preparationId
      || currentPreview.pluginId !== preview.pluginId
      || currentPreview.packageSha256 !== preview.packageSha256) return;
    phase.value = "installing";
    permissionDecision?.({ kind: "confirm", preview: currentPreview, grants });
  }

  function skipCapabilities() {
    if (!isAwaitingCapabilities.value) return;
    permissionDecision?.({ kind: "skip" });
  }

  function stop() {
    if (!isActive.value) return;
    stopRequested.value = true;
    if (phase.value === "awaitingCapabilities") permissionDecision?.({ kind: "stop" });
    if (phase.value !== "installing") phase.value = "stopping";
  }

  function dismissSummary() {
    if (isActive.value) return;
    candidates.value = [];
    results.value = [];
    hasStarted.value = false;
    current.value = null;
    phase.value = "idle";
  }

  function dispose() {
    active = false;
    stop();
    if (phase.value === "awaitingCapabilities") void discardPrepared();
  }

  return {
    current,
    results,
    hasStarted,
    total,
    processed,
    phase,
    hasUpdates,
    isActive,
    isAwaitingCapabilities,
    start,
    confirmCapabilities,
    skipCapabilities,
    stop,
    dismissSummary,
    dispose,
  };
}
