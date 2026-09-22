<script setup lang="ts">
import { Fingerprint, RefreshCw, Trash2 } from "lucide-vue-next";
import { computed, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";

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
import { useTipsStore } from "../stores/tips";

const { t, locale } = useI18n();
const tips = useTipsStore();

const knownHosts = ref<KnownHostSummary[]>([]);
const loading = ref(true);
const loadFailed = ref(false);
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
  tips.dismissScope("known-hosts-action");
}

async function confirmDelete() {
  const target = deleteTarget.value;
  if (!target || actionPending.value) return;
  actionPending.value = true;
  tips.dismissScope("known-hosts-action");
  try {
    await deleteKnownHost(target.knownHostId, target.stateVersion);
    deleteTarget.value = null;
    await load();
  } catch {
    tips.show({
      scope: "known-hosts-action",
      tone: "error",
      title: t("knownHostsSettings.actionFailed"),
    });
  } finally {
    actionPending.value = false;
  }
}

onMounted(load);
</script>

<template>
  <section class="known-hosts-page">
    <header class="known-hosts-heading">
      <div>
        <h2>{{ t('knownHostsSettings.title') }}</h2>
        <p>{{ t('knownHostsSettings.description') }}</p>
      </div>
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
    </header>

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
        class="known-host-card"
      >
        <div class="known-host-card__header">
          <div class="known-host-card__identity">
            <span
              class="known-host-card__icon"
              aria-hidden="true"
            >
              <NvxIcon
                :icon="Fingerprint"
                :size="16"
              />
            </span>
            <div>
              <h2>{{ knownHost.normalizedAddress }}:{{ knownHost.port }}</h2>
              <p>{{ knownHost.keyAlgorithm }}</p>
            </div>
          </div>
          <NvxButton
            size="sm"
            variant="ghost"
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
  display: grid;
  gap: var(--nvx-space-4);
}

.known-hosts-heading {
  display: flex;
  gap: var(--nvx-space-4);
  align-items: center;
  justify-content: space-between;
}

.known-hosts-heading h2,
.known-hosts-heading p {
  margin: 0;
}

.known-hosts-heading h2 {
  font-size: var(--nvx-font-size-lg);
}

.known-hosts-heading p {
  margin-top: var(--nvx-space-1);
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-sm);
}

.known-hosts-list {
  display: grid;
  gap: var(--nvx-space-2);
}

.known-host-card {
  padding: var(--nvx-space-3);
}

.known-host-card__header,
.known-host-card__identity {
  display: flex;
  align-items: center;
}

.known-host-card__header {
  gap: var(--nvx-space-3);
  justify-content: space-between;
}

.known-host-card__identity {
  gap: var(--nvx-space-2);
}

.known-host-card__icon {
  display: grid;
  width: 30px;
  height: 30px;
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
  gap: var(--nvx-space-3);
  margin-block: var(--nvx-space-2) !important;
  padding-block: var(--nvx-space-2);
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
  .known-hosts-heading,
  .known-host-card__header {
    align-items: flex-start;
    flex-direction: column;
  }

  .known-host-card__details {
    grid-template-columns: 1fr;
  }
}
</style>
