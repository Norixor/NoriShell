<script setup lang="ts">
import { ArrowDown, ArrowUp, FileInput, Folder, FolderPlus, HeartPulse, KeyRound, Network, Plus, Server, ShieldCheck, Tag, Trash2, Workflow, X } from "lucide-vue-next";
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import NvxHostMarkerEditor from "./NvxHostMarkerEditor.vue";
import { normalizeHostMarker, type HostMarker } from "../../host-markers";
import { useHostMarkersStore } from "../../stores/hostMarkers";
import { NvxButton, NvxCheckbox, NvxDialog, NvxField, NvxIcon, NvxIconButton, NvxInlineNotice, NvxInput, NvxSelect, NvxStatusLabel, NvxTextarea } from "../ui";
import { canUseDesktopCore, cancelHostCreatePassword, cancelLoginAutomationSecret, createConfiguredHost, createHostGroup, createIdentity, createLoginAutomationSecret, createHostTag, getHostConnectionConfig, getAlgorithmPolicyCatalog, fetchVaultStatus, importPrivateKeyFile, listHostCatalog, listHostGroups, listHostTags, listIdentities, listCredentialRefs, replaceHostOrganization, replaceAlgorithmPolicy, replaceHeartbeatPolicy, replaceMonitoringPolicy, replaceLoginAutomation, replaceRoutePlan, stageHostCreatePassword, confirmLoginAutomation, testSshConnection, updateHost, parseCoreApiError, prepareTransientCredential } from "../../core-api/client";
import type { AlgorithmCategory, AlgorithmCompatibilityException, AlgorithmPolicyCatalog, CredentialRefSummary, HostCatalogEntry, HostCreateLoginAutomationStep, HostGroupSummary, HostSummary, HostTagSummary, HeartbeatPolicy, IdentitySummary, LoginAutomationStepInput, LoginAutomationStepSummary, MonitoringPolicy, RouteIngress, ShellHeartbeatLineEnding, VaultState } from "../../core-api/generated/core-api";
import { createUuidV7 } from "../../core-api/ids";
import { useTipsStore } from "../../stores/tips";
import { requestSecureVault } from "../../core-api/secure-vault-client";

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


const catalog = ref<HostCatalogEntry[]>([]);

const groups = ref<HostGroupSummary[]>([]);

const tags = ref<HostTagSummary[]>([]);

const identities = ref<IdentitySummary[]>([]);

const loading = ref(false);

const loadFailed = ref(false);


const hostEditorSection = ref<HostEditorSection>("connection");

const saving = ref(false);

const testingConnection = ref(false);

const saveFailed = ref(false);

const saveFailureKey = ref("sshHosts.saveFailed");

const editingHost = ref<HostSummary | null>(null);

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

const vaultSubmitting = ref(false);

const favorite = ref(false);

const groupId = ref("");

const selectedTagIds = ref<string[]>([]);

const newGroupLabel = ref("");

const newTagLabel = ref("");

const creatingGroup = ref(false);

const creatingTag = ref(false);



const privateKeyDialogOpen = ref(false);

const privateKeyIdentityId = ref("");

const newPrivateKeyIdentityLabel = ref("");

const newPrivateKeyIdentityUsername = ref("");

const creatingPrivateKeyIdentity = ref(false);

const privateKeyCredentialLabel = ref("");

const privateKeyCredentialPriority = ref("100");

const privateKeyPassphrase = ref("");

const importingPrivateKey = ref(false);

const privateKeyValidationFailed = ref(false);

const privateKeyCredentialSummary = ref<CredentialRefSummary | null>(null);

const privateKeyHostEditorBinding = ref(false);

const privateKeyImportOperationId = ref<string | null>(null);

const privateKeyImportIdempotencyKey = ref<string | null>(null);

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

const monitoringSelectionTouched = ref(false);

const monitoringIntervalSeconds = ref("1.5");

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

watch([authenticationMode, hostPassword, stagedHostPasswordId, identityId, routeCredentials], () => {
  if (editingHost.value || monitoringSelectionTouched.value) return;
  const hasSavedPassword = authenticationMode.value === "savedIdentity"
    && routeCredentials.value.some((credential) => credential.identityId === identityId.value);
  monitoringEnabled.value = (authenticationMode.value === "password"
    && Boolean(hostPassword.value || stagedHostPasswordId.value)) || hasSavedPassword;
});

