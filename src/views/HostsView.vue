<script setup lang="ts">
import { ArrowDown, ArrowUp, FileInput, Folder, FolderPlus, HeartPulse, KeyRound, Network, Pencil, Plus, RefreshCw, Server, ShieldCheck, Star, Tag, Trash2, Workflow, X } from "lucide-vue-next";
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useRoute, useRouter } from "vue-router";

import NvxHostMarker from "../components/hosts/NvxHostMarker.vue";
import NvxHostMarkerEditor from "../components/hosts/NvxHostMarkerEditor.vue";
import { normalizeHostMarker, type HostMarker } from "../host-markers";
import { useHostMarkersStore } from "../stores/hostMarkers";
import { NvxPageHeader } from "../components/layout";
import { NvxPluginExtensionTarget } from "../components/plugins";
import { NvxPluginContributionSlot } from "../components/terminal";
import {
  NvxButton,
  NvxCheckbox,
  NvxDialog,
  NvxField,
  NvxIcon,
  NvxIconButton,
  NvxInlineNotice,
  NvxInput,
  NvxSelect,
  NvxStatusLabel,
  NvxTextarea,
} from "../components/ui";
import {
  canUseDesktopCore,
  cancelHostCreatePassword,
  cancelLoginAutomationSecret,
  createConfiguredHost,
  createHostGroup,
  createIdentity,
  createKeyboardInteractiveCredential,
  createLoginAutomationSecret,
  createSshAgentCredential,
  createVault,
  commitOpenSshConfig,
  createHostTag,
  deleteHostGroup,
  deleteHostTag,
  deleteHost,
  getHostConnectionConfig,
  getAlgorithmPolicyCatalog,
  fetchVaultStatus,
  importPrivateKeyFile,
  listHostCatalog,
  listHostGroups,
  listHostTags,
  listIdentities,
  listCredentialRefs,
  listSshAgentKeys,
  previewOpenSshConfig,
  replaceHostOrganization,
  replaceAlgorithmPolicy,
  replaceHeartbeatPolicy,
  replaceMonitoringPolicy,
  replaceLoginAutomation,
  replaceRoutePlan,
  stageHostCreatePassword,
  confirmLoginAutomation,
  testSshConnection,
  updateHost,
  updateHostFavorite,
  updateHostGroup,
  updateHostTag,
  unlockVault,
  parseCoreApiError,
  prepareTransientCredential,
} from "../core-api/client";
import type {
  AlgorithmCategory,
  AlgorithmCompatibilityException,
  AlgorithmPolicyCatalog,
  CredentialRefSummary,
  HostCatalogEntry,
  HostCreateLoginAutomationStep,
  HostGroupSummary,
  HostSummary,
  HostTagSummary,
  HeartbeatPolicy,
  IdentitySummary,
  LoginAutomationStepInput,
  LoginAutomationStepSummary,
  MonitoringPolicy,
  OpenSshConfigPreviewResponse,
  OpenSshImportRoutePreview,
  RouteIngress,
  ShellHeartbeatLineEnding,
  SshAgentKeySummary,
  VaultState,
} from "../core-api/generated/core-api";
import { createUuidV7 } from "../core-api/ids";
import { useTipsStore } from "../stores/tips";

type LoginAutomationDraftStep = {
  id: string;
  kind: "expect" | "sendText" | "sendSecret";
  value: string;
  appendEnter: boolean;
  timeoutSeconds: string;
  existingOrdinal: number | null;
  stagedSecretId: string | null;
  secretLabel: string;
  secretValue: string;
};

type HostEditorSection =
  | "connection"
  | "classification"
  | "connectionRoute"
  | "algorithms"
  | "loginAutomation"
  | "connectionHealth"
  | "advanced";

type HostAuthenticationMode = "askEveryTime" | "savedIdentity" | "password" | "privateKey";

const { t, te } = useI18n();
const tips = useTipsStore();
const hostMarkers = useHostMarkersStore();
const hostMarkerDraft = ref<HostMarker | null>(null);
const route = useRoute();
const router = useRouter();
const catalog = ref<HostCatalogEntry[]>([]);
const groups = ref<HostGroupSummary[]>([]);
const tags = ref<HostTagSummary[]>([]);
const identities = ref<IdentitySummary[]>([]);
const loading = ref(false);
const loadFailed = ref(false);
const dialogOpen = ref(false);
const hostEditorSection = ref<HostEditorSection>("connection");
const saving = ref(false);
const testingConnection = ref(false);
const saveFailed = ref(false);
const saveFailureKey = ref("sshHosts.saveFailed");
const editingHost = ref<HostSummary | null>(null);
const deleteCandidate = ref<HostSummary | null>(null);
const deleting = ref(false);
const deleteFailed = ref(false);
const label = ref("");
const address = ref("");
const port = ref("22");
const username = ref("");
const identityId = ref("");
const authenticationMode = ref<HostAuthenticationMode>("askEveryTime");
const hostPassword = ref("");
const vaultState = ref<VaultState | null>(null);
const vaultStatusLoading = ref(false);
const vaultStatusUnavailable = ref(false);
const vaultDialogOpen = ref(false);
const vaultDialogMode = ref<"create" | "unlock" | null>(null);
const vaultPassword = ref("");
const vaultPasswordConfirmation = ref("");
const vaultSubmitting = ref(false);
const vaultActionFailed = ref(false);
const favorite = ref(false);
const groupId = ref("");
const selectedTagIds = ref<string[]>([]);
const newGroupLabel = ref("");
const newTagLabel = ref("");
const creatingGroup = ref(false);
const creatingTag = ref(false);
const favoriteUpdatingHostId = ref<string | null>(null);
const classificationDialogOpen = ref(false);
const classificationEdit = ref<{
  kind: "group" | "tag";
  id: string;
  stateVersion: string;
} | null>(null);
const classificationEditLabel = ref("");
const classificationSaving = ref(false);
const classificationFailed = ref(false);
const classificationNewGroupLabel = ref("");
const classificationCreatingGroup = ref(false);
const classificationDeleteCandidate = ref<{
  kind: "group" | "tag";
  id: string;
  label: string;
  stateVersion: string;
} | null>(null);
const classificationDeleting = ref(false);
const agentDialogOpen = ref(false);
const agentIdentityId = ref("");
const newIdentityLabel = ref("");
const newIdentityUsername = ref("");
const creatingIdentity = ref(false);
const agentKeys = ref<SshAgentKeySummary[]>([]);
const selectedAgentKeyHandle = ref("");
const loadingAgentKeys = ref(false);
const agentCredentialLabel = ref("");
const agentCredentialPriority = ref("100");
const savingAgentCredential = ref(false);
const agentActionFailed = ref(false);
const agentCredentialSaved = ref(false);
const privateKeyDialogOpen = ref(false);
const privateKeyIdentityId = ref("");
const newPrivateKeyIdentityLabel = ref("");
const newPrivateKeyIdentityUsername = ref("");
const creatingPrivateKeyIdentity = ref(false);
const privateKeyCredentialLabel = ref("");
const privateKeyCredentialPriority = ref("100");
const privateKeyPassphrase = ref("");
const importingPrivateKey = ref(false);
const privateKeyActionFailed = ref(false);
const privateKeyCredentialSaved = ref(false);
const privateKeyCredentialSummary = ref<CredentialRefSummary | null>(null);
const privateKeyHostEditorBinding = ref(false);
const privateKeyImportOperationId = ref<string | null>(null);
const privateKeyImportIdempotencyKey = ref<string | null>(null);
const keyboardInteractiveDialogOpen = ref(false);
const keyboardInteractiveIdentityId = ref("");
const keyboardInteractiveLabel = ref("");
const keyboardInteractivePriority = ref("110");
const keyboardInteractiveMaxRounds = ref("8");
const savingKeyboardInteractive = ref(false);
const keyboardInteractiveFailed = ref(false);
const keyboardInteractiveSaved = ref(false);
const opensshDialogOpen = ref(false);
const opensshConfigText = ref("");
const opensshPreview = ref<OpenSshConfigPreviewResponse | null>(null);
const selectedOpenSshCandidateIds = ref<string[]>([]);
const previewingOpenSsh = ref(false);
const importingOpenSsh = ref(false);
const opensshActionFailed = ref(false);
const opensshImportedCount = ref(0);
const routeHost = ref<HostSummary | null>(null);
const routeRevision = ref("1");
const routeSaveFailed = ref(false);
const routeIngressKind = ref<"directTcp" | "httpConnectProxy" | "socks5Proxy">("directTcp");
const routeProxyAddress = ref("");
const routeProxyPort = ref("8080");
const routeProxyDnsMode = ref<"local" | "proxy">("proxy");
const routeProxyCredentialRefId = ref("");
const routeJumpHostIds = ref<string[]>([]);
const routeCredentials = ref<CredentialRefSummary[]>([]);
const routeSavedFingerprint = ref("");
const algorithmCatalog = ref<AlgorithmPolicyCatalog | null>(null);
const algorithmRevision = ref("1");
const algorithmSelectedExceptionIds = ref<string[]>([]);
const algorithmStoredExceptions = ref<AlgorithmCompatibilityException[]>([]);
const algorithmSavedFingerprint = ref("");
const loginAutomationHost = ref<HostSummary | null>(null);
const loginAutomationRevision = ref("1");
const loginAutomationConfirmedRevision = ref<string | null>(null);
const loginAutomationEnabled = ref(false);
const loginAutomationSteps = ref<LoginAutomationDraftStep[]>([]);
const loginAutomationSaveFailed = ref(false);
const loginAutomationConfirmFailed = ref(false);
const loginAutomationSavedFingerprint = ref("");
const loginAutomationConfirmOnSave = ref(false);
const heartbeatRevision = ref("1");
const heartbeatMode = ref<HeartbeatPolicy["mode"]>("disabled");
const heartbeatIntervalSeconds = ref("30");
const heartbeatReplyTimeoutSeconds = ref("10");
const heartbeatFailureThreshold = ref("3");
const heartbeatPayloadText = ref("");
const heartbeatLineEnding = ref<ShellHeartbeatLineEnding>("cr");
const heartbeatUserIdleSeconds = ref("30");
const heartbeatSavedFingerprint = ref("");
const monitoringRevision = ref("1");
const monitoringEnabled = ref(false);
const monitoringIntervalSeconds = ref("15");
const monitoringTimeoutSeconds = ref("5");
const monitoringSavedFingerprint = ref("");
const monitoringRuntimePending = ref(false);
const hostConfigLoading = ref(false);
const hostConfigLoadFailed = ref(false);
const advancedConfigSaveFailed = ref(false);
const configuredCreateOperationId = ref(createUuidV7());
const configuredCreateIdempotencyKey = ref(`host-configured-create-${configuredCreateOperationId.value}`);
const stagedHostPasswordId = ref<string | null>(null);

const hostEditorSections = computed(() => [
  { value: "connection" as const, label: t("sshHosts.editorSections.connection"), icon: KeyRound },
  { value: "classification" as const, label: t("sshHosts.editorSections.classification"), icon: Folder },
  { value: "connectionRoute" as const, label: t("sshHosts.editorSections.connectionRoute"), icon: Network },
  { value: "algorithms" as const, label: t("sshHosts.editorSections.algorithms"), icon: ShieldCheck },
  { value: "loginAutomation" as const, label: t("sshHosts.editorSections.loginAutomation"), icon: Workflow },
  { value: "connectionHealth" as const, label: t("sshHosts.editorSections.connectionHealth"), icon: HeartPulse },
  { value: "advanced" as const, label: t("sshHosts.editorSections.advanced"), icon: Server },
]);

const authenticationModeOptions = computed(() => [
  ...(!editingHost.value
    ? [
        { value: "password", label: t("sshHosts.authenticationModes.password") },
        { value: "privateKey", label: t("sshHosts.authenticationModes.privateKey") },
      ]
    : []),
  { value: "savedIdentity", label: t("sshHosts.authenticationModes.savedIdentity") },
  { value: "askEveryTime", label: t("sshHosts.authenticationModes.askEveryTime") },
]);

const groupOptions = computed(() => [
  { value: "", label: t("sshHosts.noGroup") },
  ...groups.value.map((group) => ({ value: group.groupId, label: group.label })),
]);
const HOSTS_UNGROUPED_SECTION = "ungrouped";
const hostSections = computed(() => {
  const sections = new Map<string, {
    key: string;
    label: string;
    ungrouped: boolean;
    entries: HostCatalogEntry[];
  }>();
  for (const entry of catalog.value) {
    const group = entry.group;
    const key = group?.groupId ?? HOSTS_UNGROUPED_SECTION;
    const current = sections.get(key);
    if (current) {
      current.entries.push(entry);
      continue;
    }
    sections.set(key, {
      key,
      label: group?.label ?? t("sshHosts.noGroup"),
      ungrouped: group === null,
      entries: [entry],
    });
  }
  return [...sections.values()].sort((left, right) => (
    Number(left.ungrouped) - Number(right.ungrouped)
    || left.label.localeCompare(right.label)
  ));
});
const identityOptions = computed(() => [
  { value: "", label: t("sshHosts.noIdentity") },
  ...identities.value.map((identity) => ({
    value: identity.identityId,
    label: identity.username ? `${identity.label} · ${identity.username}` : identity.label,
  })),
]);
const routeIngressOptions = computed(() => [
  { value: "directTcp", label: t("sshHosts.connectionRoute.direct") },
  { value: "httpConnectProxy", label: t("sshHosts.connectionRoute.httpConnect") },
  { value: "socks5Proxy", label: t("sshHosts.connectionRoute.socks5") },
]);
const routeDnsOptions = computed(() => [
  { value: "proxy", label: t("sshHosts.connectionRoute.proxyDns") },
  { value: "local", label: t("sshHosts.connectionRoute.localDns") },
]);
const routeCredentialOptions = computed(() => {
  const identityById = new Map(identities.value.map((identity) => [identity.identityId, identity]));
  const options = routeCredentials.value.map((credential) => {
    const identity = identityById.get(credential.identityId);
    const owner = identity?.username || identity?.label || credential.identityId;
    return {
      value: credential.credentialRefId,
      label: `${owner} · ${credential.label}`,
    };
  });
  if (
    routeProxyCredentialRefId.value
    && !options.some((option) => option.value === routeProxyCredentialRefId.value)
  ) {
    options.push({
      value: routeProxyCredentialRefId.value,
      label: t("sshHosts.connectionRoute.unavailableCredential"),
    });
  }
  return [
    { value: "", label: t("sshHosts.connectionRoute.noProxyAuthentication") },
    ...options,
  ];
});
const canAddRouteJumpHost = computed(() => routeJumpHostIds.value.length < 5
  && catalog.value.some((entry) => entry.host.hostId !== routeHost.value?.hostId
    && !routeJumpHostIds.value.includes(entry.host.hostId)));
const algorithmCategories = computed(() => ([
  { value: "keyExchange" as AlgorithmCategory, label: t("sshHosts.algorithms.categoryKeyExchange") },
  { value: "hostKey" as AlgorithmCategory, label: t("sshHosts.algorithms.categoryHostKey") },
  { value: "cipher" as AlgorithmCategory, label: t("sshHosts.algorithms.categoryCipher") },
  { value: "mac" as AlgorithmCategory, label: t("sshHosts.algorithms.categoryMac") },
]));
const unavailableAlgorithmExceptions = computed(() => algorithmStoredExceptions.value.filter(
  (exception) => !algorithmCatalog.value?.entries.some(
    (entry) => entry.stableId === exception.exceptionId && entry.selectableException,
  ),
));
const hasSelectedUnavailableAlgorithm = computed(() => unavailableAlgorithmExceptions.value.some(
  (exception) => algorithmSelectedExceptionIds.value.includes(exception.exceptionId),
));
const loginAutomationStepTypeOptions = computed(() => {
  const options = [
    { value: "expect", label: t("sshHosts.loginAutomation.stepTypes.expect") },
    { value: "sendText", label: t("sshHosts.loginAutomation.stepTypes.sendText") },
    { value: "sendSecret", label: t("sshHosts.loginAutomation.stepTypes.sendSecret") },
  ];
  return editingHost.value ? options : options.filter((option) => option.value !== "sendSecret");
});
const heartbeatModeOptions = computed(() => [
  { value: "disabled", label: t("sshHosts.heartbeat.modes.disabled") },
  { value: "transportKeepalive", label: t("sshHosts.heartbeat.modes.transportKeepalive") },
  { value: "shellHeartbeat", label: t("sshHosts.heartbeat.modes.shellHeartbeat") },
]);
const heartbeatLineEndingOptions = computed(() => [
  { value: "none", label: t("sshHosts.heartbeat.lineEndings.none") },
  { value: "cr", label: t("sshHosts.heartbeat.lineEndings.cr") },
  { value: "lf", label: t("sshHosts.heartbeat.lineEndings.lf") },
  { value: "crlf", label: t("sshHosts.heartbeat.lineEndings.crlf") },
]);
const heartbeatPayloadPreview = computed(() => {
  const escaped = heartbeatPayloadText.value
    .replaceAll("\\", "\\\\")
    .replaceAll("\r", "\\r")
    .replaceAll("\n", "\\n")
    .replaceAll("\t", "\\t");
  const suffix = heartbeatLineEnding.value === "none"
    ? ""
    : heartbeatLineEnding.value === "cr"
      ? "\\r"
      : heartbeatLineEnding.value === "lf" ? "\\n" : "\\r\\n";
  return `${escaped}${suffix}`;
});
const vaultDialogPasswordValid = computed(() => Boolean(vaultPassword.value)
  && (vaultDialogMode.value !== "create" || (
    new TextEncoder().encode(vaultPassword.value).byteLength >= 12
    && vaultPassword.value === vaultPasswordConfirmation.value
  )));
const loginAutomationNeedsConfirmation = computed(() => loginAutomationEnabled.value
  && loginAutomationConfirmedRevision.value !== loginAutomationRevision.value);
const loginAutomationDraftFingerprint = computed(() => JSON.stringify({
  enabled: loginAutomationEnabled.value,
  steps: loginAutomationSteps.value.map((step) => ({
    kind: step.kind,
    value: step.value,
    appendEnter: step.appendEnter,
    timeoutSeconds: step.timeoutSeconds,
    existingOrdinal: step.existingOrdinal,
    stagedSecretId: step.stagedSecretId,
    secretLabel: step.secretLabel,
    hasSecretValue: Boolean(step.secretValue),
  })),
}));
const loginAutomationDirty = computed(() => loginAutomationDraftFingerprint.value
  !== loginAutomationSavedFingerprint.value);

watch(authenticationMode, (mode, previousMode) => {
  if (previousMode === "password" && mode !== "password") {
    hostPassword.value = "";
    void cancelStagedHostPassword();
  }
  if (mode === "privateKey" && previousMode !== "privateKey") {
    identityId.value = "";
    privateKeyCredentialSummary.value = null;
  } else if (mode !== "savedIdentity" && mode !== "privateKey") {
    identityId.value = "";
  }
  if (previousMode === "privateKey" && mode !== "privateKey") {
    privateKeyCredentialSummary.value = null;
  }
});

async function cancelStagedHostPassword() {
  if (!stagedHostPasswordId.value) return true;
  try {
    await cancelHostCreatePassword({
      operationId: configuredCreateOperationId.value,
      idempotencyKey: configuredCreateIdempotencyKey.value,
    });
    stagedHostPasswordId.value = null;
    return true;
  } catch {
    saveFailed.value = true;
    return false;
  }
}

function showHostSaveFailure(error: unknown, fallback = "sshHosts.saveFailed") {
  const coreError = parseCoreApiError(error);
  const mappedKey = coreError?.code === "vault.locked"
    ? "sshHosts.errors.vaultLocked"
    : coreError?.code === "vault.requires_reload"
      ? "sshHosts.errors.vaultRequiresReload"
      : coreError?.code === "host.invalid_staged_password"
        ? "sshHosts.errors.invalidPassword"
        : coreError?.code === "ssh_metadata.conflict"
          ? "sshHosts.errors.conflict"
          : coreError?.code === "ssh_metadata.invalid_input"
            ? "sshHosts.errors.invalidInput"
            : coreError?.messageKey && te(coreError.messageKey)
              ? coreError.messageKey
              : fallback;
  saveFailureKey.value = mappedKey;
  saveFailed.value = true;
}

function clearHostVaultInputs() {
  vaultPassword.value = "";
  vaultPasswordConfirmation.value = "";
}

