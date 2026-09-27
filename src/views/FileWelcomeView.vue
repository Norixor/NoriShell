<script setup lang="ts">
import { ArrowRight, Folder, FolderOpen, Plus, Server } from "lucide-vue-next";
import { useI18n } from "vue-i18n";

import { NvxIcon } from "../components/ui";
import type { HostSummary } from "../core-api/generated/core-api";

defineProps<{
  hosts: readonly HostSummary[];
  loading: boolean;
}>();

const emit = defineEmits<{
  createLocal: [];
  createEmptyRemote: [];
  createRemote: [hostId: string];
}>();

const { t } = useI18n();
</script>

<template>
  <main class="file-welcome">
    <div class="file-welcome__content">
      <header class="file-welcome__header">
        <h1>{{ t("fileWorkspace.title") }}</h1>
        <p>{{ t("fileWorkspace.subtitle") }}</p>
      </header>

      <div class="file-welcome__choices">
        <button
          class="file-welcome__choice"
          type="button"
          @click="emit('createLocal')"
        >
          <span
            class="file-welcome__preview"
            aria-hidden="true"
          >
            <span class="file-welcome__mini-pane">
              <span class="file-welcome__mini-toolbar">
                <span class="file-welcome__mini-dots"><i /><i /><i /></span>
                <span class="file-welcome__mini-add"><NvxIcon
                  :icon="Plus"
                  :size="16"
                />{{ t("fileWorkspace.addPane") }}</span>
              </span>
              <NvxIcon
                :icon="FolderOpen"
                :size="22"
              />
            </span>
          </span>
          <span class="file-welcome__choice-title">{{ t("fileWorkspace.localTitle") }}</span>
          <span class="file-welcome__choice-description">{{ t("fileWorkspace.localDescription") }}</span>
        </button>

        <button
          class="file-welcome__choice file-welcome__choice--remote"
          type="button"
          @click="emit('createEmptyRemote')"
        >
          <span
            class="file-welcome__preview file-welcome__preview--split"
            aria-hidden="true"
          >
            <span class="file-welcome__mini-pane">
              <span class="file-welcome__mini-toolbar">
                <span class="file-welcome__mini-dots"><i /><i /><i /></span>
                <span class="file-welcome__mini-add"><NvxIcon
                  :icon="Plus"
                  :size="16"
                />{{ t("fileWorkspace.addPane") }}</span>
              </span>
              <NvxIcon
                :icon="FolderOpen"
                :size="22"
              />
            </span>
            <span class="file-welcome__mini-pane">
              <span class="file-welcome__mini-toolbar">
                <span class="file-welcome__mini-dots"><i /><i /><i /></span>
                <span class="file-welcome__mini-add"><NvxIcon
                  :icon="Plus"
                  :size="16"
                />{{ t("fileWorkspace.addPane") }}</span>
              </span>
              <NvxIcon
                :icon="Folder"
                :size="22"
              />
            </span>
          </span>
          <span
            class="file-welcome__preview-labels"
            aria-hidden="true"
          >
            <span>{{ t("fileWorkspace.localPane") }}</span>
            <span>{{ t("fileWorkspace.remotePane") }}</span>
          </span>
          <span class="file-welcome__choice-title">{{ t("fileWorkspace.remoteTitle") }}</span>
          <span class="file-welcome__choice-description">{{ t("fileWorkspace.remoteDescription") }}</span>
        </button>
      </div>

      <section
        id="file-welcome-saved-hosts"
        class="file-welcome__hosts"
        :aria-label="t('fileWorkspace.savedHostsTitle')"
      >
        <h2>{{ t("fileWorkspace.savedHostsTitle") }}</h2>
        <p
          v-if="loading"
          class="file-welcome__hosts-message"
          role="status"
        >
          {{ t("fileWorkspace.loadingHosts") }}
        </p>
        <p
          v-else-if="hosts.length === 0"
          class="file-welcome__hosts-message"
        >
          {{ t("fileWorkspace.noHosts") }}
        </p>
        <ul
          v-else
          class="file-welcome__host-list"
        >
          <li
            v-for="host in hosts"
            :key="host.hostId"
          >
            <button
              class="file-welcome__host"
              type="button"
              :aria-label="t('fileWorkspace.openHost', { host: host.label })"
              @click="emit('createRemote', host.hostId)"
            >
              <span class="file-welcome__host-icon"><NvxIcon
                :icon="Server"
                :size="20"
              /></span>
              <span class="file-welcome__host-identity">
                <strong>{{ host.label }}</strong>
                <small>{{ host.normalizedAddress }}:{{ host.port }}</small>
              </span>
              <span class="file-welcome__host-action">
                <NvxIcon
                  :icon="FolderOpen"
                  :size="20"
                />
                {{ t("fileWorkspace.openWorkspace") }}
                <NvxIcon
                  :icon="ArrowRight"
                  :size="16"
                />
              </span>
            </button>
          </li>
        </ul>
      </section>
    </div>
  </main>
