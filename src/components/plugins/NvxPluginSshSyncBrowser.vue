<script setup lang="ts">
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { ArrowDown, ArrowUp, ArrowUpDown, Cloud, Info, KeyRound, LayoutList, Monitor, Search, Server } from "lucide-vue-next";
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";

import { readPluginSshSyncBrowser } from "../../core-api/client";
import type {
  PluginSshSyncBrowserInvalidated,
  PluginSshSyncBrowserSnapshot,
  PluginUiContribution,
} from "../../core-api/generated/core-api";
import { preparePluginProtectedMount } from "../../plugins/hostDomBroker";
import { NvxButton, NvxIcon, NvxInput } from "../ui";

type BrowserView = "overview" | "hosts" | "credentials" | "desktops";

const props = defineProps<{ contribution: PluginUiContribution; nodeId: string; profileId: string }>();
defineSlots<{ default(): unknown }>();
const { t, locale } = useI18n();
const protectedRoot = ref<HTMLElement | null>(null);
// Details belong only to this protected view and never enter the plugin document, form values, Pinia, or action payloads.
const snapshot = ref<PluginSshSyncBrowserSnapshot | null>(null);
const view = ref<BrowserView>("overview");
const query = ref("");
const selectedId = ref<string | null>(null);
const page = ref(1);
type SortKey = "name" | "address" | "kind" | "updated";
const sortKey = ref<SortKey>("name");
const sortDirection = ref<"ascending" | "descending">("ascending");
const phase = ref<"loading" | "ready" | "unavailable" | "failed">("loading");
const PAGE_SIZE = 25;
let requestSequence = 0;
let mounted = false;
let unlisten: UnlistenFn | null = null;