async function refreshHostVaultState() {
  vaultStatusLoading.value = true;
  vaultStatusUnavailable.value = false;
  vaultState.value = null;
  try {
    const status = await fetchVaultStatus();
    vaultState.value = status.state;
    return status.state;
  } catch {
    vaultStatusUnavailable.value = true;
    return null;
  } finally {
    vaultStatusLoading.value = false;
  }
}

function closeHostVaultDialog(force = false) {
  if (vaultSubmitting.value && !force) return;
  clearHostVaultInputs();
  vaultDialogMode.value = null;
  vaultDialogOpen.value = false;
}

async function openHostVaultDialog() {
  if (vaultSubmitting.value) return;
  const state = await refreshHostVaultState();
  if (state === "unlocked") return;
  if (state !== "missing" && state !== "locked" && state !== "requiresReload") {
    vaultActionFailed.value = true;
    return;
  }
  vaultDialogMode.value = state === "missing" ? "create" : "unlock";
  clearHostVaultInputs();
  vaultActionFailed.value = false;
  vaultDialogOpen.value = true;
}

async function prepareVaultForHostPassword() {
  const mode = vaultDialogMode.value;
  if (vaultSubmitting.value || !mode || !vaultDialogPasswordValid.value) {
    vaultActionFailed.value = true;
    return;
  }
  vaultSubmitting.value = true;
  vaultActionFailed.value = false;
  const submittedPassword = vaultPassword.value;
  const submittedConfirmation = vaultPasswordConfirmation.value;
  clearHostVaultInputs();
  try {
    const status = mode === "create"
      ? await createVault(submittedPassword, submittedConfirmation)
      : await unlockVault(submittedPassword);
    vaultState.value = status.state;
    if (status.state !== "unlocked") throw new Error("Vault unavailable");
    closeHostVaultDialog(true);
  } catch {
    clearHostVaultInputs();
    vaultActionFailed.value = true;
  } finally {
    vaultSubmitting.value = false;
  }
}

function setTransportHeartbeatEnabled(enabled: boolean) {
  heartbeatMode.value = enabled ? "transportKeepalive" : "disabled";
}

function resetRouteDraft() {
  routeHost.value = null;
  routeRevision.value = "1";
  routeIngressKind.value = "directTcp";
  routeProxyAddress.value = "";
  routeProxyPort.value = "8080";
  routeProxyDnsMode.value = "proxy";
  routeProxyCredentialRefId.value = "";
  routeJumpHostIds.value = [];
  routeSaveFailed.value = false;
  routeSavedFingerprint.value = JSON.stringify({ ingress: { kind: "directTcp" }, jumpHostIds: [] });
}

function applyRouteSummary(config: Awaited<ReturnType<typeof getHostConnectionConfig>>) {
  routeRevision.value = config.routePlan.revision;
  routeIngressKind.value = config.routePlan.ingress.kind;
  routeJumpHostIds.value = [...config.routePlan.jumpHostIds];
  if (config.routePlan.ingress.kind === "directTcp") {
    routeProxyAddress.value = "";
    routeProxyPort.value = "8080";
    routeProxyDnsMode.value = "proxy";
    routeProxyCredentialRefId.value = "";
  } else {
    routeProxyAddress.value = config.routePlan.ingress.endpoint.address;
    routeProxyPort.value = String(config.routePlan.ingress.endpoint.port);
    routeProxyCredentialRefId.value = config.routePlan.ingress.proxyAuthCredentialRefId ?? "";
    routeProxyDnsMode.value = config.routePlan.ingress.kind === "socks5Proxy"
      ? config.routePlan.ingress.dnsMode
      : "proxy";
  }
  routeSavedFingerprint.value = JSON.stringify({
    ingress: config.routePlan.ingress,
    jumpHostIds: config.routePlan.jumpHostIds,
  });
}

function resetLoginAutomationDraft() {
  loginAutomationHost.value = null;
  loginAutomationRevision.value = "1";
  loginAutomationConfirmedRevision.value = null;
  loginAutomationEnabled.value = false;
  loginAutomationSteps.value = [];
  loginAutomationConfirmOnSave.value = false;
  loginAutomationSaveFailed.value = false;
  loginAutomationConfirmFailed.value = false;
  loginAutomationSavedFingerprint.value = loginAutomationDraftFingerprint.value;
}

function applyLoginAutomationSummary(config: Awaited<ReturnType<typeof getHostConnectionConfig>>) {
  const summary = config.loginAutomation;
  loginAutomationRevision.value = summary.revision;
  loginAutomationConfirmedRevision.value = summary.confirmedRevision;
  loginAutomationEnabled.value = summary.enabled;
  loginAutomationSteps.value = summary.steps.map(loginAutomationDraftFromSummary);
  loginAutomationConfirmOnSave.value = summary.enabled
    && summary.confirmedRevision === summary.revision;
  loginAutomationSavedFingerprint.value = loginAutomationDraftFingerprint.value;
}

async function loadRouteCredentialOptions() {
  const credentialLists = await Promise.all(
    identities.value.map((identity) => listCredentialRefs(identity.identityId)),
  );
  routeCredentials.value = credentialLists
    .flat()
    .filter((credential) => credential.method === "password")
    .sort((left, right) => left.priority - right.priority || left.label.localeCompare(right.label));
}

async function refresh() {
  if (!canUseDesktopCore()) return;
  loading.value = true;
  loadFailed.value = false;
  try {
    [catalog.value, groups.value, tags.value, identities.value] = await Promise.all([
      listHostCatalog("favoriteThenLabel"),
      listHostGroups(),
      listHostTags(),
      listIdentities(),
    ]);
  } catch {
    loadFailed.value = true;
  } finally {
    loading.value = false;
  }
}

function openCreate() {
  configuredCreateOperationId.value = createUuidV7();
  configuredCreateIdempotencyKey.value = `host-configured-create-${configuredCreateOperationId.value}`;
  stagedHostPasswordId.value = null;
  editingHost.value = null;
  hostMarkerDraft.value = null;
  hostEditorSection.value = "connection";
  label.value = "";
  address.value = "";
  port.value = "22";
  username.value = "";
  identityId.value = "";
  authenticationMode.value = "askEveryTime";
  privateKeyCredentialSummary.value = null;
  privateKeyHostEditorBinding.value = false;
  hostPassword.value = "";
  clearHostVaultInputs();
  vaultDialogMode.value = null;
  vaultActionFailed.value = false;
  favorite.value = false;
  groupId.value = "";
  selectedTagIds.value = [];
  newGroupLabel.value = "";
  newTagLabel.value = "";
  saveFailed.value = false;
  saveFailureKey.value = "sshHosts.saveFailed";
  advancedConfigSaveFailed.value = false;
  tips.dismissScope("hosts-operation");
  resetRouteDraft();
  resetLoginAutomationDraft();
  dialogOpen.value = true;
  void loadCreateAdvancedConfig();
  void refreshHostVaultState();
}

function openEdit(host: HostSummary, initialSection: HostEditorSection = "connection") {
  const entry = catalog.value.find((candidate) => candidate.host.hostId === host.hostId);
  editingHost.value = host;
  const marker = hostMarkers.get(host.hostId);
  hostMarkerDraft.value = marker ? { ...marker } : null;
  hostEditorSection.value = initialSection;
  label.value = host.label;
  address.value = host.address;
  port.value = String(host.port);
  username.value = host.username ?? "";
  identityId.value = host.identityId ?? "";
  authenticationMode.value = host.identityId ? "savedIdentity" : "askEveryTime";
  hostPassword.value = "";
  clearHostVaultInputs();
  vaultDialogMode.value = null;
  vaultActionFailed.value = false;
  favorite.value = host.favorite;
  groupId.value = entry?.group?.groupId ?? "";
  selectedTagIds.value = entry?.tags.map((tag) => tag.tagId) ?? [];
  newGroupLabel.value = "";
  newTagLabel.value = "";
  saveFailed.value = false;
  saveFailureKey.value = "sshHosts.saveFailed";
  advancedConfigSaveFailed.value = false;
  tips.dismissScope("hosts-operation");
  resetRouteDraft();
  routeHost.value = host;
  resetLoginAutomationDraft();
  loginAutomationHost.value = host;
  dialogOpen.value = true;
  void loadSavedAdvancedConfig(host);
  void refreshHostVaultState();
}

function routeJumpHostOptions(index: number) {
  const selectedByOtherRows = new Set(
    routeJumpHostIds.value.filter((_, candidateIndex) => candidateIndex !== index),
  );
  return catalog.value
    .filter((entry) => entry.host.hostId !== routeHost.value?.hostId)
    .filter((entry) => !selectedByOtherRows.has(entry.host.hostId))
    .map((entry) => ({
      value: entry.host.hostId,
      label: `${entry.host.label} · ${entry.host.normalizedAddress}:${entry.host.port}`,
    }));
}

function openConnectionRoute(host: HostSummary) {
  openEdit(host, "connectionRoute");
}

function addRouteJumpHost() {
  if (!canAddRouteJumpHost.value) return;
  const candidate = catalog.value.find((entry) => entry.host.hostId !== routeHost.value?.hostId
    && !routeJumpHostIds.value.includes(entry.host.hostId));
  if (candidate) routeJumpHostIds.value = [...routeJumpHostIds.value, candidate.host.hostId];
}

function moveRouteJumpHost(index: number, offset: -1 | 1) {
  const destination = index + offset;
  if (destination < 0 || destination >= routeJumpHostIds.value.length) return;
  const reordered = [...routeJumpHostIds.value];
  const moving = reordered[index];
  const displaced = reordered[destination];
  if (!moving || !displaced) return;
  reordered[index] = displaced;
  reordered[destination] = moving;
  routeJumpHostIds.value = reordered;
}

function updateRouteJumpHost(index: number, hostId: string) {
  if (!hostId || index < 0 || index >= routeJumpHostIds.value.length) return;
  const next = [...routeJumpHostIds.value];
  next[index] = hostId;
  routeJumpHostIds.value = next;
}

function removeRouteJumpHost(index: number) {
  routeJumpHostIds.value = routeJumpHostIds.value.filter(
    (_, candidateIndex) => candidateIndex !== index,
  );
}

function routeDraftIngress(): RouteIngress | null {
  let ingress: RouteIngress = { kind: "directTcp" };
  if (routeIngressKind.value !== "directTcp") {
    const address = routeProxyAddress.value.trim();
    const port = Number(routeProxyPort.value);
    if (!address || !Number.isInteger(port) || port < 1 || port > 65_535) {
      return null;
    }
    const endpoint = { address, normalizedAddress: address, port };
    ingress = routeIngressKind.value === "httpConnectProxy"
      ? {
          kind: "httpConnectProxy",
          endpoint,
          proxyAuthCredentialRefId: routeProxyCredentialRefId.value || null,
        }
      : {
          kind: "socks5Proxy",
          endpoint,
          dnsMode: routeProxyDnsMode.value,
          proxyAuthCredentialRefId: routeProxyCredentialRefId.value || null,
        };
  }
  return ingress;
}

async function saveConnectionRoute(host: HostSummary) {
  const ingress = routeDraftIngress();
  if (!ingress) throw new Error("invalid connection route");
  const nextFingerprint = JSON.stringify({ ingress, jumpHostIds: routeJumpHostIds.value });
  if (nextFingerprint !== routeSavedFingerprint.value) {
    const routePlan = await replaceRoutePlan({
      hostId: host.hostId,
      expectedRevision: routeRevision.value,
      ingress,
      jumpHostIds: routeJumpHostIds.value,
    });
    routeRevision.value = routePlan.revision;
    routeSavedFingerprint.value = JSON.stringify({
      ingress: routePlan.ingress,
      jumpHostIds: routePlan.jumpHostIds,
    });
  }
}

function toggleAlgorithmException(exceptionId: string, selected: boolean) {
  algorithmSelectedExceptionIds.value = selected
    ? [...new Set([...algorithmSelectedExceptionIds.value, exceptionId])]
    : algorithmSelectedExceptionIds.value.filter((candidate) => candidate !== exceptionId);
}

function algorithmDraftExceptions() {
  const catalog = algorithmCatalog.value;
  if (!catalog || hasSelectedUnavailableAlgorithm.value) return null;
  return algorithmSelectedExceptionIds.value.flatMap((exceptionId) => {
    const entry = catalog.entries.find((candidate) => candidate.stableId === exceptionId
      && candidate.selectableException
      && candidate.available);
    if (!entry) return [];
    const previous = algorithmStoredExceptions.value.find(
      (candidate) => candidate.exceptionId === exceptionId,
    );
    return [{ category: entry.category, exceptionId, reason: previous?.reason ?? null }];
  });
}

function applyHeartbeatPolicy(policy: HeartbeatPolicy) {
  heartbeatMode.value = policy.mode;
  if (policy.mode === "transportKeepalive") {
    heartbeatIntervalSeconds.value = String(policy.intervalSeconds);
    heartbeatReplyTimeoutSeconds.value = String(policy.replyTimeoutSeconds);
    heartbeatFailureThreshold.value = String(policy.failureThreshold);
    heartbeatPayloadText.value = "";
    heartbeatLineEnding.value = "cr";
    heartbeatUserIdleSeconds.value = "30";
  } else if (policy.mode === "shellHeartbeat") {
    heartbeatIntervalSeconds.value = String(policy.intervalSeconds);
    heartbeatReplyTimeoutSeconds.value = "10";
    heartbeatFailureThreshold.value = "3";
    heartbeatPayloadText.value = policy.payloadText;
    heartbeatLineEnding.value = policy.lineEnding;
    heartbeatUserIdleSeconds.value = String(policy.userIdleSeconds);
  } else {
    heartbeatIntervalSeconds.value = "30";
    heartbeatReplyTimeoutSeconds.value = "10";
    heartbeatFailureThreshold.value = "3";
    heartbeatPayloadText.value = "";
    heartbeatLineEnding.value = "cr";
    heartbeatUserIdleSeconds.value = "30";
  }
}

function heartbeatDraftPolicy(): HeartbeatPolicy | null {
  const intervalSeconds = Number(heartbeatIntervalSeconds.value);
  if (heartbeatMode.value === "disabled") {
    return { mode: "disabled" };
  }
  if (heartbeatMode.value === "transportKeepalive") {
    const replyTimeoutSeconds = Number(heartbeatReplyTimeoutSeconds.value);
    const failureThreshold = Number(heartbeatFailureThreshold.value);
    if (
      !Number.isInteger(intervalSeconds)
      || intervalSeconds < 10
      || intervalSeconds > 3600
      || !Number.isInteger(replyTimeoutSeconds)
      || replyTimeoutSeconds < 5
      || replyTimeoutSeconds > 60
      || replyTimeoutSeconds >= intervalSeconds
      || !Number.isInteger(failureThreshold)
      || failureThreshold < 1
      || failureThreshold > 10
    ) {
      return null;
    }
    return {
      mode: "transportKeepalive",
      intervalSeconds,
      replyTimeoutSeconds,
      failureThreshold,
    };
  }
  const userIdleSeconds = Number(heartbeatUserIdleSeconds.value);
  const payload = heartbeatPayloadText.value;
  if (
    !Number.isInteger(intervalSeconds)
    || intervalSeconds < 30
    || intervalSeconds > 3600
    || !Number.isInteger(userIdleSeconds)
    || userIdleSeconds < 5
    || userIdleSeconds > 3600
    || new TextEncoder().encode(payload).length > 1024
    || [...payload].some((character) => /\p{Cc}/u.test(character))
    || (!payload && heartbeatLineEnding.value === "none")
  ) {
    return null;
  }
  return {
    mode: "shellHeartbeat",
    payloadText: payload,
    lineEnding: heartbeatLineEnding.value,
    intervalSeconds,
    userIdleSeconds,
  };
}

function monitoringDraftPolicy(): MonitoringPolicy | null {
  const sampleIntervalSeconds = Number(monitoringIntervalSeconds.value);
  const sampleTimeoutSeconds = Number(monitoringTimeoutSeconds.value);
  if (
    !Number.isInteger(sampleIntervalSeconds)
    || sampleIntervalSeconds < 5
    || sampleIntervalSeconds > 300
    || !Number.isInteger(sampleTimeoutSeconds)
    || sampleTimeoutSeconds < 2
    || sampleTimeoutSeconds > 30
    || sampleTimeoutSeconds >= sampleIntervalSeconds
  ) {
    return null;
  }
  return {
    enabled: monitoringEnabled.value,
    sampleIntervalSeconds,
    sampleTimeoutSeconds,
    diskMountIds: ["root"],
    networkInterfaceIds: ["aggregateNonLoopback"],
  };
}

function rememberAdvancedFingerprints() {
  const heartbeatPolicy = heartbeatDraftPolicy();
  const monitoringPolicy = monitoringDraftPolicy();
  algorithmSavedFingerprint.value = JSON.stringify([...algorithmSelectedExceptionIds.value].sort());
  heartbeatSavedFingerprint.value = heartbeatPolicy ? JSON.stringify(heartbeatPolicy) : "";
  monitoringSavedFingerprint.value = monitoringPolicy ? JSON.stringify(monitoringPolicy) : "";
}

function resetAdvancedConfigDraft() {
  algorithmRevision.value = "1";
  algorithmStoredExceptions.value = [];
  algorithmSelectedExceptionIds.value = [];
  heartbeatRevision.value = "1";
  applyHeartbeatPolicy({ mode: "disabled" });
  monitoringRevision.value = "1";
  monitoringEnabled.value = false;
  monitoringIntervalSeconds.value = "15";
  monitoringTimeoutSeconds.value = "5";
  monitoringRuntimePending.value = false;
}

async function loadCreateAdvancedConfig() {
  hostConfigLoading.value = true;
  hostConfigLoadFailed.value = false;
  resetAdvancedConfigDraft();
  try {
    await loadRouteCredentialOptions();
    algorithmCatalog.value = await getAlgorithmPolicyCatalog();
    rememberAdvancedFingerprints();
  } catch {
    hostConfigLoadFailed.value = true;
  } finally {
    hostConfigLoading.value = false;
  }
}

async function loadSavedAdvancedConfig(host: HostSummary) {
  hostConfigLoading.value = true;
  hostConfigLoadFailed.value = false;
  resetAdvancedConfigDraft();
  try {
    const [config, catalog] = await Promise.all([
      getHostConnectionConfig(host.hostId),
      getAlgorithmPolicyCatalog(),
      loadRouteCredentialOptions(),
    ]);
    algorithmCatalog.value = catalog;
    algorithmRevision.value = config.algorithmPolicy.revision;
    algorithmStoredExceptions.value = [...config.algorithmPolicy.compatibilityExceptions];
    algorithmSelectedExceptionIds.value = config.algorithmPolicy.compatibilityExceptions.map(
      (exception) => exception.exceptionId,
    );
    heartbeatRevision.value = config.heartbeatPolicy.revision;
    applyHeartbeatPolicy(config.heartbeatPolicy.policy);
    monitoringRevision.value = config.monitoringPolicy.revision;
    monitoringEnabled.value = config.monitoringPolicy.policy.enabled;
    monitoringIntervalSeconds.value = String(config.monitoringPolicy.policy.sampleIntervalSeconds);
    monitoringTimeoutSeconds.value = String(config.monitoringPolicy.policy.sampleTimeoutSeconds);
    applyRouteSummary(config);
    applyLoginAutomationSummary(config);
    rememberAdvancedFingerprints();
  } catch {
    hostConfigLoadFailed.value = true;
  } finally {
    hostConfigLoading.value = false;
  }
}