</template>

<style scoped>
.file-welcome {
  width: 100%;
  height: 100%;
  min-width: 0;
  overflow: auto;
  background: var(--nvx-color-bg-canvas);
  color: var(--nvx-color-text-primary);
}

.file-welcome__content {
  box-sizing: border-box;
  width: min(100%, 1040px);
  min-height: 100%;
  margin-inline: auto;
  padding: clamp(36px, 8vh, 92px) var(--nvx-space-6) var(--nvx-space-8);
}

.file-welcome__header {
  margin-bottom: var(--nvx-space-8);
  text-align: center;
}

.file-welcome__header h1 {
  margin: 0;
  font-size: var(--nvx-font-size-xl);
  line-height: var(--nvx-line-height-xl);
  font-weight: var(--nvx-font-weight-semibold);
}

.file-welcome__header p {
  margin: var(--nvx-space-2) 0 0;
  color: var(--nvx-color-text-secondary);
}

.file-welcome__choices {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: var(--nvx-space-5);
}

.file-welcome__choice {
  display: flex;
  flex-direction: column;
  align-items: center;
  min-width: 0;
  min-height: 288px;
  padding: var(--nvx-space-6);
  border: var(--nvx-border-width) solid var(--nvx-color-border-strong);
  border-radius: var(--nvx-radius-md);
  background: var(--nvx-color-bg-surface);
  color: var(--nvx-color-text-primary);
  font: inherit;
  text-align: center;
  cursor: pointer;
  transition: border-color var(--nvx-motion-fast), background-color var(--nvx-motion-fast);
}

.file-welcome__choice:hover:not(:disabled),
.file-welcome__choice:focus-visible {
  border-color: var(--nvx-color-accent);
  background: var(--nvx-color-accent-soft);
}

.file-welcome__choice:focus-visible,
.file-welcome__host:focus-visible {
  outline: var(--nvx-focus-ring-width) solid var(--nvx-color-focus-ring);
  outline-offset: 2px;
}

.file-welcome__choice:disabled {
  cursor: not-allowed;
  opacity: .56;
}

.file-welcome__preview {
  display: flex;
  width: min(100%, 340px);
  height: 144px;
  margin-bottom: var(--nvx-space-5);
}

.file-welcome__preview--split {
  gap: var(--nvx-space-1);
  margin-bottom: 0;
}

.file-welcome__mini-pane {
  position: relative;
  display: flex;
  flex: 1 1 0;
  align-items: center;
  justify-content: center;
  min-width: 0;
  border: var(--nvx-border-width) solid var(--nvx-color-border);
  border-radius: var(--nvx-radius-sm);
  background: var(--nvx-color-bg-surface);
  color: var(--nvx-color-text-tertiary);
}

.file-welcome__mini-toolbar {
  position: absolute;
  inset: 0 0 auto;
  display: flex;
  align-items: center;
  justify-content: space-between;
  min-height: 26px;
  padding: 0 var(--nvx-space-2);
  border-bottom: var(--nvx-border-width) solid var(--nvx-color-border);
  background: var(--nvx-color-bg-subtle);
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-xs);
}

