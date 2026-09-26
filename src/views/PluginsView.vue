<script setup lang="ts">
import { Blocks, ChevronDown, CircleAlert, PackageCheck, PackagePlus, Power, ScrollText, ShieldCheck } from "lucide-vue-next";
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";

import NvxPluginAppIntegrations from "../components/plugins/NvxPluginAppIntegrations.vue";
import NvxPluginSettingsDialog from "../components/plugins/NvxPluginSettingsDialog.vue";
import NvxPluginCapabilityChoice from "../components/plugins/NvxPluginCapabilityChoice.vue";
import { NvxPluginExtensionTarget, NvxPluginManageActions, NvxPluginOperationPermissionsDialog } from "../components/plugins";
import { isSpecialPluginCapability as isSpecialCapability } from "../components/plugins/pluginCapabilities";
import { NvxButton, NvxCard, NvxCheckbox, NvxDialog, NvxIcon, NvxInlineNotice, NvxProgress, NvxStatusLabel, NvxTextAction } from "../components/ui";
import { openPluginSpecialPermission } from "../core-api/client";
import type { InstalledPluginSummary, PluginCapability, PluginCapabilityGrant, PluginOperationSummary, PluginSpecialPermissionOutcome } from "../core-api/generated/core-api";
import { pluginFailureCode, usePluginsStore } from "../stores/plugins";
import { useTipsStore } from "../stores/tips";
import { useRouteReveal } from "../routeReveal";

const { locale, t } = useI18n();
const router = useRouter();
const plugins = usePluginsStore();
const tips = useTipsStore();
const revealRoute = useRouteReveal();
const feedbackScope = "plugins-operation";
const permissionTarget = ref<InstalledPluginSummary | null>(null);
const operationPermissionTarget = ref<InstalledPluginSummary | null>(null);
const settingsTarget = ref<InstalledPluginSummary | null>(null);
const detailsTarget = ref<InstalledPluginSummary | null>(null);
const permissionDraft = ref<PluginCapabilityGrant[]>([]);
const dangerTarget = ref<InstalledPluginSummary | null>(null);
const dangerAction = ref<"disable" | "uninstall">("disable");
const deleteData = ref(false);
const actionPending = ref(false);
const specialPermissionPending = ref<string | null>(null);
const preparedApprovalExpired = ref(false);
const visualFixtureMode = ref(false);
const operationList = computed(() => Object.values(plugins.operations)
  .filter((operation) => operation.kind !== "uninstall")
  .sort((a, b) => Number(b.updatedAtUnixMs - a.updatedAtUnixMs)));
const pluginsPageContributions = computed(() => plugins.contributions.filter((panel) => panel.slot === "pluginsPage"));
const enabledPluginCount = computed(() => plugins.installed.filter((plugin) => plugin.state === "enabled").length);
const attentionPluginCount = computed(() => plugins.installed.filter((plugin) => ["crashed", "quarantined", "incompatible"].includes(plugin.state)).length);
const installedPlugins = computed(() => [...plugins.installed].sort((left, right) => (
  Number(right.pluginId === "org.norixor") - Number(left.pluginId === "org.norixor")
)));
const preparedIsUpdate = computed(() => plugins.preparedPackage?.currentVersion !== null && plugins.preparedPackage?.currentVersion !== undefined);
const preparedCurrentPlugin = computed(() => plugins.installed.find((plugin) => plugin.pluginId === plugins.preparedPackage?.pluginId) ?? null);
const preparedSameVersionOverwrite = computed(() => !!plugins.preparedPackage?.priorPackageSha256
  && plugins.preparedPackage.packageSha256 !== plugins.preparedPackage.priorPackageSha256);
const preparedPluginWasEnabled = computed(() => {
  const pluginId = plugins.preparedPackage?.pluginId;
  return pluginId !== undefined && plugins.installed.some((plugin) => plugin.pluginId === pluginId && plugin.state === "enabled");
});
const preparedRetainedGrants = computed(() => {
  const preview = plugins.preparedPackage;
  return (preview?.retainedCapabilityGrants ?? []).filter((grant) => {
    const approved = preview?.approvedSpecialGrants.find((item) => item.capability === grant.capability);
    return !approved || approved.granted === grant.granted;
  });
});
const permissionChanges = (retained: PluginCapabilityGrant[]) => permissionDraft.value.filter(
  (grant) => !retained.some((previous) => previous.capability === grant.capability),
);
const activeOperationStates = new Set(["pending", "running", "awaitingCapabilities"]);
const contributionActionKey = (panel: { pluginId: string; instanceGeneration: string; contributionRevision: string }, actionId: string) => `${panel.pluginId}:${panel.instanceGeneration}:${panel.contributionRevision}:${actionId}`;
const formatAuditTime = (value: bigint) => new Intl.DateTimeFormat(locale.value, {
  dateStyle: "short",
  timeStyle: "medium",
}).format(new Date(Number(value)));