function setMonitoringEnabled(enabled: boolean) {
  monitoringSelectionTouched.value = true;
  monitoringEnabled.value = enabled;
}

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

async function openCreate() {
  monitoringSelectionTouched.value = false;
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
  await loadCreateAdvancedConfig();
  void refreshHostVaultState();
}

async function openEdit(host: HostSummary, initialSection: HostEditorSection = "connection") {
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
  await loadSavedAdvancedConfig(host);
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
  const sampleIntervalMillis = Math.round(sampleIntervalSeconds * 1000);
  const sampleTimeoutMillis = Math.round(sampleTimeoutSeconds * 1000);
  if (
    !Number.isFinite(sampleIntervalSeconds)
    || Math.abs(sampleIntervalMillis / 1000 - sampleIntervalSeconds) > 1e-9
    || sampleIntervalSeconds < 1.5
    || sampleIntervalSeconds > 300
    || !Number.isFinite(sampleTimeoutSeconds)
    || Math.abs(sampleTimeoutMillis / 1000 - sampleTimeoutSeconds) > 1e-9
    || sampleTimeoutSeconds < 0.5
    || sampleTimeoutSeconds > 30
  ) {
    return null;
  }
  return {
    enabled: monitoringEnabled.value,
    sampleIntervalMillis,
    sampleTimeoutMillis,
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
  monitoringIntervalSeconds.value = "1.5";
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
    monitoringIntervalSeconds.value = String(config.monitoringPolicy.policy.sampleIntervalMillis / 1000);
    monitoringTimeoutSeconds.value = String(config.monitoringPolicy.policy.sampleTimeoutMillis / 1000);
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

onBeforeUnmount(() => {
  hostPassword.value = "";
  privateKeyPassphrase.value = "";
  for (const step of loginAutomationSteps.value) {
    step.secretValue = "";
    if (step.stagedSecretId) void cancelStagedLoginAutomationSecret(step);
  }
  if (stagedHostPasswordId.value) void cancelStagedHostPassword();
});

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

async function saveHost() {
  if (busy.value || !initialized.value) return;
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
    dirty.value = false;
    emit("saved", workingHost.hostId);
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

function openPrivateKeyImport(selectedIdentityId = "", bindToHostEditor = false) {
  privateKeyIdentityId.value = selectedIdentityId || (identities.value[0]?.identityId ?? "");
  privateKeyHostEditorBinding.value = bindToHostEditor;
  newPrivateKeyIdentityLabel.value = "";
  newPrivateKeyIdentityUsername.value = "";
  privateKeyCredentialLabel.value = t("sshHosts.privateKeyImport.defaultCredentialLabel");
  privateKeyCredentialPriority.value = "100";
  privateKeyPassphrase.value = "";
  privateKeyValidationFailed.value = false;
  tips.dismissScope("host-editor-private-key");
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
  privateKeyValidationFailed.value = false;
  tips.dismissScope("host-editor-private-key");
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
    tips.show({
      scope: "host-editor-private-key",
      tone: "error",
      title: t("sshHosts.privateKeyImport.failed"),
    });
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
    privateKeyValidationFailed.value = true;
    return;
  }
  importingPrivateKey.value = true;
  privateKeyValidationFailed.value = false;
  tips.dismissScope("host-editor-private-key");
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
    privateKeyCredentialSummary.value = imported;
    if (privateKeyHostEditorBinding.value) {
      identityId.value = privateKeyIdentityId.value;
      authenticationMode.value = editingHost.value ? "savedIdentity" : "privateKey";
    }
    privateKeyImportOperationId.value = null;
    privateKeyImportIdempotencyKey.value = null;
    await refresh();
    tips.show({
      scope: "host-editor-private-key",
      tone: "success",
      title: t("sshHosts.privateKeyImport.saved"),
    });
  } catch {
    privateKeyPassphrase.value = "";
    tips.show({
      scope: "host-editor-private-key",
      tone: "error",
      title: t("sshHosts.privateKeyImport.failed"),
    });
  } finally {
    importingPrivateKey.value = false;
  }
}

watch(privateKeyIdentityId, () => {
  if (!importingPrivateKey.value) void suggestPrivateKeyPriority();
});

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
    tips.show({
      scope: "host-editor-classification",
      tone: "error",
      title: t("sshHosts.saveFailed"),
    });
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
    tips.show({
      scope: "host-editor-classification",
      tone: "error",
      title: t("sshHosts.saveFailed"),
    });
  } finally {
    creatingTag.value = false;
  }
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

