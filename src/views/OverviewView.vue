<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";

import { NvxOverviewPanel } from "../components/overview";
import { NvxButton, NvxDialog, NvxField, NvxInlineNotice, NvxInput } from "../components/ui";
import {
  canUseDesktopCore,
  decideMetricsHostKey,
  fetchServerOverview,
  prepareMetricsKeyboardInteractiveAnswer,
  reconcileMetrics,
  respondMetricsKeyboardInteractive,
  retryMetrics,
} from "../core-api/client";
import type {
  MetricsHostKeyChallenge,
  MetricsKeyboardInteractiveChallenge,
  MetricsSessionSummary,
  ServerOverviewSnapshot,
} from "../core-api/generated/core-api";
import { createUuidV7 } from "../core-api/ids";
import { overviewVisualFixture } from "../overview/visual-fixture";
import { useTipsStore } from "../stores/tips";

const { t } = useI18n();
const router = useRouter();
const tips = useTipsStore();
const snapshot = ref<ServerOverviewSnapshot | null>(null);
const loading = ref(true);
const loadFailed = ref(false);
const actionFailed = ref(false);
const actionSaving = ref(false);
const hostKeyChallenge = ref<MetricsHostKeyChallenge | null>(null);
const keyboardChallenge = ref<MetricsKeyboardInteractiveChallenge | null>(null);
const keyboardAnswers = ref<string[]>([]);
let refreshTimer: number | null = null;
let refreshInFlight = false;

async function refresh() {
  if (!canUseDesktopCore() || refreshInFlight) return;
  refreshInFlight = true;
  try {
    snapshot.value = await fetchServerOverview();
    loadFailed.value = false;
  } catch {
    loadFailed.value = true;
  } finally {
    loading.value = false;
    refreshInFlight = false;
  }
}

function connect(hostId: string) {
  void router.push({
    path: "/terminal",
    query: {
      hostId,
      source: "overview",
      connectOperationId: createUuidV7(),
    },
  });
}

function focusTerminal(sessionId: string) {
  void router.push({ path: "/terminal", query: { focusSessionId: sessionId } });
}

function findMetricsSession(hostId: string): MetricsSessionSummary | null {
  return snapshot.value?.cards.find((card) => card.catalogEntry.host.hostId === hostId)
    ?.metricsSession ?? null;
}

async function beginMetricsAction(hostId: string) {
  const session = findMetricsSession(hostId);
  if (!session || actionSaving.value) return;
  actionFailed.value = false;
  if (session.hostKeyChallenge) {
    hostKeyChallenge.value = session.hostKeyChallenge;
    return;
  }
  if (session.keyboardInteractiveChallenge) {
    keyboardChallenge.value = session.keyboardInteractiveChallenge;
    keyboardAnswers.value = session.keyboardInteractiveChallenge.prompts.map(() => "");
    return;
  }
  if (session.authenticationReason === "vaultLocked") {
    await router.push({ path: "/settings", query: { section: "vault" } });
    return;
  }
  if (session.authenticationReason === "credentialUnavailable") {
    await router.push("/hosts");
    return;
  }
  actionSaving.value = true;
  try {
    await retryMetrics({ hostId, expectedGeneration: session.generation });
    await refresh();
  } catch {
    tips.show({ scope: "overview-metrics-action", tone: "error", title: t("overview.actionFailed") });
  } finally {
    actionSaving.value = false;
  }
}

async function decideHostKey(decision: "accept" | "reject") {
  const challenge = hostKeyChallenge.value;
  if (!challenge || actionSaving.value) return;
  actionSaving.value = true;
  actionFailed.value = false;
  try {
    await decideMetricsHostKey({
      metricsSessionId: challenge.metricsSessionId,
      hostId: challenge.hostId,
      expectedGeneration: challenge.generation,
      challengeId: challenge.challengeId,
      decision,
    });
    hostKeyChallenge.value = null;
    await refresh();
  } catch {
    actionFailed.value = true;
  } finally {
    actionSaving.value = false;
  }
}

async function submitKeyboardAnswers() {
  const challenge = keyboardChallenge.value;
  if (!challenge || actionSaving.value || keyboardAnswers.value.length !== challenge.prompts.length) {
    return;
  }
  actionSaving.value = true;
  actionFailed.value = false;
  try {
    const prepared = await Promise.all(challenge.prompts.map((prompt, index) => (
      prepareMetricsKeyboardInteractiveAnswer({
        metricsSessionId: challenge.metricsSessionId,
        expectedGeneration: challenge.generation,
        challengeId: challenge.challengeId,
        roundIndex: challenge.roundIndex,
        promptIndex: prompt.promptIndex,
        value: keyboardAnswers.value[index] ?? "",
      })
    )));
    await respondMetricsKeyboardInteractive({
      metricsSessionId: challenge.metricsSessionId,
      hostId: challenge.hostId,
      expectedGeneration: challenge.generation,
      challengeId: challenge.challengeId,
      roundIndex: challenge.roundIndex,
      answerRefIds: prepared.map((answer) => answer.answerRefId),
    });
    keyboardChallenge.value = null;
    keyboardAnswers.value = [];
    await refresh();
  } catch {
    actionFailed.value = true;
  } finally {
    actionSaving.value = false;
  }
}

function setHostKeyDialogOpen(open: boolean) {
  if (!open && !actionSaving.value) hostKeyChallenge.value = null;
}