const capabilityLabel = (capability: PluginCapability) => t(`plugins.capabilities.${capability}.label`);
const capabilityDescription = (capability: PluginCapability) => t(`plugins.capabilities.${capability}.description`);
const grantedCapabilityCount = (plugin: InstalledPluginSummary) => plugin.grants.filter((grant) => grant.granted).length;
watch(() => plugins.errorCode, (errorCode) => {
  if (!errorCode) return;
  tips.show({
    scope: feedbackScope,
    tone: "error",
    title: t("plugins.feedback.errorTitle"),
    message: t(`plugins.errors.${errorCode}`),
  });
});
watch(() => plugins.success, (success) => {
  if (!success) return;
  tips.show({
    scope: feedbackScope,
    tone: "success",
    title: t("plugins.feedback.successTitle"),
    message: t(`plugins.success.${success.kind}`, { name: success.pluginName ?? "" }),
  });
});
function installStateTone(plugin: InstalledPluginSummary) {
  if (plugin.state === "enabled") return "success";
  if (["crashed", "quarantined", "incompatible"].includes(plugin.state)) return "danger";
  return "neutral";
}
function operationTone(operation: PluginOperationSummary) {
  if (operation.state === "succeeded") return "success";
  if (operation.state === "failed") return "danger";
  if (operation.state === "cancelled") return "warning";
  return "info";
}
function auditTone(outcome: string) {
  const normalized = outcome.toLowerCase();
  if (["committed", "succeeded", "approved", "accepted"].some((value) => normalized.includes(value))) return "success";
  if (["failed", "rejected", "denied", "blocked"].some((value) => normalized.includes(value))) return "danger";
  if (["pending", "unknown"].some((value) => normalized.includes(value))) return "warning";
  return "neutral";
}
function updatePermission(capability: PluginCapability, granted: boolean) {
  permissionDraft.value = permissionDraft.value.map((grant) => grant.capability === capability ? { ...grant, granted } : grant);
}
async function choosePackage() {
  await plugins.prepareImport();
}
async function confirmImport() {
  const preview = plugins.preparedPackage;
  if (!preview || actionPending.value || specialPermissionPending.value || preparedApprovalExpired.value) return;
  actionPending.value = true;
  await plugins.installPrepared(preview, permissionDraft.value);
  actionPending.value = false;
}
function openPermissions(plugin: InstalledPluginSummary) {
  permissionTarget.value = plugin;
  permissionDraft.value = plugin.capabilities.map((capability) => ({ capability, granted: plugin.grants.find((grant) => grant.capability === capability)?.granted ?? false }));
}
async function confirmPermissions() {
  if (!permissionTarget.value || actionPending.value) return;
  actionPending.value = true;
  const updated = await plugins.replaceGrants(permissionTarget.value, permissionDraft.value);
  actionPending.value = false;
  if (updated) permissionTarget.value = null;
}
function closePreparedPackage() {
  void plugins.discardPrepared();
}
async function openSpecialPermissions(plugin: InstalledPluginSummary | null, capability: PluginCapability | null = null) {
  if (!plugin || actionPending.value) return;
  actionPending.value = true;
  try {
    await openPluginSpecialPermission({
      target: { kind: "installed", pluginId: plugin.pluginId, expectedPluginStateVersion: plugin.stateVersion },
      requestedCapability: capability,
    });
  } catch (error) {
    plugins.clearFeedback();
    plugins.errorCode = pluginFailureCode(error);
  }
  finally { actionPending.value = false; }
}
async function openPreparedSpecialPermission(capability: PluginCapability) {
  const preview = plugins.preparedPackage;
  if (!preview || actionPending.value || specialPermissionPending.value) return;
  specialPermissionPending.value = preview.preparationId;
  try {
    await openPluginSpecialPermission({
      target: {
        kind: "preparedPackage", preparationId: preview.preparationId,
        expectedPackageSha256: preview.packageSha256,
        expectedStateVersion: preview.currentStateVersion,
      },
      requestedCapability: capability,
    });
  } catch (error) {
    specialPermissionPending.value = null;
    plugins.clearFeedback();
    plugins.errorCode = pluginFailureCode(error);
  }
}
watch(() => plugins.preparedPackage, (preview, previous, onCleanup) => {
  preparedApprovalExpired.value = false;
  if (!preview) { specialPermissionPending.value = null; return; }
  if (preview.preparationId !== previous?.preparationId) {
    specialPermissionPending.value = null;
    permissionDraft.value = preview.capabilities.map((capability) => ({ capability, granted: false }));
  }
  const approved = preview.approvedSpecialGrants ?? [];
  const hasExpiry = preview.specialPermissionExpiresAtUnixMs !== null;
  const expiresAt = Number(preview.specialPermissionExpiresAtUnixMs ?? 0);
  const synchronize = () => {
    preparedApprovalExpired.value = hasExpiry && expiresAt <= Date.now();
    permissionDraft.value = permissionDraft.value.map((grant) => isSpecialCapability(grant.capability)
      ? { ...grant, granted: !preparedApprovalExpired.value && (approved.find((item) => item.capability === grant.capability)?.granted ?? false) }
      : grant);
  };
  synchronize();
  if (expiresAt > Date.now()) {
    const timeout = window.setTimeout(synchronize, Math.min(expiresAt - Date.now(), 2_147_483_647));
    onCleanup(() => window.clearTimeout(timeout));
  }
}, { immediate: true });
async function toggleSafeModeNextStart() {
  if (actionPending.value) return;
  actionPending.value = true;
  await plugins.setSafeModeNextStart(!(plugins.readiness?.safeModeNextStart ?? false));
  actionPending.value = false;
}
function openDanger(plugin: InstalledPluginSummary, action: "disable" | "uninstall") {
  dangerTarget.value = plugin;
  dangerAction.value = action;
  deleteData.value = false;
}
async function confirmDanger() {
  if (!dangerTarget.value || actionPending.value) return;
  actionPending.value = true;
  const result = dangerAction.value === "disable" ? await plugins.setEnabled(dangerTarget.value, false) : await plugins.remove(dangerTarget.value, deleteData.value);
  actionPending.value = false;
  if (result) dangerTarget.value = null;
}
async function copyContribution(panel: Parameters<typeof plugins.copyContribution>[0], copyId: string) {
  const text = await plugins.copyContribution(panel, copyId);
  if (text === null) return;
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    plugins.clearFeedback();
  }
}
function handleSpecialPermissionChanged(event: Event) {
  const outcome = (event as CustomEvent<PluginSpecialPermissionOutcome>).detail;
  if (outcome?.kind === "preparedPackage") {
    if (specialPermissionPending.value === outcome.preview.preparationId
      && plugins.preparedPackage?.packageSha256 === outcome.preview.packageSha256) specialPermissionPending.value = null;
    return;
  }
  if (outcome?.kind === "installed" && permissionTarget.value?.pluginId === outcome.plugin.pluginId) {
    permissionTarget.value = outcome.plugin;
    permissionDraft.value = permissionDraft.value.map((grant) => isSpecialCapability(grant.capability)
      ? { ...grant, granted: outcome.plugin.grants.find((item) => item.capability === grant.capability)?.granted ?? false }
      : grant);
  }
  void plugins.initialize();
}
function handleTerminalInputDecided() {
  void plugins.loadPendingInput();
}
function applyVisualFixture() {
  if (!import.meta.env.DEV) return false;
  const hashQuery = window.location.hash.split("?", 2)[1] ?? "";
  const params = new URLSearchParams(hashQuery || window.location.search);
  if (params.get("visualFixture") !== "plugins") return false;
  visualFixtureMode.value = true;
  const utility: InstalledPluginSummary = {
    pluginId: "com.norishell.utility-demo",
    name: "NoriShell UUID Generator",
    publisher: "NoriShell",
    packageKind: "wasm",
  artifactFingerprintSha256: "a".repeat(64),
    activeVersion: "1.3.0",
    packageSha256: "a".repeat(64),
    capabilities: ["uiPanel", "clipboardWrite"],
    grants: [
      { capability: "uiPanel", granted: true },
      { capability: "clipboardWrite", granted: true },
    ],
    hasSettings: false,
    state: "enabled",
    stateVersion: "15",
    installedAtUnixMs: 1n,
    updatedAtUnixMs: 2n,
  };
  const account: InstalledPluginSummary = {
    pluginId: "org.norixor.account",
    name: "Norixor Account",
    publisher: "Norixor",
    packageKind: "wasm",
  artifactFingerprintSha256: "b".repeat(64),
    activeVersion: "1.2.0",
    packageSha256: "b".repeat(64),
    capabilities: ["uiNavigation", "uiPage", "storagePlugin"],
    grants: [
      { capability: "uiNavigation", granted: true },
      { capability: "uiPage", granted: true },
      { capability: "storagePlugin", granted: true },
    ],
    hasSettings: false,
    state: "enabled",
    stateVersion: "2",
    installedAtUnixMs: 1n,
    updatedAtUnixMs: 2n,
  };
  plugins.installed = [utility, account];
  plugins.readiness = {
    ready: true,
    protocolMajor: 1,
    protocolMinor: 3,
    safeModeActive: false,
    safeModeNextStart: false,
  };
  plugins.pendingInput = [];
  plugins.contributions = [];
  plugins.audit = [];
  const visualState = params.get("visualState") ?? "manage";
  if (visualState === "update") {
    plugins.preparedPackage = {
      preparationId: "019d0000-0000-7000-8000-000000000901",
      pluginId: utility.pluginId,
      name: utility.name,
      author: utility.publisher,
      version: "1.4.0",
      packageSize: 8_502n,
      packageSha256: "c".repeat(64),
      capabilities: utility.capabilities,
      currentVersion: utility.activeVersion,
      currentStateVersion: utility.stateVersion,
      priorPackageSha256: null,
      retainedCapabilityGrants: utility.grants,
      approvedSpecialGrants: [],
      specialPermissionExpiresAtUnixMs: null,
      publisherVerified: true,
    };
    permissionDraft.value = utility.capabilities.map((capability) => ({ capability, granted: true }));
  } else if (visualState === "permissions") {
    openPermissions(account);
  }
  return true;
}
onMounted(() => {
  if (applyVisualFixture()) revealRoute();
  else void plugins.initialize().finally(revealRoute);
  window.addEventListener("norishell:plugin-special-permission-changed", handleSpecialPermissionChanged);
  window.addEventListener("norishell:plugin-terminal-input-decided", handleTerminalInputDecided);
});
onBeforeUnmount(() => {
  window.removeEventListener("norishell:plugin-special-permission-changed", handleSpecialPermissionChanged);
  window.removeEventListener("norishell:plugin-terminal-input-decided", handleTerminalInputDecided);
});
</script>

