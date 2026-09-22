<script setup lang="ts">
import {
  ArrowDown,
  ArrowUp,
  Clock3,
  Cpu,
  HardDrive,
  KeyRound,
  MemoryStick,
  RefreshCcw,
  ShieldCheck,
  TerminalSquare,
} from "lucide-vue-next";
import { computed } from "vue";
import { useI18n } from "vue-i18n";

import type { MetricFieldState, ServerOverviewCard } from "../../core-api/generated/core-api";
import {
  NvxButton,
  NvxCard,
  NvxIcon,
  NvxIconButton,
} from "../ui";
import NvxPluginContributionSlot from "../terminal/NvxPluginContributionSlot.vue";

const props = defineProps<{ card: ServerOverviewCard }>();
defineEmits<{
  connect: [hostId: string];
  focusTerminal: [sessionId: string];
  metricsAction: [hostId: string];
}>();

const { locale, t } = useI18n();
const host = computed(() => props.card.catalogEntry.host);
const endpoint = computed(() => (
  `${host.value.username ? `${host.value.username}@` : ""}${host.value.address}:${host.value.port}`
));
const compactEndpoint = computed(() => (
  host.value.port === 22 ? host.value.address : `${host.value.address}:${host.value.port}`
));
const statusDotTone = computed(() => {
  switch (props.card.connectionState) {
    case "connected":
    case "monitoringOnly":
      return "success" as const;
    case "connecting":
      return "info" as const;
    case "degraded":
      return "warning" as const;
    case "failed":
      return "danger" as const;
    default:
      return "neutral" as const;
  }
});
const statusDetails = computed(() => {
  if (!['degraded', 'failed'].includes(props.card.connectionState)) return '';
  const issues: string[] = [];
  appendResourceIssues(issues, 'terminal', props.card.terminalCounts);
  appendResourceIssues(issues, 'sftp', props.card.sftpCounts);
  appendResourceIssues(issues, 'forward', props.card.forwardCounts);
  appendResourceIssues(issues, 'metrics', props.card.metricsCounts);
  const metricsReason = metricsIssueReason(props.card.metricsSession);
  if (metricsReason) issues.push(metricsReason);
  return new Intl.ListFormat(locale.value, { style: 'long', type: 'conjunction' }).format(issues);
});
const connectionStatusDescription = computed(() => {
  const stateLabel = t(`overview.state.${props.card.connectionState}`);
  if (statusDetails.value) return `${stateLabel}: ${statusDetails.value}`;
  if (props.card.connectionState === "degraded") {
    return `${stateLabel}: ${t("overview.resourceIssue.degradedFallback")}`;
  }
  if (props.card.connectionState === "failed") {
    return `${stateLabel}: ${t("overview.resourceIssue.failedFallback")}`;
  }
  return stateLabel;
});
const latest = computed(() => props.card.metricsSession?.latestSnapshot ?? null);
const metricStatus = computed(() => {
  if (!props.card.monitoringPolicy.policy.enabled) {
    return { state: "disabled" as const, label: t("overview.monitoringDisabled") };
  }
  const session = props.card.metricsSession;
  const failure = session?.failureCode;
  if (failure === "providerUnsupported") {
    return { state: "unsupported" as const, label: t("overview.unsupported") };
  }
  if (failure === "permissionDenied") {
    return { state: "permissionDenied" as const, label: t("overview.permissionDenied") };
  }
  if (latest.value?.stale) {
    return { state: "stale" as const, label: t("overview.stale") };
  }
  if (failure) return { state: "error" as const, label: t("overview.metricError") };
  if (session?.hostKeyChallenge || session?.state === "needsHostKeyReview") {
    return { state: "disabled" as const, label: t("overview.waitingForHostKey") };
  }
  if (session?.authenticationReason === "vaultLocked") {
    return { state: "disabled" as const, label: t("overview.waitingForVault") };
  }
  if (session?.authenticationReason === "credentialUnavailable") {
    return { state: "disabled" as const, label: t("overview.waitingForCredentials") };
  }
  if (session?.keyboardInteractiveChallenge
    || session?.authenticationReason === "keyboardInteractive") {
    return { state: "disabled" as const, label: t("overview.waitingForAuthentication") };
  }
  if (session?.state === "closed" || session?.state === "disconnecting") {
    return { state: "disabled" as const, label: t("overview.monitoringStopped") };
  }
  if (!latest.value) return { state: "loading" as const, label: t("overview.loading") };
  return { state: "available" as const, label: "" };
});
const cpuPercent = computed(() => latest.value?.cpu.basisPoints == null
  ? null
  : latest.value.cpu.basisPoints / 100);
