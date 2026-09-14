<script setup lang="ts">
import {
  ChevronRight,
  FileInput,
  KeyRound,
  Monitor,
  Network,
  Server,
  SquareTerminal,
  Star,
} from "lucide-vue-next";
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";

import type { HostSummary } from "../../core-api/generated/core-api";
import { NvxButton, NvxIcon, NvxInput } from "../ui";

export interface TerminalLauncherHost {
  host: HostSummary;
  connectedAtUnixMs: number | null;
}

const props = defineProps<{
  hosts: TerminalLauncherHost[];
}>();

const emit = defineEmits<{
  addHost: [];
  connectHost: [hostId: string];
  importSshConfig: [];
  localTerminal: [];
  quickConnect: [target: string];
  showAllHosts: [];
  telnet: [];
}>();

const { locale, t } = useI18n();
const quickTarget = ref("");
const relativeTimeFormatter = computed(() => new Intl.RelativeTimeFormat(locale.value, {
  numeric: "auto",
}));

function relativeConnectionTime(timestamp: number | null) {
  if (timestamp === null) return t("sshTerminal.launcherNeverConnected");
  const elapsedSeconds = Math.round((timestamp - Date.now()) / 1_000);
  const ranges: Array<[Intl.RelativeTimeFormatUnit, number]> = [
    ["day", 86_400],
    ["hour", 3_600],
    ["minute", 60],
  ];
  for (const [unit, seconds] of ranges) {
    if (Math.abs(elapsedSeconds) >= seconds || unit === "minute") {
      return relativeTimeFormatter.value.format(Math.round(elapsedSeconds / seconds), unit);
    }
  }
  return relativeTimeFormatter.value.format(0, "minute");
}

function submitQuickConnect() {
  emit("quickConnect", quickTarget.value.trim());
}
</script>

