<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { ArrowUpRight, ArrowUpDown, Bell, ChevronRight, Folder, LockKeyhole, MessageSquare, Network, Plus, Power, Server, Settings2, Terminal, X, Zap } from "lucide-vue-next";
import { NvxButton, NvxIcon } from "../components/ui";
import { executeTrayPanel, hideTrayPanel, readTrayPanel } from "../core-api/tray-panel";
import type { NativeTrayPanelRow, NativeTrayPanelSnapshot } from "../core-api/generated/core-api";
import brandIcon from "../assets/branding/norishell-app-icon.png";

const { t, locale } = useI18n();
const snapshot = ref<NativeTrayPanelSnapshot | null>(null);
const error = ref<"unavailable" | "actionFailed" | null>(null);
const busy = ref(false);
let alive = true;
let epoch = 0;
let reading = false;
let timer: ReturnType<typeof setInterval> | undefined;
const rows = computed(() => snapshot.value?.rows ?? []);
const show = computed(() => rows.value.find((row) => row.kind === "show"));
const quick = computed(() => rows.value.filter((row) => ["newTerminal", "newLocalTerminal", "quickConnect"].includes(row.kind)));
const statuses = computed(() => rows.value.filter((row) => row.kind === "status" && row.role !== "summary"));
const recent = computed(() => rows.value.find((row) => row.role === "recent"));
const resources = computed(() => rows.value.filter((row) => ["sessions", "tunnels", "transfers"].includes(row.role ?? "")));
const notifications = computed(() => rows.value.find((row) => row.role === "notifications"));
const vault = computed(() => rows.value.find((row) => row.role === "vault"));
const extraActions = computed(() => rows.value.filter((row) => row.kind === "action" && !row.role));
const footer = computed(() => rows.value.filter((row) => ["settings", "quit"].includes(row.kind)));
const expanded = ref<string | null>(null);
const expandedGroup = computed(() => rows.value.find((row) => row.role === expanded.value && row.kind === "group"));
const stats = computed(() => snapshot.value?.stats ?? []);
const quickIcon = (kind: NativeTrayPanelRow["kind"]) => kind === "newTerminal" ? Plus : kind === "newLocalTerminal" ? Terminal : Zap;
const resourceIcon = (kind: string | null) => kind === "tunnels" ? Network : kind === "sftp" ? Folder : kind === "transfers" ? ArrowUpDown : MessageSquare;
const count = (kind: string | null) => stats.value.find((stat) => stat.kind === kind)?.count;
const quickLabel = (kind: NativeTrayPanelRow["kind"]) => t(`trayPanel.${kind === "newLocalTerminal" ? "localTerminal" : kind}`);
const notificationLabel = computed(() => snapshot.value?.notificationState === "active"
  ? t("trayPanel.receiving") : snapshot.value?.notificationState === "paused"
    ? notifications.value?.label : t("trayPanel.unavailableShort"));
function toggleGroup(role: string | null) {
  if (!busy.value) expanded.value = expanded.value === role ? null : role;
}

async function refresh() {
  if (!alive || reading || busy.value || !document.hasFocus()) return;
  const current = epoch;
  reading = true;
  try {
    const next = await readTrayPanel();
    if (!alive || current !== epoch || !document.hasFocus()) return;
    snapshot.value = next;
    locale.value = next.locale;
    document.documentElement.lang = next.locale;
    error.value = null;
  } catch {
    if (!alive || current !== epoch) return;
    // Failure retains no clickable stale token and never wakes the main window from the tray panel.
    snapshot.value = null;
    error.value = "unavailable";
  } finally {
    reading = false;
  }
}

async function execute(row: NativeTrayPanelRow) {
  if (!row.id || busy.value) return;
  const current = epoch;
  busy.value = true;
  error.value = null;
  try {
    await executeTrayPanel(row.id);
  } catch {
    if (alive && current === epoch) {
      snapshot.value = null;
      error.value = "actionFailed";
    }
  } finally {
    busy.value = false;
  }
}

async function dismiss() {
  try {
    await hideTrayPanel();
  } catch {
    if (alive) error.value = "actionFailed";
  }
}