const requestFence = computed(() => JSON.stringify([
  props.contribution.pluginId, props.contribution.artifactFingerprintSha256,
  props.contribution.packageSha256, props.contribution.instanceGeneration,
  props.contribution.stateVersion, props.contribution.contributionRevision,
  props.contribution.target.targetId, props.contribution.target.contextHandle,
  props.contribution.target.targetRevision, props.nodeId, props.profileId,
]));
const navigation = computed(() => [
  { id: "overview" as const, label: t("desktopSync.overview"), icon: LayoutList, count: null },
  { id: "hosts" as const, label: t("desktopSync.hosts"), icon: Server, count: hasCounts.value ? snapshot.value?.hostCount : null },
  { id: "credentials" as const, label: t("desktopSync.credentials"), icon: KeyRound, count: hasCounts.value ? snapshot.value?.credentialCount : null },
  { id: "desktops" as const, label: t("desktopSync.desktops"), icon: Monitor, count: hasCounts.value ? snapshot.value?.desktopProfileCount : null },
]);
const hasCounts = computed(() => phase.value === "ready" && (snapshot.value?.state === "ready" || snapshot.value?.state === "empty"));
const hostById = computed(() => new Map((snapshot.value?.hosts ?? []).map((host) => [host.rowId, host])));
const desktopById = computed(() => new Map((snapshot.value?.desktopProfiles ?? []).map((profile) => [profile.rowId, profile])));
const normalizedQuery = computed(() => query.value.trim().toLocaleLowerCase());
const filteredHosts = computed(() => (snapshot.value?.hosts ?? []).filter((host) => (
  [host.label, host.address, String(host.port), host.username ?? "", ...host.tags]
    .some((value) => value.toLocaleLowerCase().includes(normalizedQuery.value))
)));
const filteredCredentials = computed(() => (snapshot.value?.credentials ?? []).filter((credential) => (
  [
    credential.label,
    credentialKind(credential.materialKind),
    ...credential.hostRowIds.map((rowId) => hostById.value.get(rowId)?.label ?? ""),
    ...credential.desktopProfileRowIds.map((rowId) => desktopById.value.get(rowId)?.label ?? ""),
  ]
    .some((value) => value.toLocaleLowerCase().includes(normalizedQuery.value))
)));
const filteredDesktopProfiles = computed(() => (snapshot.value?.desktopProfiles ?? []).filter((profile) => (
  [profile.label, desktopProtocol(profile.protocol), profile.protocol, profile.address, String(profile.port), profile.username, profile.domain]
    .some((value) => value.toLocaleLowerCase().includes(normalizedQuery.value))
)));
const filteredCount = computed(() => {
  if (view.value === "hosts") return filteredHosts.value.length;
  if (view.value === "credentials") return filteredCredentials.value.length;
  return filteredDesktopProfiles.value.length;
});
const pageCount = computed(() => Math.max(1, Math.ceil(filteredCount.value / PAGE_SIZE)));
const searchLabel = computed(() => t(`desktopSync.search${view.value === "hosts" ? "Hosts" : view.value === "credentials" ? "Credentials" : "Desktops"}`));
const totalCount = computed(() => navigation.value.find((item) => item.id === view.value)?.count ?? null);
const columns = computed(() => [
  { key: "name" as const, label: t("desktopSync.name") },
  { key: "address" as const, label: t(view.value === "credentials" ? "desktopSync.associatedItems" : "desktopSync.address") },
  { key: "kind" as const, label: t("desktopSync.kind") },
  { key: "status" as const, label: t("desktopSync.status") },
  { key: "updated" as const, label: t("desktopSync.updatedTime") },
]);
const sortedRows = computed(() => {
  const rows = view.value === "hosts" ? filteredHosts.value.map((host) => ({
    id: host.rowId, name: host.label, address: endpoint(host.address, host.port),
    kind: t("desktopSync.sshHost"), icon: Server, updated: itemUpdated(host),
  })) : view.value === "credentials" ? filteredCredentials.value.map((credential) => ({
    id: credential.rowId, name: credential.label,
    address: [...credential.hostRowIds.map((id) => hostById.value.get(id)?.label),
      ...credential.desktopProfileRowIds.map((id) => desktopById.value.get(id)?.label)].filter(Boolean).join(", ") || "—",
    kind: credentialKind(credential.materialKind), icon: KeyRound, updated: itemUpdated(credential),
  })) : filteredDesktopProfiles.value.map((profile) => ({
    id: profile.rowId, name: profile.label, address: endpoint(profile.address, profile.port),
    kind: desktopProtocol(profile.protocol), icon: Monitor, updated: itemUpdated(profile),
  }));
  const direction = sortDirection.value === "ascending" ? 1 : -1;
  return rows.sort((left, right) => {
    if (sortKey.value === "updated") {
      // Unknown timestamps stay last in both directions instead of pretending to be old data.
      if (left.updated === null) return right.updated === null ? 0 : 1;
      if (right.updated === null) return -1;
      return (left.updated - right.updated) * direction;
    }
    return left[sortKey.value].localeCompare(right[sortKey.value], locale.value, { numeric: true }) * direction;
  });
});
const pageRows = computed(() => sortedRows.value.slice((page.value - 1) * PAGE_SIZE, page.value * PAGE_SIZE));
function itemUpdated(row: { updatedAtUnixMs: number | null }): number | null {
  const value = row.updatedAtUnixMs;
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}
function formatUpdated(value: number | null) {
  if (value === null) return "—";
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? "—" : new Intl.DateTimeFormat(locale.value, {
    year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit",
  }).format(date);
}
function sortBy(key: SortKey) {
  sortDirection.value = sortKey.value === key && sortDirection.value === "ascending" ? "descending" : "ascending";
  sortKey.value = key;
  page.value = 1;
}
const selectedHost = computed(() => view.value === "hosts"
  ? filteredHosts.value.find((host) => host.rowId === selectedId.value) ?? null : null);
const selectedCredential = computed(() => view.value === "credentials"
  ? filteredCredentials.value.find((credential) => credential.rowId === selectedId.value) ?? null : null);
