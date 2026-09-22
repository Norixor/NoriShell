<script setup lang="ts">
import { FileInput, FolderPlus, KeyRound, Network, Pencil, Plus, RefreshCw, Server, Star, Tag, Trash2, Workflow } from "lucide-vue-next";
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { useRoute, useRouter } from "vue-router";
import NvxHostMarker from "../components/hosts/NvxHostMarker.vue";
import { useHostMarkersStore } from "../stores/hostMarkers";
import { NvxPageHeader } from "../components/layout";
import { NvxPluginExtensionTarget } from "../components/plugins";
import { NvxButton, NvxCheckbox, NvxDialog, NvxField, NvxIcon, NvxIconButton, NvxInlineNotice, NvxInput, NvxSelect, NvxStatusLabel, NvxTextarea } from "../components/ui";
import { canUseDesktopCore, createHostGroup, createIdentity, createKeyboardInteractiveCredential, createSshAgentCredential, commitOpenSshConfig, deleteHostGroup, deleteHostTag, deleteHost, listHostCatalog, listHostGroups, listHostTags, listIdentities, listSshAgentKeys, previewOpenSshConfig, updateHostFavorite, updateHostGroup, updateHostTag } from "../core-api/client";
import type { HostCatalogEntry, HostGroupSummary, HostSummary, HostTagSummary, IdentitySummary, OpenSshConfigPreviewResponse, OpenSshImportRoutePreview, SshAgentKeySummary } from "../core-api/generated/core-api";
import { useTipsStore } from "../stores/tips";
import { openToolWindow, onToolWindowChanged } from "../tool-windows";

const { t } = useI18n();

const tips = useTipsStore();

const hostMarkers = useHostMarkersStore();

const route = useRoute();

const router = useRouter();

const catalog = ref<HostCatalogEntry[]>([]);

const groups = ref<HostGroupSummary[]>([]);

const tags = ref<HostTagSummary[]>([]);

const identities = ref<IdentitySummary[]>([]);

const loading = ref(false);

const loadFailed = ref(false);

const deleteCandidate = ref<HostSummary | null>(null);

const deleting = ref(false);

const favoriteUpdatingHostId = ref<string | null>(null);

const classificationDialogOpen = ref(false);

const classificationEdit = ref<{
  kind: "group" | "tag";
  id: string;
  stateVersion: string;
} | null>(null);

const classificationEditLabel = ref("");

const classificationSaving = ref(false);

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

const agentValidationFailed = ref(false);

const keyboardInteractiveDialogOpen = ref(false);

const keyboardInteractiveIdentityId = ref("");

const keyboardInteractiveLabel = ref("");

const keyboardInteractivePriority = ref("110");

const keyboardInteractiveMaxRounds = ref("8");

const savingKeyboardInteractive = ref(false);

const keyboardInteractiveValidationFailed = ref(false);

const opensshDialogOpen = ref(false);

const opensshConfigText = ref("");

const opensshPreview = ref<OpenSshConfigPreviewResponse | null>(null);

const selectedOpenSshCandidateIds = ref<string[]>([]);

const previewingOpenSsh = ref(false);

const importingOpenSsh = ref(false);

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

async function openCreate() { await launchHostEditor(null); }

async function openEdit(host: HostSummary, initialSection: "connection" | "connectionRoute" | "loginAutomation" = "connection") { await launchHostEditor(host, initialSection); }

function openConnectionRoute(host: HostSummary) {
  void openEdit(host, "connectionRoute");
}

function openLoginAutomation(host: HostSummary) {
  void openEdit(host, "loginAutomation");
}

function requestDelete(host: HostSummary) {
  deleteCandidate.value = host;
  tips.dismissScope("hosts-operation");
}

function cancelDelete() {
  if (deleting.value) return;
  deleteCandidate.value = null;
}

async function confirmDelete() {
  const host = deleteCandidate.value;
  if (!host || deleting.value) return;
  deleting.value = true;
  tips.dismissScope("hosts-operation");
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
    tips.show({
      scope: "hosts-operation",
      tone: "error",
      title: t("sshHosts.deleteFailed"),
    });
  } finally {
    deleting.value = false;
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
  agentValidationFailed.value = false;
  tips.dismissScope("hosts-agent");
  agentDialogOpen.value = true;
}