const memoryPercent = computed(() => ratioPercent(
  latest.value?.memory.usedBytes,
  latest.value?.memory.totalBytes,
));
const disk = computed(() => latest.value?.disks[0] ?? null);
const diskPercent = computed(() => ratioPercent(disk.value?.usedBytes, disk.value?.totalBytes));
const cpuStatus = computed(() => statusForField(latest.value?.cpu.state));
const memoryStatus = computed(() => statusForField(latest.value?.memory.state));
const diskStatus = computed(() => statusForField(disk.value?.state));
const networkStatus = computed(() => statusForField(latest.value?.network.state));
const networkExposesRate = computed(() => (
  networkStatus.value.state === "available" || networkStatus.value.state === "stale"
));
const sampleLabel = computed(() => {
  if (metricStatus.value.state !== "available") return metricStatus.value.label;
  if (!latest.value) return t("overview.loading");
  const elapsedSeconds = Math.max(
    0,
    Math.floor((Date.now() - latest.value.sampleCompletedAtUnixMs) / 1_000),
  );
  const relative = new Intl.RelativeTimeFormat(locale.value, {
    numeric: "always",
    style: "narrow",
  });
  if (elapsedSeconds < 60) return relative.format(-elapsedSeconds, "second");
  if (elapsedSeconds < 3_600) return relative.format(-Math.floor(elapsedSeconds / 60), "minute");
  return relative.format(-Math.floor(elapsedSeconds / 3_600), "hour");
});
const sampleTitle = computed(() => {
  if (!latest.value) return sampleLabel.value;
  return t("overview.sampledAt", {
    time: new Intl.DateTimeFormat(locale.value, {
      dateStyle: "medium",
      timeStyle: "medium",
    }).format(new Date(latest.value.sampleCompletedAtUnixMs)),
  });
});
const metricsActionLabel = computed(() => {
  const session = props.card.metricsSession;
  if (!session || !props.card.monitoringPolicy.policy.enabled) return null;
  if (session.hostKeyChallenge) return t("overview.reviewHostKey");
  if (session.keyboardInteractiveChallenge) return t("overview.answerAuthentication");
  if (session.authenticationReason === "vaultLocked") return t("overview.unlockVault");
  if (session.authenticationReason === "credentialUnavailable") return t("overview.manageCredentials");
  if (session.state === "failed" || session.state === "backoff") {
    return t("overview.retryMonitoring");
  }
  return null;
});
const metricsActionIcon = computed(() => {
  const session = props.card.metricsSession;
  if (session?.hostKeyChallenge) return ShieldCheck;
  if (session?.authenticationReason === "vaultLocked") return KeyRound;
  if (session?.keyboardInteractiveChallenge
    || session?.authenticationReason === "credentialUnavailable") return KeyRound;
  return RefreshCcw;
});
const terminalSessionCountLabel = computed(() => t("overview.terminalSessionCount", {
  count: props.card.terminalSessionIds.length,
}));

function ratioPercent(used: string | null | undefined, total: string | null | undefined) {
  if (used == null || total == null) return null;
  const denominator = BigInt(total);
  if (denominator === 0n) return null;
  return Number((BigInt(used) * 10_000n) / denominator) / 100;
}

