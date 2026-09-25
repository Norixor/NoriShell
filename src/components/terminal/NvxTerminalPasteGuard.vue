<script setup lang="ts">
import { computed, onBeforeUnmount, ref, shallowRef } from "vue";
import { useI18n } from "vue-i18n";

import { useTerminalPreferencesStore } from "../../stores/terminalPreferences";
import { useTipsStore } from "../../stores/tips";
import { captureTerminalInput, type TerminalInputTicket } from "../../terminal-input-target";
import { analyzeTerminalPaste, encodeTerminalPaste, type PasteAnalysis } from "../../terminal/paste";
import { readText as readClipboardText } from "../../platform-clipboard";
import { NvxButton, NvxDialog, NvxInlineNotice } from "../ui";

const props = defineProps<{ paneId: string; bracketed: () => boolean }>();
const emit = defineEmits<{ focus: []; pasted: [] }>();
const { t } = useI18n();
const preferences = useTerminalPreferencesStore();
const tips = useTipsStore();
const pending = ref<PasteAnalysis | null>(null);
const ticket = shallowRef<TerminalInputTicket | null>(null);
const busy = ref(false);
const error = ref("");
const targetLabel = ref("");
let requestRevision = 0;
let disposed = false;
const open = computed(() => pending.value !== null);

function failure(key: string) {
  tips.show({ scope: `paste:${props.paneId}`, tone: "error", title: t(key) });
}

async function cancel() {
  requestRevision++;
  const current = ticket.value;
  pending.value = null;
  ticket.value = null;
  error.value = "";
  if (current) await current.cancel();
  if (!disposed) emit("focus");
}

async function commit(text: string, current: TerminalInputTicket) {
  const value = encodeTerminalPaste(text, props.bracketed());
  if (value === null) {
    error.value = "terminalEnhancements.paste.bracketedControl";
    failure(error.value);
    await current.cancel();
    return false;
  }
  const result = await current.send(value);
  if (result !== "sent") {
    failure(result === "unavailable" ? "terminalEnhancements.paste.targetChanged" : "terminalEnhancements.paste.failed");
    return false;
  }
  emit("pasted");
  return true;
}

async function accept() {
  const current = ticket.value;
  const value = pending.value;
  if (busy.value || !current || !value || value.tooLarge) return;
  busy.value = true;
  await commit(value.text, current);
  pending.value = null;
  ticket.value = null;
  busy.value = false;
  emit("focus");
}

async function prepare(text: string, current: TerminalInputTicket, revision: number) {
  if (disposed || revision !== requestRevision || !current.valid()) {
    await current.cancel();
    if (!disposed) failure("terminalEnhancements.paste.targetChanged");
    return;
  }
  const analysis = analyzeTerminalPaste(text, preferences.preferences.pasteWarning);
  if (!text) { await current.cancel(); return; }
  if (analysis.tooLarge) {
    await current.cancel();
    failure("terminalEnhancements.paste.tooLarge");
    return;
  }
  if (!analysis.requiresConfirmation) {
    await commit(text, current);
    return;
  }
  if (!await current.suspend() || disposed || revision !== requestRevision) {
    await current.cancel();
    if (!disposed) failure("terminalEnhancements.paste.targetChanged");
    return;
  }
  ticket.value = current;
  targetLabel.value = current.label;
  error.value = "";
  pending.value = analysis;
}

async function requestPaste(text: string) {
  if (pending.value || busy.value) return;
  const revision = ++requestRevision;
  const current = captureTerminalInput(props.paneId);
  if (!current) { failure("terminalEnhancements.paste.targetChanged"); return; }
  await prepare(text, current, revision);
}

async function pasteFromClipboard() {
  if (pending.value || busy.value) return;
  const revision = ++requestRevision;
  const current = captureTerminalInput(props.paneId);
  if (!current) { failure("terminalEnhancements.paste.targetChanged"); return; }
  try { await prepare(await readClipboardText(), current, revision); }
  catch {
    await current.cancel();
    failure("terminalEnhancements.paste.clipboardFailed");
  }
}

onBeforeUnmount(() => {
  disposed = true;
  requestRevision++;
  // Unmounting never restores input focus to a Pane that no longer exists.
  pending.value = null;
  ticket.value = null;
});
defineExpose({ requestPaste, pasteFromClipboard });
</script>

<template>
  <NvxDialog
    :model-value="open"
    :title="t('terminalEnhancements.paste.title')"
    :description="t('terminalEnhancements.paste.description', { target: targetLabel })"
    :close-label="t('terminalEnhancements.paste.cancel')"
    :dismissible="!busy"
    plugin-protected
    size="lg"
    @update:model-value="(value) => { if (!value) void cancel(); }"
  >
    <div
      v-if="pending"
      class="paste-review"
    >
      <div class="paste-review__summary">
        <span>{{ t('terminalEnhancements.paste.lines', { count: pending.lineCount }) }}</span>
        <span v-if="pending.hasTrailingNewline">{{ t('terminalEnhancements.paste.trailingNewline') }}</span>
      </div>
      <NvxInlineNotice
        v-if="pending.hasControls"
        tone="warning"
      >
        {{ t('terminalEnhancements.paste.controls') }}
      </NvxInlineNotice>
      <NvxInlineNotice
        v-if="error"
        tone="error"
      >
        {{ t(error) }}
      </NvxInlineNotice>
      <pre
        class="paste-review__preview"
        tabindex="0"
        :aria-label="t('terminalEnhancements.paste.preview')"
      >{{ pending.preview }}</pre>
      <p>{{ t('terminalEnhancements.paste.hint') }}</p>
    </div>
    <template #actions>
      <NvxButton
        variant="ghost"
        :disabled="busy"
        @click="cancel"
      >
        {{ t('terminalEnhancements.paste.cancel') }}
      </NvxButton>
      <NvxButton
        :loading="busy"
        :loading-label="t('terminalEnhancements.paste.pasting')"
        data-nvx-dialog-initial-focus
        @click="accept"
      >
        {{ t('terminalEnhancements.paste.confirm') }}
      </NvxButton>
    </template>
  </NvxDialog>
</template>

<style scoped>
.paste-review { display: grid; gap: var(--nvx-space-3); min-width: 0; }
.paste-review__summary { display: flex; flex-wrap: wrap; gap: var(--nvx-space-3); font-size: var(--nvx-font-size-sm); color: var(--nvx-color-text-secondary); }
.paste-review__preview { margin: 0; padding: var(--nvx-space-3); max-height: min(360px, 45vh); overflow: auto; white-space: pre-wrap; overflow-wrap: anywhere; border: 1px solid var(--nvx-color-border-subtle); border-radius: var(--nvx-radius-sm); background: var(--nvx-color-bg-canvas); color: var(--nvx-color-text-primary); font: 12px/1.6 var(--nvx-font-family-mono); }
.paste-review p { margin: 0; color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-sm); }
</style>