const selectedDesktopProfile = computed(() => view.value === "desktops"
  ? filteredDesktopProfiles.value.find((profile) => profile.rowId === selectedId.value) ?? null : null);
const associatedHosts = computed(() => (selectedCredential.value?.hostRowIds ?? []).flatMap((rowId) => {
  const host = hostById.value.get(rowId);
  return host ? [host] : [];
}));
const associatedDesktopProfiles = computed(() => (selectedCredential.value?.desktopProfileRowIds ?? []).flatMap((rowId) => {
  const profile = desktopById.value.get(rowId);
  return profile ? [profile] : [];
}));
const rowsOmitted = computed(() => {
  if (view.value === "hosts") return snapshot.value?.hostRowsOmitted ?? 0;
  if (view.value === "credentials") return snapshot.value?.credentialRowsOmitted ?? 0;
  return snapshot.value?.desktopProfileRowsOmitted ?? 0;
});
const hasDetails = computed(() => selectedHost.value !== null || selectedCredential.value !== null || selectedDesktopProfile.value !== null);
const statusText = computed(() => {
  if (phase.value === "loading") return t("desktopSync.loading");
  if (phase.value === "failed") return t("desktopSync.failed");
  if (snapshot.value?.state === "needsCreation") return t("desktopSync.needsCreation");
  if (snapshot.value?.state === "needsUnlock") return t("desktopSync.locked");
  if (snapshot.value?.state === "permissionDenied") return t("desktopSync.permissionDenied");
  if (snapshot.value?.state === "failed") return t("desktopSync.failed");
  if (snapshot.value?.state === "empty") return t("desktopSync.emptyCloud");
  return t("desktopSync.unavailable");
});
const hasSnapshot = computed(() => phase.value === "ready" && snapshot.value?.state === "ready");

function credentialKind(kind: string) {
  const key = kind === "password" ? "password" : kind === "privateKey" ? "privateKey"
    : kind === "keyboardInteractive" ? "keyboardInteractive" : kind === "certificate" ? "certificate" : "other";
  return t(`desktopSync.kinds.${key}`);
}

function desktopProtocol(protocol: string) {
  return t(`desktopSync.protocols.${protocol === "vnc" ? "vnc" : "rdp"}`);
}

function endpoint(address: string, port: number) {
  return `${address.includes(":") && !address.startsWith("[") ? `[${address}]` : address}:${port}`;
}

function clearProjection() {
  requestSequence += 1;
  snapshot.value = null;
  selectedId.value = null;
  query.value = "";
  page.value = 1;
  phase.value = "unavailable";
}

async function loadSnapshot() {
  clearProjection();
  if (!mounted || !unlisten || !protectedRoot.value) return;
  preparePluginProtectedMount(protectedRoot.value);
  phase.value = "loading";
  const sequence = requestSequence;
  const fence = requestFence.value;
  try {
    const result = await readPluginSshSyncBrowser({
      pluginId: props.contribution.pluginId,
      artifactFingerprintSha256: props.contribution.artifactFingerprintSha256,
      expectedPackageSha256: props.contribution.packageSha256,
      instanceGeneration: props.contribution.instanceGeneration,
      expectedStateVersion: props.contribution.stateVersion,
      expectedContributionRevision: props.contribution.contributionRevision,
      targetId: props.contribution.target.targetId,
      contextHandle: props.contribution.target.contextHandle,
      expectedTargetRevision: props.contribution.target.targetRevision,
      nodeId: props.nodeId,
    });
    if (!mounted || sequence !== requestSequence || fence !== requestFence.value) return;
    if (result.profileId !== props.profileId) { phase.value = "failed"; return; }
    snapshot.value = result;
    phase.value = "ready";
  } catch {
    if (!mounted || sequence !== requestSequence || fence !== requestFence.value) return;
    phase.value = "failed";
  }
}