function statusForField(fieldState: MetricFieldState | undefined) {
  if (metricStatus.value.state !== "available") return metricStatus.value;
  switch (fieldState) {
    case "available":
      return metricStatus.value;
    case "initialBaseline":
    case "counterReset":
    case "counterSetChanged":
    case "noCounterProgress":
      return { state: "loading" as const, label: t("overview.initialBaseline") };
    case "unsupported":
      return { state: "unsupported" as const, label: t("overview.unsupported") };
    case "permissionDenied":
      return { state: "permissionDenied" as const, label: t("overview.permissionDenied") };
    case "error":
      return { state: "error" as const, label: t("overview.metricError") };
    default:
      return { state: "loading" as const, label: t("overview.unavailable") };
  }
}

function appendResourceIssues(
  issues: string[],
  resource: 'terminal' | 'sftp' | 'forward' | 'metrics',
  counts: ServerOverviewCard['terminalCounts'],
) {
  if (counts.lost) {
    issues.push(t(`overview.resourceIssue.${resource}Lost`, { count: counts.lost }));
  }
  if (counts.failed) {
    issues.push(t(`overview.resourceIssue.${resource}Failed`, { count: counts.failed }));
  }
}

function metricsIssueReason(session: ServerOverviewCard['metricsSession']) {
  if (!session) return '';
  if (session.authenticationReason) {
    return t(`overview.resourceIssue.authentication.${session.authenticationReason}`);
  }
  if (session.failureCode) {
    return t(`overview.resourceIssue.metricsFailure.${session.failureCode}`);
  }
  if (session.hostKeyChallenge || session.state === 'needsHostKeyReview') {
    return t('overview.resourceIssue.hostKeyReview');
  }
  return '';
}

function formatPercent(value: number | null, status: ReturnType<typeof statusForField>) {
  return value !== null && ["available", "stale"].includes(status.state)
    ? `${Math.round(value)}%`
    : "—";
}

function metricAriaLabel(
  label: string,
  value: string,
  status: ReturnType<typeof statusForField>,
) {
  return status.label ? `${label}: ${status.label}` : `${label}: ${value}`;
}

function formatRate(value: string | null | undefined) {
  return value == null ? "—" : `${formatBytes(value)}/s`;
}

function formatCompactRate(value: string | null | undefined) {
  if (value == null) return "—";
  const bytes = BigInt(value);
  const units = ["B", "K", "M", "G", "T", "P"];
  let divisor = 1n;
  let unit = 0;
  while (unit < units.length - 1 && bytes >= divisor * 1024n) {
    divisor *= 1024n;
    unit += 1;
  }
  if (unit === 1 && bytes >= 100n * 1024n) {
    divisor *= 1024n;
    unit += 1;
  }
  const tenths = (bytes * 10n) / divisor;
  if (unit === 0) return `${tenths / 10n}${units[unit]}`;
  if (unit === 1) return `${tenths / 10n}${units[unit]}`;
  return `${tenths / 10n}.${tenths % 10n}${units[unit]}`;
}

function formatBytes(value: string) {
  const bytes = BigInt(value);
  const units = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
  let divisor = 1n;
  let unit = 0;
  while (unit < units.length - 1 && bytes >= divisor * 1024n) {
    divisor *= 1024n;
    unit += 1;
  }
  const tenths = (bytes * 10n) / divisor;
  return `${tenths / 10n}.${tenths % 10n} ${units[unit]}`;
}
</script>

