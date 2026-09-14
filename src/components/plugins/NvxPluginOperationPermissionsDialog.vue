<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";

import { NvxButton, NvxDialog, NvxInlineNotice } from "../ui";
import {
  clearPluginOperationPermissions,
  listPluginOperationPermissions,
  revokePluginOperationPermission,
} from "../../core-api/client";
import type { PluginId, PluginOperationPermissionList } from "../../core-api/generated/core-api";

const props = defineProps<{
  pluginId: PluginId;
  pluginName: string;
}>();
const emit = defineEmits<{ close: [] }>();
const { locale, t } = useI18n();
const snapshot = ref<PluginOperationPermissionList | null>(null);
const loading = ref(true);
const failed = ref(false);
const pending = ref<string | "all" | null>(null);
const empty = computed(() => snapshot.value?.permissions.length === 0);
const formatTime = (value: bigint) => new Intl.DateTimeFormat(locale.value, {
  dateStyle: "short",
  timeStyle: "short",
}).format(new Date(Number(value)));
const expiryStatus = (value: bigint | null) => {
  if (value === null) return { expired: false, text: t("plugins.approvalPolicy.management.unlimited") };
  const time = formatTime(value);
  return value <= BigInt(Date.now())
    ? { expired: true, text: t("plugins.approvalPolicy.management.expired", { time }) }
    : { expired: false, text: t("plugins.approvalPolicy.management.expires", { time }) };
};

async function refresh() {
  loading.value = true;
  failed.value = false;
  try {
    snapshot.value = await listPluginOperationPermissions(props.pluginId);
  } catch {
    failed.value = true;
  } finally {
    loading.value = false;
  }
}

async function revoke(permissionId: string) {
  if (!snapshot.value || pending.value) return;
  pending.value = permissionId;
  failed.value = false;
  try {
    await revokePluginOperationPermission({
      pluginId: props.pluginId,
      permissionId,
      expectedPolicyRevision: snapshot.value.policyRevision,
    });
    await refresh();
  } catch {
    failed.value = true;
  } finally {
    pending.value = null;
  }
}

async function clearAll() {
  if (!snapshot.value || pending.value || empty.value) return;
  pending.value = "all";
  failed.value = false;
  try {
    await clearPluginOperationPermissions({
      pluginId: props.pluginId,
      expectedPolicyRevision: snapshot.value.policyRevision,
    });
    await refresh();
  } catch {
    failed.value = true;
  } finally {
    pending.value = null;
  }
}

onMounted(() => void refresh());
</script>

