import { defineStore } from "pinia";
import { computed, ref } from "vue";

import {
  closePluginTargetContext,
  invokePluginUiAction,
  listPluginExtensionTargets,
  listPluginNavigation,
  listPluginUiContributions,
  openPluginTargetContext,
} from "../core-api/client";
import type {
  PluginExtensionTargetContext,
  PluginExtensionTargetDefinition,
  PluginHostDomSnapshot,
  PluginNavigationItem,
  PluginUiActionId,
  PluginUiContribution,
  PluginUiFieldValue,
} from "../core-api/generated/core-api";

export interface PluginTargetLease {
  key: string;
  context: PluginExtensionTargetContext;
}

interface ActiveTargetLease extends PluginTargetLease {
  references: number;
}

function sameContributionFence(left: PluginUiContribution, right: PluginUiContribution) {
  return left.pluginId === right.pluginId
    && left.artifactFingerprintSha256 === right.artifactFingerprintSha256
    && left.packageSha256 === right.packageSha256
    && left.instanceGeneration === right.instanceGeneration
    && left.stateVersion === right.stateVersion
    && left.contributionRevision === right.contributionRevision
    && left.target.targetId === right.target.targetId
    && left.target.contextHandle === right.target.contextHandle
    && left.target.targetRevision === right.target.targetRevision;
}