<template>
  <NvxCard
    class="nvx-server-card"
    :aria-labelledby="`server-card-${host.hostId}`"
  >
    <header class="nvx-server-card__header">
      <div class="nvx-server-card__identity">
        <h2
          :id="`server-card-${host.hostId}`"
          :title="host.label"
        >
          {{ host.label }}
        </h2>
        <div class="nvx-server-card__metadata">
          <span
            v-if="card.catalogEntry.group"
            class="nvx-server-card__group"
            :title="card.catalogEntry.group.label"
          >
            {{ card.catalogEntry.group.label }}
          </span>
          <span
            class="nvx-server-card__endpoint"
            :aria-label="t('overview.endpoint')"
            :title="endpoint"
          >
            {{ compactEndpoint }}
          </span>
        </div>
      </div>
      <div class="nvx-server-card__primary-actions">
        <NvxButton
          class="nvx-server-card__new-terminal"
          size="sm"
          @click="$emit('connect', host.hostId)"
        >
          <NvxIcon
            :icon="TerminalSquare"
            :size="16"
          />
          {{ t('overview.newTerminal') }}
        </NvxButton>
      </div>
    </header>

    <div
      class="nvx-server-card__metrics"
      :aria-label="t('overview.resources')"
    >
      <div
        class="nvx-server-card__metric"
        :data-status="cpuStatus.state"
        :aria-label="metricAriaLabel(t('overview.cpu'), formatPercent(cpuPercent, cpuStatus), cpuStatus)"
        role="status"
      >
        <span class="nvx-server-card__metric-label">
          <NvxIcon
            :icon="Cpu"
            :size="16"
          />
          {{ t('overview.cpu') }}
        </span>
        <strong>{{ formatPercent(cpuPercent, cpuStatus) }}</strong>
      </div>
      <div
        class="nvx-server-card__metric"
        :data-status="memoryStatus.state"
        :aria-label="metricAriaLabel(t('overview.memory'), formatPercent(memoryPercent, memoryStatus), memoryStatus)"
        role="status"
      >
        <span class="nvx-server-card__metric-label">
          <NvxIcon
            :icon="MemoryStick"
            :size="16"
          />
          {{ t('overview.memory') }}
        </span>
        <strong>{{ formatPercent(memoryPercent, memoryStatus) }}</strong>
      </div>
      <div
        class="nvx-server-card__metric"
        :data-status="diskStatus.state"
        :aria-label="metricAriaLabel(t('overview.disk'), formatPercent(diskPercent, diskStatus), diskStatus)"
        role="status"
      >
        <span class="nvx-server-card__metric-label">
          <NvxIcon
            :icon="HardDrive"
            :size="16"
          />
          {{ t('overview.disk') }}
        </span>
        <strong>{{ formatPercent(diskPercent, diskStatus) }}</strong>
      </div>
      <div
        class="nvx-server-card__metric nvx-server-card__network"
        :class="`nvx-server-card__network--${networkStatus.state}`"
        :data-status="networkStatus.state"
        :aria-label="networkExposesRate
          ? `${t('overview.receive')} ${formatRate(latest?.network.receiveBytesPerSecond)}, ${t('overview.transmit')} ${formatRate(latest?.network.transmitBytesPerSecond)}`
          : `${t('overview.network')}: ${networkStatus.label}`"
        role="status"
      >
        <span class="nvx-server-card__metric-label">{{ t('overview.network') }}</span>
        <span class="nvx-server-card__network-values">
          <template v-if="networkExposesRate">
            <span>
              <NvxIcon
                :icon="ArrowDown"
                :size="16"
              />{{ formatCompactRate(latest?.network.receiveBytesPerSecond) }}
            </span>
            <span>
              <NvxIcon
                :icon="ArrowUp"
                :size="16"
              />{{ formatCompactRate(latest?.network.transmitBytesPerSecond) }}
            </span>
          </template>
          <template v-else>
            <span>—</span>
          </template>
        </span>
      </div>
    </div>

    <footer class="nvx-server-card__footer">
      <span class="nvx-server-card__actions">
        <NvxPluginContributionSlot
          extension-slot="overviewCardActions"
          :instance-key="host.hostId"
          :display-label="host.label"
        />
        <span
          class="nvx-server-card__connection-dot"
          :class="`nvx-server-card__connection-dot--${statusDotTone}`"
          :title="connectionStatusDescription"
          :data-tooltip="connectionStatusDescription"
          :aria-label="connectionStatusDescription"
          role="status"
          tabindex="0"
        />
        <span
          v-if="card.terminalSessionIds.length"
          class="nvx-server-card__sessions"
        >
          <span class="nvx-server-card__session-count">
            <NvxIcon
              :icon="TerminalSquare"
              :size="16"
            />
            {{ terminalSessionCountLabel }}
          </span>
          <NvxIconButton
            v-for="(sessionId, index) in card.terminalSessionIds"
            :key="sessionId"
            :label="t('overview.openTerminalNumber', { index: index + 1 })"
            size="sm"
            @click="$emit('focusTerminal', sessionId)"
          >
            <NvxIcon
              :icon="TerminalSquare"
              :size="16"
            />
          </NvxIconButton>
        </span>
        <NvxIconButton
          v-if="metricsActionLabel"
          :label="metricsActionLabel"
          size="sm"
          @click="$emit('metricsAction', host.hostId)"
        >
          <NvxIcon
            :icon="metricsActionIcon"
            :size="16"
          />
        </NvxIconButton>
      </span>
      <span
        class="nvx-server-card__sample"
        :class="`nvx-server-card__sample--${metricStatus.state}`"
        :title="sampleTitle"
      >
        <NvxIcon
          :icon="Clock3"
          :size="16"
        />
        {{ sampleLabel }}
      </span>
    </footer>
  </NvxCard>
