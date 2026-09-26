<script setup lang="ts">
import { computed, nextTick, onActivated, onBeforeUnmount, onDeactivated, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";
import { Monitor, Plus, ChevronDown, RotateCw, Unplug, Scaling, Scan, Maximize2, ClipboardCopy, ClipboardPaste, Volume2, VolumeX, Hand, X, PanelLeftClose, PanelLeftOpen, Minimize2, Settings2 } from "lucide-vue-next";
import { NvxButton, NvxDialog, NvxIcon, NvxIconButton, NvxInlineNotice, NvxField, NvxSelect } from "../components/ui";
import NvxDesktopProfileMenu from "../components/desktop/NvxDesktopProfileMenu.vue";
import NvxDesktopDisplaySettings from "../components/desktop/NvxDesktopDisplaySettings.vue";
import { parseCoreApiError } from "../core-api/client";
import NvxDesktopCanvas from "../components/desktop/NvxDesktopCanvas.vue";
import { desktopClient } from "../core-api/desktop-client";
import type { DesktopAvailability, DesktopProfile, DesktopSessionSummary } from "../core-api/generated/core-api";
import { useWorkspaceTabsStore } from "../stores/workspaceTabs";
import { useTipsStore } from "../stores/tips";
import { onToolWindowChanged, openToolWindow } from "../tool-windows";
import { useRouteReveal } from "../routeReveal";
import { onSavedConnectionsChanged } from "../saved-connections";
defineOptions({ name: "DesktopView" });
const { t, te } = useI18n(), router = useRouter(), workspace = useWorkspaceTabsStore(), tips = useTipsStore();
const revealRoute = useRouteReveal();
let initialRouteReady = false;
const availability = ref<DesktopAvailability[]>([]);
const profiles = ref<DesktopProfile[]>([]), sessions = ref<DesktopSessionSummary[]>([]);
const activeId = ref(""), active = ref(true), loading = ref(true), busy = ref(false), failed = ref(false), deleteTarget = ref<DesktopProfile | null>(null), fit = ref(true), panning = ref(false);
const failureText = ref("");
const sidebarCollapsed = ref(false), fullscreen = ref(false), fullscreenBusy = ref(false);
const fullscreenTarget = ref<HTMLElement | null>(null);
const fullscreenButton = ref<InstanceType<typeof NvxIconButton> | null>(null);
const exitFullscreenButton = ref<InstanceType<typeof NvxIconButton> | null>(null);
// Safari 16 on supported macOS versions exposes the prefixed element fullscreen API.
type FullscreenDocument = Document & { webkitFullscreenElement?: Element | null; webkitExitFullscreen?: () => Promise<void> | void };
type FullscreenElement = HTMLElement & { webkitRequestFullscreen?: () => Promise<void> | void };
const fullscreenDocument = document as FullscreenDocument;
let fullscreenOperation = 0, fullscreenSession = "", escapeConsumed = false;
const display = ref<InstanceType<typeof NvxDesktopCanvas> | null>(null);
const current = computed(() => sessions.value.find((session) => session.id === activeId.value));
const settingsDraft = ref<DesktopProfile | null>(null);
const settingsSession = ref<DesktopSessionSummary | null>(null);
const settingsError = ref("");
const settingsDiagnostic = ref("");
const settingsActual = computed(() => sessions.value.find(item => item.id === settingsSession.value?.id && item.generation === settingsSession.value?.generation));
function actualValue(kind: "Transport" | "Graphics") {
  const session = settingsActual.value;
  if (!session || ["closed", "failed"].includes(session.state)) return t("desktop.pendingConnection");
  const value = kind === "Transport" ? session.rdpTransportActual : session.rdpGraphicsActual;
  return value ? t(`desktop.${kind === "Transport" ? "transportActual" : "graphicsActual"}.${value}`) : t("desktop.negotiating");
}
async function showDisplaySettings() {
  const session = current.value;
  if (!session || busy.value) return;
  await display.value?.invalidate();
  if (current.value?.id !== session.id || current.value.generation !== session.generation) return;
  settingsSession.value = { ...session, profile: { ...session.profile } };
  settingsDraft.value = { ...(profiles.value.find(profile => profile.id === session.profile.id) ?? session.profile) };
  settingsError.value = "";
  settingsDiagnostic.value = "";
}
function editConnectionProtocol() {
  const profile = settingsDraft.value;
  if (!profile || busy.value) return;
  settingsDraft.value = null;
  void edit(profile);
}
function setSessionVncVersion(version: string) {
  if (settingsDraft.value && (version === "auto" || version === "rfb33" || version === "rfb37" || version === "rfb38")) settingsDraft.value.vncProtocolVersion = version;
}
function resolutionFailure(error: unknown) {
  const parsed = parseCoreApiError(error);
  if (parsed?.code === "desktop.staleInput") return;
  const title = parsed && te(parsed.messageKey) ? t(parsed.messageKey, parsed.params) : t("desktop.resolutionFailed");
  const detail = parsed ? [parsed.code, parsed.diagnosticId].filter(Boolean).join(" · ") : undefined;
  tips.show({ scope: "desktop-resolution", tone: "error", title, message: detail });
}
function settingsFailure(error: unknown, fallback: string) {
  const parsed = parseCoreApiError(error);
  settingsError.value = parsed && te(parsed.messageKey) ? t(parsed.messageKey, parsed.params) : t(`desktop.${fallback}`);
  settingsDiagnostic.value = parsed ? [parsed.code, parsed.diagnosticId].filter(Boolean).join(" · ") : "";
}
async function applyDisplaySettings() {
  const target = settingsSession.value, draft = settingsDraft.value;
  if (!target || !draft || busy.value) return;
  if (![draft.width, draft.height].every(value => Number.isInteger(value) && value >= 200 && value <= 8192) || draft.width * draft.height > 16_777_216) {
    settingsError.value = t("desktop.displaySettingsInvalid"); return;
  }
  const stillCurrent = () => !disposed && active.value && current.value?.id === target.id && current.value?.generation === target.generation;
  busy.value = true; settingsError.value = ""; settingsDiagnostic.value = "";
  let stage = "displaySettingsSaveFailed";
  try {
    const saved = await desktopClient.save({ ...draft });
    profiles.value = profiles.value.map(profile => profile.id === saved.id ? saved : profile);
    settingsDraft.value = { ...saved };
    if (!stillCurrent()) { settingsError.value = t("desktop.displaySettingsSessionChanged"); return; }
    stage = "displaySettingsDisconnectFailed";
    await display.value?.invalidate();
    if (!stillCurrent()) { settingsError.value = t("desktop.displaySettingsSessionChanged"); return; }
    await desktopClient.disconnect(target);
    if (!stillCurrent()) { settingsError.value = t("desktop.displaySettingsSessionChanged"); return; }
    stage = "displaySettingsConnectFailed";
    workspace.terminalController?.deactivate();
    await desktopClient.focus(null);
    if (!stillCurrent()) { settingsError.value = t("desktop.displaySettingsSessionChanged"); return; }
    const session = await desktopClient.open(saved);
    sessions.value = [...sessions.value.filter(item => item.id !== session.id), session];
    if (stillCurrent()) activeId.value = session.id;
    settingsDraft.value = null; settingsSession.value = null;
  } catch (error) { settingsFailure(error, stage); await refresh(); }
  finally { busy.value = false; }
}
const collapsedProtocols = ref<DesktopProfile["protocol"][]>([]);
const profileGroups = computed(() => (["rdp", "vnc"] as const)
  .map((protocol) => ({ protocol, profiles: profiles.value.filter((profile) => profile.protocol === protocol) }))
  .filter((group) => group.profiles.length > 0));
function toggleProtocol(protocol: DesktopProfile["protocol"]) {
  collapsedProtocols.value = collapsedProtocols.value.includes(protocol)
    ? collapsedProtocols.value.filter((item) => item !== protocol)
    : [...collapsedProtocols.value, protocol];
}
let timer: ReturnType<typeof setTimeout> | undefined, disposed = false, snapshotBusy = false;
function notice(key = "error", tone: "error" | "success" = "error") { tips.show({ scope: "desktop", tone, title: t(`desktop.${key}`) }); }
function noticeFailure(error: unknown) {
  const failure = parseCoreApiError(error);
  tips.show({
    scope: "desktop",
    tone: "error",
    title: failure?.messageKey && te(failure.messageKey) ? t(failure.messageKey) : t("desktop.error"),
    message: failure?.diagnosticId ? t("diagnostics.id", { id: failure.diagnosticId }) : undefined,
  });
}
function pageFailure(error: unknown) {
  const failure = parseCoreApiError(error);
  const key = failure?.messageKey;
  failureText.value = (key && te(key) ? t(key) : t("desktop.error"))
    + (failure?.diagnosticId ? ` ${t("diagnostics.id", { id: failure.diagnosticId })}` : "");
  failed.value = true;
}
async function refresh() {
  if (snapshotBusy || disposed) return;
  snapshotBusy = true;
  try { const result = await desktopClient.snapshot(); if (!disposed) { sessions.value = result; if (!result.some((item) => item.id === activeId.value)) activeId.value = result[0]?.id ?? ""; } }
  catch (error) { pageFailure(error); }
  finally { snapshotBusy = false; }
}
async function poll() { if (disposed) return; if (active.value && !document.hidden) await refresh(); if (!disposed) timer = setTimeout(() => void poll(), 750); }
let profilesSequence = 0;
async function refreshProfiles() {
  const sequence = ++profilesSequence;
  try {
    const saved = await desktopClient.profiles();
    if (!disposed && sequence === profilesSequence) profiles.value = saved;
  } catch (error) {
    if (!disposed && sequence === profilesSequence) throw error;
  }
}
async function load() {
  loading.value = true; failed.value = false; failureText.value = "";
  try { const [, available] = await Promise.all([refreshProfiles(), desktopClient.availability()]); availability.value = available; await refresh(); }
  catch (error) { pageFailure(error); }
  finally { loading.value = false; }
}
async function edit(profile?: DesktopProfile) {
  void display.value?.invalidate();
  try {
    await openToolWindow({
      kind: "desktopEditor",
      profileId: profile?.id ?? null,
      title: t(profile ? "desktop.editProfile" : "desktop.newProfile"),
    });
  } catch (error) { noticeFailure(error); }
}
async function open(profile: DesktopProfile) {
  if (busy.value) return; busy.value = true;
  try { workspace.terminalController?.deactivate(); await desktopClient.focus(null); const session = await desktopClient.open(profile); sessions.value = [...sessions.value.filter((item) => item.id !== session.id), session]; activeId.value = session.id; await router.push("/desktop"); }
  catch (error) { noticeFailure(error); } finally { busy.value = false; }
}
async function close(tabId: string) {
  const session = sessions.value.find((item) => `desktop:${item.id}` === tabId); if (!session) return;
  try { if (session.id === activeId.value) await display.value?.invalidate(); await desktopClient.close(session); sessions.value = sessions.value.filter((item) => item.id !== session.id); if (activeId.value === session.id) activeId.value = sessions.value[0]?.id ?? ""; }
  catch (error) { noticeFailure(error); await refresh(); }
}
async function disconnect(session: DesktopSessionSummary) { busy.value = true; try { await display.value?.invalidate(); await desktopClient.disconnect(session); await refresh(); } catch (error) { noticeFailure(error); } finally { busy.value = false; } }
async function toggleAudio() {
  const session = current.value;
  if (!session || busy.value) return;
  busy.value = true;
  try { await desktopClient.mute(session, !session.audioMuted); await refresh(); }
  catch (error) { noticeFailure(error); } finally { busy.value = false; }
}
async function reconnect(session: DesktopSessionSummary) { if (!["closed", "failed"].includes(session.state)) return; await open(profiles.value.find(profile => profile.id === session.profile.id) ?? session.profile); }
async function remove() { if (!deleteTarget.value) return; busy.value = true; try { await desktopClient.delete(deleteTarget.value); profiles.value = profiles.value.filter((item) => item.id !== deleteTarget.value?.id); deleteTarget.value = null; notice("deleted", "success"); } catch (error) { noticeFailure(error); } finally { busy.value = false; } }
async function clipboard(receive: boolean) {
  const session = current.value; if (!session) return;
  const stillCurrent = () => current.value?.id === session.id && current.value?.generation === session.generation && active.value;
  try {
    if (receive) {
      const text = await desktopClient.clipboard(session);
      if (!stillCurrent()) return;
      if (text === null) { notice("clipboardEmpty"); return; }
      await navigator.clipboard.writeText(text);
    } else {
      const text = await navigator.clipboard.readText();
      if (!stillCurrent()) return;
      if (new TextEncoder().encode(text).length > 65536 || !await display.value?.clipboard(text)) throw new Error("clipboard");
    }
    if (stillCurrent()) notice("clipboardDone", "success");
  } catch { if (stillCurrent()) notice(); }
}
function fullscreenElement() { return document.fullscreenElement ?? fullscreenDocument.webkitFullscreenElement ?? null; }
function sessionKey() { return current.value ? `${current.value.id}:${current.value.generation}` : ""; }
async function leaveFullscreen() {
  fullscreenOperation++;
  fullscreenSession = "";
  if (!fullscreenTarget.value || fullscreenElement() !== fullscreenTarget.value) return;
  void display.value?.invalidate();
  try {
    if (document.exitFullscreen) await document.exitFullscreen();
    else await fullscreenDocument.webkitExitFullscreen?.();
  } catch { if (!disposed) notice("fullscreenFailed"); }
}
function fullscreenChanged() {
  const wasFullscreen = fullscreen.value;
  fullscreen.value = !!fullscreenTarget.value && fullscreenElement() === fullscreenTarget.value;
  if (fullscreen.value && (!active.value || fullscreenSession !== sessionKey() || deleteTarget.value)) {
    void leaveFullscreen();
    return;
  }
  if (wasFullscreen !== fullscreen.value) {
    void display.value?.invalidate();
    void nextTick(() => {
      if (!active.value || disposed) return;
      const button = fullscreen.value ? exitFullscreenButton.value : fullscreenButton.value;
      (button?.$el as HTMLElement | undefined)?.focus({ preventScroll: true });
    });
  }
}
async function toggleFullscreen() {
  if (fullscreenBusy.value) return;
  if (fullscreenElement() === fullscreenTarget.value) { await leaveFullscreen(); return; }
  const target = fullscreenTarget.value as FullscreenElement | null;
  if (!active.value || !current.value || !target) return;
  fullscreenBusy.value = true;
  const operation = ++fullscreenOperation;
  fullscreenSession = sessionKey();
  try {
    // Revoke input immediately, but preserve the click's transient user activation.
    void display.value?.invalidate();
    if (target.requestFullscreen) await target.requestFullscreen();
    else if (target.webkitRequestFullscreen) await target.webkitRequestFullscreen();
    else throw new Error("Element fullscreen is unavailable");
    if (disposed || operation !== fullscreenOperation || !active.value || fullscreenSession !== sessionKey()) {
      if (fullscreenElement() === target) {
        if (document.exitFullscreen) await document.exitFullscreen();
        else await fullscreenDocument.webkitExitFullscreen?.();
      }
      return;
    }
    fullscreenChanged();
  } catch { if (!disposed && operation === fullscreenOperation) notice("fullscreenFailed"); }
  finally { fullscreenBusy.value = false; }
}
function fullscreenKey(event: KeyboardEvent) {
  if (event.key !== "Escape" || (!fullscreen.value && !escapeConsumed)) return;
  // Both edges of the local exit key must stay out of the remote input stream.
  event.preventDefault();
  event.stopImmediatePropagation();
  escapeConsumed = event.type === "keydown";
  if (event.type === "keydown") void leaveFullscreen();
}
watch([active, () => current.value?.id, () => current.value?.generation, () => current.value?.state, deleteTarget], () => {
  if (!active.value || fullscreenSession !== sessionKey() || deleteTarget.value || current.value?.state !== "running") void leaveFullscreen();
});
function togglePanning() {
  panning.value = !panning.value;
  if (panning.value) fit.value = false;
}
function activate(tabId: string) { const session = sessions.value.find((item) => `desktop:${item.id}` === tabId); if (!session) return; workspace.terminalController?.deactivate(); activeId.value = session.id; void router.push("/desktop"); }
const unregister = workspace.registerDesktopController({ activate, close: (id) => void close(id), closeMany: (ids) => { void (async () => { for (const id of ids) await close(id); })(); }, deactivate: () => { active.value = false; void display.value?.invalidate(); } });
watch([sessions, activeId, busy, () => t("desktop.title")], () => workspace.syncDesktopState({ tabs: sessions.value.map((session) => ({ groupId: `desktop:${session.id}`, label: session.profile.label, stateLabel: t(`desktop.states.${session.state}`) })), activeTabId: activeId.value ? `desktop:${activeId.value}` : "", busy: busy.value }), { deep: true, immediate: true });
let focusOperation = "";
let disposeToolWindowListener: (() => void) | undefined;
let disposeSavedConnectionsListener: (() => void) | undefined;
watch(() => router.currentRoute.value.query.focusOperation, async (operation) => {
  if (typeof operation !== "string" || operation === focusOperation || router.currentRoute.value.path !== "/desktop") return;
  focusOperation = operation;
  const query = { ...router.currentRoute.value.query };
  try {
    const snapshot = await desktopClient.snapshot();
    if (disposed || router.currentRoute.value.query.focusOperation !== operation) return;
    const target = snapshot.find((session) => session.id === query.focusSessionId && session.generation === query.focusGeneration);
    if (!target || document.querySelector('[role="dialog"][aria-modal="true"]')) throw new Error("unavailable");
    sessions.value = snapshot; activeId.value = target.id;
  } catch { if (!disposed) tips.show({ tone: "error", title: t("errors.tray.actionUnavailable") }); }
}, { immediate: true });

onMounted(() => {
  document.addEventListener("fullscreenchange", fullscreenChanged);
  document.addEventListener("webkitfullscreenchange", fullscreenChanged);
  window.addEventListener("keydown", fullscreenKey, true);
  window.addEventListener("keyup", fullscreenKey, true);
  void load().finally(() => { initialRouteReady = true; revealRoute(); });
  void poll();
  void onToolWindowChanged((kind) => { if (kind === "desktopEditor") void load(); }).then((dispose) => {
    if (disposed) dispose(); else disposeToolWindowListener = dispose;
  }).catch(() => { /* The desktop list remains usable when tool-window notifications are unavailable. */ });
  void onSavedConnectionsChanged(() => { void refreshProfiles().catch(noticeFailure); }).then((dispose) => {
    if (disposed) dispose(); else disposeSavedConnectionsListener = dispose;
  }).catch(() => { /* Initial and activation reads still load saved profiles. */ });
});
onActivated(() => { active.value = true; workspace.terminalController?.deactivate(); void refresh(); void refreshProfiles().catch(noticeFailure); if (initialRouteReady) revealRoute(); });
onDeactivated(() => { active.value = false; void display.value?.invalidate(); });
onBeforeUnmount(() => {
  void leaveFullscreen();
  document.removeEventListener("fullscreenchange", fullscreenChanged);
  document.removeEventListener("webkitfullscreenchange", fullscreenChanged);
  window.removeEventListener("keydown", fullscreenKey, true);
  window.removeEventListener("keyup", fullscreenKey, true);
  disposed = true; clearTimeout(timer); disposeToolWindowListener?.(); disposeSavedConnectionsListener?.(); unregister(); });
</script>
<template>
  <section
    class="desktop-page"
    :class="{ 'desktop-page--sidebar-collapsed': sidebarCollapsed }"
  >
    <aside
      v-show="!sidebarCollapsed"
      id="desktop-profiles"
      class="desktop-profiles"
      :aria-label="t('desktop.profiles')"
    >
      <header>
        <h1>{{ t('desktop.title') }}</h1><NvxButton
          variant="ghost"
          size="sm"
          :disabled="busy"
          @click="edit()"
        >
          <NvxIcon
            :icon="Plus"
            :size="16"
          />{{ t('desktop.newProfile') }}
        </NvxButton>
      </header>
      <NvxInlineNotice
        v-if="loading"
        :title="t('desktop.loading')"
      />
      <NvxInlineNotice
        v-else-if="failed"
        tone="error"
        :title="failureText || t('desktop.error')"
      >
        <NvxButton
          variant="ghost"
          @click="load"
        >
          {{ t('desktop.refresh') }}
        </NvxButton>
      </NvxInlineNotice>
      <div class="desktop-profiles__list">
        <section
          v-for="group in profileGroups"
          :key="group.protocol"
          class="desktop-profile-group"
        >
          <NvxButton
            class="desktop-profile-group__toggle"
            variant="ghost"
            size="sm"
            :aria-expanded="!collapsedProtocols.includes(group.protocol)"
            :aria-controls="`desktop-profiles-${group.protocol}`"
            @click="toggleProtocol(group.protocol)"
          >
            <NvxIcon
              class="desktop-profile-group__chevron"
              :class="{ 'desktop-profile-group__chevron--collapsed': collapsedProtocols.includes(group.protocol) }"
              :icon="ChevronDown"
              :size="16"
            />
            {{ group.protocol.toUpperCase() }} ({{ group.profiles.length }})
          </NvxButton>
          <div
            v-show="!collapsedProtocols.includes(group.protocol)"
            :id="`desktop-profiles-${group.protocol}`"
          >
            <article
              v-for="profile in group.profiles"
              :key="profile.id"
              class="desktop-profile"
              :class="{ 'desktop-profile--active': current?.profile.id === profile.id }"
            >
              <div class="desktop-profile__row">
                <NvxIcon
                  :icon="Monitor"
                  :size="20"
                />
                <div class="desktop-profile__identity">
                  <strong :title="profile.label">{{ profile.label }}</strong>
                  <span :title="`${profile.address}:${profile.port}`">{{ profile.address }}:{{ profile.port }}</span>
                </div>
                <div class="desktop-profile__actions">
                  <NvxButton
                    size="sm"
                    variant="secondary"
                    :disabled="busy || !availability.some((item) => item.protocol === profile.protocol && item.available)"
                    @click="open(profile)"
                  >
                    {{ t('desktop.connect') }}
                  </NvxButton>
                  <NvxDesktopProfileMenu
                    :disabled="busy"
                    :active="active && !sidebarCollapsed && !deleteTarget && !collapsedProtocols.includes(group.protocol)"
                    @edit="edit(profile)"
                    @delete="deleteTarget = profile"
                  />
                </div>
              </div>
              <NvxInlineNotice
                v-if="availability.some((item) => item.protocol === profile.protocol && !item.available)"
                tone="warning"
                :title="t('desktop.error')"
              />
            </article>
          </div>
        </section>
      </div>
    </aside>
    <main class="desktop-session">
      <header class="desktop-toolbar">
        <div class="desktop-toolbar__identity">
          <NvxIconButton
            size="sm"
            :label="t(sidebarCollapsed ? 'desktop.expandProfiles' : 'desktop.collapseProfiles')"
            :aria-expanded="!sidebarCollapsed"
            aria-controls="desktop-profiles"
            @click="sidebarCollapsed = !sidebarCollapsed"
          >
            <NvxIcon
              :icon="sidebarCollapsed ? PanelLeftOpen : PanelLeftClose"
              :size="16"
            />
          </NvxIconButton>
          <template v-if="current">
            <strong :title="current.profile.label">{{ current.profile.label }}</strong>
            <span>{{ current.state === 'connecting' && te(`desktop.phases.${current.phase}`) ? t(`desktop.phases.${current.phase}`) : t(`desktop.states.${current.state}`) }}</span>
          </template>
        </div>
        <div
          v-if="current"
          class="desktop-toolbar__actions"
        >
          <NvxIconButton
            size="sm"
            :label="t('desktop.displaySettings')"
            aria-haspopup="dialog"
            :disabled="busy || ['connecting', 'needsInteraction', 'disconnecting'].includes(current.state)"
            @click="showDisplaySettings"
          >
            <NvxIcon
              :icon="Settings2"
              :size="16"
            />
          </NvxIconButton>
          <NvxIconButton
            v-if="['failed', 'closed'].includes(current.state)"
            size="sm"
            :label="t('desktop.reconnect')"
            :disabled="busy"
            @click="reconnect(current)"
          >
            <NvxIcon
              :icon="RotateCw"
              :size="16"
            />
          </NvxIconButton>
          <NvxIconButton
            v-else
            size="sm"
            :label="t('desktop.disconnect')"
            :disabled="busy || current.state === 'disconnecting'"
            @click="disconnect(current)"
          >
            <NvxIcon
              :icon="Unplug"
              :size="16"
            />
          </NvxIconButton>
          <NvxIconButton
            size="sm"
            :label="t(fit ? 'desktop.actual' : 'desktop.fit')"
            @click="fit = !fit"
          >
            <NvxIcon
              :icon="fit ? Scaling : Scan"
              :size="16"
            />
          </NvxIconButton>
          <NvxIconButton
            class="desktop-toolbar__pan"
            size="sm"
            :label="t('desktop.pan')"
            :aria-pressed="panning"
            @click="togglePanning"
          >
            <NvxIcon
              :icon="Hand"
              :size="16"
            />
          </NvxIconButton>
          <NvxIconButton
            ref="fullscreenButton"
            size="sm"
            :label="t('desktop.fullscreen')"
            :disabled="busy || fullscreenBusy || current.state !== 'running'"
            :aria-pressed="fullscreen"
            @click="toggleFullscreen"
          >
            <NvxIcon
              :icon="Maximize2"
              :size="16"
            />
          </NvxIconButton>
          <template v-if="current.profile.audioPlaybackEnabled">
            <NvxIconButton
              size="sm"
              :label="t(current.audioMuted ? 'desktop.unmuteAudio' : 'desktop.muteAudio')"
              :aria-pressed="current.audioMuted"
              :disabled="busy || current.state !== 'running'"
              @click="toggleAudio"
            >
              <NvxIcon
                :icon="current.audioMuted ? VolumeX : Volume2"
                :size="16"
              />
            </NvxIconButton>
            <span
              class="desktop-audio-state"
              role="status"
            >{{ t(`desktop.audioStates.${current.audioState}`) }}</span>
          </template>
          <template v-if="current.profile.clipboardEnabled">
            <NvxIconButton
              size="sm"
              :label="t('desktop.sendClipboard')"
              :disabled="current.state !== 'running' || panning"
              @click="clipboard(false)"
            >
              <NvxIcon
                :icon="ClipboardCopy"
                :size="16"
              />
            </NvxIconButton>
            <NvxIconButton
              size="sm"
              :label="t('desktop.receiveClipboard')"
              :disabled="current.state !== 'running'"
              @click="clipboard(true)"
            >
              <NvxIcon
                :icon="ClipboardPaste"
                :size="16"
              />
            </NvxIconButton>
          </template>
          <NvxIconButton
            size="sm"
            :label="t('desktop.close')"
            @click="close(`desktop:${current.id}`)"
          >
            <NvxIcon
              :icon="X"
              :size="16"
            />
          </NvxIconButton>
        </div>
      </header>
      <template v-if="current">
        <NvxInlineNotice
          v-if="current.state === 'failed'"
          tone="error"
          :title="current.failure && te(`desktop.errors.${current.failure}`) ? t(`desktop.errors.${current.failure}`) : t('desktop.failure')"
        />
        <div
          ref="fullscreenTarget"
          class="desktop-screen"
          :class="{ 'desktop-screen--fullscreen': fullscreen }"
        >
          <NvxDesktopCanvas
            ref="display"
            :session="current"
            :active="active && !deleteTarget && !settingsDraft"
            :fit="fit"
            :panning="panning"
            @error="notice('inputFailed')"
            @resolution-error="resolutionFailure"
          />
          <NvxIconButton
            v-if="fullscreen"
            ref="exitFullscreenButton"
            class="desktop-screen__exit"
            :label="t('desktop.exitFullscreen')"
            :title="t('desktop.exitFullscreenHint')"
            @click="leaveFullscreen"
          >
            <NvxIcon
              :icon="Minimize2"
              :size="20"
            />
          </NvxIconButton>
        </div>
      </template>
      <div
        v-else
        class="desktop-empty"
      >
        <NvxIcon
          :icon="Monitor"
          :size="22"
        /><h2>{{ t('desktop.empty') }}</h2><p>{{ t('desktop.emptyHint') }}</p><NvxButton @click="edit()">
          {{ t('desktop.newProfile') }}
        </NvxButton>
      </div>
    </main>
    <NvxDialog
      :model-value="!!settingsDraft"
      :title="t('desktop.displaySettings')"
      :description="t('desktop.displaySettingsHint')"
      :close-label="t('desktop.cancel')"
      :dismissible="!busy"
      @update:model-value="!$event && (settingsDraft = null)"
    >
      <template v-if="settingsDraft && settingsSession">
        <dl class="desktop-settings-current">
          <div><dt>{{ t('desktop.protocol') }}</dt><dd>{{ settingsSession.profile.protocol.toUpperCase() }}</dd></div>
          <div><dt>{{ t('desktop.currentSettings') }}</dt><dd>{{ settingsActual?.width && settingsActual?.height ? `${settingsActual.width} × ${settingsActual.height}` : t('desktop.pendingConnection') }}</dd></div>
          <template v-if="settingsSession.profile.protocol === 'rdp'">
            <div><dt>{{ t('desktop.actualTransport') }}</dt><dd>{{ actualValue('Transport') }}</dd></div>
            <div><dt>{{ t('desktop.actualGraphics') }}</dt><dd>{{ actualValue('Graphics') }}</dd></div>
          </template>
        </dl>
        <NvxInlineNotice
          v-if="settingsError"
          tone="error"
          :title="settingsError"
        >
          <span v-if="settingsDiagnostic">{{ settingsDiagnostic }}</span>
        </NvxInlineNotice>
        <div :inert="busy">
          <NvxField
            v-if="settingsDraft.protocol === 'vnc'"
            :label="t('desktop.vncProtocolVersion')"
            class="desktop-settings-vnc"
          >
            <NvxSelect
              :model-value="settingsDraft.vncProtocolVersion"
              :aria-label="t('desktop.vncProtocolVersion')"
              :options="[{ value: 'auto', label: t('desktop.vncVersions.auto') }, { value: 'rfb33', label: 'RFB 3.3' }, { value: 'rfb37', label: 'RFB 3.7' }, { value: 'rfb38', label: 'RFB 3.8' }]"
              @update:model-value="setSessionVncVersion"
            />
          </NvxField>
          <NvxDesktopDisplaySettings
            v-model="settingsDraft"
            id-prefix="desktop-session"
          />
        </div>
      </template>
      <template #actions>
        <NvxButton
          variant="ghost"
          :disabled="busy"
          @click="editConnectionProtocol"
        >
          {{ t('desktop.editConnectionProtocol') }}
        </NvxButton>
        <NvxButton
          variant="secondary"
          :disabled="busy"
          @click="settingsDraft = null"
        >
          {{ t('desktop.cancel') }}
        </NvxButton>
        <NvxButton
          :loading="busy"
          @click="applyDisplaySettings"
        >
          {{ t('desktop.applyDisplaySettings') }}
        </NvxButton>
      </template>
    </NvxDialog>
    <NvxDialog
      :model-value="!!deleteTarget"
      :title="t('desktop.delete')"
      :close-label="t('desktop.cancel')"
      :dismissible="!busy"
      @update:model-value="!$event && (deleteTarget = null)"
    >
      <p>{{ t('desktop.confirmDelete') }}</p><template #actions>
        <NvxButton
          variant="secondary"
          :disabled="busy"
          @click="deleteTarget = null"
        >
          {{ t('desktop.cancel') }}
        </NvxButton><NvxButton
          variant="danger"
          :loading="busy"
          @click="remove"
        >
          {{ t('desktop.delete') }}
        </NvxButton>
      </template>
    </NvxDialog>
  </section>
</template>
<style scoped>
.desktop-settings-vnc { margin-bottom: var(--nvx-space-3); }
.desktop-settings-current { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: var(--nvx-space-3); margin: 0 0 var(--nvx-space-4); padding-bottom: var(--nvx-space-3); border-bottom: 1px solid var(--nvx-color-border); font-size: var(--nvx-font-size-sm); }
.desktop-settings-current dt { color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-xs); }
.desktop-settings-current dd { margin: var(--nvx-space-1) 0 0; }
.desktop-page { height: 100%; min-height: 0; display: grid; grid-template-columns: 280px minmax(0, 1fr); overflow: hidden; }
.desktop-profiles { border-right: 1px solid var(--nvx-color-border); min-width: 0; min-height: 0; display: flex; flex-direction: column; background: var(--nvx-color-bg-surface); }
.desktop-profiles > header { display: flex; align-items: center; justify-content: space-between; gap: var(--nvx-space-1); min-height: 44px; padding: 0 var(--nvx-space-2); border-bottom: 1px solid var(--nvx-color-border); flex: none; }
h1 { font-size: var(--nvx-font-size-sm); margin: 0; white-space: nowrap; }
.desktop-profiles__list { overflow: auto; min-height: 0; }
.desktop-profile-group__toggle { display: flex; justify-content: flex-start; width: 100%; min-height: 28px; padding: 0 var(--nvx-space-2); border-radius: 0; border-bottom: 1px solid var(--nvx-color-border); background: var(--nvx-color-bg-subtle); color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-xs); }
.desktop-profile-group__chevron--collapsed { transform: rotate(-90deg); }
.desktop-profile { padding: var(--nvx-space-2); border-bottom: 1px solid var(--nvx-color-border); }
.desktop-profile:hover, .desktop-profile:focus-within { background: var(--nvx-color-bg-hover); }
.desktop-profile--active { background: var(--nvx-color-bg-hover); box-shadow: inset 2px 0 var(--nvx-color-accent); }
.desktop-profile__row { display: flex; align-items: center; gap: var(--nvx-space-2); min-height: 40px; }
.desktop-profile__row > :first-child { flex: none; color: var(--nvx-color-text-secondary); }
.desktop-profile__identity { flex: 1; min-width: 0; display: grid; gap: 2px; }
.desktop-profile__identity strong, .desktop-profile__identity span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.desktop-profile__identity strong { font-size: var(--nvx-font-size-sm); font-weight: var(--nvx-font-weight-medium); }
.desktop-profile__identity span { color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-xs); font-variant-numeric: tabular-nums; }
.desktop-profile__actions { display: flex; align-items: center; gap: 2px; flex: none; }
.desktop-profile__actions > .nvx-button, .desktop-profiles > header > .nvx-button { min-height: 26px; padding-inline: var(--nvx-space-2); font-size: var(--nvx-font-size-xs); }
.desktop-session { display: flex; flex-direction: column; min-width: 0; min-height: 0; background: var(--nvx-color-bg-surface); }
.desktop-toolbar { height: 40px; min-height: 40px; padding: 0 var(--nvx-space-3); display: flex; align-items: center; justify-content: space-between; gap: var(--nvx-space-2); border-bottom: 1px solid var(--nvx-color-border); }
.desktop-toolbar__identity { display: flex; align-items: center; gap: var(--nvx-space-2); min-width: 0; overflow: hidden; }
.desktop-toolbar__identity > .nvx-icon-button { flex: none; }
.desktop-toolbar__identity strong { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: var(--nvx-font-size-sm); }
.desktop-toolbar__identity span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: var(--nvx-font-size-xs); color: var(--nvx-color-text-secondary); }
.desktop-toolbar__actions { display: flex; align-items: center; flex: 0 0 auto; gap: var(--nvx-space-1); }
.desktop-audio-state { max-width: 140px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: var(--nvx-font-size-xs); color: var(--nvx-color-text-secondary); }
.desktop-toolbar__actions :deep(svg) { stroke-width: 1.25; }
.desktop-toolbar__pan[aria-pressed="true"] { background: var(--nvx-color-bg-hover); color: var(--nvx-color-text-primary); }
.desktop-screen { position: relative; flex: 1; min-height: 0; overflow: hidden; background: var(--nvx-color-terminal-bg); }
.desktop-screen:fullscreen, .desktop-screen:-webkit-full-screen {
  width: 100vw;
  height: 100vh;
  flex: none;
}
.desktop-screen__exit { position: absolute; top: var(--nvx-space-3); right: var(--nvx-space-3); background: var(--nvx-color-bg-surface); box-shadow: var(--nvx-shadow-toast); }
.desktop-empty { margin: auto; padding: var(--nvx-space-6); text-align: center; color: var(--nvx-color-text-secondary); }
.desktop-empty h2 { color: var(--nvx-color-text-primary); font-size: var(--nvx-font-size-lg); }
.desktop-empty p { max-width: 360px; }
@media (max-width: 850px) { .desktop-page { grid-template-columns: 245px minmax(0, 1fr); } }
@media (max-width: 600px) { .desktop-page { grid-template-columns: 1fr; grid-template-rows: minmax(120px, 35%) 1fr; } .desktop-profiles { border-right: 0; border-bottom: 1px solid var(--nvx-color-border); } }
.desktop-page--sidebar-collapsed { grid-template-columns: minmax(0, 1fr); grid-template-rows: minmax(0, 1fr); }
</style>