async function addAgentIdentity() {
  const nextLabel = newIdentityLabel.value.trim();
  if (!nextLabel || creatingIdentity.value) return;
  creatingIdentity.value = true;
  agentValidationFailed.value = false;
  tips.dismissScope("hosts-agent");
  try {
    const identity = await createIdentity(nextLabel, newIdentityUsername.value.trim() || null);
    identities.value = [...identities.value, identity]
      .sort((left, right) => left.label.localeCompare(right.label));
    agentIdentityId.value = identity.identityId;
    newIdentityLabel.value = "";
    newIdentityUsername.value = "";
  } catch {
    tips.show({
      scope: "hosts-agent",
      tone: "error",
      title: t("sshHosts.sshAgent.failed"),
    });
  } finally {
    creatingIdentity.value = false;
  }
}

async function refreshAgentKeys() {
  if (loadingAgentKeys.value) return;
  loadingAgentKeys.value = true;
  agentValidationFailed.value = false;
  tips.dismissScope("hosts-agent");
  selectedAgentKeyHandle.value = "";
  try {
    agentKeys.value = await listSshAgentKeys();
  } catch {
    agentKeys.value = [];
    tips.show({
      scope: "hosts-agent",
      tone: "error",
      title: t("sshHosts.sshAgent.failed"),
    });
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
    agentValidationFailed.value = true;
    return;
  }
  savingAgentCredential.value = true;
  agentValidationFailed.value = false;
  tips.dismissScope("hosts-agent");
  try {
    await createSshAgentCredential({
      identityId: agentIdentityId.value,
      keyHandle: selectedAgentKeyHandle.value,
      expectedIdentityKind: selectedKey.identityKind,
      priority,
      label: agentCredentialLabel.value.trim(),
    });
    agentKeys.value = [];
    selectedAgentKeyHandle.value = "";
    await refresh();
    tips.show({
      scope: "hosts-agent",
      tone: "success",
      title: t("sshHosts.sshAgent.saved"),
    });
  } catch {
    tips.show({
      scope: "hosts-agent",
      tone: "error",
      title: t("sshHosts.sshAgent.failed"),
    });
  } finally {
    savingAgentCredential.value = false;
  }
}

function openKeyboardInteractiveManager() {
  keyboardInteractiveIdentityId.value = identities.value[0]?.identityId ?? "";
  keyboardInteractiveLabel.value = t("sshHosts.keyboardInteractive.defaultCredentialLabel");
  keyboardInteractivePriority.value = "110";
  keyboardInteractiveMaxRounds.value = "8";
  keyboardInteractiveValidationFailed.value = false;
  tips.dismissScope("hosts-keyboard-interactive");
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
    keyboardInteractiveValidationFailed.value = true;
    return;
  }
  savingKeyboardInteractive.value = true;
  keyboardInteractiveValidationFailed.value = false;
  tips.dismissScope("hosts-keyboard-interactive");
  try {
    await createKeyboardInteractiveCredential({
      identityId: keyboardInteractiveIdentityId.value,
      maxRounds,
      priority,
      label: keyboardInteractiveLabel.value.trim(),
    });
    await refresh();
    tips.show({
      scope: "hosts-keyboard-interactive",
      tone: "success",
      title: t("sshHosts.keyboardInteractive.saved"),
    });
  } catch {
    tips.show({
      scope: "hosts-keyboard-interactive",
      tone: "error",
      title: t("sshHosts.keyboardInteractive.failed"),
    });
  } finally {
    savingKeyboardInteractive.value = false;
  }
}

function openOpenSshImport() {
  opensshConfigText.value = "";
  opensshPreview.value = null;
  selectedOpenSshCandidateIds.value = [];
  tips.dismissScope("hosts-openssh-import");
  opensshDialogOpen.value = true;
}