<template>
  <section
    class="terminal-launcher"
    :aria-label="t('sshTerminal.launcherTitle')"
  >
    <div class="terminal-launcher__start">
      <header class="terminal-launcher__heading">
        <h1>{{ t("sshTerminal.launcherTitle") }}</h1>
        <p>{{ t("sshTerminal.launcherDescription") }}</p>
      </header>

      <form
        class="terminal-launcher__quick"
        @submit.prevent="submitQuickConnect"
      >
        <h2>{{ t("sshTerminal.quickConnect") }}</h2>
        <div class="terminal-launcher__quick-row">
          <div class="terminal-launcher__quick-input">
            <NvxIcon
              :icon="KeyRound"
              :size="16"
            />
            <NvxInput
              v-model="quickTarget"
              :aria-label="t('sshTerminal.quickTargetLabel')"
              :placeholder="t('sshTerminal.quickTargetPlaceholder')"
            />
          </div>
          <NvxButton type="submit">
            {{ t("sshTerminal.connect") }}
          </NvxButton>
        </div>
      </form>

      <nav
        class="terminal-launcher__methods"
        :aria-label="t('sshTerminal.otherConnectionMethods')"
      >
        <button
          class="terminal-launcher__method"
          type="button"
          @click="emit('addHost')"
        >
          <span
            class="terminal-launcher__method-icon"
            aria-hidden="true"
          >
            <NvxIcon
              :icon="Server"
              :size="20"
            />
          </span>
          <span class="terminal-launcher__method-copy">
            <strong>{{ t("sshTerminal.addHost") }}</strong>
            <small>{{ t("sshTerminal.addHostDescription") }}</small>
          </span>
          <NvxIcon
            :icon="ChevronRight"
            :size="16"
          />
        </button>
        <button
          class="terminal-launcher__method"
          type="button"
          @click="emit('localTerminal')"
        >
          <span
            class="terminal-launcher__method-icon"
            aria-hidden="true"
          >
            <NvxIcon
              :icon="SquareTerminal"
              :size="20"
            />
          </span>
          <span class="terminal-launcher__method-copy">
            <strong>{{ t("sshTerminal.localTerminalAction") }}</strong>
            <small>{{ t("sshTerminal.localTerminalDescription") }}</small>
          </span>
          <NvxIcon
            :icon="ChevronRight"
            :size="16"
          />
        </button>
        <button
          class="terminal-launcher__method"
          type="button"
          @click="emit('importSshConfig')"
        >
          <span
            class="terminal-launcher__method-icon"
            aria-hidden="true"
          >
            <NvxIcon
              :icon="FileInput"
              :size="20"
            />
          </span>
          <span class="terminal-launcher__method-copy">
            <strong>{{ t("sshTerminal.importSshConfig") }}</strong>
            <small>{{ t("sshTerminal.importSshConfigDescription") }}</small>
          </span>
          <NvxIcon
            :icon="ChevronRight"
            :size="16"
          />
        </button>
        <button
          class="terminal-launcher__method"
          type="button"
          @click="emit('telnet')"
        >
          <span
            class="terminal-launcher__method-icon"
            aria-hidden="true"
          >
            <NvxIcon
              :icon="Network"
              :size="20"
            />
          </span>
          <span class="terminal-launcher__method-copy">
            <strong>{{ t("sshTerminal.telnetAction") }}</strong>
            <small>{{ t("sshTerminal.telnetDescription") }}</small>
          </span>
          <NvxIcon
            :icon="ChevronRight"
            :size="16"
          />
        </button>
      </nav>
    </div>

    <section
      class="terminal-launcher__recent"
      :aria-label="t('sshTerminal.recentConnections')"
    >
      <header class="terminal-launcher__recent-heading">
        <h2>
          {{ t("sshTerminal.recentConnections") }}
        </h2>
        <button
          type="button"
          @click="emit('showAllHosts')"
        >
          {{ t("sshTerminal.viewAllHosts") }}
        </button>
      </header>

      <div
        v-if="props.hosts.length"
        class="terminal-launcher__host-list"
      >
        <button
          v-for="entry in props.hosts"
          :key="entry.host.hostId"
          class="ssh-terminal-recent__item"
          type="button"
          @click="emit('connectHost', entry.host.hostId)"
        >
          <span
            class="terminal-launcher__host-status"
            aria-hidden="true"
          >
            <NvxIcon
              :icon="Monitor"
              :size="20"
            />
            <span />
          </span>
          <span class="terminal-launcher__host-identity">
            <strong>{{ entry.host.label }}</strong>
            <small>
              {{ entry.host.username || t("sshHosts.noUsername") }}@{{ entry.host.normalizedAddress }}:{{ entry.host.port }}
            </small>
          </span>
          <small class="terminal-launcher__host-time">
            {{ relativeConnectionTime(entry.connectedAtUnixMs) }}
          </small>
          <span
            class="terminal-launcher__favorite"
            :class="{ 'terminal-launcher__favorite--active': entry.host.favorite }"
            :title="entry.host.favorite ? t('sshTerminal.favoriteHost') : undefined"
            aria-hidden="true"
          >
            <NvxIcon
              :icon="Star"
              :size="16"
            />
          </span>
          <span class="terminal-launcher__connect-label">
            {{ t("sshTerminal.connect") }}
          </span>
        </button>
      </div>

      <div
        v-else
        class="terminal-launcher__empty"
      >
        <span aria-hidden="true"><NvxIcon
          :icon="Monitor"
          :size="22"
        /></span>
        <strong>{{ t("sshTerminal.launcherEmptyTitle") }}</strong>
        <p>{{ t("sshTerminal.launcherEmptyDescription") }}</p>
      </div>
    </section>
  </section>
</template>

<style scoped>
.terminal-launcher {
  box-sizing: border-box;
  display: grid;
  grid-template-columns: minmax(300px, 360px) minmax(0, 1fr);
  width: min(1280px, 100%);
  min-width: 0;
  color: var(--nvx-color-text-primary);
  text-align: left;
}

.terminal-launcher__start {
  min-width: 0;
  padding: var(--nvx-space-2) var(--nvx-space-10) var(--nvx-space-8) 0;
  border-right: var(--nvx-border-width) solid var(--nvx-color-border);
}

