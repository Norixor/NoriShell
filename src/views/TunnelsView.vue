<script setup lang="ts">
import { ArrowRight, Copy, Network, Pencil, Play, Plus, RefreshCw, Search, ShieldAlert, Square, Trash2, X } from "lucide-vue-next";
import { computed, onBeforeUnmount, onMounted, reactive, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";

import { NvxPageHeader } from "../components/layout";
import { NvxPluginExtensionTarget } from "../components/plugins";
import { NvxButton, NvxDialog, NvxField, NvxIcon, NvxInlineNotice, NvxInput, NvxSelect, NvxStatusLabel } from "../components/ui";
import {
  canUseDesktopCore, createForwardRule, deleteForwardRule, fetchForwardSessionSnapshot,
  listForwardRules, listHosts, preflightForwardRule, retainForwardCleanupForExitOnce,
  startForwardSession, stopForwardSession, updateForwardRule,
} from "../core-api/client";
import { ensureHostVault } from "../core-api/secure-vault-client";
import type {
  ForwardFailureCode, ForwardRulePreflightResponse, ForwardRuleSummary, ForwardSessionEvent,
  ForwardSessionState, ForwardSessionSummary, HostSummary, PortForwardRule,
} from "../core-api/generated/core-api";
import { useTipsStore } from "../stores/tips";
import { useRouteReveal } from "../routeReveal";
import { buildForwardRule, ruleListener, ruleTarget, ruleToDraft, type ForwardKind, type ForwardRuleDraft } from "../tunnels/forward-rule";
import {
  createTunnelVisualFixtureSession,
  preflightTunnelVisualFixtureRule,
  saveTunnelVisualFixtureRule,
  stopTunnelVisualFixtureSession,
  tunnelVisualFixture,
} from "../tunnels/visual-fixture";

type StateFilter = "all" | "running" | "failed" | "stopped";
interface TunnelRow {
  key: string;
  label: string;
  rule: PortForwardRule;
  savedRule: ForwardRuleSummary | null;
  session: ForwardSessionSummary | null;
  state: ForwardSessionState | "saved";
}

const { t } = useI18n();
const router = useRouter();
const tips = useTipsStore();
const revealRoute = useRouteReveal();
const tipScope = "tunnels";
const hosts = ref<HostSummary[]>([]);
const savedRules = ref<ForwardRuleSummary[]>([]);
const sessions = ref<ForwardSessionSummary[]>([]);
const loading = ref(true);
const mutating = ref(false);
const search = ref("");
const hostFilter = ref("all");
const kindFilter = ref<"all" | ForwardKind>("all");
const stateFilter = ref<StateFilter>("all");
const selectedKey = ref<string | null>(null);
const editorOpen = ref(true);
const editingRuleId = ref<string | null>(null);
const ruleLabel = ref("");
const draft = reactive<ForwardRuleDraft>({ hostId: "", kind: "local", bindAddress: "127.0.0.1", listenPort: "8022", targetHost: "127.0.0.1", targetPort: "22" });
const preflight = ref<ForwardRulePreflightResponse | null>(null);
const preflighting = ref(false);
const cleanupPendingSessionId = ref<string | null>(null);
const cleanupRetainTarget = ref<ForwardSessionSummary | null>(null);
const deleteTarget = ref<ForwardRuleSummary | null>(null);
const retainedForExit = ref(new Set<string>());
const visualFixtureMode = ref(false);
let refreshTimer: number | null = null;
let disposed = false;
const reportedSessionFailures = new Set<string>();

const hostMap = computed(() => new Map(hosts.value.map((host) => [host.hostId, host])));
const hostOptions = computed(() => hosts.value.map((host) => ({ value: host.hostId, label: `${host.label} · ${host.normalizedAddress}:${host.port}` })));
const hostFilterOptions = computed(() => [{ value: "all", label: t("tunnels.filters.allHosts") }, ...hostOptions.value]);
const kindOptions = computed(() => ([
  { value: "local", label: t("tunnels.kinds.local"), description: t("tunnels.kindDescriptions.local") },
  { value: "remote", label: t("tunnels.kinds.remote"), description: t("tunnels.kindDescriptions.remote") },
  { value: "dynamic", label: t("tunnels.kinds.dynamic"), description: t("tunnels.kindDescriptions.dynamic") },
]));
const kindFilterOptions = computed(() => [{ value: "all", label: t("tunnels.filters.allKinds") }, ...kindOptions.value]);
const stateFilterOptions = computed(() => ([
  { value: "all", label: t("tunnels.filters.allStates") },
  { value: "running", label: t("tunnels.states.running") },
  { value: "failed", label: t("tunnels.states.failed") },
  { value: "stopped", label: t("tunnels.states.saved") },
]));
const draftRule = computed(() => buildForwardRule(draft));
const flowTitle = computed(() => t(`tunnels.flowTitles.${draft.kind}`));
const flowDescription = computed(() => t(`tunnels.flowDescriptions.${draft.kind}`, {
  listen: `${draft.bindAddress || "127.0.0.1"}:${draft.listenPort || "—"}`,
  target: draft.kind === "dynamic" ? t("tunnels.dynamicTarget") : `${draft.targetHost || "—"}:${draft.targetPort || "—"}`,
}));

const rows = computed<TunnelRow[]>(() => {
  const rulesById = new Map(savedRules.value.map((rule) => [rule.ruleId, rule]));
  const sessionRows = sessions.value.map((session) => {
    const savedRule = session.ruleId ? rulesById.get(session.ruleId) ?? null : null;
    return {
      key: `session:${session.sessionId}`,
      label: savedRule?.label ?? t("tunnels.unsavedSession"),
      rule: session.ruleSnapshot,
      savedRule,
      session,
      state: session.state,
    } satisfies TunnelRow;
  });
  const activeRuleIds = new Set(sessions.value.map((session) => session.ruleId).filter(Boolean));
  const dormant = savedRules.value.filter((rule) => !activeRuleIds.has(rule.ruleId)).map((rule) => ({
    key: `rule:${rule.ruleId}`, label: rule.label, rule: rule.rule, savedRule: rule,
    session: null, state: "saved" as const,
  }));
  return [...sessionRows, ...dormant];
});

const filteredRows = computed(() => {
  const needle = search.value.trim().toLocaleLowerCase();
  return rows.value.filter((row) => {
    const host = hostMap.value.get(row.rule.hostId);
    const state = row.state === "saved" || row.state === "stopped" ? "stopped" : row.state;
    const haystack = [row.label, host?.label, host?.normalizedAddress, ruleListener(row.rule), ruleTarget(row.rule)].filter(Boolean).join(" ").toLocaleLowerCase();
    return (!needle || haystack.includes(needle))
      && (hostFilter.value === "all" || row.rule.hostId === hostFilter.value)
      && (kindFilter.value === "all" || row.rule.kind === kindFilter.value)
      && (stateFilter.value === "all" || state === stateFilter.value);
  });
});
const selectedRow = computed(() => rows.value.find((row) => row.key === selectedKey.value) ?? filteredRows.value[0] ?? null);

watch(filteredRows, (next) => {
  if (!next.some((row) => row.key === selectedKey.value)) selectedKey.value = next[0]?.key ?? null;
}, { immediate: true });
watch(() => [draft.hostId, draft.kind, draft.bindAddress, draft.listenPort, draft.targetHost, draft.targetPort], () => { preflight.value = null; });
watch(() => draft.kind, (kind, previous) => {
  if (kind !== "remote") draft.bindAddress = "127.0.0.1";
  if (kind === "dynamic") { draft.targetHost = ""; draft.targetPort = ""; }
  else if (previous === "dynamic") { draft.targetHost = "127.0.0.1"; draft.targetPort = kind === "local" ? "22" : "8080"; }
});

function stateTone(state: TunnelRow["state"]) {
  if (state === "running") return "success" as const;
  if (state === "failed") return "danger" as const;
  if (state === "starting" || state === "stopping") return "info" as const;
  return "neutral" as const;
}
function stateLabel(state: TunnelRow["state"]) { return t(`tunnels.states.${state}`); }
function kindLabel(kind: ForwardKind) { return t(`tunnels.kinds.${kind}`); }
function hostLabel(hostId: string) { return hostMap.value.get(hostId)?.label ?? hostId; }
function failureLabel(code: ForwardFailureCode) { return t(`tunnels.failures.${code}`); }
function reportSessionFailure(session: ForwardSessionSummary) {
  const failure = session.failure;
  if (!failure) return;
  const key = `${session.sessionId}:${session.generation}:${failure.code}`;
  if (reportedSessionFailures.has(key)) return;
  reportedSessionFailures.add(key);
  tips.show({
    scope: `tunnels-session:${key}`,
    tone: "error",
    title: failureLabel(failure.code),
    message: t("tunnels.sessionFailed", { stage: failure.stage }),
  });
}
function bindLabel(session: ForwardSessionSummary | null, rule: PortForwardRule) {
  return session?.actualBind ? `${session.actualBind.address}:${session.actualBind.port}` : ruleListener(rule);
}
function formatBytes(value: string) {
  const bytes = Number(value);
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 ** 2) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 ** 3) return `${(bytes / 1024 ** 2).toFixed(1)} MB`;
  return `${(bytes / 1024 ** 3).toFixed(1)} GB`;
}
function uptime(session: ForwardSessionSummary) {
  const seconds = Math.max(0, Math.floor((Date.now() - Number(session.startedAtUnixMs)) / 1000));
  if (seconds < 60) return t("tunnels.uptime.seconds", { value: seconds });
  if (seconds < 3600) return t("tunnels.uptime.minutes", { value: Math.floor(seconds / 60) });
  return t("tunnels.uptime.hours", { value: Math.floor(seconds / 3600) });
}
function applyEvent(event: ForwardSessionEvent) {
  const index = sessions.value.findIndex((session) => session.sessionId === event.session.sessionId);
  if (index < 0) sessions.value.push(event.session); else sessions.value.splice(index, 1, event.session);
  reportSessionFailure(event.session);
  selectedKey.value = `session:${event.session.sessionId}`;
}
function replaceSavedRule(rule: ForwardRuleSummary) {
  const index = savedRules.value.findIndex((candidate) => candidate.ruleId === rule.ruleId);
  if (index < 0) savedRules.value.push(rule); else savedRules.value.splice(index, 1, rule);
}
function clearFeedback() { tips.dismissScope(tipScope); }
function showOperationError(title = t("tunnels.operationFailed")) {
  tips.show({ scope: tipScope, tone: "error", title });
}
function showOperationMessage(title: string) {
  tips.show({ scope: tipScope, tone: "info", title });
}
function resetEditor() {
  editingRuleId.value = null; ruleLabel.value = "";
  Object.assign(draft, { hostId: hosts.value[0]?.hostId ?? "", kind: "local", bindAddress: "127.0.0.1", listenPort: "8022", targetHost: "127.0.0.1", targetPort: "22" } satisfies ForwardRuleDraft);
  preflight.value = null; editorOpen.value = true;
}
function editRule(rule: ForwardRuleSummary) {
  editingRuleId.value = rule.ruleId; ruleLabel.value = rule.label; Object.assign(draft, ruleToDraft(rule.rule));
  preflight.value = null; editorOpen.value = true;
}