async function previewOpenSshImport() {
  if (!opensshConfigText.value.trim() || previewingOpenSsh.value) return;
  previewingOpenSsh.value = true;
  tips.dismissScope("hosts-openssh-import");
  try {
    const preview = await previewOpenSshConfig(opensshConfigText.value);
    opensshPreview.value = preview;
    selectedOpenSshCandidateIds.value = preview.candidates
      .filter((candidate) => candidate.importable)
      .map((candidate) => candidate.candidateId);
  } catch {
    opensshPreview.value = null;
    selectedOpenSshCandidateIds.value = [];
    tips.show({
      scope: "hosts-openssh-import",
      tone: "error",
      title: t("sshHosts.opensshImport.failed"),
    });
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
  tips.dismissScope("hosts-openssh-import");
  try {
    const response = await commitOpenSshConfig(
      preview.snapshotId,
      selectedOpenSshCandidateIds.value,
    );
    selectedOpenSshCandidateIds.value = [];
    await refresh();
    tips.show({
      scope: "hosts-openssh-import",
      tone: "success",
      title: t("sshHosts.opensshImport.imported", { count: response.hosts.length }),
    });
  } catch {
    tips.show({
      scope: "hosts-openssh-import",
      tone: "error",
      title: t("sshHosts.opensshImport.failed"),
    });
  } finally {
    importingOpenSsh.value = false;
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
  classificationNewGroupLabel.value = "";
  tips.dismissScope("hosts-classification");
  classificationDialogOpen.value = true;
}

async function addClassificationGroup() {
  const nextLabel = classificationNewGroupLabel.value.trim();
  if (!nextLabel || classificationCreatingGroup.value) return;
  classificationCreatingGroup.value = true;
  tips.dismissScope("hosts-classification");
  try {
    const group = await createHostGroup(nextLabel);
    groups.value = [...groups.value, group].sort((left, right) => left.label.localeCompare(right.label));
    classificationNewGroupLabel.value = "";
  } catch {
    tips.show({
      scope: "hosts-classification",
      tone: "error",
      title: t("sshHosts.classificationFailed"),
    });
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
  tips.dismissScope("hosts-classification");
}

async function saveClassification() {
  const edit = classificationEdit.value;
  const nextLabel = classificationEditLabel.value.trim();
  if (!edit || !nextLabel || classificationSaving.value) return;
  classificationSaving.value = true;
  tips.dismissScope("hosts-classification");
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
    tips.show({
      scope: "hosts-classification",
      tone: "error",
      title: t("sshHosts.classificationFailed"),
    });
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
  tips.dismissScope("hosts-classification");
}

async function confirmClassificationDelete() {
  const candidate = classificationDeleteCandidate.value;
  if (!candidate || classificationDeleting.value) return;
  classificationDeleting.value = true;
  tips.dismissScope("hosts-classification");
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
    tips.show({
      scope: "hosts-classification",
      tone: "error",
      title: t("sshHosts.classificationFailed"),
    });
  } finally {
    classificationDeleting.value = false;
  }
}

function connect(host: HostSummary) {
  void router.push({ path: "/terminal", query: { hostId: host.hostId } });
}

onMounted(async () => {
  try {
    const stop = await onToolWindowChanged(kind => { if (kind === "hostEditor") void refresh(); });
    if (viewDisposed) stop(); else stopToolWindowListener = stop;
  } catch { /* Native events are unavailable in browser-only previews. */ }
  if (viewDisposed) return;
  await refresh();
  if (route.query.create === "1") openCreate();
  if (route.query.importSshConfig === "1") openOpenSshImport();
});

let viewDisposed = false;
let stopToolWindowListener: (() => void) | undefined;

onBeforeUnmount(() => { viewDisposed = true; stopToolWindowListener?.(); });

async function launchHostEditor(host: HostSummary | null, initialSection: "connection" | "connectionRoute" | "loginAutomation" = "connection") {
 try { await openToolWindow({ kind: "hostEditor", initialSection, hostId: host?.hostId ?? null, title: t(host ? "sshHosts.editDialogTitle" : "sshHosts.dialogTitle") }); } catch { tips.show({ scope: "hosts-operation", tone: "error", title: t("sshHosts.saveFailed") }); }
}
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
          v-if="keyboardInteractiveValidationFailed"
          tone="error"
          :title="t('sshHosts.keyboardInteractive.failed')"
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
        v-if="agentValidationFailed"
        tone="error"
        :title="t('sshHosts.sshAgent.failed')"
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
  .hosts-keyboard-interactive__limits {
    grid-template-columns: minmax(0, 1fr);
  }

}
</style>