function invalidate(payload: PluginSshSyncBrowserInvalidated) {
  if (payload.pluginId && payload.pluginId !== props.contribution.pluginId) return;
  if (payload.profileId && payload.profileId !== props.profileId) return;
  clearProjection();
  if (mounted) void loadSnapshot();
}

function selectView(next: BrowserView) {
  view.value = next;
  query.value = "";
  selectedId.value = null;
  page.value = 1;
}

watch(requestFence, () => {
  clearProjection();
  if (mounted) void loadSnapshot();
}, { flush: "sync" });
watch([query, locale], () => { page.value = 1; selectedId.value = null; });
onMounted(async () => {
  mounted = true;
  // Revoke prior DOM changes, subscribe to revocation, then permit the local cached projection into the DOM.
  if (protectedRoot.value) preparePluginProtectedMount(protectedRoot.value);
  try {
    const stop = await listen<PluginSshSyncBrowserInvalidated>("plugin-ssh-sync-browser-invalidated", ({ payload }) => invalidate(payload));
    if (!mounted) { stop(); return; }
    unlisten = stop;
    await loadSnapshot();
  } catch {
    clearProjection();
    phase.value = "failed";
  }
});
onBeforeUnmount(() => {
  mounted = false;
  clearProjection();
  unlisten?.();
  unlisten = null;
});
</script>