function blur() {
  expanded.value = null;
  epoch += 1;
  snapshot.value = null;
  error.value = null;
}
function keydown(event: KeyboardEvent) {
  if (event.key === "Escape") {
    event.preventDefault();
    void dismiss();
  }
}
onMounted(() => {
  window.addEventListener("focus", refresh);
  window.addEventListener("blur", blur);
  window.addEventListener("keydown", keydown);
  // Keep the target stable while a keyboard action operates a button; explicit refresh may still reread it.
  timer = setInterval(() => {
    if (!(document.activeElement instanceof HTMLButtonElement)) void refresh();
  }, 2_000);
  void refresh();
});
onBeforeUnmount(() => {
  alive = false;
  epoch += 1;
  clearInterval(timer);
  window.removeEventListener("focus", refresh);
  window.removeEventListener("blur", blur);
  window.removeEventListener("keydown", keydown);
});
</script>

<template>
  <main
    class="tray-panel"
    :aria-label="t('trayPanel.title')"
  >
    <header class="tray-panel__header">
      <img
        :src="brandIcon"
        width="28"
        height="28"
        alt=""
      >
      <strong>NoriShell</strong>
      <NvxButton
        v-if="show"
        variant="ghost"
        size="sm"
        :disabled="busy"
        @click="execute(show)"
      >
        <span>{{ t('trayPanel.openMain') }}</span>
        <NvxIcon
          :icon="ArrowUpRight"
          :size="16"
        />
      </NvxButton>
      <NvxButton
        variant="ghost"
        size="sm"
        class="tray-panel__close"
        :aria-label="t('trayPanel.close')"
        @click="dismiss"
      >
        <NvxIcon
          :icon="X"
          :size="16"
        />
      </NvxButton>
    </header>

    <div class="tray-panel__body">
      <div
        v-if="error"
        class="tray-panel__notice"
        role="alert"
      >
        <p>{{ t(`trayPanel.${error}`) }}</p>
        <NvxButton
          variant="secondary"
          size="sm"
          @click="refresh"
        >
          {{ t('trayPanel.retry') }}
        </NvxButton>
      </div>
      <p
        v-else-if="!snapshot"
        class="tray-panel__notice"
        role="status"
      >
        {{ t('trayPanel.loading') }}
      </p>

      <div
        v-if="stats.length"
        class="tray-panel__stats"
        :aria-label="t('trayPanel.resourceSummary')"
      >
        <div
          v-for="stat in stats"
          :key="stat.kind"
          class="tray-panel__stat"
        >
          <span><NvxIcon
            :icon="resourceIcon(stat.kind)"
            :size="16"
          />{{ t(`trayPanel.${stat.kind}`) }}</span>
          <strong :aria-label="stat.count === null ? t('trayPanel.unavailableShort') : undefined">{{ stat.count ?? '—' }}</strong>
        </div>
      </div>
      <p
        v-for="row in statuses"
        :key="row.label"
        class="tray-panel__notice"
        role="status"
      >
        {{ row.label }}
      </p>

      <p
        v-if="snapshot?.errorSummary"
        class="tray-panel__notice"
        role="status"
      >
        {{ snapshot.errorSummary }}
      </p>

      <section
        v-if="quick.length"
        class="tray-panel__quick"
        :aria-label="t('trayPanel.quickActions')"
      >
        <NvxButton
          v-for="row in quick"
          :key="row.kind"
          variant="secondary"
          size="sm"
          :disabled="busy || !row.id"
          @click="execute(row)"
        >
          <NvxIcon
            :icon="quickIcon(row.kind)"
            :size="16"
          />
          <span>{{ quickLabel(row.kind) }}</span>
        </NvxButton>
      </section>

      <section
        v-if="recent"
        class="tray-panel__recent"
        :aria-label="recent.label"
      >
        <h2>{{ recent.label }}</h2>
        <template
          v-for="(row, index) in recent.children"
          :key="row.id ?? `recent-empty-${index}`"
        >
          <NvxButton
            v-if="row.id"
            variant="ghost"
            size="sm"
            class="tray-panel__row tray-panel__host"
            :disabled="busy"
            @click="execute(row)"
          >
            <NvxIcon
              :icon="Server"
              :size="16"
            />
            <span
              class="tray-panel__label"
              :title="row.label"
            >{{ row.label }}</span>
            <NvxIcon
              :icon="ChevronRight"
              :size="16"
            />
          </NvxButton>
          <p
            v-else
            class="tray-panel__empty"
          >
            {{ row.label }}
          </p>
        </template>
      </section>

      <nav
        v-if="resources.length"
        class="tray-panel__resources"
        :aria-label="t('trayPanel.resources')"
      >
        <NvxButton
          v-for="group in resources"
          :key="group.role!"
          variant="ghost"
          size="sm"
          :disabled="busy"
          :aria-expanded="expanded === group.role"
          aria-controls="tray-panel-expanded"
          @click="toggleGroup(group.role)"
        >
          <NvxIcon
            :icon="resourceIcon(group.role)"
            :size="16"
          />
          <span>{{ group.label }}</span>
          <span
            v-if="stats.length"
            class="tray-panel__count"
          >{{ count(group.role) ?? '—' }}</span>
          <NvxIcon
            :icon="ChevronRight"
            :size="16"
          />
        </NvxButton>
      </nav>

      <section
        v-if="expandedGroup && expandedGroup.role !== 'notifications'"
        id="tray-panel-expanded"
        class="tray-panel__expanded"
        :aria-label="expandedGroup.label"
      >
        <template
          v-for="(row, index) in expandedGroup.children"
          :key="row.id ?? `resource-empty-${index}`"
        >
          <NvxButton
            v-if="row.id"
            variant="ghost"
            size="sm"
            class="tray-panel__row"
            :disabled="busy"
            @click="execute(row)"
          >
            <span
              class="tray-panel__label"
              :title="row.label"
            >{{ row.label }}</span><NvxIcon
              :icon="ChevronRight"
              :size="16"
            />
          </NvxButton>
          <p
            v-else
            class="tray-panel__empty"
          >
            {{ row.label }}
          </p>
        </template>
      </section>

      <div
        v-if="notifications || vault || extraActions.length"
        class="tray-panel__utilities"
      >
        <NvxButton
          v-if="notifications"
          variant="ghost"
          size="sm"
          class="tray-panel__row"
          :disabled="busy"
          :aria-expanded="expanded === 'notifications'"
          aria-controls="tray-panel-notifications"
          @click="toggleGroup('notifications')"
        >
          <NvxIcon
            :icon="Bell"
            :size="20"
          /><span class="tray-panel__label">{{ t('trayPanel.notifications') }}</span>
          <span class="tray-panel__secondary">{{ notificationLabel }}</span><NvxIcon
            :icon="ChevronRight"
            :size="16"
          />
        </NvxButton>
        <div
          v-if="expandedGroup?.role === 'notifications'"
          id="tray-panel-notifications"
          class="tray-panel__expanded"
        >
          <template
            v-for="(row, index) in expandedGroup.children"
            :key="row.id ?? `notification-empty-${index}`"
          >
            <NvxButton
              v-if="row.id"
              variant="ghost"
              size="sm"
              class="tray-panel__row"
              :disabled="busy"
              @click="execute(row)"
            >
              {{ row.label }}
            </NvxButton>
            <p
              v-else
              class="tray-panel__empty"
            >
              {{ row.label }}
            </p>
          </template>
        </div>
        <NvxButton
          v-if="vault"
          variant="ghost"
          size="sm"
          class="tray-panel__row"
          :disabled="busy || !vault.id"
          @click="execute(vault)"
        >
          <NvxIcon
            :icon="LockKeyhole"
            :size="20"
          /><span
            class="tray-panel__label"
            :title="vault.label"
          >{{ vault.label }}</span><NvxIcon
            :icon="ChevronRight"
            :size="16"
          />
        </NvxButton>
        <NvxButton
          v-for="row in extraActions"
          :key="row.id!"
          variant="ghost"
          size="sm"
          class="tray-panel__row"
          :disabled="busy || !row.id"
          @click="execute(row)"
        >
          {{ row.label }}
        </NvxButton>
      </div>
    </div>

    <footer
      v-if="footer.length"
      class="tray-panel__footer"
    >
      <NvxButton
        v-for="row in footer"
        :key="row.kind"
        variant="ghost"
        size="sm"
        :disabled="busy || !row.id"
        @click="execute(row)"
      >
        <NvxIcon
          :icon="row.kind === 'quit' ? Power : Settings2"
          :size="16"
        /><span>{{ t(`trayPanel.${row.kind}`) }}</span>
      </NvxButton>
    </footer>
  </main>