<template>
  <NvxDialog
    plugin-protected
    :model-value="true"
    size="lg"
    :title="t('plugins.approvalPolicy.management.title')"
    :description="t('plugins.approvalPolicy.management.description', { name: pluginName })"
    :close-label="t('plugins.approvalPolicy.management.close')"
    :dismissible="pending === null"
    @update:model-value="(open) => { if (!open) emit('close'); }"
  >
    <div class="plugin-operation-permissions">
      <NvxInlineNotice
        v-if="failed"
        tone="error"
        :title="t('plugins.approvalPolicy.management.failed')"
      >
        {{ t('plugins.approvalPolicy.management.failedHint') }}
      </NvxInlineNotice>
      <p
        v-if="loading && snapshot === null"
        role="status"
      >
        {{ t('plugins.approvalPolicy.management.loading') }}
      </p>
      <NvxInlineNotice
        v-else-if="empty"
        tone="info"
        :title="t('plugins.approvalPolicy.management.emptyTitle')"
      >
        {{ t('plugins.approvalPolicy.management.emptyDescription') }}
      </NvxInlineNotice>
      <div
        v-else-if="snapshot"
        class="plugin-operation-permissions__table-wrap"
      >
        <table class="plugin-operation-permissions__table">
          <colgroup>
            <col class="plugin-operation-permissions__operation-column">
            <col>
            <col class="plugin-operation-permissions__created-column">
            <col class="plugin-operation-permissions__expires-column">
            <col class="plugin-operation-permissions__actions-column">
          </colgroup>
          <thead>
            <tr>
              <th scope="col">
                {{ t('plugins.approvalPolicy.management.columns.operation') }}
              </th>
              <th scope="col">
                {{ t('plugins.approvalPolicy.management.columns.target') }}
              </th>
              <th scope="col">
                {{ t('plugins.approvalPolicy.management.columns.created') }}
              </th>
              <th scope="col">
                {{ t('plugins.approvalPolicy.management.columns.expires') }}
              </th>
              <th scope="col">
                {{ t('plugins.approvalPolicy.management.columns.actions') }}
              </th>
            </tr>
          </thead>
          <tbody>
            <tr
              v-for="permission in snapshot.permissions"
              :key="permission.permissionId"
            >
              <td>
                <div class="plugin-operation-permissions__operation">
                  <strong>{{ t(`plugins.approvalPolicy.operations.${permission.operation}`) }}</strong>
                  <span>{{ permission.actionLabel }}</span>
                </div>
              </td>
              <td>{{ permission.targetLabel }}</td>
              <td>{{ formatTime(permission.createdAtUnixMs) }}</td>
              <td>
                <span
                  :class="{ 'plugin-operation-permissions__expiry--expired': expiryStatus(permission.expiresAtUnixMs).expired }"
                >{{ expiryStatus(permission.expiresAtUnixMs).text }}</span>
              </td>
              <td>
                <NvxButton
                  size="sm"
                  variant="ghost"
                  :disabled="pending !== null"
                  :loading="pending === permission.permissionId"
                  @click="revoke(permission.permissionId)"
                >
                  {{ t('plugins.approvalPolicy.management.revoke') }}
                </NvxButton>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
    </div>
    <template #actions>
      <NvxButton
        variant="secondary"
        :disabled="pending !== null"
        :loading="loading && snapshot !== null"
        @click="refresh"
      >
        {{ t('plugins.approvalPolicy.management.refresh') }}
      </NvxButton>
      <NvxButton
        variant="danger"
        :disabled="pending !== null || empty"
        :loading="pending === 'all'"
        @click="clearAll"
      >
        {{ t('plugins.approvalPolicy.management.revokeAll') }}
      </NvxButton>
      <NvxButton
        variant="secondary"
        :disabled="pending !== null"
        @click="emit('close')"
      >
        {{ t('plugins.approvalPolicy.management.close') }}
      </NvxButton>
    </template>
  </NvxDialog>
</template>

<style scoped>
.plugin-operation-permissions { min-width: 0; display: grid; gap: var(--nvx-space-3); }
.plugin-operation-permissions__table-wrap { min-width: 0; overflow: auto; border: var(--nvx-border-width) solid var(--nvx-color-border); border-radius: var(--nvx-radius-md); }
.plugin-operation-permissions__table { width: 100%; min-width: 600px; table-layout: fixed; border-collapse: collapse; font-size: var(--nvx-font-size-sm); }
.plugin-operation-permissions__table th, .plugin-operation-permissions__table td { padding: var(--nvx-space-2) var(--nvx-space-3); border-bottom: var(--nvx-border-width) solid var(--nvx-color-border-subtle); text-align: start; vertical-align: top; overflow-wrap: anywhere; }
.plugin-operation-permissions__table th { color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-xs); font-weight: var(--nvx-font-weight-medium); }
.plugin-operation-permissions__table tbody tr:last-child td { border-bottom: 0; }
.plugin-operation-permissions__operation-column { width: 30%; }
.plugin-operation-permissions__created-column { width: 7rem; }
.plugin-operation-permissions__expires-column { width: 7.5rem; }
.plugin-operation-permissions__actions-column { width: 5.5rem; }
.plugin-operation-permissions__operation { display: grid; gap: var(--nvx-space-1); }
.plugin-operation-permissions__table td:first-child span { color: var(--nvx-color-text-secondary); }
.plugin-operation-permissions__table td:last-child { white-space: nowrap; }
.plugin-operation-permissions__expiry--expired { color: var(--nvx-color-text-danger); font-weight: var(--nvx-font-weight-medium); }
</style>
