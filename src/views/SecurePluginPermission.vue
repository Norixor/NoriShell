<script setup lang="ts">
import { getCurrentWindow } from "@tauri-apps/api/window";
import { computed, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";

import NvxSecureWindow from "../components/layout/NvxSecureWindow.vue";
import { NvxButton, NvxCheckbox, NvxInlineNotice } from "../components/ui";
import {
  decidePluginSpecialPermission,
  getPluginSpecialPermission,
} from "../core-api/client";
import type {
  PluginApprovalDecision,
  PluginCapability,
  PluginCapabilityGrant,
  PluginHostScopeSelection,
  PluginSpecialPermissionSnapshot,
} from "../core-api/generated/core-api";

const { t } = useI18n();
const approvalId = new URLSearchParams(window.location.search).get("approvalId") ?? "";
const snapshot = ref<PluginSpecialPermissionSnapshot | null>(null);
const grants = ref<PluginCapabilityGrant[]>([]);
const selections = ref<PluginHostScopeSelection[]>([]);
const loading = ref(true);
const pending = ref(false);
const failed = ref(false);
const outcome = ref<"approved" | "rejected" | null>(null);
const hostCapabilities = computed(() => grants.value
  .filter((grant) => ["hostMetadataRead", "hostMutationPropose", "hostSessionRequest"].includes(grant.capability))
  .map((grant) => grant.capability));

function granted(capability: PluginCapability) {
  return grants.value.some((grant) => grant.capability === capability && grant.granted);
}

function updateGrant(capability: PluginCapability, value: boolean) {
  grants.value = grants.value.map((grant) => (
    grant.capability === capability ? { ...grant, granted: value } : grant
  ));
  if (value && ["uiHostDomMutate", "uiHostCss"].includes(capability)) {
    grants.value = grants.value.map((grant) => (
      grant.capability === "uiHostDomObserve" ? { ...grant, granted: true } : grant
    ));
  }
  if (!value && capability === "uiHostDomObserve") {
    grants.value = grants.value.map((grant) => (
      ["uiHostDomMutate", "uiHostCss"].includes(grant.capability)
        ? { ...grant, granted: false }
        : grant
    ));
  }
  if (value && ["hostMutationPropose", "hostSessionRequest"].includes(capability)) {
    grants.value = grants.value.map((grant) => (
      grant.capability === "hostMetadataRead" ? { ...grant, granted: true } : grant
    ));
  }
  if (!value && capability === "hostMetadataRead") {
    grants.value = grants.value.map((grant) => (
      ["hostMutationPropose", "hostSessionRequest"].includes(grant.capability)
        ? { ...grant, granted: false }
        : grant
    ));
  }
  if (!value) {
    selections.value = selections.value.flatMap((selection) => {
      const capabilities = selection.capabilities.filter((candidate) => granted(candidate));
      return capabilities.length ? [{ ...selection, capabilities }] : [];
    });
  }
}

function hostGranted(hostId: string, capability: PluginCapability) {
  return selections.value.some((selection) => (
    selection.hostId === hostId && selection.capabilities.includes(capability)
  ));
}

function updateHost(hostId: string, capability: PluginCapability, value: boolean) {
  const current = selections.value.find((selection) => selection.hostId === hostId);
  const capabilities = new Set(current?.capabilities ?? []);
  if (value) capabilities.add(capability);
  else capabilities.delete(capability);
  if (value && ["hostMutationPropose", "hostSessionRequest"].includes(capability)) {
    capabilities.add("hostMetadataRead");
  } else if (!value && capability === "hostMetadataRead") {
    capabilities.clear();
  }
  selections.value = [
    ...selections.value.filter((selection) => selection.hostId !== hostId),
    ...(capabilities.size ? [{ hostId, capabilities: [...capabilities].sort() }] : []),
  ];
}

async function load() {
  try {
    const next = await getPluginSpecialPermission(approvalId);
    snapshot.value = next;
    grants.value = next.specialGrants.map((grant) => ({ ...grant }));
    if (next.requestedCapability) updateGrant(next.requestedCapability, true);
    selections.value = next.hosts.flatMap((host) => (
      host.grantedCapabilities.length
        ? [{ hostId: host.hostId, capabilities: [...host.grantedCapabilities] }]
        : []
    ));
  } catch {
    failed.value = true;
  } finally {
    loading.value = false;
  }
}

async function decide(decision: PluginApprovalDecision) {
  if (!snapshot.value || pending.value) return;
  pending.value = true;
  try {
    const response = await decidePluginSpecialPermission({
      approvalId: snapshot.value.approvalId,
      decision,
      expectedApprovalStateVersion: snapshot.value.approvalStateVersion,
      specialGrants: grants.value,
      hostSelections: selections.value,
    });
    outcome.value = response.decision === "approve" ? "approved" : "rejected";
    window.setTimeout(() => void getCurrentWindow().close(), 800);
  } catch {
    failed.value = true;
  } finally {
    pending.value = false;
  }
}

onMounted(() => void load());
</script>

<template>
  <NvxSecureWindow
    class="secure-permission"
    variant="permissions"
    :title="t('plugins.specialPermission.title')"
    :description="t('plugins.specialPermission.description')"
  >
    <p
      v-if="loading"
      role="status"
    >
      {{ t("plugins.specialPermission.loading") }}
    </p>
    <NvxInlineNotice
      v-else-if="failed"
      tone="error"
      :title="t('plugins.specialPermission.failed')"
    />
    <NvxInlineNotice
      v-else-if="outcome"
      :title="t(outcome === 'approved' && snapshot?.target.kind === 'preparedPackage' ? 'plugins.specialPermission.preparedApproved' : `plugins.specialPermission.${outcome}`)"
    />
    <template v-else-if="snapshot">
      <dl class="secure-facts">
        <div><dt>{{ t("plugins.specialPermission.plugin") }}</dt><dd>{{ snapshot.pluginName }}</dd></div>
        <div><dt>{{ t(snapshot.publisherVerified ? "plugins.specialPermission.verifiedPublisher" : "plugins.specialPermission.publisher") }}</dt><dd>{{ snapshot.publisher }}</dd></div>
        <div><dt>{{ t("plugins.version") }}</dt><dd>{{ snapshot.version }}</dd></div>
        <div><dt>{{ t("plugins.packageHash") }}</dt><dd><code>{{ snapshot.packageSha256 }}</code></dd></div>
        <div><dt>{{ t("plugins.specialPermission.signer") }}</dt><dd><code>{{ snapshot.artifactFingerprintSha256 }}</code></dd></div>
      </dl>
      <NvxInlineNotice
        tone="warning"
        :title="t('plugins.specialPermission.warningTitle')"
      >
        {{ t("plugins.specialPermission.warning") }}
      </NvxInlineNotice>
      <section>
        <h2>{{ t("plugins.specialPermission.capabilities") }}</h2>
        <div class="secure-permission__checks">
          <NvxCheckbox
            v-for="grant in grants"
            :key="grant.capability"
            :model-value="grant.granted"
            :disabled="pending"
            @update:model-value="updateGrant(grant.capability, $event)"
          >
            {{ t(`plugins.capabilities.${grant.capability}.label`) }}
            <template #hint>
              {{ t(`plugins.capabilities.${grant.capability}.description`) }}
            </template>
          </NvxCheckbox>
        </div>
      </section>
      <section v-if="hostCapabilities.length">
        <h2>{{ t("plugins.specialPermission.hosts") }}</h2>
        <p>{{ t("plugins.specialPermission.hostsDescription") }}</p>
        <div class="secure-permission__hosts">
          <article
            v-for="host in snapshot.hosts"
            :key="host.hostId"
          >
            <header><strong>{{ host.label }}</strong><span>{{ host.endpoint }}</span></header>
            <div class="secure-permission__host-checks">
              <NvxCheckbox
                v-for="capability in hostCapabilities"
                :key="capability"
                :model-value="hostGranted(host.hostId, capability)"
                :disabled="pending || !granted(capability)"
                @update:model-value="updateHost(host.hostId, capability, $event)"
              >
                {{ t(`plugins.capabilities.${capability}.label`) }}
              </NvxCheckbox>
            </div>
          </article>
        </div>
      </section>
    </template>
    <template
      v-if="snapshot && !loading && !failed && !outcome"
      #actions
    >
      <NvxButton
        variant="secondary"
        :disabled="pending"
        @click="decide('reject')"
      >
        {{ t("plugins.specialPermission.cancel") }}
      </NvxButton>
      <NvxButton
        :loading="pending"
        @click="decide('approve')"
      >
        {{ t("plugins.specialPermission.apply") }}
      </NvxButton>
    </template>
  </NvxSecureWindow>
</template>

<style scoped>
.secure-permission section:not(.nvx-inline-notice) { display: grid; gap: var(--nvx-space-2); }
.secure-permission section > p { color: var(--nvx-color-text-secondary); }
.secure-permission__checks { display: grid; gap: var(--nvx-space-2); }
.secure-permission__checks > :deep(.nvx-checkbox) { padding: var(--nvx-space-2); border-bottom: var(--nvx-border-width) solid var(--nvx-color-border); }
.secure-permission :deep(.nvx-checkbox__hint) { font-size: var(--nvx-font-size-xs); line-height: var(--nvx-line-height-xs); }
.secure-permission__hosts { display: grid; }
.secure-permission__hosts article { display: grid; gap: var(--nvx-space-2); padding-block: var(--nvx-space-2); border-bottom: var(--nvx-border-width) solid var(--nvx-color-border); }
.secure-permission__hosts article > header { display: flex; flex-wrap: wrap; justify-content: space-between; gap: var(--nvx-space-2); overflow-wrap: anywhere; }
.secure-permission__hosts article > header span { color: var(--nvx-color-text-secondary); }
.secure-permission__host-checks { display: grid; grid-template-columns: repeat(auto-fit, minmax(180px, 1fr)); gap: var(--nvx-space-2); }
@media (max-width: 520px) { .secure-permission__host-checks { grid-template-columns: minmax(0, 1fr); } }
</style>