</template>

<style scoped>
.tray-panel {
  display: flex;
  flex-direction: column;
  height: 100%;
  overflow: hidden;
  font-size: var(--nvx-font-size-sm);
}

.tray-panel__header {
  display: flex;
  align-items: center;
  gap: 8px;
  min-height: 54px;
  padding: 0 12px;
  border-bottom: 1px solid var(--nvx-color-border);
}

.tray-panel__header strong {
  flex: 1;
  font-size: var(--nvx-font-size-md);
  font-weight: var(--nvx-font-weight-semibold);
}

.tray-panel__header .nvx-button {
  padding: 0 4px;
}

.tray-panel__close {
  border-left: 1px solid var(--nvx-color-border);
  border-radius: 0;
}

.tray-panel__header :deep(.nvx-button__content),
.tray-panel__footer :deep(.nvx-button__content) {
  display: flex;
  align-items: center;
  gap: 6px;
}

.tray-panel__body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  overflow-x: hidden;
  padding: 12px 12px 8px;
}

.tray-panel__stats {
  display: grid;
  grid-template-columns: repeat(4, minmax(0, 1fr));
  margin-bottom: 14px;
}

.tray-panel__stat {
  padding: 0 8px;
  border-right: 1px solid var(--nvx-color-border);
}

.tray-panel__stat:last-child {
  border-right: 0;
}

