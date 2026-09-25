<script setup lang="ts">
import { ShieldCheck } from "lucide-vue-next";
import type { Component } from "vue";
import { useI18n } from "vue-i18n";

import { detectDesktopPlatform } from "../../platform";
import { NvxIcon } from "../ui";
import NvxStandaloneHeader from "./NvxStandaloneHeader.vue";

withDefaults(defineProps<{
  title: string;
  description: string;
  icon?: Component;
  layout?: "form" | "review";
  danger?: boolean;
  variant?: "approval" | "permissions";
}>(), { icon: () => ShieldCheck, layout: "review", danger: false, variant: "approval" });

const { t } = useI18n();
const platform = detectDesktopPlatform();

</script>

<template>
  <main
    class="nvx-secure-window"
    :class="[`nvx-secure-window--${layout}`, `nvx-secure-window--${variant}`, { 'nvx-secure-window--danger': danger }]"
    :data-platform="platform"
    data-plugin-protected
    data-theme-protected
  >
    <NvxStandaloneHeader
      class="nvx-secure-window__chrome"
    >
      <span class="nvx-secure-window__protected">
        <NvxIcon
          :icon="ShieldCheck"
          :size="20"
        />
        {{ t("window.protected") }}
      </span>
    </NvxStandaloneHeader>
    <div class="nvx-secure-window__content">
      <header class="nvx-secure-window__intro">
        <span
          v-if="variant === 'permissions'"
          class="nvx-secure-window__symbol"
        ><NvxIcon
          :icon="icon"
          :size="20"
        /></span>
        <h1>{{ title }}</h1>
        <p>{{ description }}</p>
      </header>
      <div class="nvx-secure-window__scroll">
        <div class="nvx-secure-window__body">
          <slot />
        </div>
        <aside
          v-if="$slots.support"
          class="nvx-secure-window__support"
        >
          <slot name="support" />
        </aside>
      </div>
      <div
        v-if="$slots.decision"
        class="nvx-secure-window__decision"
      >
        <slot name="decision" />
      </div>
      <footer
        v-if="$slots.actions"
        class="nvx-secure-window__footer"
      >
        <div
          v-if="$slots.actionHint || variant === 'approval'"
          class="nvx-secure-window__action-hint"
        >
          <slot name="actionHint">
            {{ t('window.approvalHint') }}
          </slot>
        </div>
        <div class="nvx-secure-window__actions">
          <slot name="actions" />
        </div>
      </footer>
    </div>
  </main>
</template>