export const usePluginExtensionsStore = defineStore("pluginExtensions", () => {
  const definitions = ref<PluginExtensionTargetDefinition[]>([]);
  const navigation = ref<PluginNavigationItem[]>([]);
  const leases = new Map<string, ActiveTargetLease>();
  const pendingLeases = new Map<string, Promise<ActiveTargetLease>>();
  const contributions = ref<Record<string, PluginUiContribution[]>>({});
  const loadingHandles = ref(new Set<string>());
  const busyActionKey = ref<string | null>(null);
  const busyPluginId = ref<string | null>(null);
  const error = ref<unknown>(null);
  let requestSequence = 0;
  const latestRequests = new Map<string, number>();
  let definitionsTask: Promise<void> | null = null;
  let navigationTask: Promise<void> | null = null;

  const definitionById = computed(() => new Map(
    definitions.value.map((definition) => [definition.targetId, definition]),
  ));

  async function loadDefinitions() {
    if (definitionsTask) return definitionsTask;
    definitionsTask = listPluginExtensionTargets()
      .then((next) => {
        const ids = new Set(next.map((definition) => definition.targetId));
        if (ids.size !== next.length) throw new Error("duplicate plugin extension target");
        definitions.value = next;
      })
      .finally(() => {
        definitionsTask = null;
      });
    return definitionsTask;
  }

  async function loadNavigation() {
    if (navigationTask) return navigationTask;
    navigationTask = listPluginNavigation()
      .then((next) => {
        const keys = new Set(next.map((item) => (
          `${item.pluginId}\u0000${item.navigation.navigationId}`
        )));
        if (keys.size !== next.length) throw new Error("duplicate plugin navigation item");
        navigation.value = next;
      })
      .finally(() => {
        navigationTask = null;
      });
    return navigationTask;
  }

  async function acquireTarget(
    targetId: string,
    targetInstanceKey: string,
    displayLabel?: string,
  ): Promise<PluginTargetLease> {
    await loadDefinitions();
    const definition = definitionById.value.get(targetId);
    if (!definition) throw new Error("unknown plugin extension target");
    const key = `${targetId}\u0000${targetInstanceKey}`;
    const existing = leases.get(key);
    if (existing) {
      existing.references += 1;
      return { key, context: existing.context };
    }
    const pending = pendingLeases.get(key);
    if (pending) {
      const lease = await pending;
      lease.references += 1;
      return { key, context: lease.context };
    }
    const task = openPluginTargetContext({
      targetId,
      targetInstanceKey,
      displayLabel: displayLabel ?? null,
    }).then((context) => {
      if (
        context.targetId !== targetId
        || context.surfaceKind !== definition.surfaceKind
      ) throw new Error("plugin target context mismatch");
      const lease: ActiveTargetLease = { key, context, references: 1 };
      leases.set(key, lease);
      return lease;
    }).finally(() => pendingLeases.delete(key));
    pendingLeases.set(key, task);
    const lease = await task;
    return { key, context: lease.context };
  }

  async function releaseTarget(lease: PluginTargetLease) {
    const active = leases.get(lease.key);
    if (!active || active.context.contextHandle !== lease.context.contextHandle) return;
    active.references -= 1;
    if (active.references > 0) return;
    leases.delete(lease.key);
    delete contributions.value[active.context.contextHandle];
    latestRequests.delete(active.context.contextHandle);
    loadingHandles.value = new Set([...loadingHandles.value].filter((handle) => handle !== active.context.contextHandle));
    await closePluginTargetContext({
      contextHandle: active.context.contextHandle,
      expectedTargetRevision: active.context.targetRevision,
    });
  }

  function contextIsActive(context: PluginExtensionTargetContext) {
    return [...leases.values()].some((lease) => lease.references > 0
      && lease.context.contextHandle === context.contextHandle
      && lease.context.targetRevision === context.targetRevision);
  }

  async function loadTargetContributions(context: PluginExtensionTargetContext) {
    const handle = context.contextHandle;
    if (!contextIsActive(context)) return [];
    const sequence = ++requestSequence;
    latestRequests.set(handle, sequence);
    const isCurrent = () => contextIsActive(context) && latestRequests.get(handle) === sequence;
    loadingHandles.value = new Set([...loadingHandles.value, handle]);
    try {
      const next = await listPluginUiContributions({ target: context });
      if (!isCurrent()) return [];
      if (next.some((contribution) => (
        contribution.target.targetId !== context.targetId
        || contribution.target.contextHandle !== handle
        || contribution.target.targetRevision !== context.targetRevision
      ))) throw new Error("plugin contribution target mismatch");
      // Preserve live forms when a sibling plugin refresh returns the same
      // fenced payload. Any document, metadata, or authorization change still
      // receives a new object; removed contributions are never carried forward.
      const previousById = new Map((contributions.value[handle] ?? []).map((item) => [item.pluginId, item]));
      const reconciled = next.map((item) => {
        const previous = previousById.get(item.pluginId);
        return previous && sameContributionFence(previous, item)
          && JSON.stringify(previous) === JSON.stringify(item) ? previous : item;
      });
      contributions.value = { ...contributions.value, [handle]: reconciled };
      error.value = null;
      return reconciled;
    } catch (cause) {
      if (!isCurrent()) return [];
      error.value = cause;
      throw cause;
    } finally {
      if (isCurrent()) {
        const next = new Set(loadingHandles.value);
        next.delete(handle);
        loadingHandles.value = next;
      }
    }
  }

  // Settings can reveal targets with no current contribution, so inspect every
  // live context. Action replies only invalidate siblings of the same plugin.
  async function refreshPluginContributions(pluginId: string, excludeHandle?: string) {
    const contexts = [...leases.values()].map((lease) => lease.context).filter((context) => (
      context.contextHandle !== excludeHandle
      && (excludeHandle === undefined || (contributions.value[context.contextHandle] ?? [])
        .some((item) => item.pluginId === pluginId))
    ));
    await Promise.allSettled(contexts.map(loadTargetContributions));
  }

  async function invokeAction(
    contribution: PluginUiContribution,
    actionId: PluginUiActionId,
    fields: PluginUiFieldValue[],
    hostDomSnapshot: PluginHostDomSnapshot | null,
    background = false,
  ) {
    if (busyActionKey.value !== null) return null;
    const actionKey = `${contribution.pluginId}:${contribution.target.contextHandle}:${contribution.contributionRevision}:${actionId}`;
    busyActionKey.value = actionKey;
    busyPluginId.value = contribution.pluginId;
    try {
      const response = await invokePluginUiAction({
        pluginId: contribution.pluginId,
        artifactFingerprintSha256: contribution.artifactFingerprintSha256,
        expectedPackageSha256: contribution.packageSha256,
        instanceGeneration: contribution.instanceGeneration,
        expectedStateVersion: contribution.stateVersion,
        expectedContributionRevision: contribution.contributionRevision,
        targetId: contribution.target.targetId,
        contextHandle: contribution.target.contextHandle,
        expectedTargetRevision: contribution.target.targetRevision,
        actionId,
        fields,
        hostDomSnapshot,
        background,
      });
      const updated = response.contribution;
      const handle = contribution.target.contextHandle;
      const current = contributions.value[handle] ?? [];
      const index = current.findIndex((candidate) => sameContributionFence(candidate, contribution));
      const responseMatches = sameContributionFence(
        { ...updated, contributionRevision: contribution.contributionRevision },
        contribution,
      ) && BigInt(updated.contributionRevision) === BigInt(contribution.contributionRevision) + 1n;
      if (index >= 0 && responseMatches) {
        // A list started before this action cannot replace its newer revision.
        latestRequests.set(handle, ++requestSequence);
        loadingHandles.value = new Set([...loadingHandles.value].filter((item) => item !== handle));
        const next = [...current];
        next[index] = updated;
        contributions.value = { ...contributions.value, [handle]: next };
        error.value = null;
        await refreshPluginContributions(contribution.pluginId, handle);
        return response;
      }
      await loadTargetContributions(contribution.target);
      return null;
    } catch (cause) {
      await loadTargetContributions(contribution.target).catch(() => undefined);
      // Contribution refreshes reconcile the UI but cannot overwrite or clear the failure reason from this action.
      error.value = cause;
      return null;
    } finally {
      busyActionKey.value = null;
      busyPluginId.value = null;
    }
  }

  function forContext(context: PluginExtensionTargetContext | null) {
    return context ? contributions.value[context.contextHandle] ?? [] : [];
  }

  return {
    definitions,
    navigation,
    definitionById,
    loadingHandles,
    busyActionKey,
    busyPluginId,
    error,
    loadDefinitions,
    loadNavigation,
    acquireTarget,
    releaseTarget,
    loadTargetContributions,
    refreshPluginContributions,
    invokeAction,
    forContext,
  };
});
