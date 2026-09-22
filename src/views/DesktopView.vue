<script setup lang="ts">
import { computed, onActivated, onBeforeUnmount, onDeactivated, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";
import { Monitor, Plus, ChevronDown, RotateCw, Unplug, Scaling, Scan, Maximize2, ClipboardCopy, ClipboardPaste, Volume2, VolumeX, Hand, X } from "lucide-vue-next";
import { NvxButton, NvxDialog, NvxIcon, NvxIconButton, NvxInlineNotice } from "../components/ui";
import NvxDesktopProfileMenu from "../components/desktop/NvxDesktopProfileMenu.vue";
import NvxDesktopCanvas from "../components/desktop/NvxDesktopCanvas.vue";
import { desktopClient } from "../core-api/desktop-client";
import type { DesktopAvailability, DesktopProfile, DesktopSessionSummary } from "../core-api/generated/core-api";
import { useWorkspaceTabsStore } from "../stores/workspaceTabs";
import { useTipsStore } from "../stores/tips";
import { performWindowAction } from "../platform-window";
import { onToolWindowChanged, openToolWindow } from "../tool-windows";
defineOptions({ name: "DesktopView" });
const { t, te } = useI18n(), router = useRouter(), workspace = useWorkspaceTabsStore(), tips = useTipsStore();
const availability = ref<DesktopAvailability[]>([]);
const profiles = ref<DesktopProfile[]>([]), sessions = ref<DesktopSessionSummary[]>([]);
const activeId = ref(""), active = ref(true), loading = ref(true), busy = ref(false), failed = ref(false), deleteTarget = ref<DesktopProfile | null>(null), fit = ref(true), panning = ref(false);
const display = ref<InstanceType<typeof NvxDesktopCanvas> | null>(null);
const current = computed(() => sessions.value.find((session) => session.id === activeId.value));
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
async function refresh() {
  if (snapshotBusy || disposed) return;
  snapshotBusy = true;
  try { const result = await desktopClient.snapshot(); if (!disposed) { sessions.value = result; if (!result.some((item) => item.id === activeId.value)) activeId.value = result[0]?.id ?? ""; } }
  catch { failed.value = true; }
  finally { snapshotBusy = false; }
}
async function poll() { if (disposed) return; if (active.value && !document.hidden) await refresh(); if (!disposed) timer = setTimeout(() => void poll(), 750); }
async function load() {
  loading.value = true; failed.value = false;
  try { const [saved, available] = await Promise.all([desktopClient.profiles(), desktopClient.availability()]); availability.value = available; profiles.value = saved; await refresh(); }
  catch { failed.value = true; }
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
  } catch { notice(); }
}
async function open(profile: DesktopProfile) {
  if (busy.value) return; busy.value = true;
  try { workspace.terminalController?.deactivate(); await desktopClient.focus(null); const session = await desktopClient.open(profile); sessions.value = [...sessions.value.filter((item) => item.id !== session.id), session]; activeId.value = session.id; await router.push("/desktop"); }
  catch { notice(); } finally { busy.value = false; }
}
async function close(tabId: string) {
  const session = sessions.value.find((item) => `desktop:${item.id}` === tabId); if (!session) return;
  try { if (session.id === activeId.value) await display.value?.invalidate(); await desktopClient.close(session); sessions.value = sessions.value.filter((item) => item.id !== session.id); if (activeId.value === session.id) activeId.value = sessions.value[0]?.id ?? ""; }
  catch { notice(); await refresh(); }
}
async function disconnect(session: DesktopSessionSummary) { busy.value = true; try { await display.value?.invalidate(); await desktopClient.disconnect(session); await refresh(); } catch { notice(); } finally { busy.value = false; } }
async function toggleAudio() {
  const session = current.value;
  if (!session || busy.value) return;
  busy.value = true;
  try { await desktopClient.mute(session, !session.audioMuted); await refresh(); }
  catch { notice(); } finally { busy.value = false; }
}
async function reconnect(session: DesktopSessionSummary) { if (!["closed", "failed"].includes(session.state)) return; await open(session.profile); }
async function remove() { if (!deleteTarget.value) return; busy.value = true; try { await desktopClient.delete(deleteTarget.value); profiles.value = profiles.value.filter((item) => item.id !== deleteTarget.value?.id); deleteTarget.value = null; notice("deleted", "success"); } catch { notice(); } finally { busy.value = false; } }
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
async function toggleFullscreen() {
  if (busy.value) return;
  busy.value = true;
  try {
    await display.value?.invalidate();
    await performWindowAction("toggleFullscreen");
  } catch { notice(); } finally { busy.value = false; }
}
function togglePanning() {
  panning.value = !panning.value;
  if (panning.value) fit.value = false;
}
function activate(tabId: string) { const session = sessions.value.find((item) => `desktop:${item.id}` === tabId); if (!session) return; workspace.terminalController?.deactivate(); activeId.value = session.id; void router.push("/desktop"); }
const unregister = workspace.registerDesktopController({ activate, close: (id) => void close(id), closeMany: (ids) => { void (async () => { for (const id of ids) await close(id); })(); }, deactivate: () => { active.value = false; void display.value?.invalidate(); } });
watch([sessions, activeId, busy, () => t("desktop.title")], () => workspace.syncDesktopState({ tabs: sessions.value.map((session) => ({ groupId: `desktop:${session.id}`, label: session.profile.label, stateLabel: t(`desktop.states.${session.state}`) })), activeTabId: activeId.value ? `desktop:${activeId.value}` : "", busy: busy.value }), { deep: true, immediate: true });
let focusOperation = "";
let disposeToolWindowListener: (() => void) | undefined;
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
  void load();
  void poll();
  void onToolWindowChanged((kind) => { if (kind === "desktopEditor") void load(); }).then((dispose) => {
    if (disposed) dispose(); else disposeToolWindowListener = dispose;
  }).catch(() => { /* The desktop list remains usable when tool-window notifications are unavailable. */ });
});
onActivated(() => { active.value = true; workspace.terminalController?.deactivate(); void refresh(); });
onDeactivated(() => { active.value = false; void display.value?.invalidate(); });
onBeforeUnmount(() => { disposed = true; clearTimeout(timer); disposeToolWindowListener?.(); unregister(); });
</script>
<template>
  <section class="desktop-page">
    <aside class="desktop-profiles">
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
        :title="t('desktop.error')"
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
                    :active="active && !deleteTarget && !collapsedProtocols.includes(group.protocol)"
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
      <template v-if="current">
        <header class="desktop-toolbar">
          <div class="desktop-toolbar__identity">
            <strong :title="current.profile.label">{{ current.profile.label }}</strong>
            <span>{{ current.state === 'connecting' && te(`desktop.phases.${current.phase}`) ? t(`desktop.phases.${current.phase}`) : t(`desktop.states.${current.state}`) }}</span>
          </div>
          <div class="desktop-toolbar__actions">
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
              size="sm"
              :label="t('window.fullscreen')"
              :disabled="busy"
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
        <NvxInlineNotice
          v-if="current.state === 'failed'"
          tone="error"
          :title="current.failure && te(`desktop.errors.${current.failure}`) ? t(`desktop.errors.${current.failure}`) : t('desktop.failure')"
        />
        <NvxDesktopCanvas
          ref="display"
          :session="current"
          :active="active && !deleteTarget"
          :fit="fit"
          :panning="panning"
          @error="notice('inputFailed')"
        />
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
.desktop-toolbar__identity { display: flex; align-items: baseline; gap: var(--nvx-space-2); min-width: 0; overflow: hidden; }
.desktop-toolbar__identity strong { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: var(--nvx-font-size-sm); }
.desktop-toolbar__identity span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: var(--nvx-font-size-xs); color: var(--nvx-color-text-secondary); }
.desktop-toolbar__actions { display: flex; align-items: center; flex: 0 0 auto; gap: var(--nvx-space-1); }
.desktop-audio-state { max-width: 140px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: var(--nvx-font-size-xs); color: var(--nvx-color-text-secondary); }
.desktop-toolbar__actions :deep(svg) { stroke-width: 1.25; }
.desktop-toolbar__pan[aria-pressed="true"] { background: var(--nvx-color-bg-hover); color: var(--nvx-color-text-primary); }
.desktop-empty { margin: auto; padding: var(--nvx-space-6); text-align: center; color: var(--nvx-color-text-secondary); }
.desktop-empty h2 { color: var(--nvx-color-text-primary); font-size: var(--nvx-font-size-lg); }
.desktop-empty p { max-width: 360px; }
@media (max-width: 850px) { .desktop-page { grid-template-columns: 245px minmax(0, 1fr); } }
@media (max-width: 600px) { .desktop-page { grid-template-columns: 1fr; grid-template-rows: minmax(120px, 35%) 1fr; } .desktop-profiles { border-right: 0; border-bottom: 1px solid var(--nvx-color-border); } }
</style>