</template>

<style scoped>
.nvx-server-card {
  position: relative;
  display: grid;
  grid-template-rows: auto minmax(44px, 1fr) 32px;
  height: 168px;
  padding: 0 var(--nvx-space-3);
  overflow: hidden;
}

.nvx-server-card__header,
.nvx-server-card__metadata,
.nvx-server-card__footer,
.nvx-server-card__primary-actions,
.nvx-server-card__metric-label,
.nvx-server-card__network-values,
.nvx-server-card__actions,
.nvx-server-card__sessions,
.nvx-server-card__session-count,
.nvx-server-card__sample {
  display: flex;
  min-width: 0;
  align-items: center;
}

.nvx-server-card__header {
  gap: var(--nvx-space-3);
  justify-content: space-between;
  min-height: 62px;
  padding: var(--nvx-space-2) 0;
  border-bottom: var(--nvx-border-width) solid var(--nvx-color-border);
}

.nvx-server-card__identity {
  flex: 1 1 auto;
  min-width: 0;
}

.nvx-server-card__metadata {
  gap: var(--nvx-space-2);
  margin-top: 2px;
  min-width: 0;
}

.nvx-server-card__identity h2 {
  margin: 0;
  font-size: var(--nvx-font-size-body);
  line-height: 18px;
  font-weight: var(--nvx-font-weight-semibold);
  overflow-wrap: anywhere;
}

.nvx-server-card__group {
  flex: 0 0 auto;
  padding: 0 var(--nvx-space-1);
  border-radius: var(--nvx-radius-sm);
  background: var(--nvx-color-bg-subtle);
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-xs);
  line-height: var(--nvx-line-height-xs);
}

.nvx-server-card__endpoint {
  min-width: 0;
  color: var(--nvx-color-text-tertiary);
  font-family: var(--nvx-font-mono);
  font-size: var(--nvx-font-size-xs);
  line-height: var(--nvx-line-height-xs);
  overflow-wrap: anywhere;
}

.nvx-server-card__primary-actions {
  flex: 0 0 auto;
  align-self: flex-start;
}

.nvx-server-card__new-terminal {
  gap: var(--nvx-space-1);
  min-width: 94px;
  padding-inline: var(--nvx-space-2);
  white-space: nowrap;
}

.nvx-server-card__metrics {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 0.78fr)) minmax(74px, 1.15fr);
  align-items: center;
  min-width: 0;
  border-bottom: var(--nvx-border-width) solid var(--nvx-color-border);
}

.nvx-server-card__metric {
  display: grid;
  gap: 2px;
  min-width: 0;
  padding: 0 var(--nvx-space-2);
  color: var(--nvx-color-text-secondary);
}

.nvx-server-card__metric:first-child {
  padding-inline-start: 0;
}

.nvx-server-card__metric + .nvx-server-card__metric {
  border-inline-start: var(--nvx-border-width) solid var(--nvx-color-border);
}

.nvx-server-card__metric-label {
  gap: 3px;
  font-size: 10px;
  line-height: 14px;
  white-space: nowrap;
}

.nvx-server-card__metric-label :deep(svg),
.nvx-server-card__session-count :deep(svg),
.nvx-server-card__sample :deep(svg) {
  width: 14px;
  height: 14px;
}

