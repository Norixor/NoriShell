<script setup lang="ts">
import { ArrowLeft, Fingerprint, RefreshCw, Trash2 } from "lucide-vue-next";
import { computed, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";

import { NvxPageHeader } from "../components/layout";
import {
  NvxButton,
  NvxCard,
  NvxDialog,
  NvxIcon,
  NvxInlineNotice,
  NvxStatusLabel,
} from "../components/ui";
import { deleteKnownHost, listKnownHosts } from "../core-api/client";
import type { KnownHostSummary } from "../core-api/generated/core-api";

const { t, locale } = useI18n();
const router = useRouter();

const knownHosts = ref<KnownHostSummary[]>([]);
const loading = ref(true);
const loadFailed = ref(false);
const actionFailed = ref(false);
const actionPending = ref(false);
const deleteTarget = ref<KnownHostSummary | null>(null);

const formatter = computed(() => new Intl.DateTimeFormat(locale.value, {
  dateStyle: "medium",
  timeStyle: "short",
}));

function formatTime(value: number) {
  return formatter.value.format(new Date(value));
}

async function load() {
  loading.value = true;
  loadFailed.value = false;
  try {
    knownHosts.value = await listKnownHosts();
  } catch {
    loadFailed.value = true;
  } finally {
    loading.value = false;
  }
}

function openDelete(knownHost: KnownHostSummary) {
  deleteTarget.value = knownHost;
  actionFailed.value = false;
}

async function confirmDelete() {
  const target = deleteTarget.value;
  if (!target || actionPending.value) return;
  actionPending.value = true;
  actionFailed.value = false;
  try {
    await deleteKnownHost(target.knownHostId, target.stateVersion);
    deleteTarget.value = null;
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
  <section class="known-hosts-page">
    <NvxPageHeader
      :breadcrumb="t('knownHostsSettings.breadcrumb')"
      :title="t('knownHostsSettings.title')"
      :description="t('knownHostsSettings.description')"
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
          {{ t('knownHostsSettings.back') }}
        </NvxButton>
        <NvxButton
          variant="secondary"
          size="sm"
          :loading="loading"
          @click="load"
        >
          <NvxIcon
            :icon="RefreshCw"
            :size="16"
            aria-hidden="true"
          />
          {{ t('knownHostsSettings.refresh') }}
        </NvxButton>
      </template>
    </NvxPageHeader>

    <NvxInlineNotice
      v-if="loadFailed"
      tone="error"
    >
      {{ t('knownHostsSettings.loadFailed') }}
    </NvxInlineNotice>
    <NvxInlineNotice v-else-if="loading">
      {{ t('knownHostsSettings.loading') }}
    </NvxInlineNotice>
    <NvxInlineNotice v-else-if="knownHosts.length === 0">
      {{ t('knownHostsSettings.empty') }}
    </NvxInlineNotice>

    <div
      v-else
      class="known-hosts-list"
    >
      <NvxCard
        v-for="knownHost in knownHosts"
        :key="knownHost.knownHostId"
      >
        <div class="known-host-card__header">
          <div class="known-host-card__identity">
            <span
              class="known-host-card__icon"
              aria-hidden="true"
            >
              <NvxIcon
                :icon="Fingerprint"
                :size="20"
              />
            </span>
            <div>
              <h2>{{ knownHost.normalizedAddress }}:{{ knownHost.port }}</h2>
              <p>{{ knownHost.keyAlgorithm }}</p>
            </div>
          </div>
          <NvxButton
            size="sm"
            variant="danger"
            @click="openDelete(knownHost)"
          >
            <NvxIcon
              :icon="Trash2"
              :size="16"
              aria-hidden="true"
            />
            {{ t('knownHostsSettings.delete') }}
          </NvxButton>
        </div>
        <dl class="known-host-card__details">
          <div>
            <dt>{{ t('knownHostsSettings.fingerprint') }}</dt>
            <dd>{{ knownHost.fingerprintSha256 }}</dd>
          </div>
          <div>
            <dt>{{ t('knownHostsSettings.firstTrusted') }}</dt>
            <dd>{{ formatTime(knownHost.firstTrustedAtUnixMs) }}</dd>
          </div>
          <div>
            <dt>{{ t('knownHostsSettings.lastVerified') }}</dt>
            <dd>{{ formatTime(knownHost.lastVerifiedAtUnixMs) }}</dd>
          </div>
        </dl>
        <NvxStatusLabel tone="success">
          {{ t('knownHostsSettings.trusted') }}
        </NvxStatusLabel>
      </NvxCard>
    </div>

    <NvxDialog
      plugin-protected
      :model-value="deleteTarget !== null"
      :title="t('knownHostsSettings.deleteDialog.title')"
      :description="t('knownHostsSettings.deleteDialog.description')"
      :close-label="t('knownHostsSettings.close')"
      :dismissible="!actionPending"
      @update:model-value="deleteTarget = $event ? deleteTarget : null"
      @close="actionFailed = false"
    >
      <NvxInlineNotice
        v-if="deleteTarget"
        tone="warning"
      >
        {{ t('knownHostsSettings.deleteDialog.warning', {
          address: `${deleteTarget.normalizedAddress}:${deleteTarget.port}`,
          fingerprint: deleteTarget.fingerprintSha256,
        }) }}
      </NvxInlineNotice>
      <NvxInlineNotice
        v-if="actionFailed"
        tone="error"
      >
        {{ t('knownHostsSettings.actionFailed') }}
      </NvxInlineNotice>
      <template #actions>
        <NvxButton
          variant="secondary"
          :disabled="actionPending"
          @click="deleteTarget = null"
        >
          {{ t('knownHostsSettings.cancel') }}
        </NvxButton>
        <NvxButton
          variant="danger"
          :loading="actionPending"
          @click="confirmDelete"
        >
          {{ t('knownHostsSettings.delete') }}
        </NvxButton>
      </template>
    </NvxDialog>
  </section>
</template>

<style scoped>
.known-hosts-page {
  height: 100%;
  overflow: auto;
  padding: var(--nvx-space-6);
}

.known-hosts-list {
  display: grid;
  gap: var(--nvx-space-4);
}

.known-host-card__header,
.known-host-card__identity {
  display: flex;
  align-items: center;
}

.known-host-card__header {
  gap: var(--nvx-space-4);
  justify-content: space-between;
}

.known-host-card__identity {
  gap: var(--nvx-space-3);
}

.known-host-card__icon {
  display: grid;
  width: 36px;
  height: 36px;
  flex: 0 0 auto;
  place-items: center;
  border-radius: var(--nvx-radius-md);
  background: var(--nvx-color-accent-soft);
  color: var(--nvx-color-accent);
}

.known-host-card h2,
.known-host-card p,
.known-host-card dl {
  margin: 0;
}

.known-host-card h2 {
  font-size: var(--nvx-font-size-md);
}

.known-host-card p {
  color: var(--nvx-color-text-secondary);
}

.known-host-card__details {
  display: grid;
  grid-template-columns: minmax(0, 2fr) repeat(2, minmax(0, 1fr));
  gap: var(--nvx-space-4);
  margin-block: var(--nvx-space-4) !important;
  padding-block: var(--nvx-space-4);
  border-block: var(--nvx-border-width) solid var(--nvx-color-border);
}

.known-host-card__details dt {
  color: var(--nvx-color-text-tertiary);
  font-size: var(--nvx-font-size-xs);
}

.known-host-card__details dd {
  overflow-wrap: anywhere;
  margin: var(--nvx-space-1) 0 0;
  color: var(--nvx-color-text-primary);
  font-size: var(--nvx-font-size-sm);
}

@media (max-width: 760px) {
  .known-hosts-page {
    padding: var(--nvx-space-4);
  }

  .known-host-card__header {
    align-items: flex-start;
    flex-direction: column;
  }

  .known-host-card__details {
    grid-template-columns: 1fr;
  }
}
</style>
