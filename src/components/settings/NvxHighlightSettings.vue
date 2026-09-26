<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { NvxButton, NvxCheckbox, NvxDialog, NvxField, NvxInlineNotice, NvxInput, NvxSelect } from "../ui";
import { useTerminalPreferencesStore } from "../../stores/terminalPreferences";
import { applicationPreferenceFailure } from "../../core-api/application-preferences";
import { useTipsStore } from "../../stores/tips";
import { DEFAULT_HIGHLIGHT_RULES, MAX_HIGHLIGHT_PATTERN, MAX_HIGHLIGHT_RULES, validateHighlightRule, type HighlightConfiguration, type HighlightMatch, type HighlightRule, type HostHighlightConfiguration } from "../../terminal/highlighting";

const props = defineProps<{ hosts: readonly { hostId: string; label: string }[] }>();
const { t } = useI18n();
const store = useTerminalPreferencesStore();
const tips = useTipsStore();
const scope = ref("");
const mode = ref<HostHighlightConfiguration["mode"]>("custom");
const draft = ref<HighlightConfiguration>({ enabled: false, rules: [] });
const baseline = ref("");
const ruleDraft = ref<HighlightRule | null>(null);
const editingId = ref<string | null>(null);
const scopeOptions = computed(() => [{ value: "", label: t("highlighting.global") }, ...props.hosts.map(host => ({ value: host.hostId, label: host.label }))]);
const modeOptions = computed(() => ["inherit", "custom", "disabled"].map(value => ({ value, label: t(`highlighting.${value}`) })));
const matchOptions = computed(() => ["literal", "regex"].map(value => ({ value, label: t(`highlighting.${value}`) })));
const editable = computed(() => !scope.value || mode.value === "custom");
const shown = computed(() => scope.value && mode.value === "inherit" ? store.preferences.highlights : draft.value);
const dirty = computed(() => JSON.stringify({ mode: mode.value, config: draft.value }) !== baseline.value);
const ruleValid = computed(() => ruleDraft.value !== null && validateHighlightRule(ruleDraft.value));
function copy(config: HighlightConfiguration): HighlightConfiguration { return { enabled: config.enabled, rules: config.rules.map(rule => ({ ...rule })) }; }
function reset() {
  const host = scope.value ? store.preferences.hostHighlights[scope.value] : undefined;
  mode.value = scope.value ? host?.mode ?? "inherit" : "custom";
  draft.value = copy(host ?? store.preferences.highlights);
  baseline.value = JSON.stringify({ mode: mode.value, config: draft.value });
  closeRule();
}
watch(scope, reset, { immediate: true });
watch(() => props.hosts, hosts => {
  if (scope.value && !hosts.some(host => host.hostId === scope.value)) scope.value = "";
}, { deep: true });
async function save() {
  // Pass a plain object so structuredClone never receives a Vue Proxy from the persistence layer.
  try {
    const saved = await store.setHighlights(copy(draft.value), scope.value || undefined, mode.value);
    tips.show({ scope: "highlight-settings", tone: saved ? "success" : "error", title: t(saved ? "highlighting.saved" : "highlighting.saveFailed") });
    if (saved) reset();
  } catch (error) {
    tips.show({ scope: "highlight-settings", tone: "error", title: t(`applicationPreferenceErrors.${applicationPreferenceFailure(error)}`) });
  }
}
function restore() { draft.value.rules = DEFAULT_HIGHLIGHT_RULES.map(rule => ({ ...rule })); }
function edit(rule?: HighlightRule) {
  editingId.value = rule?.id ?? null;
  ruleDraft.value = rule ? { ...rule } : { id: crypto.randomUUID(), label: "", pattern: "", mode: "literal", caseSensitive: false, foreground: "#FFFFFF", background: "#A82D39", enabled: true };
}
function closeRule() { ruleDraft.value = null; }
function keepRule() {
  if (!ruleDraft.value || !ruleValid.value) return;
  const rule = { ...ruleDraft.value };
  const index = draft.value.rules.findIndex(item => item.id === editingId.value);
  if (index >= 0) draft.value.rules.splice(index, 1, rule);
  else if (draft.value.rules.length < MAX_HIGHLIGHT_RULES) draft.value.rules.push(rule);
  closeRule();
}
function remove(id: string) { draft.value.rules = draft.value.rules.filter(rule => rule.id !== id); }
function toggle(id: string, enabled: boolean) {
  const rule = draft.value.rules.find(item => item.id === id);
  if (rule) rule.enabled = enabled;
}
function luminance(hex: string) {
  const channels = [1, 3, 5].map(offset => parseInt(hex.slice(offset, offset + 2), 16) / 255).map(value => value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4);
  return channels[0]! * 0.2126 + channels[1]! * 0.7152 + channels[2]! * 0.0722;
}
const contrast = computed(() => {
  const rule = ruleDraft.value;
  if (!rule || !/^#[0-9a-f]{6}$/i.test(rule.foreground) || !/^#[0-9a-f]{6}$/i.test(rule.background)) return null;
  const values = [luminance(rule.foreground), luminance(rule.background)].sort((a, b) => b - a);
  return (values[0]! + 0.05) / (values[1]! + 0.05);
});
const sampleLines = ["2026-09-09 ERROR 连接失败 / Connection failed", "WARN 磁盘空间不足 / Disk space low", "SUCCESS 操作成功 / Operation succeeded", "INFO 服务运行正常 / Service ready", "DEBUG 调试记录 / Diagnostic details", "TRACE 请求跟踪 / Request trace"];
const previewState = ref<"idle" | "running" | "done" | "unavailable" | "timeout" | "failed">("idle");
const matches = ref<HighlightMatch[]>([]);
let worker: Worker | null = null;
let deadline: ReturnType<typeof setTimeout> | null = null;
let revision = 0;
function stopPreview() {
  revision += 1;
  worker?.terminate();
  worker = null;
  if (deadline !== null) clearTimeout(deadline);
  deadline = null;
}
function invalidatePreview() { stopPreview(); matches.value = []; previewState.value = "idle"; }
watch(ruleDraft, invalidatePreview, { deep: true, flush: "sync" });
onBeforeUnmount(stopPreview);
function preview() {
  if (!ruleDraft.value || !ruleValid.value) return;
  invalidatePreview();
  if (typeof Worker === "undefined") { previewState.value = "unavailable"; return; }
  const requestId = revision;
  try {
    const currentWorker = new Worker(new URL("../../terminal/highlight.worker.ts", import.meta.url), { type: "module" });
    worker = currentWorker;
    previewState.value = "running";
    deadline = setTimeout(() => {
      if (revision !== requestId) return;
      stopPreview();
      previewState.value = "timeout";
    }, 500);
    currentWorker.onmessage = (event: MessageEvent<{ requestId: number; matches: HighlightMatch[]; failed?: boolean }>) => {
      if (revision !== requestId || event.data.requestId !== requestId) return;
      const result = event.data;
      stopPreview();
      if (result.failed || !Array.isArray(result.matches)) { previewState.value = "failed"; return; }
      matches.value = result.matches.slice(0, 1_000).filter(match => Number.isInteger(match.line) && Number.isInteger(match.start) && Number.isInteger(match.end) && match.line >= 0 && match.line < sampleLines.length && match.start >= 0 && match.end > match.start && match.end <= sampleLines[match.line]!.length && match.ruleId === ruleDraft.value?.id);
      previewState.value = "done";
    };
    currentWorker.onerror = () => {
      if (revision !== requestId) return;
      stopPreview();
      previewState.value = "unavailable";
    };
    currentWorker.postMessage({ requestId, lines: [...sampleLines], rules: [{ ...ruleDraft.value, enabled: true }] });
  } catch { stopPreview(); previewState.value = "unavailable"; }
}
const previewMessage = computed(() => t(`highlighting.preview${previewState.value[0]!.toUpperCase()}${previewState.value.slice(1)}`, { count: matches.value.length }));
const previewLines = computed(() => sampleLines.map((line, lineIndex) => {
  const segments: { text: string; highlighted: boolean }[] = [];
  let position = 0;
  const ordered = matches.value.filter(match => match.line === lineIndex).sort((a, b) => a.start - b.start);
  for (const match of ordered) {
    if (match.start < position) continue;
    if (match.start > position) segments.push({ text: line.slice(position, match.start), highlighted: false });
    segments.push({ text: line.slice(match.start, match.end), highlighted: true });
    position = match.end;
  }
  if (position < line.length) segments.push({ text: line.slice(position), highlighted: false });
  return segments;
}));
</script>

<template>
  <section
    class="nvx-highlight-settings"
    :aria-label="t('highlighting.title')"
  >
    <p class="nvx-highlight-settings__hint">
      {{ t('highlighting.description') }}
    </p>
    <div class="nvx-highlight-settings__fields">
      <NvxField
        :label="t('highlighting.scope')"
        :hint="dirty ? t('highlighting.unsaved') : undefined"
      >
        <NvxSelect
          v-model="scope"
          :options="scopeOptions"
          :aria-label="t('highlighting.scope')"
          :disabled="dirty"
        />
      </NvxField>
      <NvxField
        v-if="scope"
        :label="t('highlighting.hostMode')"
      >
        <NvxSelect
          v-model="mode"
          :options="modeOptions"
          :aria-label="t('highlighting.hostMode')"
        />
      </NvxField>
    </div>
    <p
      v-if="scope && mode !== 'custom'"
      class="nvx-highlight-settings__hint"
    >
      {{ t(mode === 'inherit' ? 'highlighting.inheritHint' : 'highlighting.disabledHint') }}
    </p>
    <NvxCheckbox
      v-if="editable"
      v-model="draft.enabled"
    >
      {{ t('highlighting.enabled') }}
    </NvxCheckbox>
    <div class="nvx-highlight-settings__toolbar">
      <strong>{{ t('highlighting.rules') }} <small>{{ t('highlighting.ruleCount', { count: shown.rules.length, max: MAX_HIGHLIGHT_RULES }) }}</small></strong>
      <div class="nvx-highlight-settings__actions">
        <NvxButton
          variant="ghost"
          :disabled="!editable"
          @click="restore"
        >
          {{ t('highlighting.restore') }}
        </NvxButton>
        <NvxButton
          variant="ghost"
          :disabled="!editable || draft.rules.length >= MAX_HIGHLIGHT_RULES"
          @click="edit()"
        >
          {{ t('highlighting.add') }}
        </NvxButton>
      </div>
    </div>
    <p
      v-if="!shown.rules.length"
      class="nvx-highlight-settings__hint"
    >
      {{ t('highlighting.empty') }}
    </p>
    <ul
      v-else
      class="nvx-highlight-settings__rules"
    >
      <li
        v-for="rule in shown.rules"
        :key="rule.id"
      >
        <NvxCheckbox
          :model-value="rule.enabled"
          :disabled="!editable"
          @update:model-value="toggle(rule.id, $event)"
        >
          {{ rule.label }}
        </NvxCheckbox>
        <code
          class="nvx-highlight-settings__pattern"
          :title="rule.pattern"
        >{{ rule.pattern }}</code>
        <span
          class="nvx-highlight-settings__sample"
          :style="{ color: rule.foreground, backgroundColor: rule.background }"
        >Aa</span>
        <div class="nvx-highlight-settings__actions">
          <NvxButton
            variant="ghost"
            :disabled="!editable"
            @click="edit(rule)"
          >
            {{ t('highlighting.edit') }}
          </NvxButton>
          <NvxButton
            variant="ghost"
            :disabled="!editable"
            @click="remove(rule.id)"
          >
            {{ t('highlighting.remove') }}
          </NvxButton>
        </div>
      </li>
    </ul>
    <p class="nvx-highlight-settings__hint">
      {{ t('highlighting.orderHint') }}
    </p>
    <div class="nvx-highlight-settings__actions nvx-highlight-settings__footer">
      <NvxButton
        variant="ghost"
        :disabled="!dirty"
        @click="reset"
      >
        {{ t('highlighting.cancel') }}
      </NvxButton>
      <NvxButton
        :disabled="!dirty"
        @click="save"
      >
        {{ t('highlighting.save') }}
      </NvxButton>
    </div>
    <NvxDialog
      :model-value="ruleDraft !== null"
      :title="t(editingId ? 'highlighting.editRule' : 'highlighting.newRule')"
      :close-label="t('highlighting.close')"
      size="lg"
      @update:model-value="closeRule"
    >
      <div
        v-if="ruleDraft"
        class="nvx-highlight-settings__editor"
      >
        <div class="nvx-highlight-settings__fields">
          <NvxField :label="t('highlighting.name')">
            <NvxInput
              v-model="ruleDraft.label"
              :maxlength="48"
              :aria-label="t('highlighting.name')"
            />
          </NvxField>
          <NvxField :label="t('highlighting.mode')">
            <NvxSelect
              v-model="ruleDraft.mode"
              :options="matchOptions"
              :aria-label="t('highlighting.mode')"
            />
          </NvxField>
        </div>
        <NvxField :label="t('highlighting.pattern')">
          <NvxInput
            v-model="ruleDraft.pattern"
            :maxlength="MAX_HIGHLIGHT_PATTERN"
            :aria-label="t('highlighting.pattern')"
          />
        </NvxField>
        <NvxCheckbox v-model="ruleDraft.caseSensitive">
          {{ t('highlighting.caseSensitive') }}
        </NvxCheckbox>
        <div class="nvx-highlight-settings__fields">
          <NvxField
            :label="t('highlighting.foreground')"
            :hint="t('highlighting.colorHint')"
          >
            <NvxInput
              v-model="ruleDraft.foreground"
              :aria-label="t('highlighting.foreground')"
            />
          </NvxField>
          <NvxField
            :label="t('highlighting.background')"
            :hint="t('highlighting.colorHint')"
          >
            <NvxInput
              v-model="ruleDraft.background"
              :aria-label="t('highlighting.background')"
            />
          </NvxField>
        </div>
        <NvxInlineNotice
          v-if="!ruleValid"
          tone="warning"
          :title="t('highlighting.invalid')"
        />
        <NvxInlineNotice
          v-if="contrast !== null && contrast < 4.5"
          tone="warning"
          :title="t('highlighting.contrast', { ratio: contrast.toFixed(2) })"
        />
        <div class="nvx-highlight-settings__toolbar">
          <NvxButton
            variant="ghost"
            :disabled="!ruleValid || previewState === 'running'"
            @click="preview"
          >
            {{ t('highlighting.preview') }}
          </NvxButton>
          <span
            role="status"
            class="nvx-highlight-settings__hint"
          >{{ previewMessage }}</span>
        </div>
        <pre class="nvx-highlight-settings__preview"><span
v-for="(line, index) in previewLines"
                                                           :key="index"
class="nvx-highlight-settings__line"
><span
v-for="(segment, part) in line"
                                                                                                                   :key="part"
:style="segment.highlighted ? { color: ruleDraft.foreground, backgroundColor: ruleDraft.background } : undefined"
        >{{ segment.text }}</span></span></pre>
        <p class="nvx-highlight-settings__hint">
          {{ t('highlighting.previewHint') }}
        </p>
      </div>
      <template #actions>
        <NvxButton
          variant="ghost"
          @click="closeRule"
        >
          {{ t('highlighting.cancelRule') }}
        </NvxButton>
        <NvxButton
          :disabled="!ruleValid"
          @click="keepRule"
        >
          {{ t('highlighting.applyRule') }}
        </NvxButton>
      </template>
    </NvxDialog>
  </section>
</template>

<style scoped>
.nvx-highlight-settings, .nvx-highlight-settings__editor { display: grid; gap: var(--nvx-space-3); min-width: 0; }
.nvx-highlight-settings { container-type: inline-size; }
.nvx-highlight-settings__fields { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: var(--nvx-space-3); }
.nvx-highlight-settings__hint { margin: 0; color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-xs); }
.nvx-highlight-settings__toolbar, .nvx-highlight-settings__actions { display: flex; flex-wrap: wrap; gap: var(--nvx-space-2); align-items: center; }
.nvx-highlight-settings__toolbar { justify-content: space-between; }
.nvx-highlight-settings__toolbar small { color: var(--nvx-color-text-secondary); font-weight: normal; }
.nvx-highlight-settings__rules { padding: 0; margin: 0; list-style: none; }
.nvx-highlight-settings__rules li { display: grid; grid-template-columns: minmax(100px, 1fr) minmax(60px, 1fr) auto auto; align-items: center; gap: var(--nvx-space-2); padding: var(--nvx-space-1) 0; border-bottom: var(--nvx-border-width) solid var(--nvx-color-border); }
.nvx-highlight-settings__rules :deep(.nvx-checkbox__content) { min-width: 0; overflow-wrap: anywhere; }
.nvx-highlight-settings__pattern { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: var(--nvx-font-size-xs); }
.nvx-highlight-settings__sample { padding: 2px var(--nvx-space-1); border-radius: var(--nvx-radius-sm); font-family: monospace; }
.nvx-highlight-settings__footer { justify-content: flex-end; }
.nvx-highlight-settings__preview { overflow: auto; padding: var(--nvx-space-3); margin: 0; background: var(--nvx-color-bg-subtle); color: var(--nvx-color-text-primary); border: var(--nvx-border-width) solid var(--nvx-color-border); font-size: var(--nvx-font-size-xs); }
.nvx-highlight-settings__line { display: block; min-height: 1.5em; }
@container (max-width: 520px) { .nvx-highlight-settings__fields { grid-template-columns: minmax(0, 1fr); } .nvx-highlight-settings__rules li { grid-template-columns: minmax(0, 1fr) auto; } .nvx-highlight-settings__pattern { grid-row: 2; } }
@media (max-width: 560px) { .nvx-highlight-settings__fields { grid-template-columns: minmax(0, 1fr); } }
</style>
