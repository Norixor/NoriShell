<script setup lang="ts">
import { ArrowRight, Clock3, FolderOpen, Plus, Server, SquareTerminal, Star } from "lucide-vue-next";
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";

import { NvxButton, NvxDialog, NvxIcon, NvxInlineNotice } from "../components/ui";
import { canUseDesktopCore, listHostCatalog } from "../core-api/client";
import type { HostCatalogEntry } from "../core-api/generated/core-api";
import { createUuidV7 } from "../core-api/ids";
import { RECENT_FILE_HOSTS_CHANGED, readRecentFileHosts } from "../recent-file-hosts";
import { useRouteReveal } from "../routeReveal";
import { onSavedConnectionsChanged } from "../saved-connections";
import { showWorkspaceTabFailure } from "../workspace-tab-errors";
import { requestWorkspaceTabShellAction, type WorkspaceTabShellAction } from "../workspace-tab-shell-action";

defineOptions({ name: "NewWorkspacePageView" });
const { t, locale } = useI18n();
const revealRoute = useRouteReveal();
const catalog = ref<HostCatalogEntry[]>([]);
const fileHistory = ref<Record<string, number>>({});
const loading = ref(true);
const loadFailed = ref(false);
const picker = ref<"terminal" | "sftp" | null>(null);
const actionBusy = ref(false);
let stopSavedConnections: (() => void) | undefined;
let disposed = false;
let requestVersion = 0;

/** Favorited hosts surface first; ties within each group fall back to the most recent activity. */
const recentHosts = computed(() => catalog.value
  .filter((entry) => entry.recentConnection || fileHistory.value[entry.host.hostId])
  .sort((a, b) => {
    if (a.host.favorite !== b.host.favorite) return a.host.favorite ? -1 : 1;
    return Math.max(b.recentConnection?.connectedAtUnixMs ?? 0, fileHistory.value[b.host.hostId] ?? 0)
      - Math.max(a.recentConnection?.connectedAtUnixMs ?? 0, fileHistory.value[a.host.hostId] ?? 0);
  })
  .slice(0, 5));

const dateTimeFormatter = computed(() => new Intl.DateTimeFormat(locale.value, {
  year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit",
}));
function lastUsed(timestamp: number | null | undefined) {
  return timestamp ? dateTimeFormatter.value.format(timestamp) : t("newWorkspace.neverUsed");
}

async function refresh() {
  const version = ++requestVersion;
  fileHistory.value = readRecentFileHosts();
  if (!canUseDesktopCore()) { loading.value = false; revealRoute(); return; }
  try {
    const entries = await listHostCatalog("recentlyConnected");
    if (!disposed && version === requestVersion) {
      catalog.value = entries;
      loadFailed.value = false;
    }
  } catch {
    if (!disposed && version === requestVersion) loadFailed.value = true;
  } finally {
    if (!disposed && version === requestVersion) {
      loading.value = false;
      revealRoute();
    }
  }
}

function updateFileHistory() {
  fileHistory.value = readRecentFileHosts();
}

/** The New page lives in its own Tab; the owner shell replaces it with the Tab it creates. */
async function runAndReplace(action: WorkspaceTabShellAction) {
  if (actionBusy.value) return;
  actionBusy.value = true;
  try {
    await requestWorkspaceTabShellAction(action, { closeSourceOnSuccess: true });
  } catch (error) {
    showWorkspaceTabFailure(error, "new-workspace-action");
  } finally {
    actionBusy.value = false;
  }
}

function openTerminalHost(hostId: string) {
  picker.value = null;
  void runAndReplace({ type: "navigate", path: "/terminal", query: { hostId, source: "overview", connectOperationId: createUuidV7() } });
}
function openSftpHost(hostId: string) {
  picker.value = null;
  void runAndReplace({ type: "navigate", path: "/sftp", query: { hostId, fileOperationId: createUuidV7() } });
}
function openLocalTerminal() {
  picker.value = null;
  void runAndReplace({ type: "new-terminal", behavior: "local" });
}
function openLocalFiles() {
  void runAndReplace({ type: "new-file", kind: "local" });
}
function openHosts(create = false) {
  void runAndReplace({ type: "navigate", path: "/hosts", query: create ? { create: "1" } : undefined });
}
function showHostPicker(kind: "terminal" | "sftp") {
  if (actionBusy.value) return;
  if (catalog.value.length) picker.value = kind;
  else openHosts(true);
}