.terminal-launcher__heading h1,
.terminal-launcher__heading p,
.terminal-launcher__quick h2,
.terminal-launcher__recent-heading h2,
.terminal-launcher__empty p {
  margin: 0;
}

.terminal-launcher__heading h1 {
  font-size: var(--nvx-font-size-xl);
  line-height: var(--nvx-line-height-xl);
}

.terminal-launcher__heading p {
  max-width: 38ch;
  margin-top: var(--nvx-space-2);
  color: var(--nvx-color-text-secondary);
}

.terminal-launcher__quick {
  margin-top: var(--nvx-space-10);
}

.terminal-launcher__quick h2,
.terminal-launcher__recent-heading h2 {
  font-size: var(--nvx-font-size-md);
  line-height: var(--nvx-line-height-md);
}

.terminal-launcher__quick-row {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  gap: var(--nvx-space-3);
  margin-top: var(--nvx-space-3);
}

.terminal-launcher__quick-input {
  position: relative;
  min-width: 0;
}

.terminal-launcher__quick-input > :first-child {
  position: absolute;
  z-index: 1;
  top: 50%;
  left: var(--nvx-space-3);
  color: var(--nvx-color-text-tertiary);
  pointer-events: none;
  transform: translateY(-50%);
}

.terminal-launcher__quick-input :deep(.nvx-input) {
  padding-left: 38px;
}

.terminal-launcher__methods {
  display: grid;
  margin-top: var(--nvx-space-6);
  border-top: var(--nvx-border-width) solid var(--nvx-color-border);
}

.terminal-launcher__method {
  display: grid;
  grid-template-columns: 36px minmax(0, 1fr) auto;
  gap: var(--nvx-space-3);
  align-items: center;
  min-height: 76px;
  padding: var(--nvx-space-3) var(--nvx-space-2);
  border: 0;
  border-bottom: var(--nvx-border-width) solid var(--nvx-color-border);
  background: transparent;
  color: var(--nvx-color-text-primary);
  font: inherit;
  text-align: left;
  cursor: pointer;
}

.terminal-launcher__method:hover {
  background: var(--nvx-color-bg-hover);
}

.terminal-launcher__method:focus-visible,
.terminal-launcher__recent-heading button:focus-visible,
.ssh-terminal-recent__item:focus-visible {
  outline: var(--nvx-focus-ring-width) solid var(--nvx-color-focus-ring);
  outline-offset: calc(-1 * var(--nvx-focus-ring-width));
}

.terminal-launcher__method-icon {
  display: grid;
  width: 36px;
  height: 36px;
  place-items: center;
  color: var(--nvx-color-text-secondary);
}

.terminal-launcher__method-copy,
.terminal-launcher__host-identity {
  display: grid;
  min-width: 0;
}

.terminal-launcher__method-copy strong,
.terminal-launcher__host-identity strong {
  font-weight: var(--nvx-font-weight-semibold);
}