interface PreparedStart {
  rule: PortForwardRule;
  savedRule: ForwardRuleSummary | null;
  current: () => boolean;
}
async function prepareStart(rule: PortForwardRule, savedRule: ForwardRuleSummary | null, stillCurrent: () => boolean = () => true, includeOptionalCredentials = false): Promise<PreparedStart | null> {
  const frozenRule: PortForwardRule = { ...rule };
  const frozenSaved = savedRule ? { ...savedRule, rule: { ...savedRule.rule } } : null;
  const hostVersion = hostMap.value.get(frozenRule.hostId)?.stateVersion;
  const current = () => !disposed && stillCurrent()
    && hostMap.value.get(frozenRule.hostId)?.stateVersion === hostVersion
    && (!frozenSaved || savedRules.value.some((saved) => saved.ruleId === frozenSaved.ruleId
      && saved.stateVersion === frozenSaved.stateVersion && JSON.stringify(saved.rule) === JSON.stringify(frozenSaved.rule)));
  if (!current()) return null;
  if (visualFixtureMode.value) return { rule: frozenRule, savedRule: frozenSaved, current };
  if (!hostVersion) throw new Error("Host unavailable");
  if (!await ensureHostVault(frozenRule.hostId, hostVersion, includeOptionalCredentials) || !current()) return null;
  const [freshHosts, freshRules] = await Promise.all([listHosts(), listForwardRules()]);
  if (!current() || !freshHosts.some((host) => host.hostId === frozenRule.hostId && host.stateVersion === hostVersion)
    || (frozenSaved && !freshRules.rules.some((saved) => saved.ruleId === frozenSaved.ruleId
      && saved.stateVersion === frozenSaved.stateVersion && JSON.stringify(saved.rule) === JSON.stringify(frozenSaved.rule)))) return null;
  return { rule: frozenRule, savedRule: frozenSaved, current };
}
async function startPrepared(prepared: PreparedStart): Promise<boolean> {
  if (!prepared.current()) return false;
  const { rule, savedRule } = prepared;
  const summary = visualFixtureMode.value
    ? createTunnelVisualFixtureSession(rule, savedRule)
    : await startForwardSession({ ruleId: savedRule?.ruleId ?? null, ruleRevision: savedRule?.stateVersion ?? null, rule }, applyEvent);
  if (!disposed) {
    applyEvent({ schemaVersion: 1, eventSeq: summary.stateRevision, session: summary });
    if (summary.failure || summary.state === "failed" || summary.state === "stopped") {
      if (!summary.failure) showOperationError();
      return false;
    }
    showOperationMessage(t("tunnels.feedback.started"));
  }
  return !summary.failure && summary.state !== "failed" && summary.state !== "stopped";
}
async function startRule(rule: PortForwardRule, savedRule: ForwardRuleSummary | null = null, stillCurrent?: () => boolean): Promise<boolean> {
  const prepared = await prepareStart(rule, savedRule, stillCurrent);
  return prepared ? startPrepared(prepared) : false;
}
async function startSavedRow(row: TunnelRow) {
  if (mutating.value) return;
  mutating.value = true; clearFeedback();
  try { await startRule(row.rule, row.savedRule); } catch { showOperationError(); }
  finally { mutating.value = false; }
}
async function saveRule(startAfterSave: boolean) {
  const rule = draftRule.value;
  const draftFingerprint = JSON.stringify(draftRule.value);
  if (!rule || !ruleLabel.value.trim() || mutating.value) { showOperationError(); return; }
  mutating.value = true; clearFeedback(); let saved: ForwardRuleSummary | null = null;
  try {
    const editing = savedRules.value.find((item) => item.ruleId === editingRuleId.value);
    saved = visualFixtureMode.value
      ? saveTunnelVisualFixtureRule(ruleLabel.value.trim(), rule, editing ?? null)
      : editing
        ? await updateForwardRule({ ruleId: editing.ruleId, expectedStateVersion: editing.stateVersion, label: ruleLabel.value.trim(), rule })
        : await createForwardRule({ label: ruleLabel.value.trim(), rule });
    replaceSavedRule(saved); editingRuleId.value = saved.ruleId; showOperationMessage(t("tunnels.feedback.saved"));
    if (startAfterSave) await startRule(saved.rule, saved, () => JSON.stringify(draftRule.value) === draftFingerprint);
  } catch { showOperationError(saved ? t("tunnels.feedback.savedButStartFailed") : undefined); }
  finally { mutating.value = false; }
}
async function startOnce() {
  if (!draftRule.value || mutating.value) { showOperationError(); return; }
  mutating.value = true; clearFeedback();
  const rule = draftRule.value;
  const fingerprint = JSON.stringify(rule);
  try { await startRule(rule, null, () => JSON.stringify(draftRule.value) === fingerprint); } catch { showOperationError(); } finally { mutating.value = false; }
}
async function runPreflight() {
  if (!draftRule.value || preflighting.value) { showOperationError(); return; }
  preflighting.value = true; clearFeedback();
  try {
    preflight.value = visualFixtureMode.value
      ? preflightTunnelVisualFixtureRule(draftRule.value)
      : await preflightForwardRule({ rule: draftRule.value });
  } catch { showOperationError(); }
  finally { preflighting.value = false; }
}
async function stop(session: ForwardSessionSummary) {
  mutating.value = true; clearFeedback();
  try {
    const summary = visualFixtureMode.value
      ? stopTunnelVisualFixtureSession(session)
      : await stopForwardSession({ sessionId: session.sessionId, expectedGeneration: session.generation });
    applyEvent({ schemaVersion: 1, eventSeq: summary.stateRevision, session: summary });
    showOperationMessage(t("tunnels.feedback.stopped"));
    if (!visualFixtureMode.value) await refresh();
  } catch { showOperationError(); } finally { mutating.value = false; }
}
async function restart(row: TunnelRow) {
  if (mutating.value) return; mutating.value = true; clearFeedback();
  const originalSession = row.session ? { ...row.session } : null;
  let expectedSession = originalSession;
  const matchesExpected = (session: ForwardSessionSummary) => !expectedSession || (
    session.sessionId === expectedSession.sessionId && session.generation === expectedSession.generation
    && session.stateRevision === expectedSession.stateRevision && session.state === expectedSession.state
  );
  try {
    const currentSaved = row.savedRule && row.session?.ruleRevision === row.savedRule.stateVersion && JSON.stringify(row.rule) === JSON.stringify(row.savedRule.rule) ? row.savedRule : null;
    const prepared = await prepareStart(row.rule, currentSaved, () => !expectedSession || sessions.value.some(matchesExpected), originalSession?.failure?.code === "vaultLocked");
    if (!prepared) return;
    if (originalSession && !visualFixtureMode.value) {
      // A tray stop may precede the next UI poll without changing generation.
      const fresh = await fetchForwardSessionSnapshot();
      if (!prepared.current() || !fresh.sessions.some(matchesExpected)) return;
    }
    if (originalSession && !["failed", "stopped"].includes(originalSession.state)) {
      const stopped = visualFixtureMode.value
        ? stopTunnelVisualFixtureSession(originalSession)
        : await stopForwardSession({ sessionId: originalSession.sessionId, expectedGeneration: originalSession.generation });
      if (disposed) return;
      if (stopped.sessionId !== originalSession.sessionId || stopped.generation !== originalSession.generation
        || stopped.state !== "stopped" || stopped.cleanup.uncertain) throw new Error("Tunnel stop was not confirmed");
      // Advance only to the state produced by this operation's own successful stop.
      expectedSession = stopped;
      applyEvent({ schemaVersion: 1, eventSeq: stopped.stateRevision, session: stopped });
    }
    if (await startPrepared(prepared) && !disposed) showOperationMessage(t("tunnels.feedback.restarted"));
  } catch { if (!disposed) showOperationError(); } finally { mutating.value = false; }
}
async function confirmDeleteRule() {
  const target = deleteTarget.value; if (!target || mutating.value) return;
  mutating.value = true; clearFeedback();
  try {
    if (!visualFixtureMode.value) {
      await deleteForwardRule({ ruleId: target.ruleId, expectedStateVersion: target.stateVersion });
    }
    savedRules.value = savedRules.value.filter((rule) => rule.ruleId !== target.ruleId);
    if (editingRuleId.value === target.ruleId) resetEditor();
    showOperationMessage(t("tunnels.feedback.deleted")); deleteTarget.value = null;
  } catch { showOperationError(); } finally { mutating.value = false; }
}
async function copyListener(row: TunnelRow) {
  try { await navigator.clipboard.writeText(bindLabel(row.session, row.rule)); showOperationMessage(t("tunnels.feedback.copied")); }
  catch { showOperationError(); }
}
async function confirmRetainUncertainCleanup() {
  const session = cleanupRetainTarget.value; if (!session || cleanupPendingSessionId.value !== null) return;
  cleanupPendingSessionId.value = session.sessionId; clearFeedback();
  try {
    const summary = await retainForwardCleanupForExitOnce({ sessionId: session.sessionId, expectedGeneration: session.generation, expectedStateRevision: session.stateRevision, retainUncertainCleanupForExitConfirmed: true });
    applyEvent({ schemaVersion: 1, eventSeq: summary.stateRevision, session: summary });
    retainedForExit.value = new Set(retainedForExit.value).add(session.sessionId); cleanupRetainTarget.value = null;
  } catch { showOperationError(); } finally { cleanupPendingSessionId.value = null; }
}
async function refresh() {
  if (visualFixtureMode.value) return;
  if (!canUseDesktopCore()) return;
  const [nextHosts, nextRules, snapshot] = await Promise.all([listHosts(), listForwardRules(), fetchForwardSessionSnapshot()]);
  hosts.value = nextHosts; savedRules.value = nextRules.rules; sessions.value = snapshot.sessions;
  snapshot.sessions.forEach(reportSessionFailure);
  if (!draft.hostId || !nextHosts.some((host) => host.hostId === draft.hostId)) draft.hostId = nextHosts[0]?.hostId ?? "";
}
async function openHostInTerminal(session: ForwardSessionSummary) {
  await router.push({ path: "/terminal", query: { hostId: session.hostId, source: "tunnels", connectOperationId: crypto.randomUUID() } });
}