<template>
  <section class="plugins-page">
    <NvxPluginAppIntegrations />
    <header class="plugins-page__topbar">
      <h1 class="plugins-page__title">
        {{ t("plugins.title") }}
      </h1>
      <div class="plugins-page__actions">
        <NvxButton
          size="sm"
          variant="ghost"
          :disabled="actionPending || plugins.loading"
          :aria-pressed="plugins.readiness?.safeModeNextStart ?? false"
          @click="toggleSafeModeNextStart"
        >
          <NvxIcon
            :icon="ShieldCheck"
            :size="16"
            aria-hidden="true"
          />
          {{ t(plugins.readiness?.safeModeNextStart
            ? "plugins.safeMode.cancelNextStart"
            : "plugins.safeMode.enableNextStart") }}
        </NvxButton>
        <NvxButton
          size="sm"
          :loading="plugins.loading"
          @click="choosePackage"
        >
          <NvxIcon
            :icon="PackagePlus"
            :size="16"
            aria-hidden="true"
          />
          {{ t("plugins.importZip") }}
        </NvxButton>
      </div>
    </header>

    <div class="plugins-management">
      <NvxInlineNotice
        v-if="plugins.readiness?.safeModeActive"
        tone="warning"
        :title="t('plugins.safeMode.activeTitle')"
      >
        {{ t("plugins.safeMode.activeDescription") }}
      </NvxInlineNotice>
      <NvxInlineNotice
        v-else-if="plugins.readiness?.safeModeNextStart"
        :title="t('plugins.safeMode.nextTitle')"
      >
        {{ t("plugins.safeMode.nextDescription") }}
      </NvxInlineNotice>
      <section
        v-if="plugins.pendingInput.length"
        class="plugin-operations"
        aria-labelledby="pending-input-title"
      >
        <h2 id="pending-input-title">
          {{ t("plugins.pendingInput.title") }}
        </h2>
        <article
          v-for="proposal in plugins.pendingInput"
          :key="proposal.approvalId"
        >
          <div class="row-between">
            <span><strong>{{ proposal.pluginName }}</strong><small>{{ proposal.hostLabel ?? proposal.endpoint }}</small></span><NvxButton
              size="sm"
              variant="secondary"
              @click="plugins.reviewInput(proposal)"
            >
              {{ t("plugins.pendingInput.review") }}
            </NvxButton>
          </div>
        </article>
      </section>

      <NvxPluginExtensionTarget
        v-if="!visualFixtureMode"
        target-id="plugins.page"
        instance-key="global"
      />

      <section
        v-if="pluginsPageContributions.length"
        class="plugin-contributions"
        aria-labelledby="contributions-title"
      >
        <div>
          <h2 id="contributions-title">
            {{ t("plugins.contributions.title") }}
          </h2><p>{{ t("plugins.contributions.description") }}</p>
        </div>
        <div class="plugin-grid">
          <NvxCard
            v-for="panel in pluginsPageContributions"
            :key="`${panel.pluginId}:${panel.slot}:${panel.instanceGeneration}`"
          >
            <article class="plugin-card">
              <h3>{{ panel.pluginName }}</h3>
              <template
                v-for="(node, index) in panel.nodes"
                :key="node.kind === 'action' ? node.actionId : node.kind === 'copy' ? node.copyId : index"
              >
                <p v-if="node.kind === 'text'">
                  {{ node.text }}
                </p>
                <NvxStatusLabel
                  v-else-if="node.kind === 'status'"
                  :tone="node.tone"
                >
                  {{ node.label }}
                </NvxStatusLabel>
                <NvxButton
                  v-else-if="node.kind === 'action'"
                  size="sm"
                  variant="secondary"
                  :loading="plugins.invokingActionKey === contributionActionKey(panel, node.actionId)"
                  :disabled="plugins.invokingActionKey !== null"
                  @click="plugins.invokeContribution(panel, node.actionId)"
                >
                  {{ node.label }}
                </NvxButton>
                <NvxButton
                  v-else
                  size="sm"
                  variant="secondary"
                  @click="copyContribution(panel, node.copyId)"
                >
                  {{ node.label }}
                </NvxButton>
              </template>
            </article>
          </NvxCard>
        </div>
      </section>

      <section
        v-if="operationList.length"
        class="plugin-operations"
        aria-labelledby="operations-title"
      >
        <h2 id="operations-title">
          {{ t("plugins.operations.title") }}
        </h2>
        <article
          v-for="operation in operationList"
          :key="operation.operationId"
        >
          <div class="row-between">
            <strong>{{ t(`plugins.operationKinds.${operation.kind}`) }}</strong><NvxStatusLabel :tone="operationTone(operation)">
              {{ t(`plugins.operationStates.${operation.state}`) }}
            </NvxStatusLabel>
          </div>
          <NvxProgress
            :label="t('plugins.operations.progress')"
            :value="operation.progressPercent"
            :status="operation.state === 'failed' ? 'error' : 'available'"
          />
          <div class="row-between plugin-operation__footer">
            <span
              v-if="operation.errorCode"
              role="alert"
            >{{ t(`plugins.errors.${operation.errorCode}`) }}</span><NvxTextAction
              v-if="activeOperationStates.has(operation.state)"
              tone="danger"
              @click="plugins.cancel(operation.operationId)"
            >
              {{ t("plugins.operations.cancel") }}
            </NvxTextAction>
          </div>
        </article>
      </section>

      <section
        class="installed-section"
        aria-labelledby="installed-title"
      >
        <div class="section-heading">
          <div>
            <h2 id="installed-title">
              {{ t("plugins.installedTitle") }}
            </h2><p>{{ t("plugins.installedDescription") }}</p>
          </div>
          <div
            class="plugin-summary"
            :aria-label="t('plugins.summary.label')"
          >
            <span><NvxIcon
              :icon="Blocks"
              :size="16"
              aria-hidden="true"
            />{{ t("plugins.summary.installedValue", { count: plugins.installed.length }) }}</span>
            <span class="plugin-summary__enabled"><NvxIcon
              :icon="Power"
              :size="16"
              aria-hidden="true"
            />{{ t("plugins.summary.enabledValue", { count: enabledPluginCount }) }}</span>
            <span :class="{ 'plugin-summary__attention': attentionPluginCount > 0 }"><NvxIcon
              :icon="attentionPluginCount > 0 ? CircleAlert : ShieldCheck"
              :size="16"
              aria-hidden="true"
            />{{ t("plugins.summary.attentionValue", { count: attentionPluginCount }) }}</span>
          </div>
        </div>
        <p
          v-if="plugins.loading"
          class="plugin-empty"
          role="status"
        >
          {{ t("plugins.loading") }}
        </p>
        <p
          v-else-if="!plugins.installed.length"
          class="plugin-empty"
          role="status"
        >
          {{ t("plugins.empty.installed") }}
        </p>
        <div
          v-else
          class="plugin-table"
          role="table"
          :aria-label="t('plugins.installedTitle')"
        >
          <div
            class="plugin-table__header"
            role="row"
          >
            <span role="columnheader">{{ t("plugins.columns.plugin") }}</span>
            <span role="columnheader">{{ t("plugins.columns.state") }}</span>
            <span role="columnheader">{{ t("plugins.columns.version") }}</span>
            <span role="columnheader">{{ t("plugins.columns.permissions") }}</span>
            <span role="columnheader">{{ t("plugins.columns.actions") }}</span>
          </div>
          <article
            v-for="plugin in installedPlugins"
            :key="plugin.pluginId"
            class="plugin-table__row"
            role="row"
          >
            <div
              class="plugin-table__identity"
              role="cell"
            >
              <span class="plugin-table__icon"><NvxIcon
                :icon="Blocks"
                :size="20"
                aria-hidden="true"
              /></span><span class="plugin-table__identity-copy"><strong>{{ plugin.name }}</strong><small>{{ plugin.publisher }} · {{ plugin.pluginId }}</small><small
                v-if="plugin.pluginId === 'org.norixor'"
                class="plugin-table__description"
              >{{ t("plugins.norixorDescription") }}</small></span>
            </div>
            <div
              class="plugin-table__cell plugin-table__state"
              role="cell"
            >
              <span class="plugin-table__cell-label">{{ t("plugins.columns.state") }}</span><NvxStatusLabel :tone="installStateTone(plugin)">
                {{ t(`plugins.installStates.${plugin.state}`) }}
              </NvxStatusLabel>
              <NvxButton
                v-if="plugin.packageKind === 'theme'"
                variant="ghost"
                size="sm"
                @click="router.push('/settings?section=appearance')"
              >
                {{ t('appTheme.title') }}
              </NvxButton>
            </div>
            <div
              class="plugin-table__cell plugin-table__version plugin-mono"
              role="cell"
            >
              <span class="plugin-table__cell-label">{{ t("plugins.columns.version") }}</span>{{ plugin.activeVersion }}
            </div>
            <div
              class="plugin-table__cell plugin-table__permissions"
              role="cell"
            >
              <span class="plugin-table__cell-label">{{ t("plugins.columns.permissions") }}</span>{{ t("plugins.summary.grantedCompact", { granted: grantedCapabilityCount(plugin), total: plugin.grants.length }) }}
            </div>
            <div
              class="plugin-table__actions"
              role="cell"
            >
              <NvxPluginManageActions
                :has-settings="plugin.hasSettings"
                :state="plugin.state"
                :disabled="actionPending"
                @settings="settingsTarget = plugin"
                @details="detailsTarget = plugin"
                @permissions="openPermissions(plugin)"
                @operation-permissions="operationPermissionTarget = plugin"
                @enable="plugins.setEnabled(plugin, true)"
                @disable="openDanger(plugin, 'disable')"
                @uninstall="openDanger(plugin, 'uninstall')"
              />
            </div>
          </article>
        </div>
      </section>

      <NvxPluginSettingsDialog
        v-if="settingsTarget"
        :plugin-id="settingsTarget.pluginId"
        :plugin-name="settingsTarget.name"
        @close="settingsTarget = null"
      />

      <NvxPluginOperationPermissionsDialog
        v-if="operationPermissionTarget"
        :plugin-id="operationPermissionTarget.pluginId"
        :plugin-name="operationPermissionTarget.name"
        @close="operationPermissionTarget = null"
      />

      <NvxDialog
        :model-value="detailsTarget !== null"
        :title="detailsTarget?.name ?? t('plugins.details')"
        :description="detailsTarget?.pluginId === 'org.norixor' ? t('plugins.norixorDescription') : t('plugins.packageDetails.description')"
        :close-label="t('plugins.packageDetails.close')"
        @update:model-value="(open) => { if (!open) detailsTarget = null; }"
      >
        <dl
          v-if="detailsTarget"
          class="plugin-detail-facts"
        >
          <div>
            <dt>{{ t("plugins.columns.state") }}</dt><dd>
              <NvxStatusLabel :tone="installStateTone(detailsTarget)">
                {{ t(`plugins.installStates.${detailsTarget.state}`) }}
              </NvxStatusLabel>
            </dd>
          </div>
          <div><dt>{{ t("plugins.version") }}</dt><dd>{{ detailsTarget.activeVersion }}</dd></div>
          <div><dt>{{ t("plugins.importDialog.author") }}</dt><dd>{{ detailsTarget.publisher }} · {{ t("plugins.unverified") }}</dd></div>
          <div>
            <dt>{{ t("plugins.importDialog.package") }}</dt><dd class="plugin-mono">
              {{ detailsTarget.pluginId }}
            </dd>
          </div>
          <div>
            <dt>{{ t("plugins.packageHash") }}</dt><dd class="plugin-mono">
              {{ detailsTarget.packageSha256 }}
            </dd>
          </div>
        </dl>
        <div
          v-if="detailsTarget"
          class="plugin-capabilities"
          :aria-label="t('plugins.capabilitySummary')"
        >
          <span
            v-for="grant in detailsTarget.grants"
            :key="grant.capability"
            :class="{ 'plugin-capability--denied': !grant.granted }"
          >{{ capabilityLabel(grant.capability) }} · {{ t(grant.granted ? "plugins.granted" : "plugins.denied") }}</span><span v-if="!detailsTarget.grants.length">{{ t("plugins.noCapabilities") }}</span>
        </div>
      </NvxDialog>

      <details
        v-if="plugins.audit.length"
        class="plugin-audit"
      >
        <summary class="plugin-audit__summary">
          <span class="plugin-audit__icon"><NvxIcon
            :icon="ScrollText"
            :size="20"
            aria-hidden="true"
          /></span>
          <span class="plugin-audit__heading">
            <strong>{{ t("plugins.audit.title", { count: plugins.audit.length }) }}</strong>
            <small>{{ t("plugins.audit.description") }}</small>
          </span>
          <NvxIcon
            class="plugin-audit__chevron"
            :icon="ChevronDown"
            :size="20"
            aria-hidden="true"
          />
        </summary>
        <div
          class="plugin-audit__list"
          role="table"
          :aria-label="t('plugins.audit.title', { count: plugins.audit.length })"
        >
          <div
            class="plugin-audit__header"
            role="row"
          >
            <span role="columnheader">{{ t("plugins.audit.columns.time") }}</span>
            <span role="columnheader">{{ t("plugins.audit.columns.action") }}</span>
            <span role="columnheader">{{ t("plugins.audit.columns.outcome") }}</span>
            <span role="columnheader">{{ t("plugins.audit.columns.plugin") }}</span>
            <span role="columnheader">{{ t("plugins.audit.columns.detail") }}</span>
          </div>
          <article
            v-for="entry in plugins.audit"
            :key="entry.auditId.toString()"
            class="plugin-audit__row"
            role="row"
          >
            <time
              role="cell"
              :datetime="new Date(Number(entry.createdAtUnixMs)).toISOString()"
            >
              {{ formatAuditTime(entry.createdAtUnixMs) }}
            </time>
            <strong
              role="cell"
              :title="entry.action"
            >{{ entry.action }}</strong>
            <span role="cell"><NvxStatusLabel :tone="auditTone(entry.outcome)">{{ entry.outcome }}</NvxStatusLabel></span>
            <code
              role="cell"
              :title="entry.pluginId ?? t('plugins.audit.noPlugin')"
            >{{ entry.pluginId ?? t("plugins.audit.noPlugin") }}</code>
            <small
              role="cell"
              :title="entry.detailCode ?? t('plugins.audit.noDetail')"
            >{{ entry.detailCode ?? t("plugins.audit.noDetail") }}</small>
          </article>
        </div>
      </details>
    </div>

    <NvxDialog
      plugin-protected
      :model-value="plugins.preparedPackage !== null"
      :title="t(preparedIsUpdate ? 'plugins.permissions.update.title' : 'plugins.permissions.install.title')"
      :description="t(preparedIsUpdate ? 'plugins.permissions.update.description' : 'plugins.permissions.install.description', { name: plugins.preparedPackage?.name ?? '' })"
      :close-label="t('plugins.permissions.close')"
      :dismissible="!actionPending"
      @update:model-value="(open) => { if (!open) closePreparedPackage(); }"
    >
      <dl
        v-if="plugins.preparedPackage"
        class="preview-facts"
      >
        <div><dt>{{ t("plugins.importDialog.package") }}</dt><dd>{{ plugins.preparedPackage.name }}</dd></div><div><dt>{{ t("plugins.importDialog.author") }}</dt><dd>{{ plugins.preparedPackage.author }}</dd></div><div>
          <dt>{{ t("plugins.version") }}</dt><dd>
            <template v-if="plugins.preparedPackage.currentVersion">
              {{ plugins.preparedPackage.currentVersion }} → {{ plugins.preparedPackage.version }}
            </template><template v-else>
              {{ plugins.preparedPackage.version }}
            </template>
          </dd>
        </div><div>
          <dt>{{ t("plugins.packageHash") }}</dt><dd class="plugin-mono">
            {{ plugins.preparedPackage.packageSha256 }}
          </dd>
        </div>
        <div v-if="preparedSameVersionOverwrite">
          <dt>{{ t("plugins.permissions.sameVersionOverwrite.currentHash") }}</dt>
          <dd class="plugin-mono">
            {{ plugins.preparedPackage.priorPackageSha256 }}
          </dd>
        </div>
      </dl>
      <NvxInlineNotice
        v-if="preparedSameVersionOverwrite"
        tone="warning"
        :title="t('plugins.permissions.sameVersionOverwrite.title')"
      >
        {{ t(preparedCurrentPlugin ? 'plugins.permissions.sameVersionOverwrite.description' : 'plugins.permissions.sameVersionOverwrite.reinstallDescription') }}
      </NvxInlineNotice>
      <NvxInlineNotice
        v-if="preparedApprovalExpired"
        tone="warning"
        :title="t('plugins.specialPermission.expired')"
      />
      <NvxInlineNotice
        v-if="preparedPluginWasEnabled"
        :title="t('plugins.updateFlow.title')"
      >
        {{ t("plugins.updateFlow.description") }}
      </NvxInlineNotice>
      <NvxInlineNotice
        v-if="permissionChanges(preparedRetainedGrants).length"
        tone="warning"
        :title="t('plugins.permissions.warningTitle')"
      >
        {{ t("plugins.permissions.warning") }}
      </NvxInlineNotice>
      <div
        v-if="preparedRetainedGrants.length"
        class="permission-list"
      >
        <p>{{ t("plugins.permissions.update.retained") }}</p>
        <NvxStatusLabel
          v-for="grant in preparedRetainedGrants"
          :key="grant.capability"
          :tone="grant.granted ? 'success' : 'neutral'"
        >
          {{ capabilityLabel(grant.capability) }} · {{ t(grant.granted ? "plugins.permissions.update.allowed" : "plugins.permissions.update.denied") }}
        </NvxStatusLabel>
      </div>
      <div
        v-if="permissionChanges(preparedRetainedGrants).length"
        class="permission-list"
      >
        <NvxPluginCapabilityChoice
          v-for="grant in permissionChanges(preparedRetainedGrants)"
          :key="grant.capability"
          :model-value="grant.granted"
          :disabled="actionPending || specialPermissionPending !== null"
          :requires-approval="isSpecialCapability(grant.capability)"
          @request-approval="openPreparedSpecialPermission(grant.capability)"
          @update:model-value="updatePermission(grant.capability, $event)"
        >
          {{ capabilityLabel(grant.capability) }}<template #hint>
            {{ capabilityDescription(grant.capability) }}
            <template v-if="isSpecialCapability(grant.capability)">
              {{ t("plugins.specialPermission.required") }}
            </template>
          </template>
        </NvxPluginCapabilityChoice>
      </div><p v-else-if="!preparedRetainedGrants.length">
        {{ t("plugins.noCapabilities") }}
      </p>
      <template #actions>
        <NvxButton
          variant="secondary"
          :disabled="actionPending"
          @click="closePreparedPackage"
        >
          {{ t("plugins.cancel") }}
        </NvxButton><NvxButton
          :loading="actionPending"
          :disabled="specialPermissionPending !== null || preparedApprovalExpired"
          @click="confirmImport"
        >
          <NvxIcon
            :icon="PackageCheck"
            :size="16"
            aria-hidden="true"
          />{{ t(preparedIsUpdate ? "plugins.permissions.update.confirm" : "plugins.permissions.install.confirm") }}
        </NvxButton>
      </template>
    </NvxDialog>

    <NvxDialog
      plugin-protected
      :model-value="permissionTarget !== null"
      :title="t('plugins.permissions.manage.title')"
      :description="t('plugins.permissions.manage.description', { name: permissionTarget?.name ?? '' })"
      :close-label="t('plugins.permissions.close')"
      :dismissible="!actionPending"
      @update:model-value="(open) => { if (!open) permissionTarget = null; }"
    >
      <div class="permission-list">
        <NvxPluginCapabilityChoice
          v-for="grant in permissionDraft"
          :key="grant.capability"
          :model-value="grant.granted"
          :disabled="actionPending"
          :requires-approval="isSpecialCapability(grant.capability)"
          @request-approval="openSpecialPermissions(permissionTarget, grant.capability)"
          @update:model-value="updatePermission(grant.capability, $event)"
        >
          {{ capabilityLabel(grant.capability) }}<template #hint>
            {{ capabilityDescription(grant.capability) }}
            <template v-if="isSpecialCapability(grant.capability)">
              {{ t("plugins.specialPermission.required") }}
            </template>
          </template>
        </NvxPluginCapabilityChoice>
      </div>
      <template #actions>
        <NvxButton
          v-if="permissionTarget?.capabilities.some(isSpecialCapability)"
          variant="secondary"
          :disabled="actionPending"
          @click="openSpecialPermissions(permissionTarget)"
        >
          <NvxIcon
            :icon="ShieldCheck"
            :size="16"
            aria-hidden="true"
          />{{ t("plugins.specialPermission.manage") }}
        </NvxButton>
        <NvxButton
          variant="secondary"
          :disabled="actionPending"
          @click="permissionTarget = null"
        >
          {{ t("plugins.cancel") }}
        </NvxButton><NvxButton
          :loading="actionPending"
          @click="confirmPermissions"
        >
          {{ t("plugins.permissions.manage.confirm") }}
        </NvxButton>
      </template>
    </NvxDialog>

    <NvxDialog
      plugin-protected
      :model-value="dangerTarget !== null"
      :title="t(`plugins.danger.${dangerAction}.title`)"
      :description="t(`plugins.danger.${dangerAction}.${dangerTarget?.packageKind === 'theme' ? 'themeDescription' : 'description'}`, { name: dangerTarget?.name ?? '' })"
      :close-label="t('plugins.danger.close')"
      :dismissible="!actionPending"
      @update:model-value="(open) => { if (!open) dangerTarget = null; }"
    >
      <NvxCheckbox
        v-if="dangerAction === 'uninstall'"
        v-model="deleteData"
        :disabled="actionPending"
      >
        {{ t("plugins.danger.uninstall.deleteData") }}<template #hint>
          {{ t("plugins.danger.uninstall.deleteDataHint") }}
        </template>
      </NvxCheckbox>
      <template #actions>
        <NvxButton
          variant="secondary"
          :disabled="actionPending"
          @click="dangerTarget = null"
        >
          {{ t("plugins.cancel") }}
        </NvxButton><NvxButton
          variant="danger"
          :loading="actionPending"
          @click="confirmDanger"
        >
          {{ t(`plugins.danger.${dangerAction}.confirm`) }}
        </NvxButton>
      </template>
    </NvxDialog>
  </section>