async function saveAdvancedConfig(host: HostSummary) {
  const catalog = algorithmCatalog.value;
  const compatibilityExceptions = algorithmDraftExceptions();
  const heartbeatPolicy = heartbeatDraftPolicy();
  const monitoringPolicy = monitoringDraftPolicy();
  if (!catalog || !compatibilityExceptions || !heartbeatPolicy || !monitoringPolicy) {
    throw new Error("invalid Host advanced configuration");
  }

  await saveConnectionRoute(host);
  await saveLoginAutomationConfig(host);

  const nextAlgorithmFingerprint = JSON.stringify([...algorithmSelectedExceptionIds.value].sort());
  if (nextAlgorithmFingerprint !== algorithmSavedFingerprint.value) {
    const saved = await replaceAlgorithmPolicy({
      hostId: host.hostId,
      expectedRevision: algorithmRevision.value,
      policyId: catalog.defaultPolicyId,
      compatibilityExceptions,
    });
    algorithmRevision.value = saved.revision;
    algorithmStoredExceptions.value = [...saved.compatibilityExceptions];
    algorithmSavedFingerprint.value = nextAlgorithmFingerprint;
  }

  const nextHeartbeatFingerprint = JSON.stringify(heartbeatPolicy);
  if (nextHeartbeatFingerprint !== heartbeatSavedFingerprint.value) {
    const saved = await replaceHeartbeatPolicy({
      hostId: host.hostId,
      expectedRevision: heartbeatRevision.value,
      policy: heartbeatPolicy,
    });
    heartbeatRevision.value = saved.revision;
    heartbeatSavedFingerprint.value = nextHeartbeatFingerprint;
  }

  const nextMonitoringFingerprint = JSON.stringify(monitoringPolicy);
  if (nextMonitoringFingerprint !== monitoringSavedFingerprint.value || monitoringRuntimePending.value) {
    const saved = await replaceMonitoringPolicy({
      hostId: host.hostId,
      expectedRevision: monitoringRevision.value,
      policy: monitoringPolicy,
    });
    monitoringRevision.value = saved.policy.revision;
    monitoringSavedFingerprint.value = nextMonitoringFingerprint;
    monitoringRuntimePending.value = !saved.runtimeReconciled;
    if (!saved.runtimeReconciled) throw new Error("monitoring runtime reconciliation pending");
  }
}

function loginAutomationDraftFromSummary(
  step: LoginAutomationStepSummary,
  existingOrdinal: number,
): LoginAutomationDraftStep {
  return {
    id: createUuidV7(),
    kind: step.kind,
    value: step.kind === "expect" ? step.literalText : step.kind === "sendText" ? step.text : "",
    appendEnter: step.kind === "expect" ? false : step.appendEnter,
    timeoutSeconds: String(step.timeoutSeconds),
    existingOrdinal: step.kind === "sendSecret" ? existingOrdinal : null,
    stagedSecretId: null,
    secretLabel: step.kind === "sendSecret" ? step.secretLabel : "",
    secretValue: "",
  };
}

function serializeCreateLoginAutomationSteps(): HostCreateLoginAutomationStep[] | null {
  const steps: HostCreateLoginAutomationStep[] = [];
  let totalTimeout = 0;
  for (const step of loginAutomationSteps.value) {
    const timeoutSeconds = Number(step.timeoutSeconds);
    if (!Number.isInteger(timeoutSeconds) || timeoutSeconds < 1 || timeoutSeconds > 60) return null;
    totalTimeout += timeoutSeconds;
    if (step.kind === "expect") {
      if (!step.value) return null;
      steps.push({ kind: "expect", literalText: step.value, timeoutSeconds });
    } else if (step.kind === "sendText") {
      if (!step.value) return null;
      steps.push({
        kind: "sendText",
        text: step.value,
        appendEnter: step.appendEnter,
        timeoutSeconds,
      });
    } else {
      return null;
    }
  }
  return totalTimeout <= 300 ? steps : null;
}

function newLoginAutomationStep(kind: LoginAutomationDraftStep["kind"] = "expect") {
  return {
    id: createUuidV7(),
    kind,
    value: "",
    appendEnter: true,
    timeoutSeconds: "10",
    existingOrdinal: null,
    stagedSecretId: null,
    secretLabel: "",
    secretValue: "",
  } satisfies LoginAutomationDraftStep;
}

function openLoginAutomation(host: HostSummary) {
  openEdit(host, "loginAutomation");
}

function addLoginAutomationStep() {
  if (loginAutomationSteps.value.length >= 32) return;
  loginAutomationSteps.value = [...loginAutomationSteps.value, newLoginAutomationStep()];
}

async function cancelStagedLoginAutomationSecret(step: LoginAutomationDraftStep) {
  if (step.existingOrdinal !== null) return true;
  try {
    await cancelLoginAutomationSecret({
      operationId: step.id,
      idempotencyKey: `login-automation-secret-${step.id}`,
    });
    step.stagedSecretId = null;
    return true;
  } catch {
    loginAutomationSaveFailed.value = true;
    return false;
  }
}

async function updateLoginAutomationStepKind(index: number, kind: string) {
  if (!(["expect", "sendText", "sendSecret"] as string[]).includes(kind)) return;
  const next = [...loginAutomationSteps.value];
  const previous = next[index];
  if (!previous) return;
  if (!await cancelStagedLoginAutomationSecret(previous)) return;
  previous.secretValue = "";
  const replacement = newLoginAutomationStep(kind as LoginAutomationDraftStep["kind"]);
  replacement.timeoutSeconds = previous.timeoutSeconds;
  next[index] = replacement;
  loginAutomationSteps.value = next;
}

function moveLoginAutomationStep(index: number, offset: -1 | 1) {
  const destination = index + offset;
  if (destination < 0 || destination >= loginAutomationSteps.value.length) return;
  const next = [...loginAutomationSteps.value];
  const moving = next[index];
  const displaced = next[destination];
  if (!moving || !displaced) return;
  next[index] = displaced;
  next[destination] = moving;
  loginAutomationSteps.value = next;
}

async function removeLoginAutomationStep(index: number) {
  const removed = loginAutomationSteps.value[index];
  if (!removed || !await cancelStagedLoginAutomationSecret(removed)) return;
  removed.secretValue = "";
  loginAutomationSteps.value = loginAutomationSteps.value.filter(
    (_, candidateIndex) => candidateIndex !== index,
  );
}

async function requestCloseHostEditor() {
  if (saving.value || testingConnection.value) return;
  if (!await cancelStagedHostPassword()) return;
  loginAutomationSaveFailed.value = false;
  for (const step of loginAutomationSteps.value) {
    if (!await cancelStagedLoginAutomationSecret(step)) return;
  }
  for (const step of loginAutomationSteps.value) step.secretValue = "";
  hostPassword.value = "";
  clearHostVaultInputs();
  dialogOpen.value = false;
}

onBeforeUnmount(() => {
  hostPassword.value = "";
  clearHostVaultInputs();
  if (stagedHostPasswordId.value) void cancelStagedHostPassword();
});

function updateHostDialogOpen(open: boolean) {
  if (open) {
    dialogOpen.value = true;
    return;
  }
  void requestCloseHostEditor();
}

async function serializeLoginAutomationSteps(): Promise<LoginAutomationStepInput[] | null> {
  const serialized: LoginAutomationStepInput[] = [];
  let totalTimeout = 0;
  for (const step of loginAutomationSteps.value) {
    const timeoutSeconds = Number(step.timeoutSeconds);
    if (!Number.isInteger(timeoutSeconds) || timeoutSeconds < 1 || timeoutSeconds > 60) return null;
    totalTimeout += timeoutSeconds;
    if (step.kind === "expect") {
      if (!step.value) return null;
      serialized.push({ kind: "expect", literalText: step.value, timeoutSeconds });
    } else if (step.kind === "sendText") {
      if (!step.value) return null;
      serialized.push({
        kind: "sendText",
        text: step.value,
        appendEnter: step.appendEnter,
        timeoutSeconds,
      });
    } else if (step.existingOrdinal !== null) {
      serialized.push({
        kind: "preserveExistingSecret",
        existingOrdinal: step.existingOrdinal,
        appendEnter: step.appendEnter,
        timeoutSeconds,
      });
    } else {
      if (!step.secretLabel.trim() || (!step.stagedSecretId && !step.secretValue)) return null;
      if (!step.stagedSecretId) {
        let created: Awaited<ReturnType<typeof createLoginAutomationSecret>>;
        try {
          created = await createLoginAutomationSecret({
            operationId: step.id,
            idempotencyKey: `login-automation-secret-${step.id}`,
            hostId: loginAutomationHost.value!.hostId,
            expectedAutomationRevision: loginAutomationRevision.value,
            label: step.secretLabel.trim(),
            value: step.secretValue,
          });
        } catch (error) {
          if (await cancelStagedLoginAutomationSecret(step)) step.id = createUuidV7();
          throw error;
        }
        step.stagedSecretId = created.stagedSecretId;
        step.secretLabel = created.label;
        step.secretValue = "";
      }
      serialized.push({
        kind: "sendSecret",
        stagedSecretId: step.stagedSecretId,
        secretLabel: step.secretLabel.trim(),
        appendEnter: step.appendEnter,
        timeoutSeconds,
      });
    }
  }
  return totalTimeout <= 300 ? serialized : null;
}

async function saveLoginAutomationConfig(host: HostSummary) {
  loginAutomationHost.value = host;
  if (loginAutomationEnabled.value && loginAutomationSteps.value.length === 0) {
    throw new Error("enabled login automation requires at least one step");
  }
  loginAutomationSaveFailed.value = false;
  loginAutomationConfirmFailed.value = false;
  if (loginAutomationDirty.value) {
    const steps = await serializeLoginAutomationSteps();
    if (!steps) throw new Error("invalid login automation");
    const summary = await replaceLoginAutomation({
      hostId: host.hostId,
      expectedRevision: loginAutomationRevision.value,
      enabled: loginAutomationEnabled.value,
      steps,
    });
    loginAutomationRevision.value = summary.revision;
    loginAutomationConfirmedRevision.value = summary.confirmedRevision;
    loginAutomationSteps.value = summary.steps.map(loginAutomationDraftFromSummary);
    loginAutomationSavedFingerprint.value = loginAutomationDraftFingerprint.value;
  }

  if (
    loginAutomationEnabled.value
    && loginAutomationConfirmOnSave.value
    && loginAutomationConfirmedRevision.value !== loginAutomationRevision.value
  ) {
    const summary = await confirmLoginAutomation({
      hostId: host.hostId,
      expectedRevision: loginAutomationRevision.value,
    });
    loginAutomationConfirmedRevision.value = summary.confirmedRevision;
  }
}

function requestDelete(host: HostSummary) {
  deleteCandidate.value = host;
  deleteFailed.value = false;
  tips.dismissScope("hosts-operation");
}

function cancelDelete() {
  if (deleting.value) return;
  deleteCandidate.value = null;
  deleteFailed.value = false;
}

async function confirmDelete() {
  const host = deleteCandidate.value;
  if (!host || deleting.value) return;
  deleting.value = true;
  deleteFailed.value = false;
  try {
    await deleteHost(host.hostId, host.stateVersion);
    const markerRemoved = hostMarkers.remove(host.hostId) === "saved";
    catalog.value = catalog.value.filter((candidate) => candidate.host.hostId !== host.hostId);
    deleteCandidate.value = null;
    tips.show({
      scope: "hosts-operation",
      tone: markerRemoved ? "success" : "warning",
      title: t(markerRemoved ? "sshHosts.deleted" : "hostMarkers.deleteFailed"),
    });
  } catch {
    deleteFailed.value = true;
  } finally {
    deleting.value = false;
  }
}

async function saveHost() {
  if (saving.value) return;
  if (hostMarkerDraft.value && !normalizeHostMarker(hostMarkerDraft.value)) {
    hostEditorSection.value = "classification";
    showHostSaveFailure(null, "hostMarkers.invalid");
    return;
  }
  const parsedPort = Number(port.value);
  if (!address.value.trim() || !Number.isInteger(parsedPort) || parsedPort < 1 || parsedPort > 65535) {
    showHostSaveFailure(null, "sshHosts.errors.invalidInput");
    return;
  }
  const compatibilityExceptions = algorithmDraftExceptions();
  const heartbeatPolicy = heartbeatDraftPolicy();
  const monitoringPolicy = monitoringDraftPolicy();
  const ingress = routeDraftIngress();
  const createLoginAutomationSteps = editingHost.value
    ? null
    : serializeCreateLoginAutomationSteps();
  if (
    hostConfigLoading.value
    || hostConfigLoadFailed.value
    || !algorithmCatalog.value
    || !compatibilityExceptions
    || !heartbeatPolicy
    || !monitoringPolicy
    || !ingress
    || (!editingHost.value && !createLoginAutomationSteps)
    || (!editingHost.value
      && loginAutomationEnabled.value
      && createLoginAutomationSteps?.length === 0)
    || ((authenticationMode.value === "savedIdentity"
      || authenticationMode.value === "privateKey") && !identityId.value)
    || (!editingHost.value
      && authenticationMode.value === "privateKey"
      && (privateKeyCredentialSummary.value?.identityId !== identityId.value
        || privateKeyCredentialSummary.value.details.kind !== "privateKey"))
    || (!editingHost.value
      && authenticationMode.value === "password"
      && vaultState.value !== "unlocked")
    || (!editingHost.value
      && authenticationMode.value === "password"
      && !hostPassword.value
      && !stagedHostPasswordId.value)
  ) {
    advancedConfigSaveFailed.value = true;
    return;
  }
  saving.value = true;
  saveFailed.value = false;
  saveFailureKey.value = "sshHosts.saveFailed";
  advancedConfigSaveFailed.value = false;
  let persistedHost: HostSummary | null = null;
  try {
    const current = editingHost.value;
    if (!current && authenticationMode.value !== "password" && stagedHostPasswordId.value) {
      if (!await cancelStagedHostPassword()) throw new Error("staged Host password cancel failed");
    }

    let workingHost: HostSummary;
    if (current) {
      workingHost = await updateHost({
        hostId: current.hostId,
        expectedStateVersion: current.stateVersion,
        label: label.value,
        address: address.value,
        port: parsedPort,
        username: username.value || null,
        identityId: identityId.value || null,
        favorite: favorite.value,
      });
    } else {
      if (!createLoginAutomationSteps) throw new Error("invalid create login automation");
      if (authenticationMode.value === "password" && !stagedHostPasswordId.value) {
        const staged = await stageHostCreatePassword({
          operationId: configuredCreateOperationId.value,
          idempotencyKey: configuredCreateIdempotencyKey.value,
          identityLabel: `${label.value.trim() || address.value.trim()} · ${t("sshHosts.savedPasswordIdentityLabel")}`,
          credentialLabel: t("sshHosts.savedPasswordCredentialLabel"),
          password: hostPassword.value,
        });
        stagedHostPasswordId.value = staged.stagedPasswordId;
        hostPassword.value = "";
      }
      const created = await createConfiguredHost({
        operationId: configuredCreateOperationId.value,
        idempotencyKey: configuredCreateIdempotencyKey.value,
        label: label.value,
        address: address.value,
        port: parsedPort,
        username: username.value || null,
        identityId: authenticationMode.value === "savedIdentity"
          || authenticationMode.value === "privateKey"
          ? identityId.value
          : null,
        favorite: favorite.value,
        groupId: groupId.value || null,
        tagIds: [...selectedTagIds.value].sort(),
        ingress,
        jumpHostIds: routeJumpHostIds.value,
        authenticationMode: "identity",
        credentialRefIds: [],
        algorithmPolicyId: algorithmCatalog.value.defaultPolicyId,
        compatibilityExceptions,
        heartbeatPolicy,
        monitoringPolicy,
        loginAutomationEnabled: loginAutomationEnabled.value,
        loginAutomationConfirmed: loginAutomationEnabled.value
          && loginAutomationConfirmOnSave.value,
        loginAutomationSteps: createLoginAutomationSteps,
        stagedPasswordId: stagedHostPasswordId.value,
      });
      stagedHostPasswordId.value = null;
      workingHost = created.host;
    }
    persistedHost = workingHost;
    const currentEntry = current
      ? catalog.value.find((entry) => entry.host.hostId === current.hostId)
      : undefined;
    const nextTagIds = [...selectedTagIds.value].sort();
    const currentTagIds = (currentEntry?.tags.map((tag) => tag.tagId) ?? []).sort();
    if (current && (
      groupId.value !== (currentEntry?.group?.groupId ?? "")
      || nextTagIds.join("\0") !== currentTagIds.join("\0")
    )) {
      const organization = await replaceHostOrganization({
        hostId: workingHost.hostId,
        expectedHostStateVersion: workingHost.stateVersion,
        groupId: groupId.value || null,
        tagIds: nextTagIds,
      });
      workingHost = { ...workingHost, stateVersion: organization.hostStateVersion };
      persistedHost = workingHost;
    }
    if (current) await saveAdvancedConfig(workingHost);
    if (hostMarkers.set(workingHost.hostId, hostMarkerDraft.value) !== "saved") {
      await refresh();
      editingHost.value = catalog.value.find((entry) => entry.host.hostId === workingHost.hostId)?.host ?? workingHost;
      // The Host is already committed. Restore its configuration revision so retry neither creates it again nor uses stale CAS.
      await loadSavedAdvancedConfig(editingHost.value);
      hostEditorSection.value = "classification";
      saveFailed.value = true;
      saveFailureKey.value = "hostMarkers.saveFailed";
      return;
    }
    await refresh();
    dialogOpen.value = false;
    tips.show({
      scope: "hosts-operation",
      tone: "success",
      title: t("sshHosts.saved"),
    });
  } catch (error) {
    if (persistedHost) {
      advancedConfigSaveFailed.value = true;
      await refresh();
      editingHost.value = catalog.value.find(
        (entry) => entry.host.hostId === persistedHost?.hostId,
      )?.host ?? persistedHost;
    } else {
      showHostSaveFailure(error);
    }
  } finally {
    saving.value = false;
  }
}

function openAgentManager() {
  agentIdentityId.value = identities.value[0]?.identityId ?? "";
  newIdentityLabel.value = "";
  newIdentityUsername.value = "";
  agentKeys.value = [];
  selectedAgentKeyHandle.value = "";
  agentCredentialLabel.value = "";
  agentCredentialPriority.value = "100";
  agentActionFailed.value = false;
  agentCredentialSaved.value = false;
  agentDialogOpen.value = true;
}

function openPrivateKeyImport(selectedIdentityId = "", bindToHostEditor = false) {
  privateKeyIdentityId.value = selectedIdentityId || (identities.value[0]?.identityId ?? "");
  privateKeyHostEditorBinding.value = bindToHostEditor;
  newPrivateKeyIdentityLabel.value = "";
  newPrivateKeyIdentityUsername.value = "";
  privateKeyCredentialLabel.value = t("sshHosts.privateKeyImport.defaultCredentialLabel");
  privateKeyCredentialPriority.value = "100";
  privateKeyPassphrase.value = "";
  privateKeyActionFailed.value = false;
  privateKeyCredentialSaved.value = false;
  const keepsCurrentHostBinding = bindToHostEditor
    && !editingHost.value
    && authenticationMode.value === "privateKey"
    && privateKeyCredentialSummary.value?.identityId === selectedIdentityId
    && privateKeyCredentialSummary.value.details.kind === "privateKey";
  if (!keepsCurrentHostBinding) privateKeyCredentialSummary.value = null;
  privateKeyImportOperationId.value = createUuidV7();
  privateKeyImportIdempotencyKey.value = `private-key-file-import-${privateKeyImportOperationId.value}`;
  void suggestPrivateKeyPriority();
  void refreshHostVaultState();
  privateKeyDialogOpen.value = true;
}

function updatePrivateKeyDialogOpen(open: boolean) {
  if (importingPrivateKey.value) return;
  privateKeyDialogOpen.value = open;
  if (!open) privateKeyPassphrase.value = "";
}

async function suggestPrivateKeyPriority() {
  const selectedIdentityId = privateKeyIdentityId.value;
  if (!selectedIdentityId) {
    privateKeyCredentialPriority.value = "100";
    return;
  }
  try {
    const usedPriorities = new Set(
      (await listCredentialRefs(selectedIdentityId)).map((credential) => credential.priority),
    );
    let priority = 100;
    while (usedPriorities.has(priority) && priority < 65_535) priority += 1;
    if (privateKeyIdentityId.value === selectedIdentityId) {
      privateKeyCredentialPriority.value = String(priority);
    }
  } catch {
    // The explicit import still validates priority in Core; preserve the default
    // when the existing credential list cannot be read.
  }
}

async function addPrivateKeyIdentity() {
  const nextLabel = newPrivateKeyIdentityLabel.value.trim();
  if (!nextLabel || creatingPrivateKeyIdentity.value) return;
  creatingPrivateKeyIdentity.value = true;
  privateKeyActionFailed.value = false;
  try {
    const identity = await createIdentity(
      nextLabel,
      newPrivateKeyIdentityUsername.value.trim() || null,
    );
    identities.value = [...identities.value, identity]
      .sort((left, right) => left.label.localeCompare(right.label));
    privateKeyIdentityId.value = identity.identityId;
    privateKeyCredentialPriority.value = "100";
    newPrivateKeyIdentityLabel.value = "";
    newPrivateKeyIdentityUsername.value = "";
  } catch {
    privateKeyActionFailed.value = true;
  } finally {
    creatingPrivateKeyIdentity.value = false;
  }
}

