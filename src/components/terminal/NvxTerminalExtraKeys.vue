<script setup lang="ts">
import { ArrowDown, ArrowLeft, ArrowRight, ArrowUp } from "lucide-vue-next";
import { computed, type Component } from "vue";
import { useI18n } from "vue-i18n";

import { useTipsStore } from "../../stores/tips";
import {
  canSendKeysToFocusedTerminal,
  focusedTerminalUsesApplicationCursorKeys,
  sendKeysToFocusedTerminal,
} from "../../terminal-input-target";
import {
  extraKeySequence, stickyAlt, stickyCtrl, toggleStickyAlt, toggleStickyCtrl, type ExtraKey,
} from "../../terminal/extra-keys";
import { NvxIcon } from "../ui";

/**
 * Soft-keyboard helper row for phones. Every key goes through the focused Pane's
 * acknowledged input lease, exactly like Quick Commands; without a writable Pane
 * the row is disabled. Pointer-down is cancelled so the xterm textarea keeps
 * keyboard focus and the soft keyboard stays open.
 */
const { t } = useI18n();
const tips = useTipsStore();

interface KeyButton { key: ExtraKey; text?: string; icon?: Component; labelKey: string }
const keys: KeyButton[] = [
  { key: "escape", text: "Esc", labelKey: "escape" },
  { key: "tab", text: "Tab", labelKey: "tab" },
  { key: "arrowLeft", icon: ArrowLeft, labelKey: "arrowLeft" },
  { key: "arrowUp", icon: ArrowUp, labelKey: "arrowUp" },
  { key: "arrowDown", icon: ArrowDown, labelKey: "arrowDown" },
  { key: "arrowRight", icon: ArrowRight, labelKey: "arrowRight" },
  { key: "home", text: "Home", labelKey: "home" },
  { key: "end", text: "End", labelKey: "end" },
  { key: "pageUp", text: "PgUp", labelKey: "pageUp" },
  { key: "pageDown", text: "PgDn", labelKey: "pageDown" },
  { key: "pipe", text: "|", labelKey: "pipe" },
  { key: "tilde", text: "~", labelKey: "tilde" },
  { key: "slash", text: "/", labelKey: "slash" },
  { key: "dash", text: "-", labelKey: "dash" },
];
const enabled = computed(() => canSendKeysToFocusedTerminal.value);
let sending = false;

async function send(key: ExtraKey) {
  if (sending || !enabled.value) return;
  sending = true;
  try {
    const result = await sendKeysToFocusedTerminal(extraKeySequence(key, focusedTerminalUsesApplicationCursorKeys()));
    if (result !== "sent") {
      tips.show({ scope: "terminal-extra-keys", tone: "error",
        title: t(result === "unavailable" ? "mobile.extraKeys.unavailable" : "mobile.extraKeys.sendFailed") });
    }
  } finally {
    sending = false;
  }
}
</script>

<template>
  <div
    class="nvx-terminal-extra-keys"
    role="toolbar"
    :aria-label="t('mobile.extraKeys.label')"
  >
    <div class="nvx-terminal-extra-keys__scroller">
      <button
        class="nvx-terminal-extra-keys__key nvx-terminal-extra-keys__key--modifier"
        type="button"
        :aria-pressed="stickyCtrl"
        :aria-label="t('mobile.extraKeys.ctrlHint')"
        :disabled="!enabled"
        @pointerdown.prevent
        @click="toggleStickyCtrl"
      >
        {{ t("mobile.extraKeys.ctrl") }}
      </button>
      <button
        class="nvx-terminal-extra-keys__key nvx-terminal-extra-keys__key--modifier"
        type="button"
        :aria-pressed="stickyAlt"
        :aria-label="t('mobile.extraKeys.altHint')"
        :disabled="!enabled"
        @pointerdown.prevent
        @click="toggleStickyAlt"
      >
        {{ t("mobile.extraKeys.alt") }}
      </button>
      <button
        v-for="item in keys"
        :key="item.key"
        class="nvx-terminal-extra-keys__key"
        type="button"
        :aria-label="t(`mobile.extraKeys.${item.labelKey}`)"
        :disabled="!enabled"
        @pointerdown.prevent
        @click="send(item.key)"
      >
        <NvxIcon
          v-if="item.icon"
          :icon="item.icon"
          :size="16"
        />
        <template v-else>
          {{ item.text }}
        </template>
      </button>
    </div>
    <div
      v-if="$slots.trailing"
      class="nvx-terminal-extra-keys__trailing"
    >
      <slot name="trailing" />
    </div>
  </div>
</template>

<style scoped>
.nvx-terminal-extra-keys {
  display: flex;
  flex: none;
  align-items: center;
  height: 48px;
  background: var(--nvx-color-bg-subtle);
}

/* Keys scroll; the trailing entry (Quick Commands) stays pinned at the right end. */
.nvx-terminal-extra-keys__scroller {
  display: flex;
  flex: 1 1 auto;
  gap: var(--nvx-space-1);
  min-width: 0;
  height: 100%;
  padding: var(--nvx-space-1) var(--nvx-space-2);
  overflow-x: auto;
  overflow-y: hidden;
  scrollbar-width: none;
  -webkit-overflow-scrolling: touch;
  overscroll-behavior-x: contain;
}

.nvx-terminal-extra-keys__scroller::-webkit-scrollbar { display: none; }

.nvx-terminal-extra-keys__trailing {
  display: flex;
  flex: none;
  align-items: center;
  height: 100%;
  padding: var(--nvx-space-1) var(--nvx-space-2) var(--nvx-space-1) var(--nvx-space-1);
  box-shadow: -8px 0 8px -6px rgb(16 24 40 / 12%);
}

.nvx-terminal-extra-keys__key {
  display: inline-flex;
  flex: none;
  align-items: center;
  justify-content: center;
  min-width: 44px;
  height: 40px;
  padding: 0 var(--nvx-space-2);
  border: var(--nvx-border-width) solid var(--nvx-color-border);
  border-radius: var(--nvx-radius-md);
  background: var(--nvx-color-bg-surface);
  color: var(--nvx-color-text-primary);
  font-family: var(--nvx-font-mono);
  font-size: var(--nvx-font-size-sm);
  cursor: pointer;
  touch-action: manipulation;
  -webkit-tap-highlight-color: transparent;
}

.nvx-terminal-extra-keys__key:disabled {
  color: var(--nvx-color-text-tertiary);
  cursor: default;
}

.nvx-terminal-extra-keys__key--modifier[aria-pressed="true"] {
  border-color: var(--nvx-color-accent);
  background: var(--nvx-color-accent);
  color: var(--nvx-color-on-accent);
}

.nvx-terminal-extra-keys__key:focus-visible {
  outline: var(--nvx-focus-ring-width) solid var(--nvx-color-focus-ring);
  outline-offset: 1px;
}
</style>