const props = defineProps<{ hostId?: string; initialSection?: HostEditorSection }>();

const emit = defineEmits<{ saved: [hostId: string]; cancel: []; closeCancelled: [] }>();

const initialized = ref(false);

const dirty = ref(false);

const discardDialogOpen = ref(false);

const closing = ref(false);

const busy = computed(() => loading.value || hostConfigLoading.value || saving.value || testingConnection.value || importingPrivateKey.value || creatingPrivateKeyIdentity.value || creatingGroup.value || creatingTag.value || vaultSubmitting.value || closing.value);

async function openHostVaultDialog() {
  if (vaultSubmitting.value) return;
  vaultSubmitting.value = true;
  try { await requestSecureVault("ensureUnlocked"); await refreshHostVaultState(); }
  catch { vaultStatusUnavailable.value = true; }
  finally { vaultSubmitting.value = false; }
}

async function discardAndClose(emitCancellation = true): Promise<boolean> {
  if (busy.value) return false;
  closing.value = true;
  try {
    if (!await cancelStagedHostPassword()) return false;
    for (const step of loginAutomationSteps.value) {
      if (!await cancelStagedLoginAutomationSecret(step)) return false;
      step.secretValue = "";
    }
    hostPassword.value = "";
    privateKeyPassphrase.value = "";
    discardDialogOpen.value = false;
    dirty.value = false;
    if (emitCancellation) emit("cancel");
    return true;
  } finally { closing.value = false; }
}

async function requestCloseHostEditor(): Promise<boolean> {
  if (busy.value) return false;
  if (dirty.value) { discardDialogOpen.value = true; return false; }
  return discardAndClose(false);
}

function keepEditing() {
  if (closing.value) return;
  discardDialogOpen.value = false;
  emit("closeCancelled");
}

async function cancelEditor() {
  if (await requestCloseHostEditor()) emit("cancel");
}

watch([label, address, port, username, identityId, authenticationMode, hostPassword, favorite, groupId, selectedTagIds, hostMarkerDraft, routeIngressKind, routeProxyAddress, routeProxyPort, routeProxyDnsMode, routeProxyCredentialRefId, routeJumpHostIds, algorithmSelectedExceptionIds, loginAutomationEnabled, loginAutomationSteps, loginAutomationConfirmOnSave, heartbeatMode, heartbeatIntervalSeconds, heartbeatReplyTimeoutSeconds, heartbeatFailureThreshold, heartbeatPayloadText, heartbeatLineEnding, heartbeatUserIdleSeconds, monitoringEnabled, monitoringIntervalSeconds, monitoringTimeoutSeconds], () => {
  if (initialized.value) dirty.value = true;
}, { deep: true, flush: "sync" });

defineExpose({ requestClose: requestCloseHostEditor, dirty, busy });