async function importSelectedPrivateKeyFile() {
  const priority = Number(privateKeyCredentialPriority.value);
  if (!privateKeyIdentityId.value
    || !privateKeyCredentialLabel.value.trim()
    || !Number.isInteger(priority)
    || priority < 0
    || priority > 65_535
    || importingPrivateKey.value
    || vaultState.value !== "unlocked") {
    privateKeyActionFailed.value = true;
    return;
  }
  importingPrivateKey.value = true;
  privateKeyActionFailed.value = false;
  privateKeyCredentialSaved.value = false;
  try {
    const imported = await importPrivateKeyFile({
      operationId: privateKeyImportOperationId.value ?? undefined,
      idempotencyKey: privateKeyImportIdempotencyKey.value ?? undefined,
      identityId: privateKeyIdentityId.value,
      passphrase: privateKeyPassphrase.value || null,
      priority,
      label: privateKeyCredentialLabel.value.trim(),
    });
    privateKeyPassphrase.value = "";
    if (!imported) return;
    privateKeyCredentialSaved.value = true;
    privateKeyCredentialSummary.value = imported;
    if (privateKeyHostEditorBinding.value) {
      identityId.value = privateKeyIdentityId.value;
      authenticationMode.value = editingHost.value ? "savedIdentity" : "privateKey";
    }
    privateKeyImportOperationId.value = null;
    privateKeyImportIdempotencyKey.value = null;
    await refresh();
  } catch {
    privateKeyPassphrase.value = "";
    privateKeyActionFailed.value = true;
  } finally {
    importingPrivateKey.value = false;
  }
}

watch(privateKeyIdentityId, () => {
  if (!importingPrivateKey.value) void suggestPrivateKeyPriority();
});

async function addAgentIdentity() {
  const nextLabel = newIdentityLabel.value.trim();
  if (!nextLabel || creatingIdentity.value) return;
  creatingIdentity.value = true;
  agentActionFailed.value = false;
  try {
    const identity = await createIdentity(nextLabel, newIdentityUsername.value.trim() || null);
    identities.value = [...identities.value, identity]
      .sort((left, right) => left.label.localeCompare(right.label));
    agentIdentityId.value = identity.identityId;
    newIdentityLabel.value = "";
    newIdentityUsername.value = "";
  } catch {
    agentActionFailed.value = true;
  } finally {
    creatingIdentity.value = false;
  }
}

async function refreshAgentKeys() {
  if (loadingAgentKeys.value) return;
  loadingAgentKeys.value = true;
  agentActionFailed.value = false;
  agentCredentialSaved.value = false;
  selectedAgentKeyHandle.value = "";
  try {
    agentKeys.value = await listSshAgentKeys();
  } catch {
    agentKeys.value = [];
    agentActionFailed.value = true;
  } finally {
    loadingAgentKeys.value = false;
  }
}

function selectAgentKey(key: SshAgentKeySummary) {
  selectedAgentKeyHandle.value = key.keyHandle;
  if (!agentCredentialLabel.value.trim()) {
    agentCredentialLabel.value = t("sshHosts.sshAgent.defaultCredentialLabel", {
      algorithm: key.publicKeyAlgorithm,
    });
  }
}

function agentIdentityKindLabel(key: SshAgentKeySummary) {
  return t(`sshHosts.sshAgent.identityKinds.${key.identityKind}`);
}

function certificatePrincipalSummary(key: SshAgentKeySummary) {
  const principals = key.certificate?.validPrincipals ?? [];
  return principals.length
    ? principals.join(", ")
    : t("sshHosts.sshAgent.anyPrincipal");
}

async function saveAgentCredential() {
  const priority = Number(agentCredentialPriority.value);
  const selectedKey = agentKeys.value.find(
    (key) => key.keyHandle === selectedAgentKeyHandle.value,
  );
  if (!agentIdentityId.value
    || !selectedKey
    || !agentCredentialLabel.value.trim()
    || !Number.isInteger(priority)
    || priority < 0
    || priority > 65_535
    || savingAgentCredential.value) {
    agentActionFailed.value = true;
    return;
  }
  savingAgentCredential.value = true;
  agentActionFailed.value = false;
  agentCredentialSaved.value = false;
  try {
    await createSshAgentCredential({
      identityId: agentIdentityId.value,
      keyHandle: selectedAgentKeyHandle.value,
      expectedIdentityKind: selectedKey.identityKind,
      priority,
      label: agentCredentialLabel.value.trim(),
    });
    agentCredentialSaved.value = true;
    agentKeys.value = [];
    selectedAgentKeyHandle.value = "";
    await refresh();
  } catch {
    agentActionFailed.value = true;
  } finally {
    savingAgentCredential.value = false;
  }
}

function openKeyboardInteractiveManager() {
  keyboardInteractiveIdentityId.value = identities.value[0]?.identityId ?? "";
  keyboardInteractiveLabel.value = t("sshHosts.keyboardInteractive.defaultCredentialLabel");
  keyboardInteractivePriority.value = "110";
  keyboardInteractiveMaxRounds.value = "8";
  keyboardInteractiveFailed.value = false;
  keyboardInteractiveSaved.value = false;
  keyboardInteractiveDialogOpen.value = true;
}

async function saveKeyboardInteractiveCredential() {
  const priority = Number(keyboardInteractivePriority.value);
  const maxRounds = Number(keyboardInteractiveMaxRounds.value);
  if (!keyboardInteractiveIdentityId.value
    || !keyboardInteractiveLabel.value.trim()
    || !Number.isInteger(priority)
    || priority < 0
    || priority > 65_535
    || !Number.isInteger(maxRounds)
    || maxRounds < 1
    || maxRounds > 32
    || savingKeyboardInteractive.value) {
    keyboardInteractiveFailed.value = true;
    return;
  }
  savingKeyboardInteractive.value = true;
  keyboardInteractiveFailed.value = false;
  keyboardInteractiveSaved.value = false;
  try {
    await createKeyboardInteractiveCredential({
      identityId: keyboardInteractiveIdentityId.value,
      maxRounds,
      priority,
      label: keyboardInteractiveLabel.value.trim(),
    });
    keyboardInteractiveSaved.value = true;
    await refresh();
  } catch {
    keyboardInteractiveFailed.value = true;
  } finally {
    savingKeyboardInteractive.value = false;
  }
}

function openOpenSshImport() {
  opensshConfigText.value = "";
  opensshPreview.value = null;
  selectedOpenSshCandidateIds.value = [];
  opensshActionFailed.value = false;
  opensshImportedCount.value = 0;
  opensshDialogOpen.value = true;
}

async function previewOpenSshImport() {
  if (!opensshConfigText.value.trim() || previewingOpenSsh.value) return;
  previewingOpenSsh.value = true;
  opensshActionFailed.value = false;
  opensshImportedCount.value = 0;
  try {
    const preview = await previewOpenSshConfig(opensshConfigText.value);
    opensshPreview.value = preview;
    selectedOpenSshCandidateIds.value = preview.candidates
      .filter((candidate) => candidate.importable)
      .map((candidate) => candidate.candidateId);
  } catch {
    opensshPreview.value = null;
    selectedOpenSshCandidateIds.value = [];
    opensshActionFailed.value = true;
  } finally {
    previewingOpenSsh.value = false;
  }
}

function toggleOpenSshCandidate(candidateId: string, selected: boolean) {
  selectedOpenSshCandidateIds.value = selected
    ? [...new Set([...selectedOpenSshCandidateIds.value, candidateId])]
    : selectedOpenSshCandidateIds.value.filter((value) => value !== candidateId);
}

function formatOpenSshEndpoint(address: string, port: number) {
  return `${address.includes(":") ? `[${address}]` : address}:${port}`;
}

function formatOpenSshRoute(route: OpenSshImportRoutePreview) {
  switch (route.kind) {
    case "direct":
      return t("sshHosts.opensshImport.routeDirect");
    case "httpConnect":
      return t("sshHosts.opensshImport.routeHttp", {
        endpoint: formatOpenSshEndpoint(route.proxy.address, route.proxy.port),
      });
    case "socks5":
      return t("sshHosts.opensshImport.routeSocks", {
        endpoint: formatOpenSshEndpoint(route.proxy.address, route.proxy.port),
      });
    case "jumpChain":
      return t("sshHosts.opensshImport.routeJump", {
        hops: route.hops
          .map((hop) => `${hop.username ? `${hop.username}@` : ""}${formatOpenSshEndpoint(
            hop.endpoint.address,
            hop.endpoint.port,
          )}`)
          .join(" → "),
      });
  }
}

async function importSelectedOpenSshHosts() {
  const preview = opensshPreview.value;
  if (!preview || !selectedOpenSshCandidateIds.value.length || importingOpenSsh.value) return;
  importingOpenSsh.value = true;
  opensshActionFailed.value = false;
  try {
    const response = await commitOpenSshConfig(
      preview.snapshotId,
      selectedOpenSshCandidateIds.value,
    );
    opensshImportedCount.value = response.hosts.length;
    selectedOpenSshCandidateIds.value = [];
    await refresh();
  } catch {
    opensshActionFailed.value = true;
  } finally {
    importingOpenSsh.value = false;
  }
}

function toggleTag(tagId: string, selected: boolean) {
  selectedTagIds.value = selected
    ? [...new Set([...selectedTagIds.value, tagId])]
    : selectedTagIds.value.filter((candidate) => candidate !== tagId);
}

async function addGroup() {
  const nextLabel = newGroupLabel.value.trim();
  if (!nextLabel || creatingGroup.value) return;
  creatingGroup.value = true;
  try {
    const group = await createHostGroup(nextLabel);
    groups.value = [...groups.value, group].sort((left, right) => left.label.localeCompare(right.label));
    groupId.value = group.groupId;
    newGroupLabel.value = "";
  } catch {
    saveFailed.value = true;
  } finally {
    creatingGroup.value = false;
  }
}

async function addTag() {
  const nextLabel = newTagLabel.value.trim();
  if (!nextLabel || creatingTag.value) return;
  creatingTag.value = true;
  try {
    const tag = await createHostTag(nextLabel);
    tags.value = [...tags.value, tag].sort((left, right) => left.label.localeCompare(right.label));
    toggleTag(tag.tagId, true);
    newTagLabel.value = "";
  } catch {
    saveFailed.value = true;
  } finally {
    creatingTag.value = false;
  }
}

async function toggleFavorite(host: HostSummary) {
  if (favoriteUpdatingHostId.value) return;
  favoriteUpdatingHostId.value = host.hostId;
  try {
    const updated = await updateHostFavorite({
      hostId: host.hostId,
      expectedStateVersion: host.stateVersion,
      favorite: !host.favorite,
    });
    catalog.value = catalog.value
      .map((entry) => entry.host.hostId === updated.hostId ? { ...entry, host: updated } : entry)
      .sort((left, right) => Number(right.host.favorite) - Number(left.host.favorite)
        || left.host.label.localeCompare(right.host.label));
  } catch {
    loadFailed.value = true;
  } finally {
    favoriteUpdatingHostId.value = null;
  }
}

function openClassificationManager() {
  classificationEdit.value = null;
  classificationEditLabel.value = "";
  classificationFailed.value = false;
  classificationNewGroupLabel.value = "";
  classificationDialogOpen.value = true;
}

async function addClassificationGroup() {
  const nextLabel = classificationNewGroupLabel.value.trim();
  if (!nextLabel || classificationCreatingGroup.value) return;
  classificationCreatingGroup.value = true;
  classificationFailed.value = false;
  try {
    const group = await createHostGroup(nextLabel);
    groups.value = [...groups.value, group].sort((left, right) => left.label.localeCompare(right.label));
    classificationNewGroupLabel.value = "";
  } catch {
    classificationFailed.value = true;
  } finally {
    classificationCreatingGroup.value = false;
  }
}

function editClassification(
  kind: "group" | "tag",
  item: HostGroupSummary | HostTagSummary,
) {
  classificationEdit.value = {
    kind,
    id: kind === "group"
      ? (item as HostGroupSummary).groupId
      : (item as HostTagSummary).tagId,
    stateVersion: item.stateVersion,
  };
  classificationEditLabel.value = item.label;
  classificationFailed.value = false;
}

async function saveClassification() {
  const edit = classificationEdit.value;
  const nextLabel = classificationEditLabel.value.trim();
  if (!edit || !nextLabel || classificationSaving.value) return;
  classificationSaving.value = true;
  classificationFailed.value = false;
  try {
    if (edit.kind === "group") {
      const updated = await updateHostGroup({
        groupId: edit.id,
        expectedStateVersion: edit.stateVersion,
        label: nextLabel,
      });
      groups.value = groups.value.map((item) => item.groupId === updated.groupId ? updated : item);
    } else {
      const updated = await updateHostTag({
        tagId: edit.id,
        expectedStateVersion: edit.stateVersion,
        label: nextLabel,
      });
      tags.value = tags.value.map((item) => item.tagId === updated.tagId ? updated : item);
    }
    classificationEdit.value = null;
    await refresh();
  } catch {
    classificationFailed.value = true;
  } finally {
    classificationSaving.value = false;
  }
}

function requestClassificationDelete(
  kind: "group" | "tag",
  item: HostGroupSummary | HostTagSummary,
) {
  classificationDeleteCandidate.value = {
    kind,
    id: kind === "group"
      ? (item as HostGroupSummary).groupId
      : (item as HostTagSummary).tagId,
    label: item.label,
    stateVersion: item.stateVersion,
  };
  classificationFailed.value = false;
}

async function confirmClassificationDelete() {
  const candidate = classificationDeleteCandidate.value;
  if (!candidate || classificationDeleting.value) return;
  classificationDeleting.value = true;
  classificationFailed.value = false;
  try {
    if (candidate.kind === "group") {
      await deleteHostGroup(candidate.id, candidate.stateVersion);
      groups.value = groups.value.filter((item) => item.groupId !== candidate.id);
    } else {
      await deleteHostTag(candidate.id, candidate.stateVersion);
      tags.value = tags.value.filter((item) => item.tagId !== candidate.id);
    }
    classificationDeleteCandidate.value = null;
  } catch {
    classificationDeleteCandidate.value = null;
    classificationFailed.value = true;
  } finally {
    classificationDeleting.value = false;
  }
}

function connect(host: HostSummary) {
  void router.push({ path: "/terminal", query: { hostId: host.hostId } });
}

async function testConnection() {
  if (saving.value || testingConnection.value) return;
  const parsedPort = Number(port.value);
  if (
    authenticationMode.value !== "password"
    || !address.value.trim()
    || !username.value.trim()
    || !hostPassword.value
    || !Number.isInteger(parsedPort)
    || parsedPort < 1
    || parsedPort > 65535
  ) {
    showHostSaveFailure(null, "sshHosts.testConnectionInputRequired");
    return;
  }

  testingConnection.value = true;
  saveFailed.value = false;
  try {
    const credential = await prepareTransientCredential({
      kind: "password",
      secret: hostPassword.value,
    });
    await testSshConnection({
      endpoint: { address: address.value.trim(), port: parsedPort, username: username.value.trim() },
      credentialRefId: credential.credentialRefId,
    });
    tips.show({
      scope: "hosts-connection-test",
      tone: "success",
      title: t("sshHosts.testConnectionSucceeded"),
    });
  } catch (error) {
    tips.show({
      scope: "hosts-connection-test",
      tone: "error",
      title: t("sshHosts.testConnectionFailed"),
      message: connectionTestFailureMessage(error),
    });
  } finally {
    testingConnection.value = false;
  }
}

function connectionTestFailureMessage(error: unknown) {
  const coreError = parseCoreApiError(error);
  if (coreError?.messageKey && te(coreError.messageKey)) {
    return t(coreError.messageKey);
  }
  return t("sshHosts.testConnectionFailureFallback");
}

onMounted(async () => {
  await refresh();
  if (route.query.create === "1") openCreate();
  if (route.query.importSshConfig === "1") openOpenSshImport();
});
</script>