</template>

<style scoped>
.plugins-page {
  display: flex;
  overflow: hidden;
  flex-direction: column;
  min-width: 0;
  width: 100%;
  height: 100%;
  box-sizing: border-box;
}

.plugins-page__topbar {
  display: flex;
  flex: none;
  gap: var(--nvx-space-3);
  align-items: center;
  justify-content: space-between;
  min-height: 56px;
  padding: 0 var(--nvx-space-4);
  border-bottom: var(--nvx-border-width) solid var(--nvx-color-border);
  background: var(--nvx-color-bg-surface);
}

.plugins-page__title {
  min-width: 0;
  margin: 0;
  font-size: var(--nvx-font-size-md);
  white-space: nowrap;
}

.plugins-page__actions {
  display: flex;
  flex: none;
  gap: var(--nvx-space-2);
  align-items: center;
}

.plugins-management {
  flex: 1 1 auto;
  min-width: 0;
  min-height: 0;
  width: 100%;
  max-width: 1440px;
  box-sizing: border-box;
  margin: 0 auto;
  overflow-x: hidden;
  overflow-y: auto;
  padding: var(--nvx-space-6) var(--nvx-space-8);
}

.plugins-page > :deep(.nvx-inline-notice) {
  flex: none;
  margin: var(--nvx-space-3) var(--nvx-space-6);
}

