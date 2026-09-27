<script setup lang="ts">
import { ArrowRight, Clock3, FilePlus2, Folder, FolderOpen, Menu, Monitor, Plus, Server, SquareTerminal } from "lucide-vue-next";
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";

import { NvxButton, NvxDialog, NvxIcon } from "../components/ui";
import { canUseDesktopCore, listHostCatalog } from "../core-api/client";
import type { HostCatalogEntry } from "../core-api/generated/core-api";
import { createUuidV7 } from "../core-api/ids";
import { RECENT_FILE_HOSTS_CHANGED, readRecentFileHosts } from "../recent-file-hosts";
import { useRouteReveal } from "../routeReveal";
import { onSavedConnectionsChanged } from "../saved-connections";
import { useWorkspaceTabsStore } from "../stores/workspaceTabs";

defineOptions({ name: "NewWorkspacePageView" });
const { t, locale } = useI18n();
const router = useRouter();
const revealRoute = useRouteReveal();
const workspaceTabs = useWorkspaceTabsStore();
const catalog = ref<HostCatalogEntry[]>([]);
const fileHistory = ref<Record<string, number>>({});
const loading = ref(true);
const loadFailed = ref(false);
const picker = ref<"terminal" | "sftp" | null>(null);
let stopSavedConnections: (() => void) | undefined;
let disposed = false;
let requestVersion = 0;

const recentHosts = computed(() => catalog.value
  .filter((entry) => entry.recentConnection || fileHistory.value[entry.host.hostId])
  .sort((a, b) => Math.max(b.recentConnection?.connectedAtUnixMs ?? 0, fileHistory.value[b.host.hostId] ?? 0)
    - Math.max(a.recentConnection?.connectedAtUnixMs ?? 0, fileHistory.value[a.host.hostId] ?? 0))
  .slice(0, 3));

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

