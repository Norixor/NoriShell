import { defineStore } from "pinia";
import { isTauri } from "@tauri-apps/api/core";
import { onScopeDispose, ref } from "vue";

import {
  cancelPluginOperation,
  cancelLocalPluginPackage,
  disablePlugin,
  enablePlugin,
  getPluginOperation,
  getPluginReadiness,
  installLocalPlugin,
  invokePluginContribution,
  preparePluginContributionCopy,
  listPluginContributions,
  listInstalledPlugins,
  listPluginAudit,
  listPendingPluginTerminalInput,
  openPluginTerminalInput,
  prepareLocalPluginPackage,
  replacePluginCapabilityGrants,
  setPluginSafeModeNextStart,
  uninstallPlugin,
} from "../core-api/client";
import type {
  InstalledPluginSummary,
  PluginAuditEntry,
  PluginCapabilityGrant,
  PluginContributionPanel,
  PluginContributionSlot,
  PluginErrorCode,
  PluginOperationKind,
  PluginOperationSummary,
  PluginReadiness,
  PluginLocalPackagePreview,
  PluginSpecialPermissionOutcome,
  PluginTerminalInputProposal,
} from "../core-api/generated/core-api";
import { invalidatePluginHostDom } from "../plugins/hostDomBroker";
import { usePluginExtensionsStore } from "./pluginExtensions";

const ACTIVE_OPERATION_STATES = new Set(["pending", "running", "awaitingCapabilities"]);
const OPERATION_POLL_INTERVAL_MS = 250;
const MAX_OPERATION_POLLS = 240;
const TRANSIENT_OPERATION_DURATION_MS = 4_000;
const SUCCESS_FEEDBACK_DURATION_MS = 4_000;
const MAX_CONTRIBUTION_NODES = 64;
const MAX_CONTRIBUTION_TEXT_LENGTH = 2_048;
const BIDI_CONTROLS = new Set([0x061c, 0x200e, 0x200f, 0x202a, 0x202b, 0x202c, 0x202d, 0x202e, 0x2066, 0x2067, 0x2068, 0x2069]);

const CORE_PLUGIN_FAILURE_CODES: Partial<Record<string, PluginErrorCode>> = {
  "plugin.package_too_large": "packageTooLarge",
  "plugin.package_hash_mismatch": "packageHashMismatch",
  "plugin.package_archive_invalid": "packageArchiveInvalid",
  "plugin.package_path_rejected": "packagePathRejected",
  "plugin.package_limits_exceeded": "packageLimitsExceeded",
  "plugin.manifest_mismatch": "manifestMismatch",
  "plugin.capability_rejected": "capabilityRejected",
  "plugin.protocol_incompatible": "protocolIncompatible",
  "plugin.app_version_incompatible": "appVersionIncompatible",
  "plugin.conflict": "installConflict",
  "plugin.runtime_rejected": "runtimeRejected",
  "plugin.runtime_quota_exceeded": "runtimeQuotaExceeded",
  "plugin.runtime_timed_out": "runtimeTimedOut",
  "plugin.operation_not_found": "operationNotFound",
  "plugin.invalid_request": "invalidRequest",
};

export type PluginFailureCode = PluginErrorCode | "requestFailed" | "pollTimedOut";

export function pluginFailureCode(
  error: unknown,
  fallback: "requestFailed" | "pollTimedOut" = "requestFailed",
): PluginFailureCode {
  const candidate = error as { code?: unknown; errorCode?: unknown } | null;
  if (typeof candidate?.errorCode === "string") {
    return candidate.errorCode as PluginErrorCode;
  }
  if (typeof candidate?.code === "string") {
    const normalizedCode = candidate.code.startsWith("plugin.")
      ? candidate.code
      : `plugin.${candidate.code}`;
    return CORE_PLUGIN_FAILURE_CODES[normalizedCode] ?? fallback;
  }
  return fallback;
}

