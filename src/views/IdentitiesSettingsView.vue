<script setup lang="ts">
import { ArrowLeft, KeyRound, Pencil, Plus, Trash2 } from "lucide-vue-next";
import { computed, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";

import { NvxPageHeader } from "../components/layout";
import {
  NvxButton,
  NvxCard,
  NvxDialog,
  NvxField,
  NvxIcon,
  NvxInlineNotice,
  NvxInput,
  NvxStatusLabel,
} from "../components/ui";
import {
  createIdentity,
  deleteIdentity,
  fetchIdentityDeleteImpact,
  listCredentialRefs,
  listIdentities,
  updateIdentity,
} from "../core-api/client";
import type {
  CredentialRefSummary,
  IdentityDeleteImpact,
  IdentitySummary,
} from "../core-api/generated/core-api";

const { t } = useI18n();
const router = useRouter();

const identities = ref<IdentitySummary[]>([]);
const credentials = ref<Record<string, CredentialRefSummary[]>>({});
const loading = ref(true);
const loadFailed = ref(false);
const actionPending = ref(false);
const actionFailed = ref(false);
const editorOpen = ref(false);
const editTarget = ref<IdentitySummary | null>(null);
const labelDraft = ref("");
const usernameDraft = ref("");
const deleteImpact = ref<IdentityDeleteImpact | null>(null);

const editorTitle = computed(() => t(editTarget.value
  ? "identitySettings.editor.editTitle"
  : "identitySettings.editor.createTitle"));
const canSave = computed(() => labelDraft.value.trim().length > 0 && !actionPending.value);
const deleteBlocked = computed(() => (
  (deleteImpact.value?.referencingHostCount ?? 0) > 0
  || (deleteImpact.value?.referencingCredentialRefCount ?? 0) > 0
));

function credentialLabel(credential: CredentialRefSummary) {
  return t("identitySettings.credential", {
    label: credential.label,
    method: t(`identitySettings.methods.${credential.method}`),
    priority: credential.priority,
  });
}

async function load() {
  loading.value = true;
  loadFailed.value = false;
  try {
    const nextIdentities = await listIdentities();
    const pairs = await Promise.all(nextIdentities.map(async (identity) => [
      identity.identityId,
      await listCredentialRefs(identity.identityId),
    ] as const));
    identities.value = nextIdentities;
    credentials.value = Object.fromEntries(pairs);
  } catch {
    loadFailed.value = true;
  } finally {
    loading.value = false;
  }
}

function openCreate() {
  editTarget.value = null;
  labelDraft.value = "";
  usernameDraft.value = "";
  actionFailed.value = false;
  editorOpen.value = true;
}

function openEdit(identity: IdentitySummary) {
  editTarget.value = identity;
  labelDraft.value = identity.label;
  usernameDraft.value = identity.username ?? "";
  actionFailed.value = false;
  editorOpen.value = true;
}

async function saveIdentity() {
  if (!canSave.value) return;
  actionPending.value = true;
  actionFailed.value = false;
  try {
    const target = editTarget.value;
    if (target) {
      await updateIdentity({
        identityId: target.identityId,
        expectedStateVersion: target.stateVersion,
        label: labelDraft.value.trim(),
        username: usernameDraft.value.trim() || null,
      });
    } else {
      await createIdentity(labelDraft.value.trim(), usernameDraft.value.trim() || null);
    }
    editorOpen.value = false;
    await load();
  } catch {
    actionFailed.value = true;
  } finally {
    actionPending.value = false;
  }
}

async function openDelete(identity: IdentitySummary) {
  actionFailed.value = false;
  actionPending.value = true;
  try {
    deleteImpact.value = await fetchIdentityDeleteImpact(identity.identityId);
  } catch {
    actionFailed.value = true;
  } finally {
    actionPending.value = false;
  }
}

async function confirmDelete() {
  const impact = deleteImpact.value;
  if (!impact || deleteBlocked.value || actionPending.value) return;
  actionPending.value = true;
  actionFailed.value = false;
  try {
    await deleteIdentity(impact.identity.identityId, impact.identity.stateVersion);
    deleteImpact.value = null;
    await load();
  } catch {
    actionFailed.value = true;
  } finally {
    actionPending.value = false;
  }
}

onMounted(load);
</script>

<template>
  <section class="management-page">
    <NvxPageHeader
      :breadcrumb="t('identitySettings.breadcrumb')"
      :title="t('identitySettings.title')"
      :description="t('identitySettings.description')"
    >
      <template #actions>
        <NvxButton
          variant="secondary"
          size="sm"
          @click="router.push('/settings')"
        >
          <NvxIcon
            :icon="ArrowLeft"
            :size="16"
            aria-hidden="true"
          />
          {{ t('identitySettings.back') }}
        </NvxButton>
        <NvxButton
          size="sm"
          @click="openCreate"
        >
          <NvxIcon
            :icon="Plus"
            :size="16"
            aria-hidden="true"
          />
          {{ t('identitySettings.create') }}
        </NvxButton>
      </template>
    </NvxPageHeader>

    <NvxInlineNotice
      v-if="loadFailed"
      tone="error"
    >
      {{ t('identitySettings.loadFailed') }}
      <NvxButton
        size="sm"
        variant="secondary"
        @click="load"
      >
        {{ t('identitySettings.retry') }}
      </NvxButton>
    </NvxInlineNotice>
    <NvxInlineNotice v-else-if="loading">
      {{ t('identitySettings.loading') }}
    </NvxInlineNotice>
    <NvxInlineNotice v-else-if="identities.length === 0">
      {{ t('identitySettings.empty') }}
    </NvxInlineNotice>

    <div
      v-else
      class="management-list"
    >
      <NvxCard
        v-for="identity in identities"
        :key="identity.identityId"
      >
        <div class="management-card__header">
          <div class="management-card__identity">
            <span
              class="management-card__icon"
              aria-hidden="true"
            >
              <NvxIcon
                :icon="KeyRound"
                :size="20"
              />
            </span>
            <div>
              <h2>{{ identity.label }}</h2>
              <p>{{ identity.username || t('identitySettings.noUsername') }}</p>
            </div>
          </div>
          <div class="management-card__actions">
            <NvxButton
              size="sm"
              variant="secondary"
              @click="openEdit(identity)"
            >
              <NvxIcon
                :icon="Pencil"
                :size="16"
                aria-hidden="true"
              />
              {{ t('identitySettings.edit') }}
            </NvxButton>
            <NvxButton
              size="sm"
              variant="danger"
              @click="openDelete(identity)"
            >
              <NvxIcon
                :icon="Trash2"
                :size="16"
                aria-hidden="true"
              />
              {{ t('identitySettings.delete') }}
            </NvxButton>
          </div>
        </div>
        <div class="management-card__details">
          <NvxStatusLabel :tone="(credentials[identity.identityId]?.length ?? 0) > 0 ? 'info' : 'neutral'">
            {{ t('identitySettings.credentialCount', {
              count: credentials[identity.identityId]?.length ?? 0,
            }) }}
          </NvxStatusLabel>
          <ul v-if="credentials[identity.identityId]?.length">
            <li
              v-for="credential in credentials[identity.identityId]"
              :key="credential.credentialRefId"
            >
              {{ credentialLabel(credential) }}
            </li>
          </ul>
        </div>
      </NvxCard>
    </div>

    <NvxDialog
      v-model="editorOpen"
      :title="editorTitle"
      :description="t('identitySettings.editor.description')"
      :close-label="t('identitySettings.close')"
      :dismissible="!actionPending"
      @close="actionFailed = false"
    >
      <div class="management-form">
        <NvxField
          for-id="identity-label"
          :label="t('identitySettings.editor.label')"
        >
          <NvxInput
            id="identity-label"
            v-model="labelDraft"
            :maxlength="128"
            data-nvx-dialog-initial-focus
          />
        </NvxField>
        <NvxField
          for-id="identity-username"
          :label="t('identitySettings.editor.username')"
          :hint="t('identitySettings.editor.usernameHint')"
        >
          <NvxInput
            id="identity-username"
            v-model="usernameDraft"
            :maxlength="128"
          />
        </NvxField>
        <NvxInlineNotice
          v-if="actionFailed"
          tone="error"
        >
          {{ t('identitySettings.actionFailed') }}
        </NvxInlineNotice>
      </div>
      <template #actions>
        <NvxButton
          variant="secondary"
          :disabled="actionPending"
          @click="editorOpen = false"
        >
          {{ t('identitySettings.cancel') }}
        </NvxButton>
        <NvxButton
          :loading="actionPending"
          :disabled="!canSave"
          @click="saveIdentity"
        >
          {{ t('identitySettings.save') }}
        </NvxButton>
      </template>
    </NvxDialog>

    <NvxDialog
      :model-value="deleteImpact !== null"
      :title="t('identitySettings.deleteDialog.title')"
      :description="t('identitySettings.deleteDialog.description')"
      :close-label="t('identitySettings.close')"
      :dismissible="!actionPending"
      @update:model-value="deleteImpact = $event ? deleteImpact : null"
      @close="actionFailed = false"
    >
      <template v-if="deleteImpact">
        <NvxInlineNotice :tone="deleteBlocked ? 'warning' : 'info'">
          {{ deleteBlocked
            ? t('identitySettings.deleteDialog.blocked', {
              hosts: deleteImpact.referencingHostCount,
              credentials: deleteImpact.referencingCredentialRefCount,
            })
            : t('identitySettings.deleteDialog.ready', { label: deleteImpact.identity.label }) }}
        </NvxInlineNotice>
        <div
          v-if="deleteBlocked"
          class="delete-impact"
        >
          <p
            v-for="host in deleteImpact.referencingHosts"
            :key="host.hostId"
          >
            {{ t('identitySettings.deleteDialog.hostReference', { label: host.label }) }}
          </p>
          <p
            v-for="credential in deleteImpact.referencingCredentialRefs"
            :key="credential.credentialRefId"
          >
            {{ t('identitySettings.deleteDialog.credentialReference', {
              label: credential.label,
              method: t(`identitySettings.methods.${credential.method}`),
            }) }}
          </p>
        </div>
        <NvxInlineNotice
          v-if="actionFailed"
          tone="error"
        >
          {{ t('identitySettings.actionFailed') }}
        </NvxInlineNotice>
      </template>
      <template #actions>
        <NvxButton
          variant="secondary"
          :disabled="actionPending"
          @click="deleteImpact = null"
        >
          {{ t('identitySettings.close') }}
        </NvxButton>
        <NvxButton
          v-if="!deleteBlocked"
          variant="danger"
          :loading="actionPending"
          @click="confirmDelete"
        >
          {{ t('identitySettings.delete') }}
        </NvxButton>
      </template>
    </NvxDialog>
  </section>
</template>

<style scoped>
.management-page {
  height: 100%;
  overflow: auto;
  padding: var(--nvx-space-6);
}

.management-list {
  display: grid;
  gap: var(--nvx-space-4);
}

.management-card__header,
.management-card__identity,
.management-card__actions {
  display: flex;
  align-items: center;
}

.management-card__header {
  gap: var(--nvx-space-4);
  justify-content: space-between;
}

.management-card__identity,
.management-card__actions {
  gap: var(--nvx-space-3);
}

.management-card__icon {
  display: grid;
  width: 36px;
  height: 36px;
  flex: 0 0 auto;
  place-items: center;
  border-radius: var(--nvx-radius-md);
  background: var(--nvx-color-accent-soft);
  color: var(--nvx-color-accent);
}

.management-card h2,
.management-card p,
.management-card__details ul,
.delete-impact p {
  margin: 0;
}

.management-card h2 {
  font-size: var(--nvx-font-size-md);
}

.management-card p,
.management-card__details,
.delete-impact {
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-sm);
}

.management-card__details {
  display: grid;
  gap: var(--nvx-space-2);
  margin-top: var(--nvx-space-4);
  padding-top: var(--nvx-space-4);
  border-top: var(--nvx-border-width) solid var(--nvx-color-border);
}

.management-card__details ul {
  display: grid;
  gap: var(--nvx-space-1);
  padding-left: var(--nvx-space-5);
}

.management-form,
.delete-impact {
  display: grid;
  gap: var(--nvx-space-4);
}

@media (max-width: 760px) {
  .management-page {
    padding: var(--nvx-space-4);
  }

  .management-card__header {
    align-items: flex-start;
    flex-direction: column;
  }
}
</style>
