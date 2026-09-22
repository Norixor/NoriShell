<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";

import { NvxOverviewPanel } from "../components/overview";
import { NvxInlineNotice } from "../components/ui";
import {
  canUseDesktopCore,
  fetchServerOverview,
  reconcileMetrics,
  retryMetrics,
} from "../core-api/client";
import type {
  MetricsSessionSummary,
  ServerOverviewSnapshot,
} from "../core-api/generated/core-api";
import { requestSecureVault } from "../core-api/secure-vault-client";
import { requestSecureSshChallenge } from "../core-api/secure-ssh-challenge-client";
import { createUuidV7 } from "../core-api/ids";
import { overviewVisualFixture } from "../overview/visual-fixture";
import { useTipsStore } from "../stores/tips";

const { t } = useI18n();
const router = useRouter();
const tips = useTipsStore();
const snapshot = ref<ServerOverviewSnapshot | null>(null);
const loading = ref(true);
const loadFailed = ref(false);
const actionSaving = ref(false);
let refreshTimer: number | null = null;
let refreshInFlight = false;
let disposed = false;

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
  const challenge = session.hostKeyChallenge ?? session.keyboardInteractiveChallenge;
  if (challenge) {
    actionSaving.value = true;
    try {
      const request = { metricsSessionId: challenge.metricsSessionId, hostId: challenge.hostId, expectedGeneration: challenge.generation, challengeId: challenge.challengeId };
      if (session.hostKeyChallenge) await requestSecureSshChallenge({ kind: "metricsHostKey", request });
      else if (session.keyboardInteractiveChallenge) await requestSecureSshChallenge({ kind: "metricsKeyboard", request: { ...request, roundIndex: session.keyboardInteractiveChallenge.roundIndex } });
      await refresh();
    } catch {
      tips.show({ scope: "overview-metrics-action", tone: "error", title: t("overview.actionFailed") });
    } finally { actionSaving.value = false; }
    return;
  }
  if (session.authenticationReason === "vaultLocked") {
    const hostVersion = snapshot.value?.cards.find((card) => card.catalogEntry.host.hostId === hostId)?.catalogEntry.host.stateVersion;
    actionSaving.value = true;
    try {
      if (!await requestSecureVault("ensureUnlocked") || disposed) return;
      // Read a fresh snapshot even when a periodic refresh is already in flight.
      const fresh = await fetchServerOverview();
      const card = fresh.cards.find((entry) => entry.catalogEntry.host.hostId === hostId);
      const current = card?.metricsSession;
      if (disposed || card?.catalogEntry.host.stateVersion !== hostVersion
        || current?.metricsSessionId !== session.metricsSessionId || current.generation !== session.generation) return;
      snapshot.value = fresh;
      await retryMetrics({ hostId, expectedGeneration: session.generation });
      if (!disposed) await refresh();
    } catch {
      if (!disposed) tips.show({ scope: "overview-metrics-action", tone: "error", title: t("overview.actionFailed") });
    } finally { actionSaving.value = false; }
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
  if (!disposed) refreshTimer = window.setInterval(() => void refresh(), 2_000);
});

onBeforeUnmount(() => {
  disposed = true;
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