.file-welcome__mini-dots {
  display: inline-flex;
  gap: 3px;
}

.file-welcome__mini-dots i {
  width: 3px;
  height: 3px;
  border-radius: 50%;
  background: currentColor;
}

.file-welcome__mini-add {
  display: inline-flex;
  align-items: center;
  gap: 2px;
  white-space: nowrap;
}

.file-welcome__preview-labels {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  width: min(100%, 340px);
  margin: var(--nvx-space-2) 0 var(--nvx-space-3);
  font-size: var(--nvx-font-size-xs);
}

.file-welcome__choice-title {
  margin-top: auto;
  font-size: var(--nvx-font-size-lg);
  line-height: var(--nvx-line-height-lg);
  font-weight: var(--nvx-font-weight-semibold);
}

.file-welcome__choice-description {
  margin-top: var(--nvx-space-2);
  color: var(--nvx-color-text-secondary);
  line-height: var(--nvx-line-height-sm);
}

.file-welcome__hosts {
  margin-top: var(--nvx-space-8);
  padding-top: var(--nvx-space-5);
  border-top: var(--nvx-border-width) solid var(--nvx-color-border);
}

.file-welcome__hosts h2 {
  margin: 0 0 var(--nvx-space-3);
  font-size: var(--nvx-font-size-md);
  line-height: var(--nvx-line-height-md);
  font-weight: var(--nvx-font-weight-semibold);
}

.file-welcome__hosts-message {
  margin: 0;
  padding: var(--nvx-space-4);
  border: var(--nvx-border-width) solid var(--nvx-color-border);
  border-radius: var(--nvx-radius-md);
  color: var(--nvx-color-text-secondary);
}

.file-welcome__host-list {
  display: grid;
  gap: var(--nvx-space-2);
  margin: 0;
  padding: 0;
  list-style: none;
}

.file-welcome__host {
  display: flex;
  align-items: center;
  gap: var(--nvx-space-3);
  width: 100%;
  min-height: 68px;
  padding: var(--nvx-space-3) var(--nvx-space-4);
  border: var(--nvx-border-width) solid var(--nvx-color-border);
  border-radius: var(--nvx-radius-md);
  background: var(--nvx-color-bg-surface);
  color: var(--nvx-color-text-primary);
  font: inherit;
  text-align: start;
  cursor: pointer;
}

.file-welcome__host:hover {
  border-color: var(--nvx-color-border-strong);
  background: var(--nvx-color-bg-hover);
}

.file-welcome__host-icon {
  display: flex;
  flex: 0 0 40px;
  align-items: center;
  justify-content: center;
  height: 40px;
  border-radius: var(--nvx-radius-md);
  background: var(--nvx-color-bg-subtle);
  color: var(--nvx-color-text-secondary);
}

.file-welcome__host-identity {
  display: grid;
  flex: 1 1 auto;
  gap: 2px;
  min-width: 0;
}

.file-welcome__host-identity strong,
.file-welcome__host-identity small {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.file-welcome__host-identity strong {
  font-weight: var(--nvx-font-weight-semibold);
}

.file-welcome__host-identity small {
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-sm);
}

.file-welcome__host-action {
  display: inline-flex;
  flex: 0 0 auto;
  align-items: center;
  gap: var(--nvx-space-2);
  color: var(--nvx-color-accent);
  font-weight: var(--nvx-font-weight-medium);
  white-space: nowrap;
}

@media (max-width: 720px) {
  .file-welcome__content { padding-inline: var(--nvx-space-4); }
  .file-welcome__choices { grid-template-columns: minmax(0, 1fr); }
  .file-welcome__choice { min-height: 240px; }
}

@media (max-width: 480px) {
  .file-welcome__host-action { font-size: 0; gap: var(--nvx-space-1); }
  .file-welcome__host-action :deep(svg) { width: 18px; }
}

@media (prefers-reduced-motion: reduce) {
  .file-welcome__choice { transition: none; }
}
</style>