.plugins-management > :deep(.nvx-inline-notice),
.plugin-operations,
.installed-section,
.plugin-audit {
  margin-top: var(--nvx-space-4);
}

.row-between {
  display: flex;
  gap: var(--nvx-space-3);
  align-items: center;
  justify-content: space-between;
}

.plugin-summary {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  border: var(--nvx-border-width) solid var(--nvx-color-border);
  border-radius: 999px;
  background: var(--nvx-color-bg-subtle);
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-xs);
}

.plugin-summary > span {
  display: flex;
  gap: var(--nvx-space-1);
  align-items: center;
  min-height: 30px;
  padding: 0 var(--nvx-space-3);
  white-space: nowrap;
}

.plugin-summary > span + span {
  border-inline-start: var(--nvx-border-width) solid var(--nvx-color-border);
}

.plugin-summary__enabled {
  color: var(--nvx-color-success);
}

.plugin-summary__attention {
  color: var(--nvx-color-warning);
}

.plugin-operations {
  display: grid;
  gap: var(--nvx-space-3);
  padding: var(--nvx-space-4);
  border: var(--nvx-border-width) solid var(--nvx-color-border);
  border-radius: var(--nvx-radius-md);
  background: var(--nvx-color-bg-surface);
}

.plugin-operations h2,
.section-heading h2,
.section-heading p,
.plugin-card h3,
.plugin-card p {
  margin: 0;
}