function setKeyboardDialogOpen(open: boolean) {
  if (!open && !actionSaving.value) {
    keyboardChallenge.value = null;
    keyboardAnswers.value = [];
  }
}

function setKeyboardAnswer(index: number, value: string) {
  keyboardAnswers.value[index] = value;
}

onMounted(async () => {
  const visualFixture = new URLSearchParams(window.location.search).get("visualFixture") === "overview"
    || window.location.hash.includes("visualFixture=overview");
  if (import.meta.env.DEV && visualFixture) {
    snapshot.value = overviewVisualFixture;
    loading.value = false;
    return;
  }
  if (!canUseDesktopCore()) {
    loading.value = false;
    loadFailed.value = true;
    return;
  }
  try {
    await reconcileMetrics();
  } catch {
    // Snapshot remains independently readable when one or more monitored Hosts need attention.
  }
  await refresh();
  refreshTimer = window.setInterval(() => void refresh(), 2_000);
});

onBeforeUnmount(() => {
  if (refreshTimer !== null) window.clearInterval(refreshTimer);
});
</script>

<template>
  <main class="overview-view">
    <NvxInlineNotice
      v-if="loading"
      :title="t('overview.loadingOverview')"
    />
    <NvxInlineNotice
      v-else-if="loadFailed || !snapshot"
      tone="error"
      :title="t('overview.loadFailed')"
    />
    <NvxOverviewPanel
      v-else
      :snapshot="snapshot"
      @connect="connect"
      @focus-terminal="focusTerminal"
      @metrics-action="beginMetricsAction"
    />
    <NvxDialog
      plugin-protected
      :model-value="hostKeyChallenge !== null"
      :title="t('overview.hostKeyTitle')"
      :description="t('overview.hostKeyDescription')"
      :close-label="t('sshHosts.cancel')"
      :dismissible="!actionSaving"
      @update:model-value="setHostKeyDialogOpen"
    >
      <template v-if="hostKeyChallenge">
        <p>{{ t('overview.hostKeyAlgorithm', { algorithm: hostKeyChallenge.algorithm }) }}</p>
        <code class="overview-view__fingerprint">
          {{ t('overview.hostKeyFingerprint', { fingerprint: hostKeyChallenge.fingerprintSha256 }) }}
        </code>
      </template>
      <NvxInlineNotice
        v-if="actionFailed"
        tone="error"
        :title="t('overview.actionFailed')"
      />
      <template #actions>
        <NvxButton
          variant="ghost"
          :disabled="actionSaving"
          @click="decideHostKey('reject')"
        >
          {{ t('overview.hostKeyReject') }}
        </NvxButton>
        <NvxButton
          :loading="actionSaving"
          @click="decideHostKey('accept')"
        >
          {{ t('overview.hostKeyAccept') }}
        </NvxButton>
      </template>
    </NvxDialog>

    <NvxDialog
      plugin-protected
      :model-value="keyboardChallenge !== null"
      :title="t('overview.authenticationTitle')"
      :description="t('overview.authenticationDescription')"
      :close-label="t('sshHosts.cancel')"
      :dismissible="!actionSaving"
      @update:model-value="setKeyboardDialogOpen"
    >
      <section
        v-if="keyboardChallenge"
        class="overview-view__prompts"
      >
        <p v-if="keyboardChallenge.name">
          <strong>{{ keyboardChallenge.name }}</strong>
        </p>
        <p v-if="keyboardChallenge.instruction">
          {{ keyboardChallenge.instruction }}
        </p>
        <NvxField
          v-for="(prompt, index) in keyboardChallenge.prompts"
          :key="prompt.promptIndex"
          :for-id="`metrics-prompt-${prompt.promptIndex}`"
          :label="prompt.label"
        >
          <NvxInput
            :id="`metrics-prompt-${prompt.promptIndex}`"
            :model-value="keyboardAnswers[index] ?? ''"
            :type="prompt.echo ? 'text' : 'password'"
            autocomplete="off"
            @update:model-value="setKeyboardAnswer(index, $event)"
          />
        </NvxField>
        <NvxInlineNotice
          v-if="actionFailed"
          tone="error"
          :title="t('overview.actionFailed')"
        />
      </section>
      <template #actions>
        <NvxButton
          variant="ghost"
          :disabled="actionSaving"
          @click="setKeyboardDialogOpen(false)"
        >
          {{ t('sshHosts.cancel') }}
        </NvxButton>
        <NvxButton
          :loading="actionSaving"
          @click="submitKeyboardAnswers"
        >
          {{ t('overview.authenticationSubmit') }}
        </NvxButton>
      </template>
    </NvxDialog>
  </main>
</template>

<style scoped>
.overview-view {
  min-width: 0;
  min-height: 0;
  height: 100%;
}

.overview-view > :deep(.nvx-inline-notice) {
  margin: var(--nvx-space-6);
}

.overview-view__fingerprint {
  display: block;
  padding: var(--nvx-space-3);
  overflow-wrap: anywhere;
  border-radius: var(--nvx-radius-sm);
  background: var(--nvx-color-bg-subtle);
}

.overview-view__prompts {
  display: grid;
  gap: var(--nvx-space-3);
}

.overview-view__prompts p {
  margin: 0;
  color: var(--nvx-color-text-secondary);
}
</style>