.terminal-launcher__method-copy small,
.terminal-launcher__host-identity small,
.terminal-launcher__host-time {
  overflow: hidden;
  color: var(--nvx-color-text-tertiary);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.terminal-launcher__recent {
  min-width: 0;
  padding: 104px 0 var(--nvx-space-8) var(--nvx-space-10);
}

.terminal-launcher__recent-heading {
  display: flex;
  gap: var(--nvx-space-4);
  align-items: center;
  justify-content: space-between;
}

.terminal-launcher__recent-heading button {
  padding: var(--nvx-space-2);
  border: 0;
  background: transparent;
  color: var(--nvx-color-accent);
  font: inherit;
  font-size: var(--nvx-font-size-sm);
  cursor: pointer;
}

.terminal-launcher__host-list {
  margin-top: var(--nvx-space-5);
  overflow: hidden;
  border: var(--nvx-border-width) solid var(--nvx-color-border);
  border-radius: var(--nvx-radius-md);
  background: var(--nvx-color-bg-surface);
}

.ssh-terminal-recent__item {
  display: grid;
  grid-template-columns: auto minmax(180px, 1fr) minmax(76px, auto) auto auto;
  gap: var(--nvx-space-4);
  align-items: center;
  width: 100%;
  min-height: 80px;
  padding: var(--nvx-space-3) var(--nvx-space-5);
  border: 0;
  border-bottom: var(--nvx-border-width) solid var(--nvx-color-border);
  background: transparent;
  color: var(--nvx-color-text-primary);
  font: inherit;
  text-align: left;
  cursor: pointer;
}

.ssh-terminal-recent__item:last-child {
  border-bottom: 0;
}

.ssh-terminal-recent__item:hover {
  background: var(--nvx-color-bg-hover);
}

.terminal-launcher__host-status {
  position: relative;
  display: grid;
  width: 28px;
  height: 28px;
  place-items: center;
  color: var(--nvx-color-text-secondary);
}

.terminal-launcher__host-status span {
  position: absolute;
  right: -1px;
  bottom: 1px;
  width: 7px;
  height: 7px;
  border: 2px solid var(--nvx-color-bg-surface);
  border-radius: 50%;
  background: var(--nvx-color-success);
}

.terminal-launcher__favorite {
  display: grid;
  width: 32px;
  height: 32px;
  place-items: center;
  color: var(--nvx-color-text-tertiary);
}

.terminal-launcher__favorite--active {
  color: var(--nvx-color-accent);
}

.terminal-launcher__favorite--active :deep(svg) {
  fill: currentColor;
}

.terminal-launcher__connect-label {
  color: var(--nvx-color-accent);
  font-size: var(--nvx-font-size-sm);
  font-weight: var(--nvx-font-weight-medium);
  white-space: nowrap;
}

@container (max-width: 960px) {
  .terminal-launcher__start {
    padding-right: var(--nvx-space-6);
  }

  .terminal-launcher__recent {
    padding-left: var(--nvx-space-6);
  }

  .ssh-terminal-recent__item {
    gap: var(--nvx-space-2);
    padding-inline: var(--nvx-space-3);
  }
}

.terminal-launcher__empty {
  display: grid;
  justify-items: center;
  min-height: 196px;
  margin-top: var(--nvx-space-5);
  place-content: center;
  border: var(--nvx-border-width) solid var(--nvx-color-border);
  border-radius: var(--nvx-radius-md);
  color: var(--nvx-color-text-secondary);
  text-align: center;
}

.terminal-launcher__empty > span {
  display: grid;
  width: 44px;
  height: 44px;
  margin-bottom: var(--nvx-space-3);
  place-items: center;
  border: var(--nvx-border-width) solid var(--nvx-color-border-strong);
  border-radius: var(--nvx-radius-md);
}

.terminal-launcher__empty p {
  max-width: 34ch;
  margin-top: var(--nvx-space-1);
  color: var(--nvx-color-text-tertiary);
}

@container (max-width: 820px) {
  .terminal-launcher {
    grid-template-columns: 1fr;
  }

  .terminal-launcher__start {
    padding-right: 0;
    border-right: 0;
  }

  .terminal-launcher__recent {
    padding: var(--nvx-space-6) 0 0;
    border-top: var(--nvx-border-width) solid var(--nvx-color-border);
  }
}

@container (max-width: 560px) {
  .terminal-launcher__heading h1 {
    font-size: var(--nvx-font-size-lg);
    line-height: var(--nvx-line-height-lg);
  }

  .terminal-launcher__quick {
    margin-top: var(--nvx-space-6);
  }

  .terminal-launcher__quick-row {
    grid-template-columns: 1fr;
  }

  .terminal-launcher__quick-row :deep(.nvx-button) {
    width: 100%;
  }

  .ssh-terminal-recent__item {
    grid-template-columns: auto minmax(0, 1fr) auto;
    min-height: 72px;
    padding: var(--nvx-space-3);
  }

  .terminal-launcher__host-time,
  .terminal-launcher__favorite {
    display: none;
  }
}
</style>