.plugin-operations article {
  display: grid;
  gap: var(--nvx-space-2);
  padding-top: var(--nvx-space-3);
  border-top: var(--nvx-border-width) solid var(--nvx-color-border);
}

.plugin-operation__footer {
  color: var(--nvx-color-danger);
  font-size: var(--nvx-font-size-sm);
}

.section-heading {
  display: flex;
  gap: var(--nvx-space-4);
  align-items: center;
  justify-content: space-between;
}

.section-heading p,
.plugin-card p,
.plugin-empty {
  color: var(--nvx-color-text-secondary);
}

.plugin-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(min(100%, 340px), 1fr));
  gap: var(--nvx-space-4);
  margin-top: var(--nvx-space-4);
}

.plugin-card {
  display: grid;
  gap: var(--nvx-space-4);
  height: 100%;
}

.preview-facts,
.plugin-detail-facts {
  display: grid;
  gap: var(--nvx-space-2);
  margin: 0;
}

.preview-facts dt,
.plugin-detail-facts dt {
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-xs);
}

.preview-facts dd,
.plugin-detail-facts dd {
  overflow-wrap: anywhere;
  margin: 0;
}

.preview-facts > div,
.plugin-detail-facts > div {
  display: grid;
  grid-template-columns: minmax(100px, auto) minmax(0, 1fr);
  gap: var(--nvx-space-3);
}