let focusOperation = "";
watch(() => router.currentRoute.value.query.focusOperation, async (operation) => {
  if (typeof operation !== "string" || operation === focusOperation || router.currentRoute.value.path !== "/tunnels") return;
  focusOperation = operation;
  const query = { ...router.currentRoute.value.query };
  try {
    await refresh();
    if (router.currentRoute.value.query.focusOperation !== operation) return;
    const target = sessions.value.find((session) => session.sessionId === query.focusSessionId && session.generation === query.focusGeneration);
    if (!target || document.querySelector('[role="dialog"][aria-modal="true"]')) throw new Error("unavailable");
    search.value = ""; hostFilter.value = "all"; kindFilter.value = "all"; stateFilter.value = "all";
    selectedKey.value = `session:${target.sessionId}`;
  } catch { tips.show({ tone: "error", title: t("errors.tray.actionUnavailable") }); }
}, { immediate: true });

onMounted(async () => {
  const visualFixture = new URLSearchParams(window.location.search).get("visualFixture") === "tunnels"
    || window.location.hash.includes("visualFixture=tunnels");
  if (import.meta.env.DEV && visualFixture) {
    visualFixtureMode.value = true;
    hosts.value = tunnelVisualFixture.hosts;
    savedRules.value = tunnelVisualFixture.rules;
    sessions.value = tunnelVisualFixture.sessions;
    sessions.value.forEach(reportSessionFailure);
    const rule = tunnelVisualFixture.rules[0];
    if (rule) editRule(rule);
    loading.value = false;
    revealRoute();
    return;
  }
  try { await refresh(); } catch { showOperationError(); } finally { loading.value = false; revealRoute(); }
  if (!disposed) refreshTimer = window.setInterval(() => void refresh().catch(() => undefined), 2_000);
});
onBeforeUnmount(() => { disposed = true; if (refreshTimer !== null) window.clearInterval(refreshTimer); });
</script>