<template>
  <section class="hosts-page">
    <NvxPageHeader
      :breadcrumb="t('sshHosts.breadcrumb')"
      :title="t('sshHosts.title')"
      :description="t('sshHosts.description')"
    >
      <template #actions>
        <div class="hosts-page__primary-actions">
          <NvxPluginExtensionTarget
            target-id="hosts.toolbar"
            instance-key="global"
          />
          <NvxButton
            class="hosts-page__primary-action"
            variant="secondary"
            size="sm"
            :disabled="!canUseDesktopCore()"
            @click="openOpenSshImport"
          >
            <NvxIcon
              :icon="FileInput"
              :size="16"
            />
            {{ t("sshHosts.opensshImport.manage") }}
          </NvxButton>
          <NvxButton
            class="hosts-page__primary-action"
            variant="secondary"
            size="sm"
            :disabled="!canUseDesktopCore()"
            @click="openAgentManager"
          >
            <NvxIcon
              :icon="KeyRound"
              :size="16"
            />
            {{ t("sshHosts.sshAgent.manage") }}
          </NvxButton>
          <NvxButton
            class="hosts-page__primary-action"
            variant="secondary"
            size="sm"
            :disabled="!canUseDesktopCore()"
            @click="openKeyboardInteractiveManager"
          >
            <NvxIcon
              :icon="KeyRound"
              :size="16"
            />
            {{ t("sshHosts.keyboardInteractive.manage") }}
          </NvxButton>
          <NvxButton
            class="hosts-page__primary-action"
            variant="secondary"
            size="sm"
            :disabled="!canUseDesktopCore()"
            @click="openClassificationManager"
          >
            <NvxIcon
              :icon="Tag"
              :size="16"
            />
            {{ t("sshHosts.manageClassification") }}
          </NvxButton>
          <NvxButton
            class="hosts-page__primary-action"
            size="sm"
            :disabled="!canUseDesktopCore()"
            @click="openCreate"
          >
            <NvxIcon
              :icon="Plus"
              :size="16"
            />
            {{ t("sshHosts.add") }}
          </NvxButton>
        </div>
      </template>
    </NvxPageHeader>

    <NvxInlineNotice
      v-if="!canUseDesktopCore()"
      :title="t('sshHosts.desktopOnlyTitle')"
    >
      {{ t("sshHosts.desktopOnlyBody") }}
    </NvxInlineNotice>
    <NvxInlineNotice
      v-else-if="loadFailed"
      tone="error"
      :title="t('sshHosts.loadFailed')"
    />
    <div
      v-if="catalog.length"
      class="hosts-list"
      :aria-busy="loading"
    >
      <section
        v-for="section in hostSections"
        :key="section.key"
        class="hosts-list__section"
        :aria-labelledby="`hosts-list-group-${section.key}`"
      >
        <header class="hosts-list__section-header">
          <h2 :id="`hosts-list-group-${section.key}`">
            {{ t("sshHosts.groupSection", { label: section.label, count: section.entries.length }) }}
          </h2>
        </header>
        <article
          v-for="entry in section.entries"
          :key="entry.host.hostId"
          class="hosts-list__row"
        >
          <span
            class="hosts-list__icon"
            aria-hidden="true"
          >
            <NvxIcon
              :icon="Server"
              :size="20"
            />
          </span>
          <div class="hosts-list__identity">
            <div class="hosts-list__title-row">
              <strong :title="entry.host.label">{{ entry.host.label }}</strong>
              <NvxHostMarker :marker="hostMarkers.visibleMarker(entry.host.hostId)" />
              <span
                v-if="entry.group || entry.tags.length"
                class="hosts-list__classification"
              >
                <NvxStatusLabel
                  v-if="entry.group"
                  tone="neutral"
                >
                  {{ entry.group.label }}
                </NvxStatusLabel>
                <NvxStatusLabel
                  v-for="tagItem in entry.tags"
                  :key="tagItem.tagId"
                  tone="neutral"
                >
                  {{ tagItem.label }}
                </NvxStatusLabel>
              </span>
            </div>
            <span class="hosts-list__endpoint">{{ entry.host.username || t("sshHosts.noUsername") }} · {{ entry.host.normalizedAddress }}:{{ entry.host.port }}</span>
          </div>
          <NvxStatusLabel tone="neutral">
            {{ t("sshHosts.endpoint") }}
          </NvxStatusLabel>
          <div class="hosts-list__actions">
            <NvxIconButton
              class="hosts-list__favorite"
              :label="t(entry.host.favorite ? 'sshHosts.removeFavorite' : 'sshHosts.addFavorite')"
              :disabled="favoriteUpdatingHostId !== null"
              :aria-pressed="entry.host.favorite"
              @click="toggleFavorite(entry.host)"
            >
              <NvxIcon
                :icon="Star"
                :size="16"
                :class="{ 'hosts-list__favorite-icon--filled': entry.host.favorite }"
              />
            </NvxIconButton>
            <NvxButton
              variant="ghost"
              size="sm"
              @click="openConnectionRoute(entry.host)"
            >
              <NvxIcon
                :icon="Network"
                :size="16"
              />
              {{ t("sshHosts.connectionRoute.manage") }}
            </NvxButton>
            <NvxButton
              variant="ghost"
              size="sm"
              @click="openLoginAutomation(entry.host)"
            >
              <NvxIcon
                :icon="Workflow"
                :size="16"
              />
              {{ t("sshHosts.loginAutomation.manage") }}
            </NvxButton>
            <NvxButton
              variant="ghost"
              size="sm"
              @click="openEdit(entry.host)"
            >
              <NvxIcon
                :icon="Pencil"
                :size="16"
              />
              {{ t("sshHosts.edit") }}
            </NvxButton>
            <NvxButton
              variant="ghost"
              size="sm"
              @click="requestDelete(entry.host)"
            >
              <NvxIcon
                :icon="Trash2"
                :size="16"
              />
              {{ t("sshHosts.delete") }}
            </NvxButton>
            <NvxButton
              variant="secondary"
              size="sm"
              @click="connect(entry.host)"
            >
              {{ t("sshHosts.connect") }}
            </NvxButton>
          </div>
        </article>
      </section>
    </div>
    <section
      v-else-if="!loading && canUseDesktopCore() && !loadFailed"
      class="hosts-empty"
    >
      <NvxIcon
        :icon="Server"
        :size="22"
      />
      <h2>{{ t("sshHosts.emptyTitle") }}</h2>
      <p>{{ t("sshHosts.emptyBody") }}</p>
      <NvxButton @click="openCreate">
        <NvxIcon
          :icon="Plus"
          :size="16"
        />
        {{ t("sshHosts.add") }}
      </NvxButton>
    </section>

    <NvxDialog
      :model-value="dialogOpen"
      plugin-protected
      size="xl"
      :title="t(editingHost ? 'sshHosts.editDialogTitle' : 'sshHosts.dialogTitle')"
      :description="t(editingHost ? 'sshHosts.editDialogDescription' : 'sshHosts.dialogDescription')"
      :close-label="t(editingHost ? 'sshHosts.closeEditDialog' : 'sshHosts.closeDialog')"
      :dismissible="!saving && !testingConnection"
      @update:model-value="updateHostDialogOpen"
    >
      <NvxPluginContributionSlot
        v-if="editingHost"
        extension-slot="hostDetailTools"
        :instance-key="editingHost.hostId"
        :display-label="editingHost.label"
      />
      <div class="hosts-editor">
        <nav
          class="hosts-editor__nav"
          :aria-label="t('sshHosts.editorSections.navigationLabel')"
        >
          <button
            v-for="section in hostEditorSections"
            :key="section.value"
            type="button"
            class="hosts-editor__nav-item"
            :class="{ 'hosts-editor__nav-item--active': hostEditorSection === section.value }"
            :aria-current="hostEditorSection === section.value ? 'page' : undefined"
            @click="hostEditorSection = section.value"
          >
            <NvxIcon
              :icon="section.icon"
              :size="20"
            />
            <span>{{ section.label }}</span>
          </button>
        </nav>

        <section class="hosts-editor__content">
          <template v-if="hostEditorSection === 'connection'">
            <h3>{{ t("sshHosts.editorSections.connection") }}</h3>
            <NvxField
              for-id="host-label"
              :label="t('sshHosts.label')"
            >
              <NvxInput
                id="host-label"
                v-model="label"
                :placeholder="t('sshHosts.labelPlaceholder')"
                data-nvx-dialog-initial-focus
              />
            </NvxField>
            <div class="hosts-dialog__endpoint-row">
              <NvxField
                for-id="host-address"
                :label="t('sshHosts.address')"
              >
                <NvxInput
                  id="host-address"
                  v-model="address"
                  :placeholder="t('sshHosts.addressPlaceholder')"
                />
              </NvxField>
              <NvxField
                for-id="host-port"
                :label="t('sshHosts.port')"
              >
                <NvxInput
                  id="host-port"
                  v-model="port"
                  type="number"
                  :min="1"
                  :max="65535"
                  placeholder="22"
                />
              </NvxField>
            </div>
            <NvxField
              for-id="host-username"
              :label="t('sshHosts.username')"
            >
              <NvxInput
                id="host-username"
                v-model="username"
                :placeholder="t('sshHosts.usernamePlaceholder')"
                autocomplete="username"
              />
            </NvxField>
            <NvxField :label="t('sshHosts.authenticationMode')">
              <NvxSelect
                id="host-authentication-mode"
                v-model="authenticationMode"
                :options="authenticationModeOptions"
                :aria-label="t('sshHosts.authenticationMode')"
              />
            </NvxField>
            <NvxField
              v-if="authenticationMode === 'savedIdentity'"
              :label="t('sshHosts.identity')"
            >
              <NvxSelect
                v-model="identityId"
                :options="identityOptions"
                :aria-label="t('sshHosts.identity')"
              />
            </NvxField>
            <NvxButton
              v-if="authenticationMode === 'savedIdentity'"
              variant="secondary"
              size="sm"
              @click="openPrivateKeyImport(identityId, true)"
            >
              <NvxIcon
                :icon="FileInput"
                :size="16"
              />
              {{ t('sshHosts.privateKeyImport.manage') }}
            </NvxButton>
            <template v-else-if="authenticationMode === 'privateKey'">
              <NvxInlineNotice
                v-if="privateKeyCredentialSummary?.identityId === identityId
                  && privateKeyCredentialSummary.details.kind === 'privateKey'"
                :title="t('sshHosts.privateKeyImport.saved')"
              >
                {{ t('sshHosts.privateKeyImport.importedMetadata', {
                  algorithm: privateKeyCredentialSummary.details.publicKeyAlgorithm,
                  fingerprint: privateKeyCredentialSummary.details.publicKeyFingerprint,
                }) }}
              </NvxInlineNotice>
              <NvxInlineNotice
                v-else
                :title="t('sshHosts.privateKeyImport.required')"
              >
                {{ t('sshHosts.privateKeyImport.requiredHint') }}
              </NvxInlineNotice>
              <NvxButton
                variant="secondary"
                size="sm"
                @click="openPrivateKeyImport(identityId, true)"
              >
                <NvxIcon
                  :icon="FileInput"
                  :size="16"
                />
                {{ t('sshHosts.privateKeyImport.manage') }}
              </NvxButton>
            </template>
            <NvxField
              v-else-if="authenticationMode === 'password'"
              for-id="host-password"
              :label="t('sshHosts.password')"
            >
              <NvxInput
                id="host-password"
                v-model="hostPassword"
                type="password"
                autocomplete="new-password"
                :placeholder="t('sshHosts.passwordPlaceholder')"
              />
            </NvxField>
            <section
              v-if="authenticationMode === 'password' && vaultState !== 'unlocked'"
              class="hosts-dialog__vault-gate"
            >
              <NvxInlineNotice
                :tone="vaultStatusUnavailable ? 'error' : 'info'"
                :title="t(vaultStatusLoading ? 'sshHosts.vault.statusLoading' : vaultStatusUnavailable ? 'sshHosts.vault.statusUnavailable' : vaultState === 'missing' ? 'sshHosts.vault.createRequired' : 'sshHosts.vault.unlockRequired')"
              >
                {{ t("sshHosts.vault.gateHint") }}
              </NvxInlineNotice>
              <NvxButton
                v-if="!vaultStatusLoading && !vaultStatusUnavailable"
                variant="secondary"
                @click="openHostVaultDialog"
              >
                {{ t(vaultState === 'missing' ? 'sshHosts.vault.createAction' : 'sshHosts.vault.unlockAction') }}
              </NvxButton>
              <NvxButton
                v-else-if="vaultStatusUnavailable"
                variant="secondary"
                @click="refreshHostVaultState"
              >
                {{ t('sshHosts.vault.retryStatus') }}
              </NvxButton>
            </section>
            <p class="hosts-dialog__identity-hint">
              {{ t(`sshHosts.authenticationHints.${authenticationMode}`) }}
            </p>
            <NvxCheckbox
              id="host-favorite"
              v-model="favorite"
            >
              {{ t("sshHosts.favorite") }}
            </NvxCheckbox>
          </template>

          <template v-else-if="hostEditorSection === 'classification'">
            <h3>{{ t("sshHosts.editorSections.classification") }}</h3>
            <NvxHostMarkerEditor
              v-model="hostMarkerDraft"
              :disabled="saving"
            />
            <section
              class="hosts-dialog__classification"
              :aria-label="t('sshHosts.classification')"
            >
              <NvxField :label="t('sshHosts.group')">
                <NvxSelect
                  v-model="groupId"
                  :options="groupOptions"
                  :aria-label="t('sshHosts.group')"
                />
              </NvxField>
              <div class="hosts-dialog__create-row">
                <NvxInput
                  v-model="newGroupLabel"
                  :placeholder="t('sshHosts.newGroupPlaceholder')"
                  @keydown.enter.prevent="addGroup"
                />
                <NvxButton
                  variant="secondary"
                  size="sm"
                  :loading="creatingGroup"
                  :disabled="!newGroupLabel.trim()"
                  @click="addGroup"
                >
                  <NvxIcon
                    :icon="FolderPlus"
                    :size="16"
                  />
                  {{ t("sshHosts.createGroup") }}
                </NvxButton>
              </div>
              <div class="hosts-dialog__tags">
                <span class="hosts-dialog__section-label">{{ t("sshHosts.tags") }}</span>
                <div
                  v-if="tags.length"
                  class="hosts-dialog__tag-options"
                >
                  <NvxCheckbox
                    v-for="tagItem in tags"
                    :id="`host-tag-${tagItem.tagId}`"
                    :key="tagItem.tagId"
                    :model-value="selectedTagIds.includes(tagItem.tagId)"
                    @update:model-value="toggleTag(tagItem.tagId, $event)"
                  >
                    {{ tagItem.label }}
                  </NvxCheckbox>
                </div>
                <span
                  v-else
                  class="hosts-dialog__empty-tags"
                >{{ t("sshHosts.noTags") }}</span>
              </div>
              <div class="hosts-dialog__create-row">
                <NvxInput
                  v-model="newTagLabel"
                  :placeholder="t('sshHosts.newTagPlaceholder')"
                  @keydown.enter.prevent="addTag"
                />
                <NvxButton
                  variant="secondary"
                  size="sm"
                  :loading="creatingTag"
                  :disabled="!newTagLabel.trim()"
                  @click="addTag"
                >
                  <NvxIcon
                    :icon="Tag"
                    :size="16"
                  />
                  {{ t("sshHosts.createTag") }}
                </NvxButton>
              </div>
              <p class="hosts-dialog__classification-note">
                {{ t("sshHosts.classificationHint") }}
              </p>
            </section>
          </template>

          <template v-else-if="hostEditorSection === 'connectionRoute'">
            <h3>{{ t("sshHosts.editorSections.connectionRoute") }}</h3>
            <NvxInlineNotice
              v-if="hostConfigLoading"
              :title="t('sshHosts.advanced.loading')"
            />
            <NvxInlineNotice
              v-else-if="hostConfigLoadFailed"
              tone="error"
              :title="t('sshHosts.advanced.loadFailed')"
            />
            <template v-else>
              <section
                class="hosts-route__section"
                :aria-label="t('sshHosts.connectionRoute.ingress')"
              >
                <div class="hosts-route__control-grid">
                  <NvxField :label="t('sshHosts.connectionRoute.ingress')">
                    <NvxSelect
                      id="route-ingress"
                      v-model="routeIngressKind"
                      :options="routeIngressOptions"
                      :aria-label="t('sshHosts.connectionRoute.ingress')"
                    />
                  </NvxField>
                  <NvxField
                    v-if="routeIngressKind !== 'directTcp'"
                    for-id="route-proxy-address"
                    :label="t('sshHosts.connectionRoute.proxyAddress')"
                  >
                    <NvxInput
                      id="route-proxy-address"
                      v-model="routeProxyAddress"
                      placeholder="proxy.example.com"
                    />
                  </NvxField>
                  <NvxField
                    v-if="routeIngressKind !== 'directTcp'"
                    for-id="route-proxy-port"
                    :label="t('sshHosts.connectionRoute.proxyPort')"
                  >
                    <NvxInput
                      id="route-proxy-port"
                      v-model="routeProxyPort"
                      placeholder="8080"
                    />
                  </NvxField>
                  <NvxField
                    v-if="routeIngressKind === 'socks5Proxy'"
                    :label="t('sshHosts.connectionRoute.dnsMode')"
                  >
                    <NvxSelect
                      id="route-proxy-dns"
                      v-model="routeProxyDnsMode"
                      :options="routeDnsOptions"
                      :aria-label="t('sshHosts.connectionRoute.dnsMode')"
                    />
                  </NvxField>
                  <NvxField
                    v-if="routeIngressKind !== 'directTcp'"
                    :label="t('sshHosts.connectionRoute.proxyCredential')"
                  >
                    <NvxSelect
                      id="route-proxy-credential"
                      v-model="routeProxyCredentialRefId"
                      :options="routeCredentialOptions"
                      :aria-label="t('sshHosts.connectionRoute.proxyCredential')"
                    />
                  </NvxField>
                </div>
                <p class="hosts-route__hint">
                  {{ t("sshHosts.connectionRoute.ingressHint") }}
                </p>
              </section>

              <section
                class="hosts-route__section"
                :aria-label="t('sshHosts.connectionRoute.jumpHosts')"
              >
                <div class="hosts-route__heading">
                  <div>
                    <h4>{{ t("sshHosts.connectionRoute.jumpHosts") }}</h4>
                    <span>{{ t("sshHosts.connectionRoute.jumpCount", { count: routeJumpHostIds.length }) }}</span>
                  </div>
                  <NvxButton
                    variant="secondary"
                    size="sm"
                    :disabled="!canAddRouteJumpHost"
                    @click="addRouteJumpHost"
                  >
                    <NvxIcon
                      :icon="Plus"
                      :size="16"
                    />
                    {{ t("sshHosts.connectionRoute.addJumpHost") }}
                  </NvxButton>
                </div>
                <p class="hosts-route__hint">
                  {{ t("sshHosts.connectionRoute.jumpHint") }}
                </p>
                <p
                  v-if="!routeJumpHostIds.length"
                  class="hosts-route__empty"
                >
                  {{ t("sshHosts.connectionRoute.noJumpHosts") }}
                </p>
                <div
                  v-else
                  class="hosts-route__jump-list"
                >
                  <div
                    v-for="(jumpHostId, index) in routeJumpHostIds"
                    :key="`${jumpHostId}-${index}`"
                    class="hosts-route__jump-row"
                  >
                    <span class="hosts-route__jump-index">{{ index + 1 }}</span>
                    <NvxSelect
                      :id="`route-jump-${index}`"
                      :model-value="jumpHostId"
                      :options="routeJumpHostOptions(index)"
                      :aria-label="t('sshHosts.connectionRoute.jumpHostAt', { index: index + 1 })"
                      @update:model-value="updateRouteJumpHost(index, $event)"
                    />
                    <NvxIconButton
                      :label="t('sshHosts.connectionRoute.moveJumpUp')"
                      :disabled="index === 0"
                      @click="moveRouteJumpHost(index, -1)"
                    >
                      <NvxIcon
                        :icon="ArrowUp"
                        :size="16"
                      />
                    </NvxIconButton>
                    <NvxIconButton
                      :label="t('sshHosts.connectionRoute.moveJumpDown')"
                      :disabled="index === routeJumpHostIds.length - 1"
                      @click="moveRouteJumpHost(index, 1)"
                    >
                      <NvxIcon
                        :icon="ArrowDown"
                        :size="16"
                      />
                    </NvxIconButton>
                    <NvxIconButton
                      :label="t('sshHosts.connectionRoute.removeJumpHost')"
                      @click="removeRouteJumpHost(index)"
                    >
                      <NvxIcon
                        :icon="X"
                        :size="16"
                      />
                    </NvxIconButton>
                  </div>
                </div>
              </section>
              <NvxInlineNotice
                v-if="routeSaveFailed"
                tone="error"
                :title="t('sshHosts.connectionRoute.saveFailed')"
              />
            </template>
          </template>

          <template v-else-if="hostEditorSection === 'algorithms'">
            <h3>{{ t("sshHosts.editorSections.algorithms") }}</h3>
            <NvxInlineNotice
              v-if="hostConfigLoading"
              :title="t('sshHosts.advanced.loading')"
            />
            <NvxInlineNotice
              v-else-if="hostConfigLoadFailed"
              tone="error"
              :title="t('sshHosts.advanced.loadFailed')"
            />
            <section
              v-else-if="algorithmCatalog"
              class="hosts-algorithms"
            >
              <NvxInlineNotice :title="t('sshHosts.algorithms.secureDefault')">
                {{ t('sshHosts.algorithms.catalogVersion', { version: algorithmCatalog.catalogVersion }) }}
              </NvxInlineNotice>
              <section
                v-for="category in algorithmCategories"
                :key="category.value"
                class="hosts-algorithms__category"
                :aria-label="category.label"
              >
                <h4>{{ category.label }}</h4>
                <div class="hosts-algorithms__defaults">
                  <span
                    v-for="entry in algorithmCatalog.entries.filter((candidate) => candidate.category === category.value && candidate.enabledByDefault)"
                    :key="entry.stableId"
                    class="hosts-algorithms__default"
                  >
                    <code>{{ entry.algorithmName }}</code>
                    <NvxStatusLabel tone="success">{{ t('sshHosts.algorithms.defaultEnabled') }}</NvxStatusLabel>
                  </span>
                </div>
                <div
                  v-if="algorithmCatalog.entries.some((candidate) => candidate.category === category.value && candidate.selectableException)"
                  class="hosts-algorithms__exceptions"
                >
                  <strong>{{ t('sshHosts.algorithms.compatibilityExceptions') }}</strong>
                  <label
                    v-for="entry in algorithmCatalog.entries.filter((candidate) => candidate.category === category.value && candidate.selectableException)"
                    :key="entry.stableId"
                    class="hosts-algorithms__exception"
                  >
                    <NvxCheckbox
                      :id="`algorithm-${entry.stableId}`"
                      class="hosts-algorithms__selector"
                      :model-value="algorithmSelectedExceptionIds.includes(entry.stableId)"
                      :disabled="!entry.available"
                      @update:model-value="toggleAlgorithmException(entry.stableId, $event)"
                    >
                      <code>{{ entry.algorithmName }}</code>
                    </NvxCheckbox>
                    <span v-if="entry.riskMessageKey">{{ t(entry.riskMessageKey) }}</span>
                  </label>
                </div>
              </section>
            </section>
          </template>

          <template v-else-if="hostEditorSection === 'loginAutomation'">
            <h3>{{ t("sshHosts.editorSections.loginAutomation") }}</h3>
            <NvxInlineNotice
              v-if="hostConfigLoading"
              :title="t('sshHosts.advanced.loading')"
            />
            <NvxInlineNotice
              v-else-if="hostConfigLoadFailed"
              tone="error"
              :title="t('sshHosts.advanced.loadFailed')"
            />
            <section
              v-else
              class="hosts-login-automation"
            >
              <div class="hosts-login-automation__heading">
                <div>
                  <strong>{{ t("sshHosts.loginAutomation.enabled") }}</strong>
                  <p>{{ t("sshHosts.loginAutomation.enabledHint") }}</p>
                </div>
                <NvxCheckbox
                  id="login-automation-enabled"
                  v-model="loginAutomationEnabled"
                  class="hosts-login-automation__enable"
                >
                  {{ t("sshHosts.loginAutomation.enable") }}
                </NvxCheckbox>
              </div>

              <NvxInlineNotice
                v-if="loginAutomationNeedsConfirmation && !loginAutomationDirty"
                tone="warning"
                :title="t('sshHosts.loginAutomation.confirmRequired')"
              >
                {{ t("sshHosts.loginAutomation.confirmRequiredHint") }}
              </NvxInlineNotice>

              <div class="hosts-login-automation__step-heading">
                <div>
                  <strong>{{ t("sshHosts.loginAutomation.steps") }}</strong>
                  <span>{{ t("sshHosts.loginAutomation.stepCount", { count: loginAutomationSteps.length }) }}</span>
                </div>
                <NvxButton
                  variant="secondary"
                  size="sm"
                  :disabled="loginAutomationSteps.length >= 32"
                  @click="addLoginAutomationStep"
                >
                  <NvxIcon
                    :icon="Plus"
                    :size="16"
                  />
                  {{ t("sshHosts.loginAutomation.addStep") }}
                </NvxButton>
              </div>

              <p
                v-if="!loginAutomationSteps.length"
                class="hosts-login-automation__empty"
              >
                {{ t("sshHosts.loginAutomation.empty") }}
              </p>

              <article
                v-for="(step, index) in loginAutomationSteps"
                :key="step.id"
                class="hosts-login-automation__step"
              >
                <div class="hosts-login-automation__controls">
                  <span class="hosts-login-automation__index">{{ index + 1 }}</span>
                  <NvxField
                    :for-id="`login-automation-type-${step.id}`"
                    :label="t('sshHosts.loginAutomation.stepType')"
                  >
                    <NvxSelect
                      :id="`login-automation-type-${step.id}`"
                      :model-value="step.kind"
                      :options="loginAutomationStepTypeOptions"
                      @update:model-value="updateLoginAutomationStepKind(index, $event)"
                    />
                  </NvxField>
                  <NvxField
                    :for-id="`login-automation-timeout-${step.id}`"
                    :label="t('sshHosts.loginAutomation.timeout')"
                  >
                    <NvxInput
                      :id="`login-automation-timeout-${step.id}`"
                      v-model="step.timeoutSeconds"
                      type="number"
                      :min="1"
                      :max="60"
                    />
                  </NvxField>
                  <div class="hosts-login-automation__reorder">
                    <NvxIconButton
                      :label="t('sshHosts.loginAutomation.moveUp')"
                      :disabled="index === 0"
                      @click="moveLoginAutomationStep(index, -1)"
                    >
                      <NvxIcon
                        :icon="ArrowUp"
                        :size="16"
                      />
                    </NvxIconButton>
                    <NvxIconButton
                      :label="t('sshHosts.loginAutomation.moveDown')"
                      :disabled="index === loginAutomationSteps.length - 1"
                      @click="moveLoginAutomationStep(index, 1)"
                    >
                      <NvxIcon
                        :icon="ArrowDown"
                        :size="16"
                      />
                    </NvxIconButton>
                    <NvxIconButton
                      :label="t('sshHosts.loginAutomation.removeStep')"
                      @click="removeLoginAutomationStep(index)"
                    >
                      <NvxIcon
                        :icon="Trash2"
                        :size="16"
                      />
                    </NvxIconButton>
                  </div>
                </div>

                <p class="hosts-login-automation__step-hint">
                  {{ t(`sshHosts.loginAutomation.stepHints.${step.kind}`) }}
                </p>
                <NvxField
                  v-if="step.kind === 'expect'"
                  :for-id="`login-automation-value-${step.id}`"
                  :label="t('sshHosts.loginAutomation.expectText')"
                >
                  <NvxInput
                    :id="`login-automation-value-${step.id}`"
                    v-model="step.value"
                    :maxlength="4096"
                    autocomplete="off"
                  />
                </NvxField>
                <template v-else-if="step.kind === 'sendText'">
                  <NvxField
                    :for-id="`login-automation-value-${step.id}`"
                    :label="t('sshHosts.loginAutomation.sendText')"
                  >
                    <NvxTextarea
                      :id="`login-automation-value-${step.id}`"
                      v-model="step.value"
                      :maxlength="4096"
                    />
                  </NvxField>
                  <NvxCheckbox
                    :id="`login-automation-enter-${step.id}`"
                    v-model="step.appendEnter"
                  >
                    {{ t("sshHosts.loginAutomation.appendEnter") }}
                  </NvxCheckbox>
                </template>
                <template v-else>
                  <NvxInlineNotice
                    v-if="!editingHost"
                    tone="warning"
                    :title="t('sshHosts.loginAutomation.secretAfterCreateTitle')"
                  >
                    {{ t("sshHosts.loginAutomation.secretAfterCreateBody") }}
                  </NvxInlineNotice>
                  <NvxInlineNotice
                    v-else-if="step.existingOrdinal !== null"
                    :title="t('sshHosts.loginAutomation.keepSecret', { label: step.secretLabel })"
                  >
                    {{ t("sshHosts.loginAutomation.keepSecretHint") }}
                  </NvxInlineNotice>
                  <div
                    v-else
                    class="hosts-login-automation__secret-grid"
                  >
                    <NvxField
                      :for-id="`login-automation-secret-label-${step.id}`"
                      :label="t('sshHosts.loginAutomation.secretLabel')"
                    >
                      <NvxInput
                        :id="`login-automation-secret-label-${step.id}`"
                        v-model="step.secretLabel"
                        :maxlength="128"
                        autocomplete="off"
                        :disabled="step.stagedSecretId !== null"
                      />
                    </NvxField>
                    <NvxField
                      :for-id="`login-automation-secret-value-${step.id}`"
                      :label="t('sshHosts.loginAutomation.secretValue')"
                    >
                      <NvxInput
                        :id="`login-automation-secret-value-${step.id}`"
                        v-model="step.secretValue"
                        type="password"
                        :maxlength="4096"
                        autocomplete="new-password"
                        :disabled="step.stagedSecretId !== null"
                      />
                    </NvxField>
                  </div>
                  <NvxCheckbox
                    :id="`login-automation-enter-${step.id}`"
                    v-model="step.appendEnter"
                  >
                    {{ t("sshHosts.loginAutomation.appendEnter") }}
                  </NvxCheckbox>
                </template>
              </article>

              <NvxCheckbox
                v-if="loginAutomationEnabled"
                id="login-automation-confirm-on-save"
                v-model="loginAutomationConfirmOnSave"
              >
                {{ t("sshHosts.loginAutomation.confirmOnSave") }}
              </NvxCheckbox>
              <NvxInlineNotice
                v-if="loginAutomationSaveFailed"
                tone="error"
                :title="t('sshHosts.loginAutomation.saveFailed')"
              />
              <NvxInlineNotice
                v-if="loginAutomationConfirmFailed"
                tone="error"
                :title="t('sshHosts.loginAutomation.confirmFailed')"
              />
            </section>
          </template>

          <template v-else-if="hostEditorSection === 'connectionHealth'">
            <h3>{{ t("sshHosts.heartbeat.manage") }}</h3>
            <NvxInlineNotice
              v-if="hostConfigLoading"
              :title="t('sshHosts.advanced.loading')"
            />
            <NvxInlineNotice
              v-else-if="hostConfigLoadFailed"
              tone="error"
              :title="t('sshHosts.advanced.loadFailed')"
            />
            <template v-else>
              <section class="hosts-health__heartbeat-card">
                <div class="hosts-health__toggle-row">
                  <NvxCheckbox
                    id="heartbeat-transport-enabled"
                    :model-value="heartbeatMode === 'transportKeepalive'"
                    @update:model-value="setTransportHeartbeatEnabled"
                  >
                    {{ t("sshHosts.editorSections.transportKeepaliveLabel") }}
                  </NvxCheckbox>
                  <NvxStatusLabel tone="info">
                    {{ t("sshHosts.editorSections.recommended") }}
                  </NvxStatusLabel>
                </div>
                <div class="hosts-health__three-column-grid">
                  <NvxField
                    for-id="heartbeat-interval"
                    :label="t('sshHosts.editorSections.heartbeatInterval')"
                  >
                    <NvxInput
                      id="heartbeat-interval"
                      v-model="heartbeatIntervalSeconds"
                      type="number"
                      :min="10"
                      :max="3600"
                      :disabled="heartbeatMode !== 'transportKeepalive'"
                    />
                  </NvxField>
                  <NvxField
                    for-id="heartbeat-reply-timeout"
                    :label="t('sshHosts.editorSections.heartbeatReplyTimeout')"
                  >
                    <NvxInput
                      id="heartbeat-reply-timeout"
                      v-model="heartbeatReplyTimeoutSeconds"
                      type="number"
                      :min="5"
                      :max="60"
                      :disabled="heartbeatMode !== 'transportKeepalive'"
                    />
                  </NvxField>
                  <NvxField
                    for-id="heartbeat-failure-threshold"
                    :label="t('sshHosts.editorSections.heartbeatFailureThreshold')"
                  >
                    <NvxInput
                      id="heartbeat-failure-threshold"
                      v-model="heartbeatFailureThreshold"
                      type="number"
                      :min="1"
                      :max="10"
                      :disabled="heartbeatMode !== 'transportKeepalive'"
                    />
                  </NvxField>
                </div>
                <p class="hosts-heartbeat__hint">
                  {{ t("sshHosts.editorSections.transportKeepaliveHint") }}
                </p>
              </section>

              <section class="hosts-health__monitoring">
                <h4>{{ t("sshHosts.editorSections.serverMonitoring") }}</h4>
                <NvxCheckbox
                  id="monitoring-enabled"
                  v-model="monitoringEnabled"
                >
                  {{ t("sshHosts.editorSections.enableMonitoring") }}
                </NvxCheckbox>
                <div class="hosts-monitoring__control-grid">
                  <NvxField
                    for-id="monitoring-interval"
                    :label="t('sshHosts.editorSections.monitoringInterval')"
                  >
                    <NvxInput
                      id="monitoring-interval"
                      v-model="monitoringIntervalSeconds"
                      type="number"
                      :min="5"
                      :max="300"
                      :disabled="!monitoringEnabled"
                    />
                  </NvxField>
                  <NvxField
                    for-id="monitoring-timeout"
                    :label="t('sshHosts.editorSections.monitoringTimeout')"
                  >
                    <NvxInput
                      id="monitoring-timeout"
                      v-model="monitoringTimeoutSeconds"
                      type="number"
                      :min="2"
                      :max="30"
                      :disabled="!monitoringEnabled"
                    />
                  </NvxField>
                </div>
                <div class="hosts-health__resource-grid">
                  <div class="hosts-health__resource-option">
                    <span>{{ t("sshHosts.editorSections.disk") }}</span>
                    <NvxCheckbox
                      id="monitoring-disk-root"
                      :model-value="true"
                      disabled
                    >
                      {{ t("sshHosts.editorSections.rootDisk") }}
                    </NvxCheckbox>
                  </div>
                  <div class="hosts-health__resource-option">
                    <span>{{ t("sshHosts.editorSections.network") }}</span>
                    <NvxCheckbox
                      id="monitoring-network-aggregate"
                      :model-value="true"
                      disabled
                    >
                      {{ t("sshHosts.editorSections.nonLoopbackNetwork") }}
                    </NvxCheckbox>
                  </div>
                </div>
                <NvxInlineNotice
                  v-if="monitoringRuntimePending"
                  tone="warning"
                  :title="t('sshHosts.monitoring.runtimePending')"
                />
              </section>
            </template>
          </template>

          <template v-else-if="hostEditorSection === 'advanced'">
            <h3>{{ t("sshHosts.editorSections.advanced") }}</h3>
            <section class="hosts-health__shell-content">
              <NvxField :label="t('sshHosts.heartbeat.mode')">
                <NvxSelect
                  id="heartbeat-mode"
                  v-model="heartbeatMode"
                  :options="heartbeatModeOptions"
                  :aria-label="t('sshHosts.heartbeat.mode')"
                />
              </NvxField>
              <template v-if="heartbeatMode === 'shellHeartbeat'">
                <NvxInlineNotice
                  tone="warning"
                  :title="t('sshHosts.heartbeat.shellWarningTitle')"
                >
                  {{ t("sshHosts.heartbeat.shellWarningBody") }}
                </NvxInlineNotice>
                <div class="hosts-health__three-column-grid">
                  <NvxField
                    for-id="heartbeat-shell-interval"
                    :label="t('sshHosts.heartbeat.interval')"
                  >
                    <NvxInput
                      id="heartbeat-shell-interval"
                      v-model="heartbeatIntervalSeconds"
                      type="number"
                      :min="30"
                      :max="3600"
                    />
                  </NvxField>
                  <NvxField
                    for-id="heartbeat-user-idle"
                    :label="t('sshHosts.heartbeat.userIdle')"
                  >
                    <NvxInput
                      id="heartbeat-user-idle"
                      v-model="heartbeatUserIdleSeconds"
                      type="number"
                      :min="5"
                      :max="3600"
                    />
                  </NvxField>
                  <NvxField :label="t('sshHosts.heartbeat.lineEnding')">
                    <NvxSelect
                      id="heartbeat-line-ending"
                      v-model="heartbeatLineEnding"
                      :options="heartbeatLineEndingOptions"
                      :aria-label="t('sshHosts.heartbeat.lineEnding')"
                    />
                  </NvxField>
                </div>
                <NvxField
                  for-id="heartbeat-payload"
                  :label="t('sshHosts.heartbeat.payload')"
                >
                  <NvxTextarea
                    id="heartbeat-payload"
                    v-model="heartbeatPayloadText"
                    :maxlength="1024"
                    autocomplete="off"
                  />
                </NvxField>
                <p class="hosts-heartbeat__preview">
                  {{ t("sshHosts.heartbeat.preview") }} <code>{{ heartbeatPayloadPreview }}</code>
                </p>
              </template>
            </section>
          </template>

          <NvxInlineNotice
            v-if="saveFailed"
            tone="error"
            :title="t(saveFailureKey)"
          />
          <NvxInlineNotice
            v-if="advancedConfigSaveFailed"
            tone="error"
            :title="t('sshHosts.advanced.saveFailed')"
          />
        </section>
      </div>
      <template #actions>
        <NvxButton
          variant="ghost"
          :disabled="saving || testingConnection"
          @click="requestCloseHostEditor"
        >
          {{ t("sshHosts.cancel") }}
        </NvxButton>
        <NvxButton
          variant="secondary"
          :loading="testingConnection"
          :disabled="saving || testingConnection"
          :title="t('sshHosts.testConnectionHint')"
          @click="testConnection"
        >
          <NvxIcon
            :icon="Network"
            :size="16"
          />
          {{ t("sshHosts.testConnection") }}
        </NvxButton>
        <NvxButton
          :loading="saving"
          :disabled="hostConfigLoading || hostConfigLoadFailed || hasSelectedUnavailableAlgorithm"
          @click="saveHost"
        >
          {{ t(editingHost ? "sshHosts.saveChanges" : "sshHosts.save") }}
        </NvxButton>
      </template>
    </NvxDialog>

    <NvxDialog
      :model-value="vaultDialogOpen"
      plugin-protected
      :title="t(vaultDialogMode === 'create' ? 'sshHosts.vault.createTitle' : 'sshHosts.vault.unlockTitle')"
      :description="t('sshHosts.vault.description')"
      :close-label="t('sshHosts.vault.close')"
      :dismissible="!vaultSubmitting"
      @update:model-value="(open) => { if (!open) closeHostVaultDialog(); }"
    >
      <NvxField
        for-id="host-vault-password"
        :label="t('sshHosts.vault.password')"
      >
        <NvxInput
          id="host-vault-password"
          v-model="vaultPassword"
          type="password"
          autocomplete="current-password"
          data-nvx-dialog-initial-focus
        />
      </NvxField>
      <NvxField
        v-if="vaultDialogMode === 'create'"
        for-id="host-vault-password-confirmation"
        :label="t('sshHosts.vault.passwordConfirmation')"
      >
        <NvxInput
          id="host-vault-password-confirmation"
          v-model="vaultPasswordConfirmation"
          type="password"
          autocomplete="new-password"
        />
      </NvxField>
      <NvxInlineNotice
        v-if="vaultActionFailed"
        tone="error"
        :title="t('sshHosts.vault.failed')"
      />
      <template #actions>
        <NvxButton
          variant="ghost"
          :disabled="vaultSubmitting"
          @click="() => closeHostVaultDialog()"
        >
          {{ t("sshHosts.cancel") }}
        </NvxButton>
        <NvxButton
          :loading="vaultSubmitting"
          :disabled="!vaultDialogPasswordValid"
          @click="prepareVaultForHostPassword"
        >
          {{ t(vaultDialogMode === 'create' ? 'sshHosts.vault.createAction' : 'sshHosts.vault.unlockAction') }}
        </NvxButton>
      </template>
    </NvxDialog>

    <NvxDialog
      v-model="keyboardInteractiveDialogOpen"
      plugin-protected
      :title="t('sshHosts.keyboardInteractive.title')"
      :description="t('sshHosts.keyboardInteractive.description')"
      :close-label="t('sshHosts.keyboardInteractive.close')"
      :dismissible="!savingKeyboardInteractive"
    >
      <section class="hosts-keyboard-interactive">
        <NvxField :label="t('sshHosts.identity')">
          <NvxSelect
            v-model="keyboardInteractiveIdentityId"
            :options="identityOptions"
            :aria-label="t('sshHosts.identity')"
          />
        </NvxField>
        <NvxField
          for-id="keyboard-interactive-label"
          :label="t('sshHosts.keyboardInteractive.credentialLabel')"
        >
          <NvxInput
            id="keyboard-interactive-label"
            v-model="keyboardInteractiveLabel"
          />
        </NvxField>
        <div class="hosts-keyboard-interactive__limits">
          <NvxField
            for-id="keyboard-interactive-priority"
            :label="t('sshHosts.keyboardInteractive.priority')"
          >
            <NvxInput
              id="keyboard-interactive-priority"
              v-model="keyboardInteractivePriority"
              type="number"
              :min="0"
              :max="65535"
            />
          </NvxField>
          <NvxField
            for-id="keyboard-interactive-max-rounds"
            :label="t('sshHosts.keyboardInteractive.maxRounds')"
          >
            <NvxInput
              id="keyboard-interactive-max-rounds"
              v-model="keyboardInteractiveMaxRounds"
              type="number"
              :min="1"
              :max="32"
            />
          </NvxField>
        </div>
        <p>{{ t('sshHosts.keyboardInteractive.persistenceHint') }}</p>
        <NvxInlineNotice
          v-if="keyboardInteractiveFailed"
          tone="error"
          :title="t('sshHosts.keyboardInteractive.failed')"
        />
        <NvxInlineNotice
          v-if="keyboardInteractiveSaved"
          :title="t('sshHosts.keyboardInteractive.saved')"
        />
      </section>
      <template #actions>
        <NvxButton
          variant="ghost"
          :disabled="savingKeyboardInteractive"
          @click="keyboardInteractiveDialogOpen = false"
        >
          {{ t('sshHosts.cancel') }}
        </NvxButton>
        <NvxButton
          :loading="savingKeyboardInteractive"
          :disabled="!keyboardInteractiveIdentityId || !keyboardInteractiveLabel.trim()"
          @click="saveKeyboardInteractiveCredential"
        >
          {{ t('sshHosts.keyboardInteractive.save') }}
        </NvxButton>
      </template>
    </NvxDialog>

    <NvxDialog
      v-model="opensshDialogOpen"
      plugin-protected
      :title="t('sshHosts.opensshImport.title')"
      :description="t('sshHosts.opensshImport.description')"
      :close-label="t('sshHosts.opensshImport.close')"
      :dismissible="!previewingOpenSsh && !importingOpenSsh"
    >
      <NvxField
        for-id="openssh-config-text"
        :label="t('sshHosts.opensshImport.configText')"
      >
        <NvxTextarea
          id="openssh-config-text"
          v-model="opensshConfigText"
          :placeholder="t('sshHosts.opensshImport.placeholder')"
          :disabled="importingOpenSsh"
          data-nvx-dialog-initial-focus
        />
      </NvxField>
      <p class="hosts-openssh__note">
        {{ t("sshHosts.opensshImport.sourceHint") }}
      </p>
      <NvxButton
        variant="secondary"
        :loading="previewingOpenSsh"
        :disabled="!opensshConfigText.trim() || importingOpenSsh"
        @click="previewOpenSshImport"
      >
        {{ t("sshHosts.opensshImport.preview") }}
      </NvxButton>

      <section
        v-if="opensshPreview"
        class="hosts-openssh__preview"
      >
        <div
          v-for="candidate in opensshPreview.candidates"
          :key="candidate.candidateId"
          class="hosts-openssh__candidate"
        >
          <NvxCheckbox
            :id="`openssh-candidate-${candidate.candidateId}`"
            :model-value="selectedOpenSshCandidateIds.includes(candidate.candidateId)"
            :disabled="!candidate.importable || importingOpenSsh"
            @update:model-value="toggleOpenSshCandidate(candidate.candidateId, $event)"
          >
            {{ candidate.alias }}
          </NvxCheckbox>
          <span v-if="candidate.endpoint">
            {{ candidate.username ? `${candidate.username}@` : "" }}{{ candidate.endpoint.address }}:{{ candidate.endpoint.port }}
          </span>
          <span v-else>{{ t("sshHosts.opensshImport.invalidEndpoint") }}</span>
          <small v-if="candidate.identityFileHints.length">
            {{ t("sshHosts.opensshImport.identityHints", { count: candidate.identityFileHints.length }) }}
          </small>
          <small>{{ formatOpenSshRoute(candidate.route) }}</small>
          <small
            v-for="diagnostic in candidate.diagnostics"
            :key="`${diagnostic.code}-${diagnostic.line ?? 0}`"
            :class="{ 'hosts-openssh__diagnostic--blocking': diagnostic.blocking }"
          >
            {{ diagnostic.line ? `${t("sshHosts.opensshImport.line", { line: diagnostic.line })} · ` : "" }}{{ diagnostic.message }}
          </small>
        </div>
        <p
          v-if="!opensshPreview.candidates.length"
          class="hosts-openssh__note"
        >
          {{ t("sshHosts.opensshImport.noCandidates") }}
        </p>
        <p
          v-for="diagnostic in opensshPreview.diagnostics"
          :key="`${diagnostic.code}-${diagnostic.line ?? 0}`"
          class="hosts-openssh__note"
        >
          {{ diagnostic.line ? `${t("sshHosts.opensshImport.line", { line: diagnostic.line })} · ` : "" }}{{ diagnostic.message }}
        </p>
      </section>
      <NvxInlineNotice
        v-if="opensshActionFailed"
        tone="error"
        :title="t('sshHosts.opensshImport.failed')"
      />
      <NvxInlineNotice
        v-if="opensshImportedCount"
        :title="t('sshHosts.opensshImport.imported', { count: opensshImportedCount })"
      />
      <template #actions>
        <NvxButton
          variant="ghost"
          :disabled="importingOpenSsh"
          @click="opensshDialogOpen = false"
        >
          {{ t("sshHosts.cancel") }}
        </NvxButton>
        <NvxButton
          :loading="importingOpenSsh"
          :disabled="!opensshPreview || !selectedOpenSshCandidateIds.length"
          @click="importSelectedOpenSshHosts"
        >
          {{ t("sshHosts.opensshImport.importSelected", { count: selectedOpenSshCandidateIds.length }) }}
        </NvxButton>
      </template>
    </NvxDialog>

    <NvxDialog
      v-model="agentDialogOpen"
      plugin-protected
      :title="t('sshHosts.sshAgent.title')"
      :description="t('sshHosts.sshAgent.description')"
      :close-label="t('sshHosts.sshAgent.close')"
      :dismissible="!savingAgentCredential && !loadingAgentKeys"
    >
      <section class="hosts-agent__identity">
        <NvxField :label="t('sshHosts.identity')">
          <NvxSelect
            v-model="agentIdentityId"
            :options="identityOptions"
            :aria-label="t('sshHosts.identity')"
          />
        </NvxField>
        <p>{{ t("sshHosts.sshAgent.identityHint") }}</p>
        <div class="hosts-dialog__create-row">
          <NvxInput
            v-model="newIdentityLabel"
            :placeholder="t('sshHosts.sshAgent.newIdentityLabel')"
          />
          <NvxInput
            v-model="newIdentityUsername"
            :placeholder="t('sshHosts.sshAgent.newIdentityUsername')"
          />
          <NvxButton
            variant="secondary"
            size="sm"
            :loading="creatingIdentity"
            :disabled="!newIdentityLabel.trim()"
            @click="addAgentIdentity"
          >
            {{ t("sshHosts.sshAgent.createIdentity") }}
          </NvxButton>
        </div>
      </section>

      <section class="hosts-agent__keys">
        <div class="hosts-agent__section-heading">
          <div>
            <strong>{{ t("sshHosts.sshAgent.keys") }}</strong>
            <p>{{ t("sshHosts.sshAgent.keysHint") }}</p>
          </div>
          <NvxButton
            variant="secondary"
            size="sm"
            :loading="loadingAgentKeys"
            @click="refreshAgentKeys"
          >
            <NvxIcon
              :icon="RefreshCw"
              :size="16"
            />
            {{ t("sshHosts.sshAgent.readKeys") }}
          </NvxButton>
        </div>
        <div
          v-if="agentKeys.length"
          class="hosts-agent__key-list"
        >
          <button
            v-for="key in agentKeys"
            :key="key.keyHandle"
            type="button"
            class="hosts-agent__key"
            :class="{ 'hosts-agent__key--selected': selectedAgentKeyHandle === key.keyHandle }"
            :aria-pressed="selectedAgentKeyHandle === key.keyHandle"
            @click="selectAgentKey(key)"
          >
            <strong>{{ key.comment || key.publicKeyAlgorithm }}</strong>
            <span>{{ agentIdentityKindLabel(key) }} · {{ key.publicKeyAlgorithm }}</span>
            <code>{{ key.publicKeyFingerprint }}</code>
            <span v-if="key.hardwareKeyApplication">
              {{ t("sshHosts.sshAgent.hardwareApplication", { application: key.hardwareKeyApplication }) }}
            </span>
            <span v-if="key.certificate">
              {{ t("sshHosts.sshAgent.certificateSummary", {
                serial: key.certificate.serial,
                keyId: key.certificate.keyId || t("sshHosts.sshAgent.noKeyId"),
              }) }}
            </span>
            <span v-if="key.certificate">
              {{ t("sshHosts.sshAgent.certificatePrincipals", {
                principals: certificatePrincipalSummary(key),
              }) }}
            </span>
          </button>
        </div>
        <p
          v-else
          class="hosts-agent__empty"
        >
          {{ t("sshHosts.sshAgent.noKeys") }}
        </p>
      </section>

      <div class="hosts-agent__credential-fields">
        <NvxField
          for-id="agent-credential-label"
          :label="t('sshHosts.sshAgent.credentialLabel')"
        >
          <NvxInput
            id="agent-credential-label"
            v-model="agentCredentialLabel"
          />
        </NvxField>
        <NvxField
          for-id="agent-credential-priority"
          :label="t('sshHosts.sshAgent.priority')"
        >
          <NvxInput
            id="agent-credential-priority"
            v-model="agentCredentialPriority"
            type="number"
            :min="0"
            :max="65535"
          />
        </NvxField>
      </div>
      <p class="hosts-agent__note">
        {{ t("sshHosts.sshAgent.persistenceHint") }}
      </p>
      <NvxInlineNotice
        v-if="agentActionFailed"
        tone="error"
        :title="t('sshHosts.sshAgent.failed')"
      />
      <NvxInlineNotice
        v-if="agentCredentialSaved"
        :title="t('sshHosts.sshAgent.saved')"
      />
      <template #actions>
        <NvxButton
          variant="ghost"
          :disabled="savingAgentCredential"
          @click="agentDialogOpen = false"
        >
          {{ t("sshHosts.cancel") }}
        </NvxButton>
        <NvxButton
          :loading="savingAgentCredential"
          :disabled="!agentIdentityId || !selectedAgentKeyHandle || !agentCredentialLabel.trim()"
          @click="saveAgentCredential"
        >
          {{ t("sshHosts.sshAgent.save") }}
        </NvxButton>
      </template>
    </NvxDialog>

    <NvxDialog
      :model-value="privateKeyDialogOpen"
      plugin-protected
      :title="t('sshHosts.privateKeyImport.title')"
      :description="t('sshHosts.privateKeyImport.description')"
      :close-label="t('sshHosts.privateKeyImport.close')"
      :dismissible="!importingPrivateKey"
      @update:model-value="updatePrivateKeyDialogOpen"
    >
      <section class="hosts-agent__identity">
        <NvxField :label="t('sshHosts.identity')">
          <NvxSelect
            v-model="privateKeyIdentityId"
            :options="identityOptions"
            :aria-label="t('sshHosts.identity')"
          />
        </NvxField>
        <p>{{ t('sshHosts.privateKeyImport.identityHint') }}</p>
        <div class="hosts-dialog__create-row">
          <NvxInput
            v-model="newPrivateKeyIdentityLabel"
            :placeholder="t('sshHosts.privateKeyImport.newIdentityLabel')"
          />
          <NvxInput
            v-model="newPrivateKeyIdentityUsername"
            :placeholder="t('sshHosts.privateKeyImport.newIdentityUsername')"
          />
          <NvxButton
            variant="secondary"
            size="sm"
            :loading="creatingPrivateKeyIdentity"
            :disabled="!newPrivateKeyIdentityLabel.trim()"
            @click="addPrivateKeyIdentity"
          >
            {{ t('sshHosts.privateKeyImport.createIdentity') }}
          </NvxButton>
        </div>
      </section>

      <section
        v-if="vaultState !== 'unlocked'"
        class="hosts-dialog__vault-gate"
      >
        <NvxInlineNotice
          :tone="vaultStatusUnavailable ? 'error' : 'info'"
          :title="t(vaultStatusLoading ? 'sshHosts.vault.statusLoading' : vaultStatusUnavailable ? 'sshHosts.vault.statusUnavailable' : vaultState === 'missing' ? 'sshHosts.vault.createRequired' : 'sshHosts.vault.unlockRequired')"
        >
          {{ t('sshHosts.privateKeyImport.vaultGateHint') }}
        </NvxInlineNotice>
        <NvxButton
          v-if="!vaultStatusLoading && !vaultStatusUnavailable"
          variant="secondary"
          @click="openHostVaultDialog"
        >
          {{ t(vaultState === 'missing' ? 'sshHosts.vault.createAction' : 'sshHosts.vault.unlockAction') }}
        </NvxButton>
        <NvxButton
          v-else-if="vaultStatusUnavailable"
          variant="secondary"
          @click="refreshHostVaultState"
        >
          {{ t('sshHosts.vault.retryStatus') }}
        </NvxButton>
      </section>
      <template v-else>
        <div class="hosts-agent__credential-fields">
          <NvxField
            for-id="private-key-credential-label"
            :label="t('sshHosts.privateKeyImport.credentialLabel')"
          >
            <NvxInput
              id="private-key-credential-label"
              v-model="privateKeyCredentialLabel"
            />
          </NvxField>
          <NvxField
            for-id="private-key-credential-priority"
            :label="t('sshHosts.privateKeyImport.priority')"
          >
            <NvxInput
              id="private-key-credential-priority"
              v-model="privateKeyCredentialPriority"
              type="number"
              :min="0"
              :max="65535"
            />
          </NvxField>
        </div>
        <NvxField
          for-id="private-key-passphrase"
          :label="t('sshHosts.privateKeyImport.passphrase')"
        >
          <NvxInput
            id="private-key-passphrase"
            v-model="privateKeyPassphrase"
            type="password"
            autocomplete="new-password"
            :placeholder="t('sshHosts.privateKeyImport.passphrasePlaceholder')"
          />
        </NvxField>
        <p class="hosts-agent__note">
          {{ t('sshHosts.privateKeyImport.persistenceHint') }}
        </p>
        <p class="hosts-agent__note">
          {{ t('sshHosts.privateKeyImport.formatHint') }}
        </p>
      </template>
      <NvxInlineNotice
        v-if="privateKeyActionFailed"
        tone="error"
        :title="t('sshHosts.privateKeyImport.failed')"
      />
      <NvxInlineNotice
        v-if="privateKeyCredentialSaved"
        :title="t('sshHosts.privateKeyImport.saved')"
      />
      <p
        v-if="privateKeyCredentialSummary?.identityId === privateKeyIdentityId
          && privateKeyCredentialSummary.details.kind === 'privateKey'"
        class="hosts-agent__note"
      >
        {{ t('sshHosts.privateKeyImport.importedMetadata', {
          algorithm: privateKeyCredentialSummary.details.publicKeyAlgorithm,
          fingerprint: privateKeyCredentialSummary.details.publicKeyFingerprint,
        }) }}
      </p>
      <template #actions>
        <NvxButton
          variant="ghost"
          :disabled="importingPrivateKey"
          @click="updatePrivateKeyDialogOpen(false)"
        >
          {{ t('sshHosts.cancel') }}
        </NvxButton>
        <NvxButton
          :loading="importingPrivateKey"
          :disabled="vaultState !== 'unlocked' || !privateKeyIdentityId || !privateKeyCredentialLabel.trim()"
          @click="importSelectedPrivateKeyFile"
        >
          <NvxIcon
            :icon="FileInput"
            :size="16"
          />
          {{ t('sshHosts.privateKeyImport.chooseFile') }}
        </NvxButton>
      </template>
    </NvxDialog>

    <NvxDialog
      :model-value="classificationDialogOpen && classificationDeleteCandidate === null"
      :title="t('sshHosts.manageClassification')"
      :description="t('sshHosts.manageClassificationDescription')"
      :close-label="t('sshHosts.closeClassificationManager')"
      :dismissible="!classificationSaving && !classificationCreatingGroup"
      @update:model-value="classificationDialogOpen = $event"
    >
      <section
        v-if="classificationEdit"
        class="hosts-classification-editor"
      >
        <NvxField
          for-id="classification-label"
          :label="t(classificationEdit.kind === 'group' ? 'sshHosts.groupName' : 'sshHosts.tagName')"
        >
          <NvxInput
            id="classification-label"
            v-model="classificationEditLabel"
            data-nvx-dialog-initial-focus
            @keydown.enter.prevent="saveClassification"
          />
        </NvxField>
        <div class="hosts-classification-editor__actions">
          <NvxButton
            variant="ghost"
            @click="classificationEdit = null"
          >
            {{ t("sshHosts.cancel") }}
          </NvxButton>
          <NvxButton
            :loading="classificationSaving"
            :disabled="!classificationEditLabel.trim()"
            @click="saveClassification"
          >
            {{ t("sshHosts.saveClassification") }}
          </NvxButton>
        </div>
      </section>
      <section class="hosts-classification-list">
        <h3>{{ t("sshHosts.groups") }}</h3>
        <div class="hosts-classification-list__create">
          <NvxField
            for-id="classification-new-group-label"
            :label="t('sshHosts.groupName')"
          >
            <NvxInput
              id="classification-new-group-label"
              v-model="classificationNewGroupLabel"
              :placeholder="t('sshHosts.newGroupPlaceholder')"
              :disabled="classificationCreatingGroup"
              @keydown.enter.prevent="addClassificationGroup"
            />
          </NvxField>
          <NvxButton
            variant="secondary"
            :loading="classificationCreatingGroup"
            :disabled="!classificationNewGroupLabel.trim()"
            @click="addClassificationGroup"
          >
            <NvxIcon
              :icon="FolderPlus"
              :size="16"
            />
            {{ t("sshHosts.createGroup") }}
          </NvxButton>
        </div>
        <div v-if="groups.length">
          <div
            v-for="group in groups"
            :key="group.groupId"
            class="hosts-classification-list__row"
          >
            <span>{{ group.label }}</span>
            <NvxButton
              variant="ghost"
              size="sm"
              @click="editClassification('group', group)"
            >
              {{ t("sshHosts.rename") }}
            </NvxButton>
            <NvxButton
              variant="ghost"
              size="sm"
              @click="requestClassificationDelete('group', group)"
            >
              {{ t("sshHosts.delete") }}
            </NvxButton>
          </div>
        </div>
        <p v-else>
          {{ t("sshHosts.noGroups") }}
        </p>
      </section>
      <section class="hosts-classification-list">
        <h3>{{ t("sshHosts.tags") }}</h3>
        <div v-if="tags.length">
          <div
            v-for="tagItem in tags"
            :key="tagItem.tagId"
            class="hosts-classification-list__row"
          >
            <span>{{ tagItem.label }}</span>
            <NvxButton
              variant="ghost"
              size="sm"
              @click="editClassification('tag', tagItem)"
            >
              {{ t("sshHosts.rename") }}
            </NvxButton>
            <NvxButton
              variant="ghost"
              size="sm"
              @click="requestClassificationDelete('tag', tagItem)"
            >
              {{ t("sshHosts.delete") }}
            </NvxButton>
          </div>
        </div>
        <p v-else>
          {{ t("sshHosts.noTags") }}
        </p>
      </section>
      <NvxInlineNotice
        v-if="classificationFailed"
        tone="error"
        :title="t('sshHosts.classificationFailed')"
      />
      <template #actions>
        <NvxButton
          variant="secondary"
          @click="classificationDialogOpen = false"
        >
          {{ t("sshHosts.done") }}
        </NvxButton>
      </template>
    </NvxDialog>

    <NvxDialog
      :model-value="classificationDeleteCandidate !== null"
      :title="t('sshHosts.deleteClassificationTitle')"
      :description="t('sshHosts.deleteClassificationDescription', { label: classificationDeleteCandidate?.label ?? '' })"
      :close-label="t('sshHosts.closeClassificationDelete')"
      :dismissible="!classificationDeleting"
      @update:model-value="(open) => { if (!open) classificationDeleteCandidate = null; }"
    >
      <NvxInlineNotice :title="t('sshHosts.deleteClassificationReferenceTitle')">
        {{ t("sshHosts.deleteClassificationReferenceBody") }}
      </NvxInlineNotice>
      <template #actions>
        <NvxButton
          variant="ghost"
          :disabled="classificationDeleting"
          @click="classificationDeleteCandidate = null"
        >
          {{ t("sshHosts.cancel") }}
        </NvxButton>
        <NvxButton
          variant="danger"
          :loading="classificationDeleting"
          @click="confirmClassificationDelete"
        >
          {{ t("sshHosts.confirmDeleteClassification") }}
        </NvxButton>
      </template>
    </NvxDialog>

    <NvxDialog
      :model-value="deleteCandidate !== null"
      :title="t('sshHosts.deleteDialogTitle')"
      :description="t('sshHosts.deleteDialogDescription', { label: deleteCandidate?.label ?? '' })"
      :close-label="t('sshHosts.closeDeleteDialog')"
      :dismissible="!deleting"
      @update:model-value="(open) => { if (!open) cancelDelete(); }"
    >
      <NvxInlineNotice
        :title="t('sshHosts.deleteKeepsSharedTitle')"
      >
        {{ t("sshHosts.deleteKeepsSharedBody") }}
      </NvxInlineNotice>
      <NvxInlineNotice
        v-if="deleteFailed"
        tone="error"
        :title="t('sshHosts.deleteFailed')"
      />
      <template #actions>
        <NvxButton
          variant="ghost"
          :disabled="deleting"
          @click="cancelDelete"
        >
          {{ t("sshHosts.cancel") }}
        </NvxButton>
        <NvxButton
          variant="danger"
          :loading="deleting"
          @click="confirmDelete"
        >
          {{ t("sshHosts.confirmDelete") }}
        </NvxButton>
      </template>
    </NvxDialog>
  </section>