.plugin-table {
  overflow: visible;
  margin-top: var(--nvx-space-3);
  border: var(--nvx-border-width) solid var(--nvx-color-border);
  border-radius: var(--nvx-radius-md);
  background: var(--nvx-color-bg-surface);
}

.plugin-table__header,
.plugin-table__row {
  display: grid;
  grid-template-columns: minmax(280px, 1fr) 118px 88px 136px 180px;
  column-gap: var(--nvx-space-3);
  align-items: center;
}

.plugin-table__header {
  min-height: 34px;
  padding: 0 var(--nvx-space-3);
  border-bottom: var(--nvx-border-width) solid var(--nvx-color-border);
  background: var(--nvx-color-bg-subtle);
  color: var(--nvx-color-text-tertiary);
  font-size: var(--nvx-font-size-xs);
  font-weight: var(--nvx-font-weight-medium);
}

.plugin-table__header > :last-child {
  text-align: end;
}

.plugin-table__row {
  min-height: 60px;
  padding: 0 var(--nvx-space-3);
}

.plugin-table__row + .plugin-table__row {
  border-top: var(--nvx-border-width) solid var(--nvx-color-border);
}

.plugin-table__row:hover {
  background: var(--nvx-color-bg-hover);
}

.plugin-table__identity {
  display: flex;
  gap: var(--nvx-space-3);
  align-items: center;
  min-width: 0;
}

.plugin-table__icon {
  display: inline-flex;
  flex: 0 0 auto;
  align-items: center;
  justify-content: center;
  width: 30px;
  height: 30px;
  border-radius: var(--nvx-radius-sm);
  background: var(--nvx-color-bg-subtle);
  color: var(--nvx-color-text-secondary);
}

.plugin-table__identity-copy {
  display: grid;
  gap: 2px;
  min-width: 0;
}