.nvx-server-card__metric strong {
  overflow: hidden;
  color: var(--nvx-color-text-primary);
  font-size: var(--nvx-font-size-sm);
  font-variant-numeric: tabular-nums;
  font-weight: var(--nvx-font-weight-semibold);
  line-height: var(--nvx-line-height-sm);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.nvx-server-card__metric[data-status="stale"] strong,
.nvx-server-card__network--stale {
  color: var(--nvx-color-warning);
}

.nvx-server-card__metric[data-status="error"] strong,
.nvx-server-card__metric[data-status="permissionDenied"] strong,
.nvx-server-card__network--error,
.nvx-server-card__network--permissionDenied {
  color: var(--nvx-color-danger);
}

.nvx-server-card__footer {
  gap: var(--nvx-space-1);
  justify-content: space-between;
  min-width: 0;
}

.nvx-server-card__sample {
  flex: 0 1 auto;
  gap: var(--nvx-space-1);
  min-width: 0;
  overflow: hidden;
  color: var(--nvx-color-text-tertiary);
  font-size: var(--nvx-font-size-xs);
  line-height: var(--nvx-line-height-xs);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.nvx-server-card__sample--stale {
  color: var(--nvx-color-warning);
}

.nvx-server-card__sample--error,
.nvx-server-card__sample--permissionDenied {
  color: var(--nvx-color-danger);
}

.nvx-server-card__network-values {
  gap: var(--nvx-space-1);
  color: var(--nvx-color-text-primary);
  font-family: var(--nvx-font-mono);
  font-size: 10px;
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}

.nvx-server-card__network-values > span {
  display: inline-flex;
  align-items: center;
}

.nvx-server-card__network-values :deep(svg) {
  width: 12px;
  height: 12px;
}

.nvx-server-card__actions {
  flex: 0 0 auto;
  gap: var(--nvx-space-1);
  min-width: 0;
}

.nvx-server-card__connection-dot {
  position: relative;
  flex: 0 0 auto;
  width: 8px;
  height: 8px;
  margin-inline-end: 5px;
  border-radius: 50%;
  background: var(--nvx-color-border-strong);
}

.nvx-server-card__connection-dot--info { background: var(--nvx-color-accent); }
.nvx-server-card__connection-dot--success { background: var(--nvx-color-success); }
.nvx-server-card__connection-dot--warning { background: var(--nvx-color-warning); }
.nvx-server-card__connection-dot--danger { background: var(--nvx-color-danger); }

.nvx-server-card__connection-dot--warning::after,
.nvx-server-card__connection-dot--danger::after {
  position: absolute;
  z-index: var(--nvx-z-popover);
  inset-inline-start: -4px;
  bottom: calc(100% + 8px);
  width: max-content;
  max-width: 240px;
  padding: var(--nvx-space-1) var(--nvx-space-2);
  border: var(--nvx-border-width) solid var(--nvx-color-border-strong);
  border-radius: var(--nvx-radius-sm);
  background: var(--nvx-color-bg-surface);
  box-shadow: var(--nvx-shadow-overlay);
  color: var(--nvx-color-text-primary);
  content: attr(data-tooltip);
  font-size: var(--nvx-font-size-xs);
  line-height: var(--nvx-line-height-xs);
  opacity: 0;
  pointer-events: none;
  transform: translateY(2px);
  transition: opacity 120ms ease, transform 120ms ease;
  visibility: hidden;
  white-space: normal;
}

.nvx-server-card__connection-dot--warning:hover::after,
.nvx-server-card__connection-dot--warning:focus-visible::after,
.nvx-server-card__connection-dot--danger:hover::after,
.nvx-server-card__connection-dot--danger:focus-visible::after {
  opacity: 1;
  transform: translateY(0);
  visibility: visible;
}

.nvx-server-card__connection-dot:focus-visible {
  outline: 2px solid var(--nvx-color-focus-ring);
  outline-offset: 3px;
}

.nvx-server-card__sessions {
  gap: 2px;
  max-width: 128px;
  overflow-x: auto;
  scrollbar-width: none;
}

.nvx-server-card__session-count {
  gap: var(--nvx-space-1);
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-xs);
  white-space: nowrap;
}

.nvx-server-card__sessions::-webkit-scrollbar {
  display: none;
}
</style>