</template>

<style scoped>
.hosts-page {
  min-height: 100%;
  padding: var(--nvx-space-6);
}

.hosts-page__primary-actions {
  display: flex;
  flex-wrap: wrap;
  gap: var(--nvx-space-2);
  justify-content: flex-end;
}

.hosts-page__primary-action {
  flex: 0 0 auto;
  white-space: nowrap;
}

.hosts-list {
  margin-top: var(--nvx-space-5);
}

.hosts-list__section + .hosts-list__section {
  margin-top: var(--nvx-space-5);
}

.hosts-list__section-header {
  display: flex;
  align-items: center;
  min-height: 40px;
  border-bottom: var(--nvx-border-width) solid var(--nvx-color-border);
}

.hosts-list__section-header h2 {
  margin: 0;
  font-size: var(--nvx-font-size-sm);
  font-weight: var(--nvx-font-weight-semibold);
}

.hosts-list__row {
  display: grid;
  grid-template-columns: 40px minmax(0, 1fr) auto auto;
  gap: var(--nvx-space-4);
  align-items: center;
  min-height: 72px;
  border-bottom: var(--nvx-border-width) solid var(--nvx-color-border);
}

.hosts-list__actions {
  display: flex;
  gap: var(--nvx-space-2);
  align-items: center;
}