<template>
  <section
    ref="protectedRoot"
    class="ssh-sync-browser"
    data-plugin-protected
    :aria-label="t('desktopSync.label')"
  >
    <nav
      class="ssh-sync-browser__nav"
      :aria-label="t('desktopSync.navigation')"
    >
      <button
        v-for="item in navigation"
        :key="item.id"
        type="button"
        class="ssh-sync-browser__nav-item"
        :class="{ 'is-active': view === item.id }"
        :aria-current="view === item.id ? 'page' : undefined"
        @click="selectView(item.id)"
      >
        <NvxIcon
          :icon="item.icon"
          :size="20"
        />
        <span>{{ item.label }}</span>
        <span
          v-if="item.id !== 'overview'"
          class="ssh-sync-browser__count"
        >{{ item.count ?? '—' }}</span>
      </button>
    </nav>
    <div class="ssh-sync-browser__content">
      <div
        v-if="view === 'overview'"
        class="ssh-sync-browser__overview"
      >
        <slot />
      </div>
      <section
        v-else
        class="ssh-sync-browser__panel"
        :aria-busy="phase === 'loading'"
      >
        <header class="ssh-sync-browser__header">
          <div class="ssh-sync-browser__search">
            <NvxIcon
              :icon="Search"
              :size="20"
              class="ssh-sync-browser__search-icon"
            />
            <NvxInput
              v-model="query"
              :aria-label="searchLabel"
              :placeholder="searchLabel"
              :maxlength="200"
              :disabled="!hasSnapshot"
            />
          </div>
          <span class="ssh-sync-browser__total">{{ totalCount === null ? '—' : t('desktopSync.resultCount', { count: totalCount }) }}</span>
        </header>
        <div
          v-if="!hasSnapshot"
          class="ssh-sync-browser__empty"
          role="status"
        >
          <NvxIcon
            :icon="Cloud"
            :size="22"
          />
          <p>{{ statusText }}</p>
        </div>
        <div
          v-else
          class="ssh-sync-browser__data"
          :class="{ 'ssh-sync-browser__data--selected': hasDetails }"
        >
          <div class="ssh-sync-browser__list">
            <table>
              <caption class="ssh-sync-browser__sr-only">
                {{ t(`desktopSync.${view}`) }}
              </caption>
              <thead>
                <tr>
                  <th
                    v-for="column in columns"
                    :key="column.key"
                    scope="col"
                    :aria-sort="column.key === sortKey ? sortDirection : undefined"
                  >
                    <span v-if="column.key === 'status'">{{ column.label }}</span>
                    <button
                      v-else
                      type="button"
                      @click="sortBy(column.key)"
                    >
                      {{ column.label }}
                      <NvxIcon
                        :icon="column.key === sortKey ? sortDirection === 'ascending' ? ArrowUp : ArrowDown : ArrowUpDown"
                        :size="16"
                      />
                    </button>
                  </th>
                </tr>
              </thead>
              <tbody>
                <tr
                  v-for="row in pageRows"
                  :key="row.id"
                  :class="{ 'is-selected': row.id === selectedId }"
                >
                  <td>
                    <button
                      type="button"
                      :aria-pressed="row.id === selectedId"
                      @click="selectedId = row.id"
                    >
                      <NvxIcon
                        :icon="row.icon"
                        :size="20"
                      />
                      <span>{{ row.name }}</span>
                    </button>
                  </td>
                  <td class="ssh-sync-browser__endpoint">
                    {{ row.address }}
                  </td>
                  <td>{{ row.kind }}</td>
                  <td>
                    <span class="ssh-sync-browser__row-status"><NvxIcon
                      :icon="Cloud"
                      :size="16"
                    />{{ t('desktopSync.cloudCopy') }}</span>
                  </td>
                  <td class="ssh-sync-browser__updated">
                    {{ formatUpdated(row.updated) }}
                  </td>
                </tr>
              </tbody>
            </table>
            <p
              v-if="!filteredCount"
              class="ssh-sync-browser__no-results"
              role="status"
            >
              {{ t(query ? 'desktopSync.noMatches' : 'desktopSync.noItems') }}
            </p>
            <p
              v-if="rowsOmitted"
              class="ssh-sync-browser__omitted"
            >
              {{ t('desktopSync.omitted', { count: rowsOmitted }) }}
            </p>
            <footer
              v-if="pageCount > 1 || query"
              class="ssh-sync-browser__pagination"
            >
              <span>{{ t('desktopSync.resultCount', { count: filteredCount }) }}</span>
              <div v-if="pageCount > 1">
                <NvxButton
                  variant="ghost"
                  size="sm"
                  :disabled="page <= 1"
                  @click="page -= 1; selectedId = null"
                >
                  {{ t('desktopSync.previous') }}
                </NvxButton>
                <span>{{ page }} / {{ pageCount }}</span>
                <NvxButton
                  variant="ghost"
                  size="sm"
                  :disabled="page >= pageCount"
                  @click="page += 1; selectedId = null"
                >
                  {{ t('desktopSync.next') }}
                </NvxButton>
              </div>
            </footer>
          </div>
          <aside
            v-if="hasDetails"
            class="ssh-sync-browser__details"
            :aria-label="t('desktopSync.details')"
          >
            <header>
              <h3>{{ selectedHost?.label ?? selectedCredential?.label ?? selectedDesktopProfile?.label }}</h3><NvxButton
                variant="ghost"
                size="sm"
                @click="selectedId = null"
              >
                {{ t('desktopSync.closeDetails') }}
              </NvxButton>
            </header>
            <dl v-if="selectedHost">
              <div><dt>{{ t('desktopSync.address') }}</dt><dd>{{ selectedHost.address }}</dd></div>
              <div><dt>{{ t('desktopSync.port') }}</dt><dd>{{ selectedHost.port }}</dd></div>
              <div><dt>{{ t('desktopSync.username') }}</dt><dd>{{ selectedHost.username || '—' }}</dd></div>
              <div>
                <dt>{{ t('desktopSync.tags') }}</dt><dd>
                  <template v-if="selectedHost.tags.length">
                    <span
                      v-for="tag in selectedHost.tags"
                      :key="tag"
                      class="ssh-sync-browser__tag"
                    >{{ tag }}</span>
                  </template><template v-else>
                    —
                  </template>
                </dd>
              </div>
            </dl>
            <dl v-else-if="selectedCredential">
              <div><dt>{{ t('desktopSync.kind') }}</dt><dd>{{ credentialKind(selectedCredential.materialKind) }}</dd></div>
              <div>
                <dt>{{ t('desktopSync.associatedHosts') }}</dt><dd>
                  <ul v-if="associatedHosts.length">
                    <li
                      v-for="host in associatedHosts"
                      :key="host.rowId"
                    >
                      {{ host.label }}
                    </li>
                  </ul><template v-if="!associatedHosts.length">
                    {{ t(snapshot?.hostRowsOmitted ? 'desktopSync.hostAssociationsNotListed' : 'desktopSync.noAssociatedHosts') }}
                  </template><p v-if="associatedHosts.length && snapshot?.hostRowsOmitted">
                    {{ t('desktopSync.hostAssociationsPartial') }}
                  </p>
                </dd>
              </div>
              <div>
                <dt>{{ t('desktopSync.associatedDesktops') }}</dt><dd>
                  <ul v-if="associatedDesktopProfiles.length">
                    <li
                      v-for="profile in associatedDesktopProfiles"
                      :key="profile.rowId"
                    >
                      {{ profile.label }}
                    </li>
                  </ul><template v-if="!associatedDesktopProfiles.length">
                    {{ t(snapshot?.desktopProfileRowsOmitted ? 'desktopSync.desktopAssociationsNotListed' : 'desktopSync.noAssociatedDesktops') }}
                  </template><p v-if="associatedDesktopProfiles.length && snapshot?.desktopProfileRowsOmitted">
                    {{ t('desktopSync.desktopAssociationsPartial') }}
                  </p>
                </dd>
              </div>
            </dl>
            <dl v-else-if="selectedDesktopProfile">
              <div><dt>{{ t('desktopSync.protocol') }}</dt><dd>{{ desktopProtocol(selectedDesktopProfile.protocol) }}</dd></div>
              <div><dt>{{ t('desktopSync.address') }}</dt><dd>{{ selectedDesktopProfile.address }}</dd></div>
              <div><dt>{{ t('desktopSync.port') }}</dt><dd>{{ selectedDesktopProfile.port }}</dd></div>
              <div><dt>{{ t('desktopSync.username') }}</dt><dd>{{ selectedDesktopProfile.username || '—' }}</dd></div>
              <div><dt>{{ t('desktopSync.domain') }}</dt><dd>{{ selectedDesktopProfile.domain || '—' }}</dd></div>
            </dl>
          </aside>
        </div>
        <footer class="ssh-sync-browser__read-only">
          <NvxIcon
            :icon="Info"
            :size="20"
          />
          <span>{{ t('desktopSync.readOnlyNote') }}</span>
        </footer>
      </section>
    </div>
  </section>