onMounted(async () => {
  await refresh();
  if (loadFailed.value) return;
  if (props.hostId) {
    const host = catalog.value.find(entry => entry.host.hostId === props.hostId)?.host;
    if (!host) { hostConfigLoadFailed.value = true; return; }
    await openEdit(host, props.initialSection ?? "connection");
  } else { await openCreate(); }
  initialized.value = true;
});
</script>
<template>
  <section class="host-editor-surface">
    <NvxInlineNotice
      v-if="!initialized && !loadFailed && !hostConfigLoadFailed"
      :title="t('sshHosts.advanced.loading')"
    />
    <NvxInlineNotice
      v-if="loadFailed || hostConfigLoadFailed"
      tone="error"
      :title="t('sshHosts.advanced.loadFailed')"
    />
    <div
      v-if="initialized"
      class="hosts-editor"
      :inert="busy"
    >
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
              id="host-identity"
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
                :model-value="monitoringEnabled"
                @update:model-value="setMonitoringEnabled"
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
                    :min="1.5"
                    :max="300"
                    :step="0.001"
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
                    :min="0.5"
                    :max="30"
                    :step="0.001"
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
    <footer class="host-editor-actions">
      <NvxButton
        variant="ghost"
        :disabled="busy"
        @click="cancelEditor"
      >
        {{ t("sshHosts.cancel") }}
      </NvxButton>
      <NvxButton
        variant="secondary"
        :loading="testingConnection"
        :disabled="busy"
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
        :disabled="!initialized || busy || hostConfigLoading || hostConfigLoadFailed || hasSelectedUnavailableAlgorithm"
        @click="saveHost"
      >
        {{ t(editingHost ? "sshHosts.saveChanges" : "sshHosts.save") }}
      </NvxButton>
    </footer>
  </section>
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
      v-if="privateKeyValidationFailed"
      tone="error"
      :title="t('sshHosts.privateKeyImport.failed')"
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
    :model-value="discardDialogOpen"
    :title="t('toolWindows.unsavedTitle')"
    :description="t('toolWindows.unsavedDescription')"
    :close-label="t('sshHosts.cancel')"
    :dismissible="!closing"
    @update:model-value="open => { if (!open) keepEditing(); }"
  >
    <template #actions>
      <NvxButton
        variant="ghost"
        :disabled="closing"
        @click="keepEditing"
      >
        {{ t('toolWindows.keepEditing') }}
      </NvxButton>
      <NvxButton
        variant="danger"
        :loading="closing"
        @click="discardAndClose()"
      >
        {{ t('toolWindows.discard') }}
      </NvxButton>
    </template>
  </NvxDialog>
</template>
<style scoped>
.host-editor-surface { display: flex; flex-direction: column; height: 100%; min-height: 0; }
.host-editor-surface > .hosts-editor { flex: 1; height: auto; }
.host-editor-actions { display: flex; justify-content: flex-end; gap: var(--nvx-space-2); padding: var(--nvx-space-3) var(--nvx-space-5); border-top: var(--nvx-border-width) solid var(--nvx-color-border); }

.hosts-dialog__endpoint-row {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 120px;
  gap: var(--nvx-space-3);
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
  grid-template-columns: 224px minmax(0, 1fr);
  height: 100%;
  min-height: 0;
  overflow: hidden;
}

.hosts-editor__nav {
  display: grid;
  align-content: start;
  gap: var(--nvx-space-1);
  padding: var(--nvx-space-4) var(--nvx-space-3);
  border-right: var(--nvx-border-width) solid var(--nvx-color-border-subtle);
  background: var(--nvx-color-bg-subtle);
}

.hosts-editor__nav-item {
  display: flex;
  width: 100%;
  min-height: 38px;
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
  gap: var(--nvx-space-3);
  min-width: 0;
  overflow-y: auto;
  padding: var(--nvx-space-4) var(--nvx-space-5);
}

.hosts-editor__content > h3,
.hosts-health__monitoring > h4 {
  margin: 0;
  color: var(--nvx-color-text-primary);
  font-size: var(--nvx-font-size-md);
  font-weight: var(--nvx-font-weight-semibold);
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

.hosts-health__shell-content {
  display: grid;
  gap: var(--nvx-space-4);
  padding-top: var(--nvx-space-4);
}

.hosts-dialog__classification {
  display: grid;
  gap: var(--nvx-space-3);
}

.hosts-agent__identity p,
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
.hosts-algorithms__exceptions {
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

.hosts-agent__identity {
  display: grid;
  gap: var(--nvx-space-3);
}

.hosts-agent__identity .hosts-dialog__create-row {
  grid-template-columns: minmax(0, 1fr) minmax(0, 1fr) auto;
}

.hosts-agent__credential-fields {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 120px;
  gap: var(--nvx-space-3);
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
    padding: var(--nvx-space-3) var(--nvx-space-4);
  }

  .hosts-agent__identity .hosts-dialog__create-row,
  .hosts-agent__credential-fields,
  .hosts-dialog__endpoint-row,
  .hosts-route__control-grid,
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

}
</style>