.hosts-list__icon {
  display: grid;
  width: 36px;
  height: 36px;
  place-items: center;
  border-radius: var(--nvx-radius-md);
  background: var(--nvx-color-accent-soft);
  color: var(--nvx-color-accent);
}

.hosts-list__identity {
  display: grid;
  min-width: 0;
}

.hosts-list__title-row {
  display: flex;
  min-width: 0;
  gap: var(--nvx-space-2);
  align-items: center;
}

.hosts-list__identity strong,
.hosts-list__endpoint {
  overflow: hidden;
  color: var(--nvx-color-text-secondary);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.hosts-list__identity strong {
  min-width: 0;
  color: var(--nvx-color-text-primary);
}

.hosts-list__classification {
  display: flex;
  flex: 0 1 auto;
  gap: var(--nvx-space-1);
  min-width: 0;
  overflow: hidden;
  white-space: nowrap;
}

.hosts-list__favorite[aria-pressed="true"] {
  color: var(--nvx-color-warning);
}

.hosts-list__favorite-icon--filled {
  fill: currentcolor;
}

.hosts-empty {
  display: grid;
  justify-items: center;
  max-width: 480px;
  margin: 96px auto 0;
  text-align: center;
}

.hosts-empty h2 {
  margin: var(--nvx-space-4) 0 var(--nvx-space-2);
}

.hosts-empty p {
  margin: 0 0 var(--nvx-space-5);
  color: var(--nvx-color-text-secondary);
}

.hosts-dialog__endpoint-row {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 120px;
  gap: var(--nvx-space-3);
}

.hosts-dialog__preference-row {
  display: grid;
  grid-template-columns: minmax(0, 0.8fr) minmax(220px, 1.2fr);
  gap: var(--nvx-space-4);
  align-items: end;
}

.hosts-dialog__preference-row > :first-child {
  min-height: var(--nvx-control-height-md);
}

.hosts-dialog__identity-hint {
  margin: calc(var(--nvx-space-2) * -1) 0 0;
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-sm);
}