export type SafePluginContributionNode =
  | { kind: "text"; text: string }
  | { kind: "status"; label: string; tone: "neutral" | "info" | "success" | "warning" | "danger" }
  | { kind: "action"; actionId: string; label: string }
  | { kind: "copy"; copyId: string; label: string };

export interface SafePluginContributionPanel {
  pluginId: string;
  pluginName: string;
  artifactFingerprintSha256: string;
  packageSha256: string;
  instanceGeneration: string;
  stateVersion: string;
  contributionRevision: string;
  slot: PluginContributionSlot;
  nodes: SafePluginContributionNode[];
}

export function samePluginContributionFence(
  left: SafePluginContributionPanel,
  right: SafePluginContributionPanel,
) {
  return left.pluginId === right.pluginId
    && left.slot === right.slot
    && left.artifactFingerprintSha256 === right.artifactFingerprintSha256
    && left.packageSha256 === right.packageSha256
    && left.instanceGeneration === right.instanceGeneration
    && left.stateVersion === right.stateVersion
    && left.contributionRevision === right.contributionRevision;
}

export interface PluginSuccessState {
  kind: PluginOperationKind | "enable" | "permissions" | "terminalInputApproved" | "terminalInputRejected";
  pluginName: string | null;
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const prototype = Object.getPrototypeOf(value) as unknown;
  return prototype === Object.prototype || prototype === null;
}

function hasOnlyKeys(value: Record<string, unknown>, keys: readonly string[]) {
  return Object.keys(value).every((key) => keys.includes(key));
}

function boundedText(value: unknown) {
  if (
    typeof value !== "string"
    || value.length === 0
    || new TextEncoder().encode(value).byteLength > MAX_CONTRIBUTION_TEXT_LENGTH
    || Array.from(value).some((character) => {
      const codePoint = character.codePointAt(0) ?? 0;
      return (codePoint < 32 || codePoint === 127) && character !== "\t" && character !== "\n";
    })
  ) return null;
  return value;
}

function boundedActionId(value: unknown) {
  return typeof value === "string"
    && value.length > 0
    && value.length <= 80
    && /^[A-Za-z0-9._-]+$/.test(value)
    ? value
    : null;
}

function boundedActionLabel(value: unknown) {
  if (typeof value !== "string" || value.trim().length === 0) return null;
  const bytes = new TextEncoder().encode(value).byteLength;
  return bytes <= 160 && !Array.from(value).some((character) => {
    const codePoint = character.codePointAt(0) ?? 0;
    return codePoint < 32 || codePoint === 127 || BIDI_CONTROLS.has(codePoint);
  }) ? value : null;
}

/**
 * Converts untrusted declarative plugin output into a small host-owned node set.
 * Unknown fields and all HTML, CSS, SVG, iframe and URL-bearing shapes fail closed.
 */
export function normalizePluginContributionNodes(value: unknown): SafePluginContributionNode[] {
  if (!Array.isArray(value) || value.length > MAX_CONTRIBUTION_NODES) return [];
  const nodes: SafePluginContributionNode[] = [];
  const actionIds = new Set<string>();
  const copyIds = new Set<string>();
  for (const rawNode of value) {
    if (!isPlainObject(rawNode) || typeof rawNode.kind !== "string") return [];
    if (rawNode.kind === "text" && hasOnlyKeys(rawNode, ["kind", "text"])) {
      const text = boundedText(rawNode.text);
      if (text === null) return [];
      nodes.push({ kind: "text", text });
      continue;
    }
    if (rawNode.kind === "status" && hasOnlyKeys(rawNode, ["kind", "label", "tone"])) {
      const label = boundedText(rawNode.label);
      const tone = rawNode.tone;
      if (
        label !== null
        && ["neutral", "info", "success", "warning", "danger"].includes(String(tone))
      ) {
        nodes.push({
          kind: "status",
          label,
          tone: tone as "neutral" | "info" | "success" | "warning" | "danger",
        });
        continue;
      }
      return [];
    }
    if (rawNode.kind === "action" && hasOnlyKeys(rawNode, ["kind", "actionId", "label"])) {
      const actionId = boundedActionId(rawNode.actionId);
      const label = boundedActionLabel(rawNode.label);
      if (actionId === null || label === null || actionIds.has(actionId)) return [];
      actionIds.add(actionId);
      nodes.push({ kind: "action", actionId, label });
      continue;
    }
    if (rawNode.kind === "copy" && hasOnlyKeys(rawNode, ["kind", "copyId", "label"])) {
      const copyId = boundedActionId(rawNode.copyId);
      const label = boundedActionLabel(rawNode.label);
      if (copyId === null || label === null || copyIds.has(copyId)) return [];
      copyIds.add(copyId);
      nodes.push({ kind: "copy", copyId, label });
      continue;
    }
    return [];
  }
  return nodes;
}