.plugin-table__identity-copy strong,
.plugin-table__identity-copy small {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.plugin-table__identity-copy strong {
  font-size: var(--nvx-font-size-sm);
}

.plugin-table__identity-copy small {
  color: var(--nvx-color-text-tertiary);
  font-size: var(--nvx-font-size-xs);
}

.plugin-table__identity-copy .plugin-table__description {
  overflow: visible;
  white-space: normal;
}

.plugin-table__cell {
  min-width: 0;
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-sm);
}

.plugin-table__cell-label {
  display: none;
}

.plugin-table__actions {
  display: flex;
  justify-content: flex-end;
}

.plugin-capabilities {
  display: flex;
  flex-wrap: wrap;
  gap: var(--nvx-space-2);
  margin-top: var(--nvx-space-3);
}

.plugin-capabilities > span {
  padding: 2px var(--nvx-space-2);
  border: var(--nvx-border-width) solid var(--nvx-color-border);
  border-radius: 999px;
  background: var(--nvx-color-bg-subtle);
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-xs);
}

.plugin-capabilities > .plugin-capability--denied {
  border-style: dashed;
  text-decoration: line-through;
}

.plugin-mono {
  font-family: var(--nvx-font-mono);
  font-size: var(--nvx-font-size-xs);
}

.permission-list {
  display: grid;
  gap: var(--nvx-space-2);
  margin-top: var(--nvx-space-4);
}

.plugin-audit {
  overflow: hidden;
  border: var(--nvx-border-width) solid var(--nvx-color-border);
  border-radius: var(--nvx-radius-md);
  background: var(--nvx-color-bg-surface);
}

.plugin-audit__summary {
  display: grid;
  grid-template-columns: 34px minmax(0, 1fr) 28px;
  gap: var(--nvx-space-3);
  align-items: center;
  min-height: 58px;
  padding: 0 var(--nvx-space-4);
  cursor: pointer;
  list-style: none;
}

.plugin-audit__summary::-webkit-details-marker {
  display: none;
}

.plugin-audit__summary:hover {
  background: var(--nvx-color-bg-hover);
}

.plugin-audit__summary:focus-visible {
  outline: var(--nvx-focus-ring-width) solid var(--nvx-color-focus-ring);
  outline-offset: -2px;
}

.plugin-audit__icon {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 32px;
  height: 32px;
  border-radius: var(--nvx-radius-md);
  background: var(--nvx-color-accent-soft);
  color: var(--nvx-color-accent);
}

.plugin-audit__heading {
  display: grid;
  min-width: 0;
}

.plugin-audit__heading small {
  overflow: hidden;
  color: var(--nvx-color-text-tertiary);
  font-size: var(--nvx-font-size-xs);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.plugin-audit__chevron {
  color: var(--nvx-color-text-tertiary);
  transition: transform var(--nvx-motion-fast);
}

.plugin-audit[open] .plugin-audit__chevron {
  transform: rotate(180deg);
}

.plugin-audit__list {
  border-top: var(--nvx-border-width) solid var(--nvx-color-border);
}

.plugin-audit__header,
.plugin-audit__row {
  display: grid;
  grid-template-columns: 160px minmax(150px, 1fr) 112px minmax(170px, 1fr) minmax(120px, 0.8fr);
  gap: var(--nvx-space-3);
  align-items: center;
  padding: 0 var(--nvx-space-4);
}

.plugin-audit__header {
  min-height: 32px;
  background: var(--nvx-color-bg-subtle);
  color: var(--nvx-color-text-tertiary);
  font-size: var(--nvx-font-size-xs);
  font-weight: var(--nvx-font-weight-medium);
}

.plugin-audit__row {
  min-height: 46px;
  font-size: var(--nvx-font-size-xs);
}

.plugin-audit__row + .plugin-audit__row {
  border-top: var(--nvx-border-width) solid var(--nvx-color-border);
}

.plugin-audit__row:hover {
  background: var(--nvx-color-bg-hover);
}

.plugin-audit__row time,
.plugin-audit__row small {
  color: var(--nvx-color-text-secondary);
}

.plugin-audit__row strong,
.plugin-audit__row code,
.plugin-audit__row small {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.plugin-audit__row code {
  color: var(--nvx-color-text-secondary);
  font-family: var(--nvx-font-mono);
}

.plugin-contributions {
  display: grid;
  gap: var(--nvx-space-3);
  margin-top: var(--nvx-space-5);
}

.plugin-contributions h2,
.plugin-contributions p {
  margin: 0;
}

@media (max-width: 900px) {
  .plugin-table__header,
  .plugin-table__row {
    grid-template-columns: minmax(240px, 1fr) 112px 76px 118px 180px;
    column-gap: var(--nvx-space-2);
  }
}

@media (max-width: 960px) {
  .plugin-table__header {
    display: none;
  }

  .plugin-table__row {
    grid-template-columns: minmax(0, 1fr) auto;
    gap: var(--nvx-space-2) var(--nvx-space-3);
    padding: var(--nvx-space-3);
  }

  .plugin-table__identity {
    grid-column: 1;
  }

  .plugin-table__state {
    grid-column: 2;
    grid-row: 1;
  }

  .plugin-table__version,
  .plugin-table__permissions {
    display: inline-flex;
    grid-row: 2;
    gap: var(--nvx-space-1);
    align-items: center;
    font-size: var(--nvx-font-size-xs);
  }

  .plugin-table__version {
    grid-column: 1;
  }

  .plugin-table__permissions {
    display: none;
  }

  .plugin-table__actions {
    grid-column: 2;
    grid-row: 2;
  }

  .plugin-table__cell-label {
    display: inline;
    color: var(--nvx-color-text-tertiary);
  }
}

@media (max-width: 760px) {
  .plugins-page__topbar {
    padding: 0 var(--nvx-space-3);
  }

  .plugins-management {
    padding: var(--nvx-space-5);
  }

  .plugin-summary {
    border-radius: var(--nvx-radius-md);
  }

  .section-heading {
    align-items: flex-start;
    flex-direction: column;
  }

  .row-between {
    align-items: flex-start;
    flex-direction: column;
  }

  .plugin-audit__header {
    display: none;
  }

  .plugin-audit__row {
    grid-template-columns: 120px minmax(0, 1fr) 100px;
  }

  .plugin-audit__row code,
  .plugin-audit__row small {
    display: none;
  }
}
</style>