<style scoped>
:global(html:has(#secure-app)), :global(body:has(#secure-app)), :global(#secure-app) {
  width: 100%; min-width: 0; height: 100%; min-height: 0; margin: 0;
}
.nvx-secure-window {
  display: grid; grid-template-rows: auto minmax(0, 1fr); height: 100dvh;
  min-width: 0; overflow: hidden; background: var(--nvx-color-bg-canvas); color: var(--nvx-color-text-primary);
  font-size: var(--nvx-font-size-sm); line-height: var(--nvx-line-height-sm);
}
.nvx-secure-window__protected {
  display: inline-flex; align-items: center; gap: var(--nvx-space-1);
  color: var(--nvx-color-success); font-size: var(--nvx-font-size-xs); font-weight: var(--nvx-font-weight-medium);
}
.nvx-secure-window__protected svg { flex: none; width: 16px; height: 16px; }
.nvx-secure-window__content {
  display: flex; flex-direction: column; width: 100%; max-width: 840px;
  min-width: 0; min-height: 0; margin-inline: auto; padding-inline: var(--nvx-space-5);
}
.nvx-secure-window__intro { flex: none; padding-block: var(--nvx-space-4) var(--nvx-space-3); }
.nvx-secure-window__intro h1 {
  margin: 0; font-size: var(--nvx-font-size-lg); line-height: var(--nvx-line-height-lg);
  font-weight: var(--nvx-font-weight-semibold); overflow-wrap: anywhere;
}
.nvx-secure-window__intro p { margin: var(--nvx-space-1) 0 0; color: var(--nvx-color-text-secondary); }
.nvx-secure-window__scroll {
  flex: 1;
  min-height: 0;
  margin-inline: calc(-1 * var(--nvx-focus-ring-width));
  padding-inline: var(--nvx-focus-ring-width);
  padding-bottom: var(--nvx-space-3);
  overflow: auto;
  scrollbar-gutter: stable;
}
.nvx-secure-window__decision {
  flex: none; min-width: 0; padding-block: var(--nvx-space-2);
  border-top: var(--nvx-border-width) solid var(--nvx-color-border);
}
.nvx-secure-window__body { display: grid; align-content: start; gap: var(--nvx-space-3); min-width: 0; }
.nvx-secure-window__body :deep(h2) { margin: 0; font-size: var(--nvx-font-size-sm); line-height: var(--nvx-line-height-sm); }
.nvx-secure-window__body :deep(p) { margin: 0; }
.nvx-secure-window__footer {
  display: flex; align-items: center; justify-content: space-between; flex: none; gap: var(--nvx-space-3);
  min-height: 60px; padding-block: var(--nvx-space-3); border-top: var(--nvx-border-width) solid var(--nvx-color-border);
}
.nvx-secure-window__action-hint { color: var(--nvx-color-text-tertiary); font-size: var(--nvx-font-size-xs); line-height: var(--nvx-line-height-xs); }
.nvx-secure-window__actions { display: flex; align-items: center; justify-content: flex-end; gap: var(--nvx-space-2); margin-left: auto; }
.nvx-secure-window__actions :deep(.nvx-button) { min-height: var(--nvx-control-height-sm); height: auto; padding: 6px var(--nvx-space-3); font-size: var(--nvx-font-size-sm); line-height: var(--nvx-line-height-xs); white-space: normal; }
.nvx-secure-window__support { border-top: var(--nvx-border-width) solid var(--nvx-color-border); padding-top: var(--nvx-space-3); margin-top: var(--nvx-space-3); color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-xs); }
.nvx-secure-window__body :deep(.secure-facts) { display: grid; gap: var(--nvx-space-1); min-width: 0; margin: 0; padding-block: var(--nvx-space-2); border-block: var(--nvx-border-width) solid var(--nvx-color-border); }
.nvx-secure-window__body :deep(.secure-facts > div) { display: grid; grid-template-columns: 104px minmax(0, 1fr); gap: var(--nvx-space-3); }
.nvx-secure-window__body :deep(dt) { color: var(--nvx-color-text-secondary); }
.nvx-secure-window__body :deep(dd) { min-width: 0; margin: 0; overflow-wrap: anywhere; }
.nvx-secure-window__body :deep(code) { font-family: var(--nvx-font-mono); font-size: var(--nvx-font-size-xs); }
.nvx-secure-window--approval .nvx-secure-window__body :deep(.nvx-inline-notice) { gap: var(--nvx-space-2); padding: var(--nvx-space-2) 0; border: 0; border-radius: 0; background: transparent; font-size: var(--nvx-font-size-xs); line-height: var(--nvx-line-height-xs); }
.nvx-secure-window--approval .nvx-secure-window__body :deep(.nvx-inline-notice strong) { font-weight: var(--nvx-font-weight-medium); }
.nvx-secure-window--approval .nvx-secure-window__body :deep(.nvx-inline-notice > svg) { width: 16px; height: 16px; flex: none; }
.nvx-secure-window--permissions .nvx-secure-window__body :deep(.nvx-inline-notice) { padding: var(--nvx-space-2) var(--nvx-space-3); gap: var(--nvx-space-2); font-size: var(--nvx-font-size-xs); line-height: var(--nvx-line-height-xs); }
.nvx-secure-window--permissions .nvx-secure-window__intro { display: grid; grid-template-columns: 20px minmax(0, 1fr); column-gap: var(--nvx-space-2); align-items: center; }
.nvx-secure-window--permissions .nvx-secure-window__intro p { grid-column: 2; }
.nvx-secure-window__symbol { display: flex; color: var(--nvx-color-accent); }
@media (max-width: 520px) {
  .nvx-secure-window__content { padding-inline: var(--nvx-space-4); }
  .nvx-secure-window__body :deep(.secure-facts > div) { grid-template-columns: 88px minmax(0, 1fr); gap: var(--nvx-space-2); }
}
</style>