.tray-panel__stat > span {
  display: flex;
  align-items: center;
  gap: 7px;
  color: var(--nvx-color-text-secondary);
  white-space: nowrap;
  font-size: var(--nvx-font-size-xs);
}

.tray-panel__stat strong {
  display: block;
  margin: 3px 0 0 23px;
  font-size: 18px;
  line-height: 22px;
  font-weight: var(--nvx-font-weight-medium);
}

.tray-panel__quick {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 6px;
  margin-bottom: 12px;
}

.tray-panel__quick .nvx-button {
  padding: 0 6px;
  min-height: 38px;
  font-size: var(--nvx-font-size-xs);
}

.tray-panel__quick :deep(.nvx-button__content) {
  display: flex;
  align-items: center;
  gap: 7px;
}

.tray-panel__quick svg, .tray-panel__resources svg:first-child {
  color: var(--nvx-color-accent);
}

.tray-panel__recent {
  margin-bottom: 10px;
}

.tray-panel__recent h2 {
  margin: 0 0 5px;
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-xs);
  line-height: 18px;
  font-weight: var(--nvx-font-weight-medium);
}

.tray-panel__row {
  width: 100%;
  padding: 0 6px;
  text-align: left;
  font-weight: var(--nvx-font-weight-regular);
}

.tray-panel__row :deep(.nvx-button__content) {
  display: flex;
  align-items: center;
  gap: 10px;
  width: 100%;
}

.tray-panel__host {
  min-height: 32px;
}

.tray-panel__label {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.tray-panel__row svg {
  flex-shrink: 0;
  color: var(--nvx-color-text-secondary);
}

.tray-panel__resources {
  display: flex;
  border-top: 1px solid var(--nvx-color-border);
  border-bottom: 1px solid var(--nvx-color-border);
  padding: 10px 0;
}

.tray-panel__resources .nvx-button {
  flex: 1;
  min-width: 0;
  padding: 0 6px;
  border-radius: 0;
  border-right: 1px solid var(--nvx-color-border);
  font-weight: var(--nvx-font-weight-regular);
}

.tray-panel__resources .nvx-button:last-child {
  border-right: 0;
}

.tray-panel__resources :deep(.nvx-button__content) {
  display: flex;
  align-items: center;
  gap: 5px;
}

.tray-panel__count {
  font-variant-numeric: tabular-nums;
}

.tray-panel__utilities {
  padding-top: 7px;
}

.tray-panel__utilities > .nvx-button {
  min-height: 34px;
}

.tray-panel__secondary {
  max-width: 58%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-xs);
}

.tray-panel__expanded {
  padding: 4px;
  margin: 4px 0;
  background: var(--nvx-color-bg-subtle);
  border-radius: var(--nvx-radius-md);
}

.tray-panel__empty {
  margin: 0;
  padding: 6px;
  color: var(--nvx-color-text-tertiary);
  font-size: var(--nvx-font-size-xs);
}

.tray-panel__footer {
  display: flex;
  justify-content: space-between;
  min-height: 46px;
  padding: 6px 12px;
  border-top: 1px solid var(--nvx-color-border);
}

.tray-panel__footer .nvx-button {
  padding: 0 6px;
  font-weight: var(--nvx-font-weight-regular);
}

.tray-panel__notice {
  margin: 4px 0 10px;
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-xs);
  overflow-wrap: anywhere;
}
</style>