onMounted(async () => {
  window.addEventListener(RECENT_FILE_HOSTS_CHANGED, updateFileHistory);
  await refresh();
  if (!disposed && canUseDesktopCore()) {
    try {
      const stop = await onSavedConnectionsChanged(() => { void refresh(); });
      if (disposed) stop();
      else stopSavedConnections = stop;
    }
    catch { /* The page can still refresh on a later visit. */ }
  }
});
onBeforeUnmount(() => {
  disposed = true;
  requestVersion++;
  stopSavedConnections?.();
  window.removeEventListener(RECENT_FILE_HOSTS_CHANGED, updateFileHistory);
});
</script>

<template>
  <main class="new-workspace">
    <div class="new-workspace__content">
      <header class="new-workspace__header">
        <div>
          <h1>{{ t("newWorkspace.title") }}</h1>
          <p>{{ t("newWorkspace.subtitle") }}</p>
        </div>
        <NvxButton
          variant="secondary"
          :disabled="actionBusy"
          @click="openHosts(true)"
        >
          <NvxIcon
            :icon="Plus"
            :size="16"
          />
          {{ t("newWorkspace.addHost") }}
        </NvxButton>
      </header>

      <section
        class="new-workspace__recent"
        :aria-label="t('newWorkspace.recentHosts')"
      >
        <div class="new-workspace__recent-heading">
          <h2>
            <NvxIcon
              :icon="Clock3"
              :size="22"
            />{{ t("newWorkspace.recentHosts") }}
            <span class="new-workspace__recent-hint">{{ t("newWorkspace.recentHostsSortHint") }}</span>
          </h2>
          <button
            type="button"
            :disabled="actionBusy"
            @click="openHosts()"
          >
            {{ t("newWorkspace.viewAll") }} <NvxIcon
              :icon="ArrowRight"
              :size="16"
            />
          </button>
        </div>

        <div
          v-if="loading"
          class="new-workspace__recent-list"
          role="status"
        >
          <span class="new-workspace__sr-only">{{ t("newWorkspace.loading") }}</span>
          <div
            v-for="n in 3"
            :key="n"
            class="new-workspace__skeleton-row"
            aria-hidden="true"
          >
            <span class="new-workspace__skeleton new-workspace__skeleton--avatar" />
            <span class="new-workspace__skeleton-lines">
              <span class="new-workspace__skeleton new-workspace__skeleton--label" />
              <span class="new-workspace__skeleton new-workspace__skeleton--address" />
            </span>
            <span class="new-workspace__skeleton new-workspace__skeleton--meta" />
          </div>
        </div>
        <NvxInlineNotice
          v-else-if="loadFailed"
          tone="error"
          :title="t('newWorkspace.loadFailed')"
        >
          <p>{{ t("newWorkspace.loadFailedDescription") }}</p>
          <NvxButton
            variant="secondary"
            size="sm"
            @click="refresh"
          >
            {{ t("newWorkspace.retry") }}
          </NvxButton>
        </NvxInlineNotice>
        <NvxInlineNotice
          v-else-if="!recentHosts.length"
          tone="info"
          :title="t('newWorkspace.emptyRecentTitle')"
        >
          <p>{{ t("newWorkspace.emptyRecentDescription") }}</p>
          <div class="new-workspace__empty-actions">
            <NvxButton
              size="sm"
              :disabled="actionBusy"
              @click="openHosts(true)"
            >
              {{ t("newWorkspace.addHost") }}
            </NvxButton>
            <NvxButton
              variant="secondary"
              size="sm"
              :disabled="actionBusy"
              @click="openLocalTerminal"
            >
              {{ t("newWorkspace.localTerminal") }}
            </NvxButton>
          </div>
        </NvxInlineNotice>
        <div
          v-else
          class="new-workspace__recent-list"
        >
          <div
            v-for="entry in recentHosts"
            :key="entry.host.hostId"
            class="new-workspace__recent-row"
            tabindex="0"
          >
            <NvxIcon
              :icon="Server"
              :size="22"
            />
            <span class="new-workspace__host-name"><strong>{{ entry.host.label }}</strong><small>{{ entry.host.normalizedAddress }}:{{ entry.host.port }}</small></span>
            <span class="new-workspace__used"><NvxIcon
              :icon="SquareTerminal"
              :size="20"
            /><span>{{ t("newWorkspace.lastTerminal") }}<small>{{ lastUsed(entry.recentConnection?.connectedAtUnixMs) }}</small></span></span>
            <span class="new-workspace__used"><NvxIcon
              :icon="FolderOpen"
              :size="20"
            /><span>{{ t("newWorkspace.lastSftp") }}<small>{{ lastUsed(fileHistory[entry.host.hostId]) }}</small></span></span>
            <span
              class="new-workspace__favorite"
              :class="{ 'new-workspace__favorite--active': entry.host.favorite }"
              :title="entry.host.favorite ? t('sshTerminal.favoriteHost') : undefined"
              aria-hidden="true"
            ><NvxIcon
              :icon="Star"
              :size="16"
            /></span>
            <div class="new-workspace__row-actions">
              <NvxButton
                variant="secondary"
                size="sm"
                :disabled="actionBusy"
                @click="openTerminalHost(entry.host.hostId)"
              >
                <NvxIcon
                  :icon="SquareTerminal"
                  :size="16"
                />{{ t("newWorkspace.terminalAction") }}
              </NvxButton>
              <NvxButton
                variant="secondary"
                size="sm"
                :disabled="actionBusy"
                @click="openSftpHost(entry.host.hostId)"
              >
                <NvxIcon
                  :icon="FolderOpen"
                  :size="16"
                />{{ t("newWorkspace.sftpAction") }}
              </NvxButton>
            </div>
          </div>
        </div>
      </section>

      <section class="new-workspace__new-connection">
        <h2 class="new-workspace__section-title">
          {{ t("newWorkspace.newConnectionHeading") }}
        </h2>
        <div class="new-workspace__tiles">
          <div
            class="new-workspace__tile"
            :aria-label="t('newWorkspace.terminalTitle')"
          >
            <div class="new-workspace__tile-info">
              <span class="new-workspace__tile-icon"><NvxIcon
                :icon="SquareTerminal"
                :size="20"
              /></span>
              <div>
                <div class="new-workspace__tile-title">
                  {{ t("newWorkspace.terminalTitle") }}
                </div>
                <p class="new-workspace__tile-description">
                  {{ t("newWorkspace.terminalDescription") }}
                </p>
              </div>
            </div>
            <div class="new-workspace__tile-actions">
              <button
                type="button"
                class="new-workspace__tile-link"
                :disabled="actionBusy"
                @click="openLocalTerminal"
              >
                {{ t("newWorkspace.localTerminal") }}
              </button>
              <NvxButton
                size="sm"
                :disabled="actionBusy || loading || loadFailed"
                @click="showHostPicker('terminal')"
              >
                {{ t("newWorkspace.connectTerminal") }} <NvxIcon
                  :icon="ArrowRight"
                  :size="16"
                />
              </NvxButton>
            </div>
          </div>

          <div
            class="new-workspace__tile"
            :aria-label="t('newWorkspace.sftpTitle')"
          >
            <div class="new-workspace__tile-info">
              <span class="new-workspace__tile-icon"><NvxIcon
                :icon="FolderOpen"
                :size="20"
              /></span>
              <div>
                <div class="new-workspace__tile-title">
                  {{ t("newWorkspace.sftpTitle") }}
                </div>
                <p class="new-workspace__tile-description">
                  {{ t("newWorkspace.sftpDescription") }}
                </p>
              </div>
            </div>
            <div class="new-workspace__tile-actions">
              <button
                type="button"
                class="new-workspace__tile-link"
                :disabled="actionBusy"
                @click="openLocalFiles"
              >
                {{ t("newWorkspace.localFiles") }}
              </button>
              <NvxButton
                size="sm"
                :disabled="actionBusy || loading || loadFailed"
                @click="showHostPicker('sftp')"
              >
                {{ t("newWorkspace.connectSftp") }} <NvxIcon
                  :icon="ArrowRight"
                  :size="16"
                />
              </NvxButton>
            </div>
          </div>
        </div>
      </section>
    </div>

    <NvxDialog
      :model-value="picker !== null"
      :title="t(picker === 'sftp' ? 'newWorkspace.selectSftpHost' : 'newWorkspace.selectTerminalHost')"
      :close-label="t('newWorkspace.closePicker')"
      @update:model-value="(open) => { if (!open) picker = null; }"
    >
      <div class="new-workspace__picker-list">
        <button
          v-for="entry in catalog"
          :key="entry.host.hostId"
          :disabled="actionBusy"
          type="button"
          @click="picker === 'sftp' ? openSftpHost(entry.host.hostId) : openTerminalHost(entry.host.hostId)"
        >
          <NvxIcon
            :icon="Server"
            :size="20"
          />
          <span><strong>{{ entry.host.label }}</strong><small>{{ entry.host.normalizedAddress }}:{{ entry.host.port }}</small></span>
          <NvxIcon
            :icon="ArrowRight"
            :size="16"
          />
        </button>
      </div>
    </NvxDialog>
  </main>