<template>
  <main
    class="tunnels-view"
    :class="{ 'tunnels-view--editor-open': editorOpen }"
  >
    <section class="tunnels-view__workspace">
      <NvxPageHeader
        :breadcrumb="t('tunnels.breadcrumb')"
        :title="t('tunnels.title')"
        :description="t('tunnels.description')"
      >
        <template #actions>
          <NvxPluginExtensionTarget
            target-id="tunnels.toolbar"
            instance-key="global"
          />
          <NvxButton
            variant="secondary"
            size="sm"
            :disabled="loading"
            @click="refresh"
          >
            <NvxIcon
              :icon="RefreshCw"
              :size="16"
            />{{ t("tunnels.refresh") }}
          </NvxButton>
          <NvxButton
            size="sm"
            @click="resetEditor"
          >
            <NvxIcon
              :icon="Plus"
              :size="16"
            />{{ t("tunnels.newRule") }}
          </NvxButton>
        </template>
      </NvxPageHeader>

      <section
        class="tunnels-view__filters"
        :aria-label="t('tunnels.filters.label')"
      >
        <label class="tunnels-view__search"><span class="sr-only">{{ t("tunnels.filters.search") }}</span><NvxIcon
          :icon="Search"
          :size="16"
        /><NvxInput
          v-model="search"
          :placeholder="t('tunnels.filters.search')"
        /></label>
        <NvxSelect
          v-model="hostFilter"
          :options="hostFilterOptions"
          :aria-label="t('tunnels.filters.host')"
        />
        <NvxSelect
          v-model="kindFilter"
          :options="kindFilterOptions"
          :aria-label="t('tunnels.filters.kind')"
        />
        <NvxSelect
          v-model="stateFilter"
          :options="stateFilterOptions"
          :aria-label="t('tunnels.filters.state')"
        />
      </section>

      <section
        class="tunnels-view__table-shell"
        :aria-label="t('tunnels.runningList')"
      >
        <table
          v-if="filteredRows.length"
          class="tunnels-view__table"
        >
          <thead><tr><th>{{ t("tunnels.columns.name") }}</th><th>{{ t("tunnels.columns.listener") }}</th><th>{{ t("tunnels.columns.target") }}</th><th>{{ t("tunnels.columns.state") }}</th><th>{{ t("tunnels.columns.connections") }}</th><th>{{ t("tunnels.columns.traffic") }}</th><th><span class="sr-only">{{ t("tunnels.columns.actions") }}</span></th></tr></thead>
          <tbody>
            <tr
              v-for="row in filteredRows"
              :key="row.key"
              :class="{ 'is-selected': selectedRow?.key === row.key }"
              tabindex="0"
              @click="selectedKey = row.key"
              @keydown.enter="selectedKey = row.key"
            >
              <td>
                <div class="tunnels-view__identity">
                  <span class="tunnels-view__kind-mark"><NvxIcon
                    :icon="Network"
                    :size="16"
                  /></span><span><strong>{{ row.label }}</strong><small>{{ kindLabel(row.rule.kind) }} · {{ hostLabel(row.rule.hostId) }}</small></span>
                </div>
              </td>
              <td class="tunnels-view__mono">
                {{ bindLabel(row.session, row.rule) }}
              </td>
              <td class="tunnels-view__mono">
                {{ ruleTarget(row.rule) ?? t("tunnels.dynamicTarget") }}
              </td>
              <td>
                <NvxStatusLabel :tone="stateTone(row.state)">
                  {{ stateLabel(row.state) }}
                </NvxStatusLabel>
              </td>
              <td>{{ row.session?.childCount ?? 0 }}</td>
              <td>
                <span
                  v-if="row.session"
                  class="tunnels-view__traffic"
                ><span>↑ {{ formatBytes(row.session.listenerToTargetBytes) }}</span><span>↓ {{ formatBytes(row.session.targetToListenerBytes) }}</span></span><span v-else>—</span>
              </td>
              <td>
                <div
                  class="tunnels-view__row-actions"
                  @click.stop
                >
                  <NvxButton
                    v-if="!row.session"
                    variant="ghost"
                    size="sm"
                    :disabled="mutating"
                    :aria-label="t('tunnels.start')"
                    @click="startSavedRow(row)"
                  >
                    <NvxIcon
                      :icon="Play"
                      :size="16"
                    />
                  </NvxButton>
                  <NvxButton
                    v-else-if="row.session.state !== 'stopped' && !row.session.cleanup.uncertain"
                    variant="ghost"
                    size="sm"
                    :disabled="mutating"
                    :aria-label="t('tunnels.stop')"
                    @click="stop(row.session)"
                  >
                    <NvxIcon
                      :icon="Square"
                      :size="16"
                    />
                  </NvxButton>
                  <NvxButton
                    variant="ghost"
                    size="sm"
                    :disabled="mutating || row.session?.cleanup.uncertain"
                    :aria-label="t('tunnels.restart')"
                    @click="restart(row)"
                  >
                    <NvxIcon
                      :icon="RefreshCw"
                      :size="16"
                    />
                  </NvxButton>
                  <NvxButton
                    variant="ghost"
                    size="sm"
                    :aria-label="t('tunnels.copyListener')"
                    @click="copyListener(row)"
                  >
                    <NvxIcon
                      :icon="Copy"
                      :size="16"
                    />
                  </NvxButton>
                  <NvxButton
                    v-if="row.savedRule"
                    variant="ghost"
                    size="sm"
                    :aria-label="t('tunnels.edit')"
                    @click="editRule(row.savedRule)"
                  >
                    <NvxIcon
                      :icon="Pencil"
                      :size="16"
                    />
                  </NvxButton>
                </div>
              </td>
            </tr>
          </tbody>
        </table>
        <div
          v-else
          class="tunnels-view__empty"
        >
          <NvxIcon
            :icon="Network"
            :size="22"
          /><strong>{{ loading ? t("tunnels.loading") : t("tunnels.empty") }}</strong><span v-if="!loading">{{ t("tunnels.emptyDescription") }}</span>
        </div>
      </section>

      <section
        v-if="selectedRow"
        class="tunnels-view__details"
        :aria-label="t('tunnels.details.title')"
      >
        <header>
          <div><span class="tunnels-view__eyebrow">{{ t("tunnels.details.title") }}</span><h2>{{ selectedRow.label }}</h2></div><NvxStatusLabel :tone="stateTone(selectedRow.state)">
            {{ stateLabel(selectedRow.state) }}
          </NvxStatusLabel>
        </header>
        <div class="tunnels-view__detail-grid">
          <div><span>{{ t("tunnels.host") }}</span><strong>{{ hostLabel(selectedRow.rule.hostId) }}</strong></div>
          <div><span>{{ t("tunnels.columns.listener") }}</span><strong class="tunnels-view__mono">{{ bindLabel(selectedRow.session, selectedRow.rule) }}</strong></div>
          <div><span>{{ t("tunnels.columns.target") }}</span><strong class="tunnels-view__mono">{{ ruleTarget(selectedRow.rule) ?? t("tunnels.dynamicTarget") }}</strong></div>
          <div><span>{{ t("tunnels.children") }}</span><strong>{{ selectedRow.session?.childCount ?? 0 }}</strong></div>
          <div><span>{{ t("tunnels.details.uptime") }}</span><strong>{{ selectedRow.session ? uptime(selectedRow.session) : "—" }}</strong></div>
          <div><span>{{ t("tunnels.details.transport") }}</span><strong>{{ t("tunnels.details.independentTransport") }}</strong></div>
        </div>
        <footer v-if="selectedRow.savedRule || selectedRow.session?.failure">
          <NvxButton
            v-if="selectedRow.session?.failure?.code === 'vaultLocked'"
            :disabled="mutating"
            size="sm"
            variant="secondary"
            @click="restart(selectedRow)"
          >
            {{ t("tunnels.unlockVaultAndRetry") }}
          </NvxButton>
          <NvxButton
            v-else-if="selectedRow.session?.failure && ['hostKeyReviewRequired', 'hostKeyMismatch', 'credentialUnavailable', 'authenticationRejected'].includes(selectedRow.session.failure.code)"
            size="sm"
            variant="secondary"
            @click="openHostInTerminal(selectedRow.session)"
          >
            {{ t("tunnels.openTerminalToResolve") }}
          </NvxButton>
          <NvxButton
            v-if="selectedRow.savedRule"
            variant="secondary"
            size="sm"
            @click="editRule(selectedRow.savedRule)"
          >
            <NvxIcon
              :icon="Pencil"
              :size="16"
            />{{ t("tunnels.edit") }}
          </NvxButton><NvxButton
            v-if="selectedRow.savedRule"
            variant="ghost"
            size="sm"
            @click="deleteTarget = selectedRow.savedRule"
          >
            <NvxIcon
              :icon="Trash2"
              :size="16"
            />{{ t("tunnels.delete") }}
          </NvxButton>
        </footer>
        <NvxInlineNotice
          v-if="selectedRow.session?.cleanup.uncertain"
          tone="warning"
          :title="t('tunnels.cleanupUncertain.title')"
        >
          <p>{{ t("tunnels.cleanupUncertain.description") }}</p>
          <dl class="tunnels-view__cleanup-facts">
            <div><dt>{{ t("tunnels.cleanupUncertain.listener") }}</dt><dd>{{ t(`tunnels.cleanupStates.${selectedRow.session.cleanup.listenerClosedOrRemoteCancelled}`) }}</dd></div><div><dt>{{ t("tunnels.cleanupUncertain.children") }}</dt><dd>{{ t(`tunnels.cleanupStates.${selectedRow.session.cleanup.childrenCleared}`) }}</dd></div><div><dt>{{ t("tunnels.cleanupUncertain.transport") }}</dt><dd>{{ t(`tunnels.cleanupStates.${selectedRow.session.cleanup.transportDisconnected}`) }}</dd></div><div><dt>{{ t("tunnels.cleanupUncertain.abandonedChildren") }}</dt><dd>{{ selectedRow.session.cleanup.abandonedChildCount }}</dd></div>
          </dl>
          <p v-if="retainedForExit.has(selectedRow.session.sessionId)">
            {{ t("tunnels.cleanupUncertain.retainedForNextExit") }}
          </p>
          <NvxButton
            v-else
            size="sm"
            variant="danger"
            :disabled="cleanupPendingSessionId !== null"
            @click="cleanupRetainTarget = selectedRow.session"
          >
            {{ t("tunnels.cleanupUncertain.allowNextExit") }}
          </NvxButton>
        </NvxInlineNotice>
      </section>
    </section>

    <aside
      v-if="editorOpen"
      class="tunnels-view__editor"
      :aria-label="t('tunnels.editor.title')"
    >
      <header>
        <div><span class="tunnels-view__eyebrow">{{ editingRuleId ? t("tunnels.editor.editing") : t("tunnels.editor.new") }}</span><h2>{{ t("tunnels.editor.title") }}</h2></div><NvxButton
          variant="ghost"
          size="sm"
          :aria-label="t('tunnels.editor.close')"
          @click="editorOpen = false"
        >
          <NvxIcon
            :icon="X"
            :size="20"
          />
        </NvxButton>
      </header>
      <div class="tunnels-view__editor-body">
        <NvxField
          for-id="forward-label"
          :label="t('tunnels.ruleName')"
        >
          <NvxInput
            id="forward-label"
            v-model="ruleLabel"
            :placeholder="t('tunnels.ruleNamePlaceholder')"
            :maxlength="120"
          />
        </NvxField>
        <NvxField
          for-id="forward-host"
          :label="t('tunnels.host')"
        >
          <NvxSelect
            id="forward-host"
            v-model="draft.hostId"
            :options="hostOptions"
          />
        </NvxField>
        <fieldset class="tunnels-view__segments">
          <legend>{{ t("tunnels.kind") }}</legend><button
            v-for="option in kindOptions"
            :key="option.value"
            type="button"
            :class="{ 'is-active': draft.kind === option.value }"
            role="radio"
            :aria-checked="draft.kind === option.value"
            @click="draft.kind = option.value as ForwardKind"
          >
            <strong>{{ option.label }}</strong>
            <span>{{ option.description }}</span>
          </button>
        </fieldset>
        <div class="tunnels-view__endpoint-block">
          <span class="tunnels-view__eyebrow">{{ t(`tunnels.listenHeadings.${draft.kind}`) }}</span><div class="tunnels-view__endpoint-grid">
            <NvxField
              for-id="forward-bind-address"
              :label="t('tunnels.address')"
            >
              <NvxInput
                id="forward-bind-address"
                v-model="draft.bindAddress"
                :readonly="draft.kind !== 'remote'"
              />
            </NvxField><NvxField
              for-id="forward-listen-port"
              :label="t('tunnels.port')"
            >
              <NvxInput
                id="forward-listen-port"
                v-model="draft.listenPort"
                inputmode="numeric"
              />
            </NvxField>
          </div>
        </div>
        <div
          v-if="draft.kind !== 'dynamic'"
          class="tunnels-view__endpoint-block"
        >
          <span class="tunnels-view__eyebrow">{{ t(`tunnels.targetHeadings.${draft.kind}`) }}</span><div class="tunnels-view__endpoint-grid">
            <NvxField
              for-id="forward-target-host"
              :label="t('tunnels.address')"
            >
              <NvxInput
                id="forward-target-host"
                v-model="draft.targetHost"
              />
            </NvxField><NvxField
              for-id="forward-target-port"
              :label="t('tunnels.port')"
            >
              <NvxInput
                id="forward-target-port"
                v-model="draft.targetPort"
                inputmode="numeric"
              />
            </NvxField>
          </div>
        </div>
        <section
          class="tunnels-view__flow-summary"
          :aria-label="flowTitle"
        >
          <span class="tunnels-view__flow-icon"><NvxIcon
            :icon="Network"
            :size="20"
          /></span>
          <div>
            <strong>{{ flowTitle }}</strong>
            <span>{{ flowDescription }}</span>
          </div>
          <NvxIcon
            class="tunnels-view__flow-arrow"
            :icon="ArrowRight"
            :size="20"
          />
        </section>
        <NvxInlineNotice
          v-if="draft.kind === 'remote'"
          tone="warning"
          :title="t('tunnels.remoteExposure')"
        >
          <span><NvxIcon
            :icon="ShieldAlert"
            :size="16"
          /> {{ t("tunnels.remoteExposureDetail") }}</span>
        </NvxInlineNotice>
        <p
          v-else
          class="tunnels-view__loopback-hint"
        >
          {{ t("tunnels.loopbackOnly") }}
        </p>
        <section class="tunnels-view__preflight">
          <div><strong>{{ t("tunnels.preflight.title") }}</strong><span>{{ t("tunnels.preflight.description") }}</span></div><NvxButton
            variant="secondary"
            size="sm"
            :loading="preflighting"
            @click="runPreflight"
          >
            {{ t("tunnels.preflight.run") }}
          </NvxButton>
        </section>
        <NvxInlineNotice
          v-if="preflight"
          :tone="preflight.localBindAvailableAtCheck === false ? 'error' : 'info'"
          :title="preflight.localBindAvailableAtCheck === null ? t('tunnels.preflight.remoteAdvisory') : preflight.localBindAvailableAtCheck ? t('tunnels.preflight.availableAtCheck') : t('tunnels.preflight.unavailableAtCheck')"
        />
      </div>
      <footer>
        <NvxButton
          variant="secondary"
          :disabled="mutating || loading || !hosts.length"
          @click="saveRule(false)"
        >
          {{ t("tunnels.saveRule") }}
        </NvxButton><NvxButton
          variant="ghost"
          :disabled="mutating || loading || !hosts.length"
          @click="startOnce"
        >
          {{ t("tunnels.startOnce") }}
        </NvxButton><NvxButton
          :loading="mutating"
          :disabled="loading || !hosts.length"
          @click="saveRule(true)"
        >
          <NvxIcon
            :icon="Play"
            :size="16"
          />{{ t("tunnels.saveAndStart") }}
        </NvxButton>
      </footer>
    </aside>

    <NvxDialog
      plugin-protected
      :model-value="cleanupRetainTarget !== null"
      :title="t('tunnels.cleanupRetainDialog.title')"
      :description="t('tunnels.cleanupRetainDialog.description')"
      :close-label="t('tunnels.cleanupRetainDialog.close')"
      :dismissible="cleanupPendingSessionId === null"
      @update:model-value="(open) => { if (!open) cleanupRetainTarget = null; }"
    >
      <NvxInlineNotice
        tone="warning"
        :title="t('tunnels.cleanupRetainDialog.warning')"
      >
        {{ cleanupRetainTarget ? bindLabel(cleanupRetainTarget, cleanupRetainTarget.ruleSnapshot) : "" }}
      </NvxInlineNotice>
      <template #actions>
        <NvxButton
          variant="secondary"
          :disabled="cleanupPendingSessionId !== null"
          @click="cleanupRetainTarget = null"
        >
          {{ t("tunnels.cleanupRetainDialog.cancel") }}
        </NvxButton><NvxButton
          variant="danger"
          :loading="cleanupPendingSessionId !== null"
          @click="confirmRetainUncertainCleanup"
        >
          {{ t("tunnels.cleanupRetainDialog.confirm") }}
        </NvxButton>
      </template>
    </NvxDialog>
    <NvxDialog
      plugin-protected
      :model-value="deleteTarget !== null"
      :title="t('tunnels.deleteDialog.title')"
      :description="t('tunnels.deleteDialog.description')"
      :close-label="t('tunnels.deleteDialog.close')"
      :dismissible="!mutating"
      @update:model-value="(open) => { if (!open) deleteTarget = null; }"
    >
      <strong>{{ deleteTarget?.label }}</strong><template #actions>
        <NvxButton
          variant="secondary"
          :disabled="mutating"
          @click="deleteTarget = null"
        >
          {{ t("tunnels.deleteDialog.cancel") }}
        </NvxButton><NvxButton
          variant="danger"
          :loading="mutating"
          @click="confirmDeleteRule"
        >
          {{ t("tunnels.deleteDialog.confirm") }}
        </NvxButton>
      </template>
    </NvxDialog>
  </main>