function normalizePluginContributionPanels(
  panels: PluginContributionPanel[],
): SafePluginContributionPanel[] {
  return panels.flatMap((panel) => {
    const nodes = normalizePluginContributionNodes(panel.nodes);
    return nodes.length > 0 ? [{ ...panel, nodes }] : [];
  });
}

function waitForNextPoll() {
  return new Promise<void>((resolve) => {
    window.setTimeout(resolve, OPERATION_POLL_INTERVAL_MS);
  });
}

export const usePluginsStore = defineStore("plugins", () => {
  const pluginExtensions = usePluginExtensionsStore();
  const preparedPackage = ref<PluginLocalPackagePreview | null>(null);
  const installed = ref<InstalledPluginSummary[]>([]);
  const audit = ref<PluginAuditEntry[]>([]);
  const readiness = ref<PluginReadiness | null>(null);
  const pendingInput = ref<PluginTerminalInputProposal[]>([]);
  const contributions = ref<SafePluginContributionPanel[]>([]);
  const operations = ref<Record<string, PluginOperationSummary>>({});
  const loading = ref(false);
  const errorCode = ref<PluginFailureCode | null>(null);
  const success = ref<PluginSuccessState | null>(null);
  const invokingActionKey = ref<string | null>(null);
  let contributionLoadTask: Promise<void> | null = null;
  let installedLoadVersion = 0;
  let successTimer: number | null = null;
  const operationTimers = new Map<string, number>();

  function clearSuccessTimer() {
    if (successTimer === null) return;
    window.clearTimeout(successTimer);
    successTimer = null;
  }

  function setSuccess(next: PluginSuccessState | null) {
    clearSuccessTimer();
    success.value = next;
    if (next === null) return;
    successTimer = window.setTimeout(() => {
      success.value = null;
      successTimer = null;
    }, SUCCESS_FEEDBACK_DURATION_MS);
  }

  function recordOperation(operation: PluginOperationSummary) {
    operations.value = { ...operations.value, [operation.operationId]: operation };
    const existingTimer = operationTimers.get(operation.operationId);
    if (existingTimer !== undefined) window.clearTimeout(existingTimer);
    operationTimers.delete(operation.operationId);
    if (ACTIVE_OPERATION_STATES.has(operation.state)) return;
    operationTimers.set(operation.operationId, window.setTimeout(() => {
      const current = operations.value[operation.operationId];
      if (current && !ACTIVE_OPERATION_STATES.has(current.state)) {
        const next = { ...operations.value };
        delete next[operation.operationId];
        operations.value = next;
      }
      operationTimers.delete(operation.operationId);
    }, TRANSIENT_OPERATION_DURATION_MS));
  }

  function setFailure(error: unknown, fallback: "requestFailed" | "pollTimedOut" = "requestFailed") {
    errorCode.value = pluginFailureCode(error, fallback);
  }

  async function loadInstalled() {
    const loadVersion = ++installedLoadVersion;
    const next = await listInstalledPlugins();
    if (loadVersion === installedLoadVersion) {
      installed.value = next;
    }
    return next;
  }

  /**
   * Reconcile only the Core-owned installed-plugin projection. Runtime lifecycle
   * events must not make a failed installed-plugin read look successful.
   */
  async function refreshInstalled() {
    try {
      await loadInstalled();
      return true;
    } catch (error) {
      setFailure(error);
      return false;
    }
  }

  async function loadPendingInput() {
    pendingInput.value = await listPendingPluginTerminalInput();
  }

  async function loadContributions() {
    if (contributionLoadTask) return contributionLoadTask;
    contributionLoadTask = listPluginContributions()
      .then((panels) => {
        contributions.value = normalizePluginContributionPanels(panels);
      })
      .finally(() => {
        contributionLoadTask = null;
      });
    return contributionLoadTask;
  }

  async function reloadProjections(pluginId?: string) {
    const tasks: Promise<unknown>[] = [
      loadInstalled(),
      listPluginAudit().then((entries) => { audit.value = entries; }),
      loadPendingInput(),
      loadContributions(),
      getPluginReadiness().then((value) => { readiness.value = value; }),
    ];
    if (isTauri()) {
      tasks.push(pluginExtensions.loadNavigation());
      if (pluginId) tasks.push(pluginExtensions.refreshPluginContributions(pluginId));
    }
    await Promise.all(tasks);
  }

  async function setSafeModeNextStart(enabled: boolean) {
    try {
      readiness.value = await setPluginSafeModeNextStart(enabled);
      errorCode.value = null;
      return readiness.value;
    } catch (error) {
      setFailure(error);
      return null;
    }
  }

  async function initialize() {
    loading.value = true;
    errorCode.value = null;
    try {
      await reloadProjections();
    } catch (error) {
      setFailure(error);
    } finally {
      loading.value = false;
    }
  }

  async function trackOperation(initial: PluginOperationSummary, pluginName: string | null) {
    let operation = initial;
    recordOperation(operation);
    for (let poll = 0; ACTIVE_OPERATION_STATES.has(operation.state); poll += 1) {
      if (poll >= MAX_OPERATION_POLLS) {
        errorCode.value = "pollTimedOut";
        return operation;
      }
      await waitForNextPoll();
      operation = await getPluginOperation({ operationId: operation.operationId });
      recordOperation(operation);
    }
    if (operation.state === "succeeded") {
      setSuccess({ kind: operation.kind, pluginName });
      errorCode.value = null;
      try {
        await reloadProjections(operation.pluginId ?? undefined);
      } catch (error) {
        setFailure(error);
      }
    } else if (operation.state === "failed") {
      errorCode.value = operation.errorCode ?? "requestFailed";
    }
    return operation;
  }

  async function runOperation(
    start: () => Promise<PluginOperationSummary>,
    pluginName: string | null,
  ) {
    errorCode.value = null;
    setSuccess(null);
    try {
      return await trackOperation(await start(), pluginName);
    } catch (error) {
      setFailure(error);
      return null;
    }
  }

  async function prepareImport() {
    errorCode.value = null;
    setSuccess(null);
    try {
      await discardPrepared();
      preparedPackage.value = await prepareLocalPluginPackage();
      return preparedPackage.value;
    } catch (error) {
      setFailure(error);
      return null;
    }
  }

  function applySpecialPermissionOutcome(outcome: PluginSpecialPermissionOutcome) {
    if (outcome.kind !== "preparedPackage") return;
    const current = preparedPackage.value;
    if (current && current.preparationId === outcome.preview.preparationId
      && current.packageSha256 === outcome.preview.packageSha256) {
      preparedPackage.value = outcome.preview;
    }
  }

  async function discardPrepared() {
    const current = preparedPackage.value;
    preparedPackage.value = null;
    if (!current) return;
    try {
      await cancelLocalPluginPackage(current.preparationId);
    } catch (error) {
      setFailure(error);
    }
  }

  async function installPrepared(
    preview: PluginLocalPackagePreview,
    capabilityGrants: PluginCapabilityGrant[],
  ) {
    const current = installed.value.find((plugin) => plugin.pluginId === preview.pluginId) ?? null;
    const restoreEnabled = current?.state === "enabled";
    let expectedStateVersion = preview.currentStateVersion;
    if (restoreEnabled && current) {
      invalidatePluginHostDom(preview.pluginId);
      try {
        const disabled = await disablePlugin({
          pluginId: current.pluginId,
          expectedStateVersion: current.stateVersion,
        });
        installed.value = installed.value.map((candidate) => (
          candidate.pluginId === disabled.pluginId ? disabled : candidate
        ));
        expectedStateVersion = disabled.stateVersion;
        await reloadRuntimeProjections(disabled.pluginId);
      } catch (error) {
        setFailure(error);
        return null;
      }
    } else if (preview.currentVersion !== null) {
      invalidatePluginHostDom(preview.pluginId);
    }
    const result = await runOperation(() => installLocalPlugin({
      preparationId: preview.preparationId,
      expectedPackageSha256: preview.packageSha256,
      expectedStateVersion,
      capabilityGrants,
    }), preview.name);
    // Core consumes the preparation on every installation attempt, including failure.
    if (preparedPackage.value?.preparationId === preview.preparationId) {
      preparedPackage.value = null;
    }
    if (restoreEnabled) {
      const operationError = errorCode.value;
      const installedAfterOperation = installed.value.find((plugin) => plugin.pluginId === preview.pluginId) ?? null;
      if (installedAfterOperation?.state === "disabled") {
        try {
          const enabled = await enablePlugin({
            pluginId: installedAfterOperation.pluginId,
            expectedStateVersion: installedAfterOperation.stateVersion,
          });
          installed.value = installed.value.map((candidate) => (
            candidate.pluginId === enabled.pluginId ? enabled : candidate
          ));
          await reloadRuntimeProjections(enabled.pluginId);
          if (operationError) errorCode.value = operationError;
          if (result?.state === "succeeded") {
            setSuccess({ kind: "update", pluginName: enabled.name });
          }
        } catch (error) {
          setFailure(error);
        }
      }
    }
    return result;
  }

  async function reloadRuntimeProjections(pluginId: string) {
    const tasks: Promise<unknown>[] = [
      loadContributions(),
      loadPendingInput(),
    ];
    if (isTauri()) {
      tasks.push(
        pluginExtensions.loadNavigation(),
        pluginExtensions.refreshPluginContributions(pluginId),
      );
    }
    await Promise.all(tasks);
  }

  async function setEnabled(plugin: InstalledPluginSummary, enabled: boolean) {
    errorCode.value = null;
    setSuccess(null);
    if (!enabled) invalidatePluginHostDom(plugin.pluginId);
    try {
      const updated = await (enabled ? enablePlugin : disablePlugin)({
        pluginId: plugin.pluginId,
        expectedStateVersion: plugin.stateVersion,
      });
      installed.value = installed.value.map((candidate) => (
        candidate.pluginId === updated.pluginId ? updated : candidate
      ));
      await reloadRuntimeProjections(updated.pluginId);
      setSuccess({ kind: enabled ? "enable" : "disable", pluginName: updated.name });
      return updated;
    } catch (error) {
      setFailure(error);
      return null;
    }
  }

  async function replaceGrants(plugin: InstalledPluginSummary, grants: PluginCapabilityGrant[]) {
    errorCode.value = null;
    setSuccess(null);
    invalidatePluginHostDom(plugin.pluginId);
    try {
      const updated = await replacePluginCapabilityGrants({
        pluginId: plugin.pluginId,
        expectedStateVersion: plugin.stateVersion,
        grants,
      });
      installed.value = installed.value.map((candidate) => (
        candidate.pluginId === updated.pluginId ? updated : candidate
      ));
      await reloadRuntimeProjections(updated.pluginId);
      setSuccess({ kind: "permissions", pluginName: updated.name });
      return updated;
    } catch (error) {
      setFailure(error);
      return null;
    }
  }

  async function remove(plugin: InstalledPluginSummary, deleteData: boolean) {
    invalidatePluginHostDom(plugin.pluginId);
    const result = await runOperation(() => uninstallPlugin({
      pluginId: plugin.pluginId,
      expectedStateVersion: plugin.stateVersion,
      deleteData,
    }), plugin.name);
    return result;
  }

  async function cancel(operationId: string) {
    try {
      const operation = await cancelPluginOperation({ operationId });
      recordOperation(operation);
      return operation;
    } catch (error) {
      setFailure(error);
      return null;
    }
  }

  async function reviewInput(proposal: PluginTerminalInputProposal) {
    errorCode.value = null;
    try {
      await openPluginTerminalInput({
        approvalId: proposal.approvalId,
        expectedStateVersion: proposal.stateVersion,
      });
      return true;
    } catch (error) {
      setFailure(error);
      return false;
    }
  }

  async function invokeContribution(panel: SafePluginContributionPanel, actionId: string) {
    const actionKey = `${panel.pluginId}:${panel.slot}:${panel.instanceGeneration}:${panel.contributionRevision}:${actionId}`;
    if (invokingActionKey.value !== null) return null;
    invokingActionKey.value = actionKey;
    errorCode.value = null;
    try {
      const updated = normalizePluginContributionPanels([await invokePluginContribution({
        pluginId: panel.pluginId,
        artifactFingerprintSha256: panel.artifactFingerprintSha256,
        expectedPackageSha256: panel.packageSha256,
        instanceGeneration: panel.instanceGeneration,
        expectedStateVersion: panel.stateVersion,
        expectedContributionRevision: panel.contributionRevision,
        slot: panel.slot,
        actionId,
      })])[0] ?? null;
      const current = contributions.value.find((candidate) => candidate.pluginId === panel.pluginId && candidate.slot === panel.slot);
      const requestStillCurrent = current && samePluginContributionFence(current, panel);
      const responseMatchesRequest = updated
        && updated.pluginId === panel.pluginId
        && updated.artifactFingerprintSha256 === panel.artifactFingerprintSha256
        && updated.packageSha256 === panel.packageSha256
        && updated.instanceGeneration === panel.instanceGeneration
        && updated.stateVersion === panel.stateVersion
        && updated.slot === panel.slot
        && BigInt(updated.contributionRevision) === BigInt(panel.contributionRevision) + 1n;
      if (requestStillCurrent && responseMatchesRequest) {
        contributions.value = contributions.value.map((candidate) => (
          candidate === current ? updated : candidate
        ));
      } else {
        await loadContributions();
      }
      return responseMatchesRequest ? updated : null;
    } catch (error) {
      setFailure(error);
      await loadContributions().catch(() => undefined);
      return null;
    } finally {
      invokingActionKey.value = null;
    }
  }

  async function copyContribution(panel: SafePluginContributionPanel, copyId: string) {
    errorCode.value = null;
    try {
      const response = await preparePluginContributionCopy({
        pluginId: panel.pluginId,
        artifactFingerprintSha256: panel.artifactFingerprintSha256,
        expectedPackageSha256: panel.packageSha256,
        instanceGeneration: panel.instanceGeneration,
        expectedStateVersion: panel.stateVersion,
        expectedContributionRevision: panel.contributionRevision,
        slot: panel.slot,
        copyId,
      });
      return response.text;
    } catch (error) {
      setFailure(error);
      await loadContributions().catch(() => undefined);
      return null;
    }
  }

  function clearFeedback() {
    errorCode.value = null;
    setSuccess(null);
  }

  onScopeDispose(() => {
    clearSuccessTimer();
    for (const timer of operationTimers.values()) window.clearTimeout(timer);
    operationTimers.clear();
  });

  return {
    preparedPackage,
    installed,
    audit,
    readiness,
    pendingInput,
    contributions,
    operations,
    loading,
    errorCode,
    success,
    invokingActionKey,
    initialize,
    refreshInstalled,
    loadContributions,
    loadPendingInput,
    prepareImport,
    applySpecialPermissionOutcome,
    discardPrepared,
    installPrepared,
    setEnabled,
    setSafeModeNextStart,
    replaceGrants,
    remove,
    cancel,
    reviewInput,
    invokeContribution,
    copyContribution,
    clearFeedback,
  };
});