</template>

<style scoped>
.new-workspace { width:100%; height:100%; overflow:auto; background:var(--nvx-color-bg-canvas); }
.new-workspace__content { max-width:1320px; margin:0 auto; padding:36px 40px 50px; }
.new-workspace__header,.new-workspace__recent-heading { display:flex; align-items:center; justify-content:space-between; gap:24px; }
.new-workspace__header { margin-bottom:28px; }
.new-workspace__header h1 { margin:0 0 5px; font-size:32px; line-height:1.25; }
.new-workspace__header p { margin:0; color:var(--nvx-color-text-secondary); font-size:17px; }

.new-workspace__recent { margin-bottom:28px; }
.new-workspace__recent-heading { margin-bottom:13px; }
.new-workspace__recent-heading h2 { display:flex; align-items:center; gap:10px; margin:0; font-size:18px; }
.new-workspace__recent-hint { font-size:11.5px; font-weight:400; color:var(--nvx-color-text-tertiary); }
.new-workspace__recent-heading button { display:flex; align-items:center; gap:8px; border:0; background:transparent; color:var(--nvx-color-accent); cursor:pointer; font:inherit; }
.new-workspace__recent-list { overflow:hidden; border:1px solid var(--nvx-color-border); border-radius:7px; background:var(--nvx-color-bg-surface); }
.new-workspace__recent-row { display:grid; grid-template-columns:36px minmax(140px,1.2fr) minmax(150px,1fr) minmax(150px,1fr) 24px auto; align-items:center; gap:16px; min-height:74px; padding:0 24px; border-bottom:1px solid var(--nvx-color-border); outline:none; transition:background .12s ease; }
.new-workspace__recent-row:last-child { border-bottom:0; }
.new-workspace__recent-row:hover { background:var(--nvx-color-bg-hover); }
.new-workspace__recent-row:focus-visible { background:var(--nvx-color-bg-hover); box-shadow:inset 0 0 0 2px var(--nvx-color-focus-ring); }
.new-workspace__host-name,.new-workspace__used span { display:flex; flex-direction:column; min-width:0; }
.new-workspace__host-name strong { overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
.new-workspace__host-name small,.new-workspace__used small { overflow:hidden; color:var(--nvx-color-text-tertiary); text-overflow:ellipsis; white-space:nowrap; }
.new-workspace__used { display:flex; align-items:center; gap:12px; min-width:0; color:var(--nvx-color-text-secondary); font-size:12px; }
.new-workspace__favorite { display:grid; place-items:center; width:24px; height:24px; color:var(--nvx-color-text-tertiary); }
.new-workspace__favorite--active { color:var(--nvx-color-accent); }
.new-workspace__favorite--active :deep(svg) { fill:currentColor; }
.new-workspace__row-actions { display:flex; gap:9px; }
.new-workspace__row-actions :deep(.nvx-button) { min-width:104px; }
.new-workspace__empty-actions { display:flex; gap:10px; margin-top:10px; }
.new-workspace__sr-only { position:absolute; width:1px; height:1px; overflow:hidden; clip:rect(0,0,0,0); white-space:nowrap; }
.new-workspace__skeleton-row { display:grid; grid-template-columns:32px 1.2fr 1fr; align-items:center; gap:16px; min-height:74px; padding:0 24px; border-bottom:1px solid var(--nvx-color-border); }
.new-workspace__skeleton-row:last-child { border-bottom:0; }
.new-workspace__skeleton-lines { display:flex; flex-direction:column; gap:6px; min-width:0; }
.new-workspace__skeleton { display:block; height:10px; border-radius:4px; background:linear-gradient(90deg,var(--nvx-color-bg-subtle) 25%,var(--nvx-color-border) 37%,var(--nvx-color-bg-subtle) 63%); background-size:400% 100%; animation:new-workspace-shimmer 1.6s ease infinite; }
.new-workspace__skeleton--avatar { width:32px; height:32px; border-radius:50%; }
.new-workspace__skeleton--label { width:55%; height:12px; }
.new-workspace__skeleton--address { width:38%; }
.new-workspace__skeleton--meta { width:65%; justify-self:end; }
@keyframes new-workspace-shimmer { 0% { background-position:100% 50%; } 100% { background-position:0 50%; } }

.new-workspace__section-title { margin:0 0 10px; font-size:13px; font-weight:700; color:var(--nvx-color-text-secondary); }
.new-workspace__tiles { display:grid; grid-template-columns:repeat(2,minmax(0,1fr)); gap:14px; }
.new-workspace__tile { display:flex; align-items:center; justify-content:space-between; gap:14px; min-width:0; padding:16px; border:1px solid var(--nvx-color-border); border-radius:8px; background:var(--nvx-color-bg-surface); transition:border-color .15s ease,background .15s ease; }
.new-workspace__tile:hover { border-color:var(--nvx-color-accent); background:var(--nvx-color-bg-hover); }
.new-workspace__tile-info { display:flex; align-items:center; gap:12px; min-width:0; }
.new-workspace__tile-icon { display:grid; place-items:center; width:36px; height:36px; flex:none; border-radius:8px; color:var(--nvx-color-accent); background:var(--nvx-color-accent-soft); }
.new-workspace__tile-title { font-size:14px; font-weight:700; }
.new-workspace__tile-description { margin:0; font-size:12px; color:var(--nvx-color-text-secondary); }
.new-workspace__tile-actions { display:flex; align-items:center; gap:12px; flex:none; }
/* Linux system fonts run wider than the designed metrics: wrap the actions under the description instead of squeezing it. */
html[data-nvx-shape] .new-workspace__tile { flex-wrap:wrap; row-gap:12px; }
html[data-nvx-shape] .new-workspace__tile-info { flex:1 1 240px; }
html[data-nvx-shape] .new-workspace__tile-actions { flex-wrap:wrap; }
.new-workspace__tile-link { border:0; background:transparent; color:var(--nvx-color-accent); font-size:12.5px; cursor:pointer; font:inherit; }
.new-workspace__tile-link:hover { text-decoration:underline; }
.new-workspace__tile-link:disabled { color:var(--nvx-color-text-tertiary); cursor:default; }

.new-workspace__picker-list { display:grid; gap:6px; max-height:min(410px,60vh); overflow:auto; }
.new-workspace__picker-list button { display:flex; align-items:center; gap:14px; width:100%; padding:12px; border:1px solid var(--nvx-color-border); border-radius:6px; background:var(--nvx-color-bg-surface); color:var(--nvx-color-text-primary); text-align:left; cursor:pointer; }
.new-workspace__picker-list button:hover { background:var(--nvx-color-bg-hover); }.new-workspace__picker-list button span { display:flex; flex:1; flex-direction:column; }.new-workspace__picker-list button small { color:var(--nvx-color-text-tertiary); }
button:focus-visible { outline:2px solid var(--nvx-color-focus-ring); outline-offset:2px; }
@media (max-width:1100px) { .new-workspace__content { padding:28px 24px; }.new-workspace__recent-row { grid-template-columns:28px minmax(130px,1fr) minmax(110px,1fr) minmax(110px,1fr) 20px; }.new-workspace__row-actions { grid-column:1/-1; padding:10px 0 14px; } }
@media (max-width:850px) { .new-workspace__tiles { grid-template-columns:1fr; }.new-workspace__recent-row { grid-template-columns:28px minmax(120px,1fr) minmax(110px,1fr) 20px; }.new-workspace__used:nth-of-type(2) { display:none; } }
@media (max-width:560px) { .new-workspace__content { padding:20px 14px; }.new-workspace__header { align-items:flex-start; flex-direction:column; }.new-workspace__tile { flex-direction:column; align-items:flex-start; }.new-workspace__tile-actions { width:100%; justify-content:space-between; }.new-workspace__recent-row { padding:0 12px; gap:8px; }.new-workspace__used { display:none; } }
</style>