</template>

<style scoped>
.ssh-sync-browser { display: grid; grid-template-columns: minmax(0, 1fr); min-width: 0; gap: var(--nvx-space-4); }
.ssh-sync-browser__nav { display: flex; align-items: stretch; gap: var(--nvx-space-3); overflow-x: auto; border-bottom: var(--nvx-border-width) solid var(--nvx-color-border); }
.ssh-sync-browser__nav-item { display: grid; grid-template-columns: 20px minmax(0, 1fr) auto; align-items: center; gap: var(--nvx-space-2); min-height: 54px; flex: 0 0 auto; padding: var(--nvx-space-3) var(--nvx-space-5); border: 0; border-bottom: 2px solid transparent; border-radius: 0; background: transparent; color: var(--nvx-color-text-secondary); font: inherit; font-size: var(--nvx-font-size-md); text-align: start; cursor: pointer; }
.ssh-sync-browser__nav-item:hover { background: var(--nvx-color-bg-hover); color: var(--nvx-color-text-primary); }
.ssh-sync-browser__nav-item.is-active { border-bottom-color: var(--nvx-color-accent); background: transparent; color: var(--nvx-color-accent); font-weight: var(--nvx-font-weight-semibold); }
.ssh-sync-browser button:focus-visible { outline: var(--nvx-focus-ring-width) solid var(--nvx-color-focus-ring); outline-offset: 2px; }
.ssh-sync-browser__count { font-variant-numeric: tabular-nums; font-size: var(--nvx-font-size-xs); }
.ssh-sync-browser__content, .ssh-sync-browser__overview, .ssh-sync-browser__list { min-width: 0; }
.ssh-sync-browser__overview { display: grid; gap: var(--nvx-space-4); }
.ssh-sync-browser__panel { min-width: 0; }
.ssh-sync-browser__header { display: flex; align-items: center; justify-content: space-between; flex-wrap: wrap; gap: var(--nvx-space-4); padding: 0 0 var(--nvx-space-3); }
.ssh-sync-browser__header h2, .ssh-sync-browser__details h3 { margin: 0; color: var(--nvx-color-text-primary); font-size: var(--nvx-font-size-md); font-weight: var(--nvx-font-weight-semibold); }
.ssh-sync-browser__header p { margin: var(--nvx-space-1) 0 0; color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-xs); }
.ssh-sync-browser__search { position: relative; display: flex; align-items: center; flex: 1 1 260px; max-width: 100%; }
.ssh-sync-browser__search-icon { position: absolute; left: var(--nvx-space-3); color: var(--nvx-color-text-tertiary); pointer-events: none; }
.ssh-sync-browser__search :deep(input) { height: var(--nvx-control-height-md); min-height: 36px; padding-inline-start: var(--nvx-space-9, 36px); font-size: var(--nvx-font-size-sm); }
.ssh-sync-browser__data { min-width: 0; overflow: hidden; border: var(--nvx-border-width) solid var(--nvx-color-border); border-radius: var(--nvx-radius-md) var(--nvx-radius-md) 0 0; background: var(--nvx-color-bg-surface); }
.ssh-sync-browser__list { overflow-x: auto; }
.ssh-sync-browser__data--selected { display: grid; grid-template-columns: minmax(0, 1fr) 260px; }
.ssh-sync-browser table { width: 100%; min-width: 720px; table-layout: fixed; border-collapse: collapse; text-align: start; }
.ssh-sync-browser th { height: 44px; background: var(--nvx-color-bg-subtle); color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-sm); font-weight: var(--nvx-font-weight-semibold); text-align: start; }
.ssh-sync-browser th:first-child { width: 23%; }
.ssh-sync-browser th:nth-child(2) { width: 25%; }
.ssh-sync-browser th:last-child { width: 21%; }
.ssh-sync-browser th button { display: inline-flex; align-items: center; gap: var(--nvx-space-2); padding: 0; border: 0; background: transparent; color: inherit; font: inherit; cursor: pointer; }
.ssh-sync-browser th button svg { color: var(--nvx-color-text-tertiary); }
.ssh-sync-browser th, .ssh-sync-browser td { padding: var(--nvx-space-2) var(--nvx-space-4); overflow-wrap: anywhere; border-bottom: var(--nvx-border-width) solid var(--nvx-color-border); }
.ssh-sync-browser td { height: 52px; font-size: var(--nvx-font-size-md); }
.ssh-sync-browser td button { display: flex; align-items: center; gap: var(--nvx-space-3); width: 100%; min-height: var(--nvx-control-height-sm); padding: 0; border: 0; background: transparent; color: var(--nvx-color-text-primary); font: inherit; font-weight: var(--nvx-font-weight-medium); text-align: start; overflow-wrap: anywhere; cursor: pointer; }
.ssh-sync-browser tbody tr:hover { background: var(--nvx-color-bg-hover); }
.ssh-sync-browser tbody tr.is-selected { background: var(--nvx-color-accent-soft); }
.ssh-sync-browser__endpoint, .ssh-sync-browser__updated { color: var(--nvx-color-text-secondary); font-variant-numeric: tabular-nums; }
.ssh-sync-browser__row-status { display: inline-flex; align-items: center; gap: var(--nvx-space-1); color: var(--nvx-color-text-secondary); }
.ssh-sync-browser td button svg, .ssh-sync-browser__row-status svg { flex-shrink: 0; }
.ssh-sync-browser__total { color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-sm); font-variant-numeric: tabular-nums; }
.ssh-sync-browser__read-only { display: flex; align-items: center; gap: var(--nvx-space-2); min-height: 54px; padding: var(--nvx-space-3) var(--nvx-space-4); border: var(--nvx-border-width) solid var(--nvx-color-border); border-top: 0; border-radius: 0 0 var(--nvx-radius-md) var(--nvx-radius-md); background: var(--nvx-color-bg-surface); color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-sm); }
.ssh-sync-browser__read-only svg { flex-shrink: 0; color: var(--nvx-color-accent); }
.ssh-sync-browser__details { min-width: 0; padding: var(--nvx-space-4); border-inline-start: var(--nvx-border-width) solid var(--nvx-color-border); background: var(--nvx-color-bg-subtle); }
.ssh-sync-browser__details header { display: flex; align-items: flex-start; justify-content: space-between; gap: var(--nvx-space-2); }
.ssh-sync-browser__details h3 { overflow-wrap: anywhere; }
.ssh-sync-browser__details header .nvx-button { flex: none; }
.ssh-sync-browser__details dl { display: grid; gap: var(--nvx-space-4); margin: var(--nvx-space-5) 0 0; }
.ssh-sync-browser__details dt { margin-bottom: var(--nvx-space-1); color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-xs); }
.ssh-sync-browser__details dd { margin: 0; overflow-wrap: anywhere; font-size: var(--nvx-font-size-sm); }
.ssh-sync-browser__details ul { margin: 0; padding-inline-start: var(--nvx-space-4); }
.ssh-sync-browser__tag { display: inline-block; margin-inline-end: var(--nvx-space-1); margin-bottom: var(--nvx-space-1); padding: 1px var(--nvx-space-1); border: var(--nvx-border-width) solid var(--nvx-color-border); border-radius: var(--nvx-radius-sm); }
.ssh-sync-browser__empty { display: grid; min-height: 220px; place-content: center; justify-items: center; gap: var(--nvx-space-3); padding: var(--nvx-space-6); color: var(--nvx-color-text-secondary); text-align: center; }
.ssh-sync-browser__empty p { max-width: 440px; margin: 0; font-size: var(--nvx-font-size-sm); }
.ssh-sync-browser__no-results { margin: 0; padding: var(--nvx-space-6); color: var(--nvx-color-text-secondary); text-align: center; }
.ssh-sync-browser__omitted { margin: 0; padding: var(--nvx-space-3) var(--nvx-space-4); color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-xs); }
.ssh-sync-browser__pagination, .ssh-sync-browser__pagination > div { display: flex; align-items: center; justify-content: space-between; flex-wrap: wrap; gap: var(--nvx-space-2); }
.ssh-sync-browser__pagination { min-height: 48px; padding: var(--nvx-space-2) var(--nvx-space-4); color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-xs); }
.ssh-sync-browser__sr-only { position: absolute; width: 1px; height: 1px; margin: -1px; padding: 0; overflow: hidden; clip: rect(0, 0, 0, 0); white-space: nowrap; }
@container plugin-page (max-width: 1040px) {
  .ssh-sync-browser__data--selected { grid-template-columns: minmax(0, 1fr); }
  .ssh-sync-browser__details { border-inline-start: 0; border-top: var(--nvx-border-width) solid var(--nvx-color-border); }
}
@container plugin-page (max-width: 700px) {
  .ssh-sync-browser { grid-template-columns: minmax(0, 1fr); gap: var(--nvx-space-4); }
  .ssh-sync-browser__nav { gap: 0; }
  .ssh-sync-browser__nav-item { padding-inline: var(--nvx-space-3); }
}
</style>