function openTerminalHost(hostId: string) {
  picker.value = null;
  void router.push({
    path: "/terminal",
    query: { hostId, source: "overview", connectOperationId: createUuidV7() },
  });
}
function openSftpHost(hostId: string) {
  picker.value = null;
  void router.push({
    path: "/sftp",
    query: { hostId, fileOperationId: createUuidV7() },
  });
}
function openLocalTerminal() {
  picker.value = null;
  const controller = workspaceTabs.terminalController;
  if (controller) {
    if (controller.createLocal()) void router.push("/terminal");
    return;
  }
  workspaceTabs.queueTerminalCreation("local");
  void router.push("/terminal");
}
function openLocalFiles() {
  workspaceTabs.createFileTab("local");
  void router.push("/sftp");
}
function showHostPicker(kind: "terminal" | "sftp") {
  if (catalog.value.length) picker.value = kind;
  else void router.push({ path: "/hosts", query: { create: "1" } });
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
          @click="router.push({ path: '/hosts', query: { create: '1' } })"
        >
          <NvxIcon
            :icon="Plus"
            :size="16"
          />
          {{ t("newWorkspace.addHost") }}
        </NvxButton>
      </header>

      <div class="new-workspace__cards">
        <section
          class="new-workspace__card"
          :aria-label="t('newWorkspace.terminalTitle')"
        >
          <div class="new-workspace__card-heading">
            <span class="new-workspace__card-icon"><NvxIcon
              :icon="SquareTerminal"
              :size="22"
            /></span>
            <div>
              <h2>{{ t("newWorkspace.terminalTitle") }}</h2>
              <p>{{ t("newWorkspace.terminalDescription") }}</p>
            </div>
          </div>
          <div
            class="new-workspace__terminal-preview"
            aria-hidden="true"
          >
            <div class="new-workspace__terminal-chrome">
              <span class="new-workspace__traffic"><i /><i /><i /></span>
              <span class="new-workspace__terminal-tab">{{ t("newWorkspace.previewTerminalTab") }} <span>×</span></span>
              <NvxIcon
                :icon="Plus"
                :size="16"
              />
              <NvxIcon
                class="new-workspace__terminal-menu"
                :icon="Menu"
                :size="16"
              />
            </div>
            <div class="new-workspace__terminal-screen">
              <div><em>user@server:~$</em> hostname</div>
              <div>prod.example.com</div>
              <div><em>user@server:~$</em> ls -la</div>
              <div>total 28</div>
              <div>drwxr-xr-x &nbsp; 5 user user &nbsp; 4096 &nbsp; .</div>
              <div>drwxr-xr-x &nbsp; 3 root root &nbsp; 4096 &nbsp; ..</div>
              <div>drwxr-xr-x &nbsp; 2 user user &nbsp; 4096 &nbsp; logs</div>
              <div>-rw-r--r-- &nbsp; 1 user user &nbsp; 220 &nbsp; README.md</div>
              <div><em>user@server:~$</em> ▌</div>
            </div>
          </div>
          <div class="new-workspace__actions">
            <NvxButton
              :disabled="loading || loadFailed"
              @click="showHostPicker('terminal')"
            >
              {{ t("newWorkspace.connectTerminal") }} <NvxIcon
                :icon="ArrowRight"
                :size="16"
              />
            </NvxButton>
            <NvxButton
              variant="secondary"
              @click="openLocalTerminal"
            >
              {{ t("newWorkspace.localTerminal") }} <NvxIcon
                :icon="ArrowRight"
                :size="16"
              />
            </NvxButton>
          </div>
        </section>

        <section
          class="new-workspace__card"
          :aria-label="t('newWorkspace.sftpTitle')"
        >
          <div class="new-workspace__card-heading">
            <span class="new-workspace__card-icon"><NvxIcon
              :icon="FolderOpen"
              :size="22"
            /></span>
            <div>
              <h2>{{ t("newWorkspace.sftpTitle") }}</h2>
              <p>{{ t("newWorkspace.sftpDescription") }}</p>
            </div>
          </div>
          <div
            class="new-workspace__files-preview"
            aria-hidden="true"
          >
            <div
              v-for="kind in ['local', 'remote'] as const"
              :key="kind"
              class="new-workspace__mini-file-pane"
            >
              <div class="new-workspace__mini-toolbar">
                <NvxIcon
                  :icon="kind === 'local' ? Monitor : Server"
                  :size="16"
                />
                {{ t(`newWorkspace.preview.${kind}`) }}
                <NvxIcon
                  class="new-workspace__mini-menu"
                  :icon="Menu"
                  :size="16"
                />
              </div>
              <div class="new-workspace__mini-path">
                {{ kind === "local" ? "~/" : "/home/user" }}
              </div>
              <div class="new-workspace__mini-sort">
                {{ t("newWorkspace.preview.name") }} ↓
              </div>
              <div
                v-for="name in (kind === 'local' ? ['Desktop', 'Documents', 'Downloads', 'Pictures', 'README.md'] : ['logs', 'nginx', 'projects', 'scripts', 'config.ini'])"
                :key="name"
                class="new-workspace__mini-entry"
              >
                <NvxIcon
                  :icon="name.includes('.') ? FilePlus2 : Folder"
                  :size="16"
                />
                {{ name }}
              </div>
            </div>
          </div>
          <div class="new-workspace__actions">
            <NvxButton
              :disabled="loading || loadFailed"
              @click="showHostPicker('sftp')"
            >
              {{ t("newWorkspace.connectSftp") }} <NvxIcon
                :icon="ArrowRight"
                :size="16"
              />
            </NvxButton>
            <NvxButton
              variant="secondary"
              @click="openLocalFiles"
            >
              {{ t("newWorkspace.localFiles") }} <NvxIcon
                :icon="ArrowRight"
                :size="16"
              />
            </NvxButton>
          </div>
        </section>
      </div>

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
          </h2>
          <button
            type="button"
            @click="router.push('/hosts')"
          >
            {{ t("newWorkspace.viewAll") }} <NvxIcon
              :icon="ArrowRight"
              :size="16"
            />
          </button>
        </div>
        <div class="new-workspace__recent-list">
          <p
            v-if="loading"
            class="new-workspace__message"
            role="status"
          >
            {{ t("newWorkspace.loading") }}
          </p>
          <p
            v-else-if="loadFailed"
            class="new-workspace__message"
            role="alert"
          >
            {{ t("newWorkspace.loadFailed") }} <button
              type="button"
              @click="refresh"
            >
              {{ t("newWorkspace.retry") }}
            </button>
          </p>
          <p
            v-else-if="!recentHosts.length"
            class="new-workspace__message"
          >
            {{ t("newWorkspace.emptyRecent") }}
          </p>
          <div
            v-for="entry in recentHosts"
            :key="entry.host.hostId"
            class="new-workspace__recent-row"
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
            <div class="new-workspace__row-actions">
              <NvxButton
                variant="secondary"
                size="sm"
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
.new-workspace__header,.new-workspace__card-heading,.new-workspace__recent-heading { display:flex; align-items:center; justify-content:space-between; gap:24px; }
.new-workspace__header { margin-bottom:28px; }
.new-workspace__header h1 { margin:0 0 5px; font-size:32px; line-height:1.25; }
.new-workspace__header p,.new-workspace__card-heading p { margin:0; color:var(--nvx-color-text-secondary); }
.new-workspace__header p { font-size:17px; }
.new-workspace__cards { display:grid; grid-template-columns:repeat(2,minmax(0,1fr)); gap:20px; }
.new-workspace__card { display:flex; flex-direction:column; min-width:0; padding:24px; border:1px solid var(--nvx-color-border); border-radius:8px; background:var(--nvx-color-bg-surface); }
.new-workspace__card-heading { justify-content:flex-start; min-height:72px; margin-bottom:18px; }
.new-workspace__card-heading h2 { margin:0 0 5px; font-size:24px; line-height:1.25; }
.new-workspace__card-icon { display:grid; place-items:center; width:64px; height:64px; flex:none; border-radius:10px; color:var(--nvx-color-accent); background:var(--nvx-color-accent-soft); }
.new-workspace__terminal-preview,.new-workspace__files-preview { height:278px; min-height:278px; border:1px solid var(--nvx-color-border); border-radius:6px; overflow:hidden; }
.new-workspace__terminal-preview { display:flex; flex-direction:column; padding:4px; background:var(--nvx-color-bg-subtle); }
.new-workspace__terminal-chrome { display:flex; align-items:center; gap:16px; height:36px; padding:0 12px; color:var(--nvx-color-text-secondary); font-size:12px; }
.new-workspace__traffic { display:flex; gap:5px; }.new-workspace__traffic i { width:10px; height:10px; border-radius:50%; background:#fa5d59; }.new-workspace__traffic i:nth-child(2){background:#ffbd2e}.new-workspace__traffic i:nth-child(3){background:#28c840}
.new-workspace__terminal-tab { display:flex; align-items:center; justify-content:space-between; min-width:145px; padding:7px 10px; align-self:stretch; border:1px solid var(--nvx-color-border); border-bottom:0; border-radius:5px 5px 0 0; background:var(--nvx-color-bg-surface); color:var(--nvx-color-text-primary); }
.new-workspace__terminal-tab span,.new-workspace__terminal-menu,.new-workspace__mini-menu { margin-left:auto; }
.new-workspace__terminal-screen { flex:1; overflow:hidden; padding:13px 15px; border-radius:5px; color:#d5dfed; background:#1b202b; font:12px/1.55 var(--nvx-font-mono); white-space:nowrap; }
.new-workspace__terminal-screen em { color:#7fde83; font-style:normal; }
.new-workspace__files-preview { display:grid; grid-template-columns:repeat(2,minmax(0,1fr)); gap:7px; border:0; }
.new-workspace__mini-file-pane { overflow:hidden; border:1px solid var(--nvx-color-border); border-radius:6px; font-size:12px; }
.new-workspace__mini-toolbar,.new-workspace__mini-path,.new-workspace__mini-sort,.new-workspace__mini-entry { display:flex; align-items:center; gap:8px; min-height:28px; padding:0 12px; }
.new-workspace__mini-toolbar,.new-workspace__mini-sort { border-bottom:1px solid var(--nvx-color-border); background:var(--nvx-color-bg-subtle); }
.new-workspace__mini-path { color:var(--nvx-color-text-secondary); border-bottom:1px solid var(--nvx-color-border); }
.new-workspace__mini-sort { color:var(--nvx-color-text-secondary); font-weight:600; }
.new-workspace__mini-entry { color:var(--nvx-color-text-primary); }
.new-workspace__mini-entry :deep(svg) { color:var(--nvx-color-accent); fill:var(--nvx-color-accent-soft); }
.new-workspace__actions { display:grid; grid-template-columns:1fr 1fr; gap:12px; margin-top:18px; }
.new-workspace__actions :deep(.nvx-button) { min-width:0; min-height:50px; }
.new-workspace__recent { margin-top:30px; }
.new-workspace__recent-heading { margin-bottom:13px; }
.new-workspace__recent-heading h2 { display:flex; align-items:center; gap:12px; margin:0; font-size:18px; }
.new-workspace__recent-heading button,.new-workspace__message button { display:flex; align-items:center; gap:8px; border:0; background:transparent; color:var(--nvx-color-accent); cursor:pointer; font:inherit; }
.new-workspace__recent-list { overflow:hidden; border:1px solid var(--nvx-color-border); border-radius:7px; background:var(--nvx-color-bg-surface); }
.new-workspace__recent-row { display:grid; grid-template-columns:36px minmax(140px,1.2fr) minmax(150px,1fr) minmax(150px,1fr) auto; align-items:center; gap:16px; min-height:74px; margin:0 24px; border-bottom:1px solid var(--nvx-color-border); }
.new-workspace__recent-row:last-child { border-bottom:0; }
.new-workspace__host-name,.new-workspace__used span { display:flex; flex-direction:column; min-width:0; }
.new-workspace__host-name strong { overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
.new-workspace__host-name small,.new-workspace__used small { overflow:hidden; color:var(--nvx-color-text-tertiary); text-overflow:ellipsis; white-space:nowrap; }
.new-workspace__used { display:flex; align-items:center; gap:12px; min-width:0; color:var(--nvx-color-text-secondary); font-size:12px; }
.new-workspace__row-actions { display:flex; gap:9px; }.new-workspace__row-actions :deep(.nvx-button) { min-width:104px; }
.new-workspace__message { margin:0; padding:28px; color:var(--nvx-color-text-secondary); }
.new-workspace__picker-list { display:grid; gap:6px; max-height:min(410px,60vh); overflow:auto; }
.new-workspace__picker-list button { display:flex; align-items:center; gap:14px; width:100%; padding:12px; border:1px solid var(--nvx-color-border); border-radius:6px; background:var(--nvx-color-bg-surface); color:var(--nvx-color-text-primary); text-align:left; cursor:pointer; }
.new-workspace__picker-list button:hover { background:var(--nvx-color-bg-hover); }.new-workspace__picker-list button span { display:flex; flex:1; flex-direction:column; }.new-workspace__picker-list button small { color:var(--nvx-color-text-tertiary); }
button:focus-visible { outline:2px solid var(--nvx-color-focus-ring); outline-offset:2px; }
@media (max-width:1100px) { .new-workspace__content { padding:28px 24px; }.new-workspace__recent-row { grid-template-columns:28px minmax(130px,1fr) minmax(110px,1fr) minmax(110px,1fr); }.new-workspace__row-actions { grid-column:2/-1; padding-bottom:12px; } }
@media (max-width:850px) { .new-workspace__cards { grid-template-columns:1fr; }.new-workspace__recent-row { grid-template-columns:28px minmax(120px,1fr) minmax(110px,1fr); }.new-workspace__used:nth-of-type(2) { display:none; } }
@media (max-width:560px) { .new-workspace__content { padding:20px 14px; }.new-workspace__header { align-items:flex-start; flex-direction:column; }.new-workspace__card { padding:16px; }.new-workspace__actions { grid-template-columns:1fr; }.new-workspace__recent-row { gap:8px; margin:0 12px; }.new-workspace__used { display:none; } }
</style>