.hosts-dialog__vault-gate {
  display: grid;
  justify-items: start;
  gap: var(--nvx-space-3);
}

.hosts-editor {
  display: grid;
  grid-template-columns: 240px minmax(0, 1fr);
  height: 100%;
  min-height: 0;
  margin: calc(var(--nvx-space-4) * -1) calc(var(--nvx-space-5) * -1);
}

.hosts-editor__nav {
  display: grid;
  align-content: start;
  gap: var(--nvx-space-1);
  padding: var(--nvx-space-5) var(--nvx-space-3);
  border-right: var(--nvx-border-width) solid var(--nvx-color-border-subtle);
  background: var(--nvx-color-bg-subtle);
}

.hosts-editor__nav-item {
  display: flex;
  width: 100%;
  min-height: 42px;
  gap: var(--nvx-space-3);
  align-items: center;
  padding: 0 var(--nvx-space-3);
  border: 0;
  border-radius: var(--nvx-radius-md);
  background: transparent;
  color: var(--nvx-color-text-secondary);
  font: inherit;
  font-weight: var(--nvx-font-weight-medium);
  text-align: start;
  cursor: pointer;
}

.hosts-editor__nav-item:hover {
  background: var(--nvx-color-bg-hover);
  color: var(--nvx-color-text-primary);
}

.hosts-editor__nav-item:focus-visible {
  outline: none;
  box-shadow: 0 0 0 var(--nvx-focus-ring-width) var(--nvx-color-focus-ring);
}

.hosts-editor__nav-item--active {
  background: var(--nvx-color-accent-subtle);
  color: var(--nvx-color-accent);
}

.hosts-editor__content {
  display: grid;
  align-content: start;
  gap: var(--nvx-space-4);
  min-width: 0;
  overflow-y: auto;
  padding: var(--nvx-space-6) var(--nvx-space-8);
}

.hosts-editor__content > h3,
.hosts-health__monitoring > h4 {
  margin: 0;
  color: var(--nvx-color-text-primary);
  font-size: var(--nvx-font-size-md);
  font-weight: var(--nvx-font-weight-semibold);
}

.hosts-editor__existing-actions {
  display: flex;
  gap: var(--nvx-space-2);
}

.hosts-health__heartbeat-card {
  display: grid;
  gap: var(--nvx-space-4);
  padding: var(--nvx-space-5);
  border: var(--nvx-border-width) solid var(--nvx-color-border);
  border-radius: var(--nvx-radius-md);
}

.hosts-health__toggle-row {
  display: flex;
  gap: var(--nvx-space-3);
  align-items: center;
}

.hosts-health__three-column-grid {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: var(--nvx-space-4);
}

.hosts-health__monitoring {
  display: grid;
  gap: var(--nvx-space-4);
  padding-top: var(--nvx-space-5);
  border-top: var(--nvx-border-width) solid var(--nvx-color-border-subtle);
}

.hosts-health__resource-grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: var(--nvx-space-4);
}

.hosts-health__resource-option {
  display: grid;
  gap: var(--nvx-space-2);
}

.hosts-health__resource-option > span {
  color: var(--nvx-color-text-primary);
  font-size: var(--nvx-font-size-sm);
  font-weight: var(--nvx-font-weight-medium);
}

.hosts-health__shell-options {
  border-top: var(--nvx-border-width) solid var(--nvx-color-border-subtle);
  color: var(--nvx-color-text-secondary);
}

.hosts-health__shell-options > summary {
  padding-top: var(--nvx-space-4);
  cursor: pointer;
  font-size: var(--nvx-font-size-sm);
  font-weight: var(--nvx-font-weight-medium);
}

.hosts-health__shell-options > summary:focus-visible {
  border-radius: var(--nvx-radius-sm);
  outline: none;
  box-shadow: 0 0 0 var(--nvx-focus-ring-width) var(--nvx-color-focus-ring);
}

.hosts-health__shell-content {
  display: grid;
  gap: var(--nvx-space-4);
  padding-top: var(--nvx-space-4);
}

.hosts-dialog__advanced {
  display: grid;
  gap: var(--nvx-space-2);
  padding-top: var(--nvx-space-2);
}

.hosts-dialog__advanced > h3 {
  margin: 0 0 var(--nvx-space-1);
  font-size: var(--nvx-font-size-body);
  font-weight: var(--nvx-font-weight-semibold);
}

.hosts-dialog__disclosure {
  overflow: clip;
  border: var(--nvx-border-width) solid var(--nvx-color-border);
  border-radius: var(--nvx-radius-md);
  background: var(--nvx-color-bg-surface);
}

.hosts-dialog__disclosure-summary {
  display: flex;
  min-height: 48px;
  gap: var(--nvx-space-3);
  align-items: center;
  justify-content: space-between;
  padding: var(--nvx-space-2) var(--nvx-space-4);
  color: var(--nvx-color-text-primary);
  cursor: pointer;
  list-style: none;
}

.hosts-dialog__disclosure-summary::-webkit-details-marker {
  display: none;
}

.hosts-dialog__disclosure-summary:hover {
  background: var(--nvx-color-bg-hover);
}

.hosts-dialog__disclosure-summary:focus-visible {
  outline: none;
  box-shadow: inset 0 0 0 var(--nvx-focus-ring-width) var(--nvx-color-focus-ring);
}

.hosts-dialog__disclosure-title {
  display: flex;
  min-width: 0;
  gap: var(--nvx-space-3);
  align-items: baseline;
}

.hosts-dialog__disclosure-title strong {
  flex: 0 0 auto;
  font-size: var(--nvx-font-size-body);
  font-weight: var(--nvx-font-weight-semibold);
}

.hosts-dialog__disclosure-title span {
  overflow: hidden;
  color: var(--nvx-color-accent);
  font-size: var(--nvx-font-size-sm);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.hosts-dialog__disclosure-chevron {
  flex: 0 0 auto;
  color: var(--nvx-color-text-secondary);
  transition: transform var(--nvx-motion-fast);
}

.hosts-dialog__disclosure[open] > .hosts-dialog__disclosure-summary {
  border-bottom: var(--nvx-border-width) solid var(--nvx-color-border-subtle);
}

.hosts-dialog__disclosure[open] > .hosts-dialog__disclosure-summary .hosts-dialog__disclosure-chevron {
  transform: rotate(180deg);
}

.hosts-dialog__disclosure > section {
  padding: var(--nvx-space-4);
}

.hosts-dialog__classification-disclosure {
  margin-top: var(--nvx-space-1);
}

.hosts-dialog__classification {
  display: grid;
  gap: var(--nvx-space-3);
}

.hosts-dialog__authentication {
  display: grid;
  gap: var(--nvx-space-2);
}

.hosts-dialog__authentication p,
.hosts-agent__identity p,
.hosts-agent__section-heading p,
.hosts-agent__empty,
.hosts-agent__note {
  margin: 0;
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-sm);
}

.hosts-dialog__create-row {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  gap: var(--nvx-space-2);
  align-items: center;
}

.hosts-dialog__tags {
  display: grid;
  gap: var(--nvx-space-2);
}

.hosts-dialog__section-label {
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-sm);
  font-weight: var(--nvx-font-weight-medium);
}

.hosts-dialog__tag-options {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: var(--nvx-space-2);
  max-height: 152px;
  overflow-y: auto;
}

.hosts-dialog__tag-options :deep(.nvx-checkbox) {
  padding: var(--nvx-space-2);
}

.hosts-dialog__empty-tags,
.hosts-dialog__classification-note {
  margin: 0;
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-sm);
}

.hosts-route__section {
  display: grid;
  gap: var(--nvx-space-3);
}

.hosts-route__section + .hosts-route__section {
  padding-top: var(--nvx-space-4);
  border-top: var(--nvx-border-width) solid var(--nvx-color-border-subtle);
}

.hosts-route__control-grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: var(--nvx-space-3);
  align-items: start;
}

.hosts-route__hint,
.hosts-route__empty {
  margin: 0;
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-sm);
}

.hosts-route__heading {
  display: flex;
  gap: var(--nvx-space-3);
  align-items: center;
  justify-content: space-between;
}

.hosts-route__heading > div {
  display: grid;
  gap: var(--nvx-space-1);
}

.hosts-route__heading h3,
.hosts-route__heading h4 {
  margin: 0;
  font-size: var(--nvx-font-size-md);
}

.hosts-route__heading span {
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-xs);
}

.hosts-route__jump-list {
  display: grid;
  gap: var(--nvx-space-2);
}

.hosts-route__jump-row {
  display: grid;
  grid-template-columns: 28px minmax(0, 1fr) auto auto auto;
  gap: var(--nvx-space-2);
  align-items: center;
}

.hosts-route__jump-index {
  display: grid;
  width: 28px;
  height: 28px;
  place-items: center;
  border-radius: var(--nvx-radius-sm);
  background: var(--nvx-color-bg-subtle);
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-xs);
  font-variant-numeric: tabular-nums;
}

.hosts-algorithms__category {
  display: grid;
  gap: var(--nvx-space-3);
  padding-top: var(--nvx-space-4);
  border-top: var(--nvx-border-width) solid var(--nvx-color-border-subtle);
}

.hosts-algorithms__category h3,
.hosts-algorithms__category h4 {
  margin: 0;
  font-size: var(--nvx-font-size-md);
}

.hosts-algorithms__defaults,
.hosts-algorithms__exceptions,
.hosts-algorithms__unavailable {
  display: grid;
  gap: var(--nvx-space-2);
}

.hosts-algorithms__default,
.hosts-algorithms__exception {
  display: grid;
  grid-template-columns: minmax(220px, 1.2fr) minmax(160px, 1fr);
  gap: var(--nvx-space-3);
  align-items: center;
  min-height: 36px;
}

.hosts-algorithms__exception > span {
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-xs);
  text-align: end;
}

.hosts-algorithms__selector {
  min-width: 0;
  padding: var(--nvx-space-2);
  font-size: var(--nvx-font-size-sm);
}

.hosts-heartbeat {
  display: grid;
  gap: var(--nvx-space-4);
}

.hosts-heartbeat__control-grid {
  display: grid;
  grid-template-columns: minmax(0, 1.35fr) repeat(3, minmax(0, 1fr));
  gap: var(--nvx-space-3);
  align-items: start;
}

.hosts-heartbeat__control-grid--single {
  grid-template-columns: minmax(0, 1fr);
}

.hosts-heartbeat__control-grid :deep(.nvx-field) {
  min-width: 0;
}

.hosts-heartbeat__hint,
.hosts-heartbeat__preview {
  margin: 0;
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-sm);
}

.hosts-heartbeat__preview code {
  overflow-wrap: anywhere;
  color: var(--nvx-color-text-primary);
}

.hosts-monitoring {
  display: grid;
  gap: var(--nvx-space-4);
}

.hosts-monitoring__control-grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: var(--nvx-space-3);
  align-items: start;
}

.hosts-monitoring__hint {
  margin: 0;
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-sm);
}

.hosts-login-automation {
  display: grid;
  grid-template-columns: minmax(0, 1fr);
  min-width: 0;
  gap: var(--nvx-space-4);
  container: login-automation / inline-size;
}

.hosts-login-automation__heading,
.hosts-login-automation__step-heading {
  display: flex;
  flex-wrap: wrap;
  gap: var(--nvx-space-4);
  align-items: center;
  justify-content: space-between;
}

.hosts-login-automation__heading > div,
.hosts-login-automation__step-heading > div {
  display: grid;
  gap: var(--nvx-space-1);
}

.hosts-login-automation__heading p,
.hosts-login-automation__step-heading span,
.hosts-login-automation__step-hint,
.hosts-login-automation__empty {
  margin: 0;
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-sm);
}

.hosts-login-automation__enable {
  flex: 0 0 auto;
  padding: var(--nvx-space-2);
  white-space: nowrap;
}

.hosts-login-automation__step {
  display: grid;
  grid-template-columns: minmax(0, 1fr);
  gap: var(--nvx-space-3);
  padding: var(--nvx-space-4);
  border: var(--nvx-border-width) solid var(--nvx-color-border);
  border-radius: var(--nvx-radius-md);
}

.hosts-login-automation__controls {
  display: grid;
  grid-template-columns: 28px minmax(180px, 1fr) minmax(120px, 0.45fr) auto;
  gap: var(--nvx-space-3);
  align-items: end;
}

.hosts-login-automation__index {
  display: grid;
  width: 28px;
  height: 36px;
  place-items: center;
  color: var(--nvx-color-text-secondary);
  font-variant-numeric: tabular-nums;
}

.hosts-login-automation__reorder {
  display: flex;
  gap: var(--nvx-space-1);
  align-items: center;
  min-height: 36px;
}

.hosts-login-automation__secret-grid {
  display: grid;
  grid-template-columns: minmax(0, 0.7fr) minmax(0, 1.3fr);
  gap: var(--nvx-space-3);
  align-items: start;
}

@container login-automation (max-width: 540px) {
  .hosts-login-automation__controls {
    grid-template-columns: minmax(0, 1fr) 112px;
  }

  .hosts-login-automation__reorder {
    grid-column: 1 / -1;
  }

  .hosts-login-automation__secret-grid {
    grid-template-columns: minmax(0, 1fr);
  }

  .hosts-login-automation__index {
    display: none;
  }
}

.hosts-algorithms__default code,
.hosts-algorithms__exception code {
  overflow-wrap: anywhere;
}

.hosts-agent__identity,
.hosts-agent__keys {
  display: grid;
  gap: var(--nvx-space-3);
}

.hosts-agent__identity .hosts-dialog__create-row {
  grid-template-columns: minmax(0, 1fr) minmax(0, 1fr) auto;
}

.hosts-agent__keys {
  padding-top: var(--nvx-space-3);
  border-top: var(--nvx-border-width) solid var(--nvx-color-border-subtle);
}

.hosts-agent__section-heading {
  display: flex;
  gap: var(--nvx-space-3);
  align-items: flex-start;
  justify-content: space-between;
}

.hosts-agent__section-heading > div {
  display: grid;
  gap: var(--nvx-space-1);
}

.hosts-agent__key-list {
  display: grid;
  gap: var(--nvx-space-2);
  max-height: 240px;
  overflow-y: auto;
}

.hosts-agent__key {
  display: grid;
  gap: var(--nvx-space-1);
  width: 100%;
  padding: var(--nvx-space-3);
  border: var(--nvx-border-width) solid var(--nvx-color-border);
  border-radius: var(--nvx-radius-md);
  background: var(--nvx-color-bg-surface);
  color: var(--nvx-color-text-primary);
  text-align: left;
  cursor: pointer;
}

.hosts-agent__key:hover {
  background: var(--nvx-color-bg-hover);
}

.hosts-agent__key:focus-visible {
  outline: none;
  box-shadow: 0 0 0 var(--nvx-focus-ring-width) var(--nvx-color-focus-ring);
}

.hosts-agent__key--selected {
  border-color: var(--nvx-color-accent);
  background: var(--nvx-color-accent-soft);
}

.hosts-agent__key span,
.hosts-agent__key code {
  overflow: hidden;
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-xs);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.hosts-agent__credential-fields {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 120px;
  gap: var(--nvx-space-3);
}

.hosts-keyboard-interactive {
  display: grid;
  gap: var(--nvx-space-4);
}

.hosts-keyboard-interactive__limits {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: var(--nvx-space-3);
  align-items: end;
}

.hosts-keyboard-interactive > p {
  margin: 0;
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-sm);
}

.hosts-openssh__note {
  margin: 0;
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-sm);
}

.hosts-openssh__preview {
  display: grid;
  gap: var(--nvx-space-2);
  max-height: 320px;
  padding-top: var(--nvx-space-3);
  overflow-y: auto;
  border-top: var(--nvx-border-width) solid var(--nvx-color-border-subtle);
}

.hosts-openssh__candidate {
  display: grid;
  gap: var(--nvx-space-1);
  padding: var(--nvx-space-3);
  border: var(--nvx-border-width) solid var(--nvx-color-border);
  border-radius: var(--nvx-radius-md);
}

.hosts-openssh__candidate > span,
.hosts-openssh__candidate > small {
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-xs);
}

.hosts-openssh__candidate .hosts-openssh__diagnostic--blocking {
  color: var(--nvx-color-danger);
}

.hosts-classification-editor,
.hosts-classification-list {
  display: grid;
  gap: var(--nvx-space-3);
}

.hosts-classification-editor {
  padding-bottom: var(--nvx-space-4);
  border-bottom: var(--nvx-border-width) solid var(--nvx-color-border-subtle);
}

.hosts-classification-editor__actions {
  display: flex;
  gap: var(--nvx-space-2);
  justify-content: flex-end;
}

.hosts-classification-list h3,
.hosts-classification-list p {
  margin: 0;
}

.hosts-classification-list__create {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  gap: var(--nvx-space-2);
  align-items: end;
}

.hosts-classification-list p {
  color: var(--nvx-color-text-secondary);
}

.hosts-classification-list__row {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto auto;
  gap: var(--nvx-space-2);
  align-items: center;
  min-height: 40px;
  border-bottom: var(--nvx-border-width) solid var(--nvx-color-border-subtle);
}

@media (max-width: 860px) {
  .hosts-editor {
    grid-template-columns: minmax(0, 1fr);
  }

  .hosts-editor__nav {
    grid-auto-flow: column;
    grid-auto-columns: max-content;
    overflow-x: auto;
    border-right: 0;
    border-bottom: var(--nvx-border-width) solid var(--nvx-color-border-subtle);
  }

  .hosts-editor__content {
    padding: var(--nvx-space-5);
  }

  .hosts-list__row {
    grid-template-columns: 40px minmax(0, 1fr);
  }

  .hosts-list__row > .nvx-status-label,
  .hosts-list__actions {
    grid-column: 2;
  }

  .hosts-list__actions {
    flex-wrap: wrap;
    padding-bottom: var(--nvx-space-3);
  }

  .hosts-agent__identity .hosts-dialog__create-row,
  .hosts-agent__credential-fields,
  .hosts-keyboard-interactive__limits,
  .hosts-dialog__endpoint-row,
  .hosts-dialog__preference-row,
  .hosts-route__control-grid,
  .hosts-heartbeat__control-grid,
  .hosts-health__three-column-grid,
  .hosts-health__resource-grid,
  .hosts-monitoring__control-grid {
    grid-template-columns: minmax(0, 1fr);
  }

  .hosts-algorithms__default,
  .hosts-algorithms__exception {
    grid-template-columns: minmax(0, 1fr);
  }

  .hosts-algorithms__exception > span {
    text-align: start;
  }

  .hosts-dialog__disclosure-title {
    display: grid;
    gap: 0;
  }

}
</style>