</template>

<style scoped>
.tunnels-view { display: grid; grid-template-columns: minmax(0, 1fr); min-height: 100%; background: var(--nvx-color-bg-canvas); }
.tunnels-view--editor-open { grid-template-columns: minmax(0, 1fr) 360px; }
.tunnels-view__workspace { min-width: 0; padding: var(--nvx-space-6); }
.tunnels-view__filters { display: grid; grid-template-columns: minmax(240px, 1fr) repeat(3, minmax(140px, .38fr)); gap: var(--nvx-space-3); margin-bottom: var(--nvx-space-4); }
.tunnels-view__search { position: relative; display: flex; align-items: center; }
.tunnels-view__search > :nth-child(2) { position: absolute; z-index: 1; left: var(--nvx-space-3); color: var(--nvx-color-text-tertiary); }
.tunnels-view__search :deep(input) { padding-left: 36px; }
.tunnels-view__table-shell, .tunnels-view__details, .tunnels-view__editor { border: var(--nvx-border-width) solid var(--nvx-color-border-subtle); background: var(--nvx-color-bg-surface); }
.tunnels-view__table-shell { overflow-x: auto; border-radius: var(--nvx-radius-lg); }
.tunnels-view__table { width: 100%; min-width: 920px; border-collapse: collapse; }
.tunnels-view__table th { height: 38px; padding: 0 var(--nvx-space-3); border-bottom: var(--nvx-border-width) solid var(--nvx-color-border-subtle); background: var(--nvx-color-bg-subtle); color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-xs); text-align: left; white-space: nowrap; }
.tunnels-view__table td { height: 58px; padding: var(--nvx-space-2) var(--nvx-space-3); border-bottom: var(--nvx-border-width) solid var(--nvx-color-border-subtle); }
.tunnels-view__table tbody tr { cursor: pointer; transition: background-color var(--nvx-motion-fast); }
.tunnels-view__table tbody tr:hover { background: var(--nvx-color-bg-hover); }
.tunnels-view__table tbody tr.is-selected { background: var(--nvx-color-accent-subtle); box-shadow: inset 3px 0 var(--nvx-color-accent); }
.tunnels-view__table tbody tr:focus-visible { outline: 2px solid var(--nvx-color-focus-ring); outline-offset: -2px; }
.tunnels-view__table tbody tr:last-child td { border-bottom: 0; }
.tunnels-view__identity { display: flex; align-items: center; gap: var(--nvx-space-3); min-width: 170px; }
.tunnels-view__identity > span:last-child { display: grid; gap: 2px; }
.tunnels-view__identity small { color: var(--nvx-color-text-secondary); white-space: nowrap; }
.tunnels-view__kind-mark { display: grid; place-items: center; width: 30px; height: 30px; border-radius: var(--nvx-radius-md); background: var(--nvx-color-accent-subtle); color: var(--nvx-color-accent); }
.tunnels-view__mono { font-family: var(--nvx-font-family-mono); font-size: var(--nvx-font-size-sm); white-space: nowrap; }
.tunnels-view__traffic { display: grid; gap: 2px; color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-xs); white-space: nowrap; }
.tunnels-view__row-actions { display: flex; justify-content: flex-end; }
.tunnels-view__row-actions :deep(.nvx-button) { width: 30px; min-height: 30px; padding: 0; }
.tunnels-view__empty { min-height: 260px; display: grid; place-content: center; justify-items: center; gap: var(--nvx-space-2); color: var(--nvx-color-text-secondary); text-align: center; }
.tunnels-view__empty strong { color: var(--nvx-color-text-primary); }
.tunnels-view__details { display: grid; gap: var(--nvx-space-4); margin-top: var(--nvx-space-4); padding: var(--nvx-space-5); border-radius: var(--nvx-radius-lg); }
.tunnels-view__details > header, .tunnels-view__editor > header { display: flex; align-items: flex-start; justify-content: space-between; gap: var(--nvx-space-3); }
.tunnels-view__details h2, .tunnels-view__editor h2 { margin: 2px 0 0; font-size: var(--nvx-font-size-lg); }
.tunnels-view__eyebrow { color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-xs); font-weight: var(--nvx-font-weight-semibold); letter-spacing: .04em; text-transform: uppercase; }
.tunnels-view__detail-grid { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: var(--nvx-space-4); }
.tunnels-view__detail-grid > div { display: grid; gap: var(--nvx-space-1); }
.tunnels-view__detail-grid span { color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-xs); }
.tunnels-view__details > footer { display: flex; gap: var(--nvx-space-2); padding-top: var(--nvx-space-2); border-top: var(--nvx-border-width) solid var(--nvx-color-border-subtle); }
.tunnels-view__cleanup-facts { display: grid; gap: var(--nvx-space-2); margin: var(--nvx-space-3) 0; }
.tunnels-view__cleanup-facts div { display: flex; justify-content: space-between; gap: var(--nvx-space-3); }
.tunnels-view__cleanup-facts dt { color: var(--nvx-color-text-secondary); } .tunnels-view__cleanup-facts dd { margin: 0; }
.tunnels-view__editor { position: sticky; top: 0; z-index: 3; display: grid; grid-template-rows: auto minmax(0, 1fr) auto; height: calc(100vh - var(--nvx-layout-header-height)); min-width: 0; border-width: 0 0 0 var(--nvx-border-width); }
.tunnels-view__editor > header { padding: var(--nvx-space-5); border-bottom: var(--nvx-border-width) solid var(--nvx-color-border-subtle); }
.tunnels-view__editor-body { overflow-y: auto; display: grid; align-content: start; gap: var(--nvx-space-4); padding: var(--nvx-space-5); }
.tunnels-view__editor > footer { display: grid; grid-template-columns: 1fr 1fr; gap: var(--nvx-space-2); padding: var(--nvx-space-4) var(--nvx-space-5); border-top: var(--nvx-border-width) solid var(--nvx-color-border-subtle); }
.tunnels-view__editor > footer :last-child { grid-column: 1 / -1; }
.tunnels-view__segments { display: grid; gap: var(--nvx-space-2); margin: 0; padding: 0; border: 0; }
.tunnels-view__segments legend { padding: 0 0 var(--nvx-space-2); color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-sm); }
.tunnels-view__segments button { display: grid; gap: 2px; min-width: 0; min-height: 58px; padding: var(--nvx-space-2) var(--nvx-space-3); border: var(--nvx-border-width) solid var(--nvx-color-border-subtle); border-radius: var(--nvx-radius-md); background: var(--nvx-color-bg-surface); color: var(--nvx-color-text-secondary); font: inherit; text-align: left; cursor: pointer; }
.tunnels-view__segments button strong { color: var(--nvx-color-text-primary); font-size: var(--nvx-font-size-sm); }
.tunnels-view__segments button span { font-size: var(--nvx-font-size-xs); line-height: var(--nvx-line-height-xs); }
.tunnels-view__segments button:hover { border-color: var(--nvx-color-border-strong); background: var(--nvx-color-bg-hover); }
.tunnels-view__segments button.is-active { border-color: var(--nvx-color-accent); background: var(--nvx-color-accent-soft); color: var(--nvx-color-text-secondary); }
.tunnels-view__segments button.is-active strong { color: var(--nvx-color-accent); }
.tunnels-view__segments button:focus-visible { outline: 2px solid var(--nvx-color-focus-ring); outline-offset: 1px; }
.tunnels-view__endpoint-block { display: grid; gap: var(--nvx-space-2); padding-top: var(--nvx-space-3); border-top: var(--nvx-border-width) solid var(--nvx-color-border-subtle); }
.tunnels-view__endpoint-grid { display: grid; grid-template-columns: minmax(0, 1fr) 92px; gap: var(--nvx-space-3); }
.tunnels-view__flow-summary { position: relative; display: grid; grid-template-columns: auto minmax(0, 1fr); gap: var(--nvx-space-3); align-items: center; padding: var(--nvx-space-3); border: var(--nvx-border-width) solid var(--nvx-color-border-subtle); border-radius: var(--nvx-radius-md); background: var(--nvx-color-bg-subtle); }
.tunnels-view__flow-summary > div { display: grid; gap: 2px; padding-right: var(--nvx-space-6); }
.tunnels-view__flow-summary span { color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-xs); line-height: var(--nvx-line-height-xs); }
.tunnels-view__flow-icon { display: grid; place-items: center; width: 32px; height: 32px; border-radius: var(--nvx-radius-md); background: var(--nvx-color-accent-soft); color: var(--nvx-color-accent); }
.tunnels-view__flow-arrow { position: absolute; right: var(--nvx-space-3); color: var(--nvx-color-text-tertiary); }
.tunnels-view__loopback-hint { margin: calc(-1 * var(--nvx-space-1)) 0 0; color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-xs); line-height: var(--nvx-line-height-xs); }
.tunnels-view__preflight { display: flex; align-items: center; justify-content: space-between; gap: var(--nvx-space-3); padding: var(--nvx-space-3); border: var(--nvx-border-width) solid var(--nvx-color-border-subtle); border-radius: var(--nvx-radius-md); background: var(--nvx-color-bg-subtle); }
.tunnels-view__preflight > div { display: grid; gap: 2px; } .tunnels-view__preflight span { color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-xs); }
.sr-only { position: absolute; width: 1px; height: 1px; padding: 0; margin: -1px; overflow: hidden; clip: rect(0, 0, 0, 0); white-space: nowrap; border: 0; }
@media (max-width: 1180px) { .tunnels-view--editor-open { grid-template-columns: minmax(0, 1fr) 330px; } .tunnels-view__filters { grid-template-columns: 1fr 1fr; } .tunnels-view__detail-grid { grid-template-columns: repeat(2, minmax(0, 1fr)); } }
@media (max-width: 900px) { .tunnels-view--editor-open { grid-template-columns: minmax(0, 1fr); } .tunnels-view__workspace { padding: var(--nvx-space-4); } .tunnels-view__editor { position: relative; grid-row: 1; height: auto; border-width: 0 0 var(--nvx-border-width); } .tunnels-view__editor-body { overflow: visible; } }
@media (max-width: 620px) { .tunnels-view__filters, .tunnels-view__detail-grid, .tunnels-view__endpoint-grid { grid-template-columns: 1fr; } }
</style>
