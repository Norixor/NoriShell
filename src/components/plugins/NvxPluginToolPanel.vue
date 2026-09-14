<script setup lang="ts">
import { ChevronDown, GripVertical, Plug, Search, X } from "lucide-vue-next";
import { computed, nextTick, onBeforeUnmount, provide, ref, watch } from "vue";
import { useI18n } from "vue-i18n";

import type { PluginUiContribution } from "../../core-api/generated/core-api";
import { NvxIcon, NvxIconButton, NvxInlineNotice, NvxInput } from "../ui";
import NvxPluginExtensionTarget from "./NvxPluginExtensionTarget.vue";
import { pluginIconMap } from "./pluginIcons";
import { activePluginPanel } from "./pluginPanelOwnership";

type Choice = Pick<PluginUiContribution, "pluginId" | "pluginName" | "icon">;
// Density belongs to this host surface, not to plugin identity or its protocol.
provide("nvx-plugin-tool-panel-compact", true);
const props = withDefaults(defineProps<{
  modelValue: boolean;
  targetId: string;
  instanceKey: string;
  contextLabel: string;
  available?: boolean;
  floating?: boolean;
  routePath?: string;
  initialPluginId?: string | null;
}>(), { available: true, floating: false, routePath: "/terminal", initialPluginId: null });
const emit = defineEmits<{
  "update:modelValue": [open: boolean];
  catalog: [items: Choice[]];
}>();
const { t, locale } = useI18n();
const panel = ref<HTMLElement | null>(null);
const picker = ref<HTMLElement | null>(null);
const pickerOpen = ref(false);
const query = ref("");
const choices = ref<Choice[]>([]);
const selectedId = ref<string | null>(props.initialPluginId);
const panelId = crypto.randomUUID();
const selected = computed(() => choices.value.find((item) => item.pluginId === selectedId.value) ?? null);
const filtered = computed(() => choices.value.filter((item) => (
  `${item.pluginName} ${item.pluginId}`.toLocaleLowerCase(locale.value).includes(query.value.trim().toLocaleLowerCase(locale.value))
)));
const widthKey = "norishell.pluginTools.width.v1";
const width = ref(480);
try {
  const saved = Number(localStorage.getItem(widthKey));
  if (Number.isFinite(saved) && saved >= 360 && saved <= 720) width.value = saved;
} catch { /* UI preferences are optional. */ }
let restoreFocus: HTMLElement | null = null;
let resizing: { pointerId: number; startX: number; startWidth: number; target: HTMLElement } | null = null;

function setCatalog(items: Choice[]) {
  choices.value = [...items].sort((a, b) => a.pluginName.localeCompare(b.pluginName, locale.value));
  if (items.length && !items.some((item) => item.pluginId === selectedId.value)) selectedId.value = items[0]!.pluginId;
  emit("catalog", choices.value);
}
function select(item: Choice) {
  selectedId.value = item.pluginId;
  pickerOpen.value = false;
  query.value = "";
  void nextTick(() => panel.value?.querySelector<HTMLElement>("[data-plugin-picker-trigger]")?.focus());
}
function close() {
  pickerOpen.value = false;
  emit("update:modelValue", false);
}
function persistWidth() {
  try { localStorage.setItem(widthKey, String(width.value)); } catch { /* UI preferences are optional. */ }
}
function beginResize(event: PointerEvent) {
  if (event.button !== 0 || props.floating) return;
  const target = event.currentTarget as HTMLElement;
  target.setPointerCapture(event.pointerId);
  resizing = { pointerId: event.pointerId, startX: event.clientX, startWidth: width.value, target };
  event.preventDefault();
}
function moveResize(event: PointerEvent) {
  if (!resizing || resizing.pointerId !== event.pointerId) return;
  width.value = Math.max(360, Math.min(720, window.innerWidth - 320, resizing.startWidth + resizing.startX - event.clientX));
}
function endResize(event: PointerEvent) {
  if (resizing?.pointerId !== event.pointerId) return;
  if (resizing.target.hasPointerCapture(event.pointerId)) resizing.target.releasePointerCapture(event.pointerId);
  resizing = null;
  persistWidth();
}
function resizeByKey(event: KeyboardEvent) {
  if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
  event.preventDefault();
  width.value = event.key === "Home" ? 360 : event.key === "End" ? 720
    : Math.max(360, Math.min(720, width.value + (event.key === "ArrowLeft" ? 24 : -24)));
  persistWidth();
}
function keydown(event: KeyboardEvent) {
  if (event.key === "Escape") {
    event.preventDefault();
    if (pickerOpen.value) pickerOpen.value = false;
    else close();
  }
  if (event.key !== "Tab" || (!props.floating && window.innerWidth > 1100)) return;
  const elements = Array.from(panel.value?.querySelectorAll<HTMLElement>("button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex='0']") ?? [])
    .filter((item) => item.getClientRects().length);
  const first = elements[0]; const last = elements.at(-1);
  if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
  else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
}
watch(() => props.initialPluginId, (value) => { if (value) selectedId.value = value; });
watch(() => props.modelValue, (open) => {
  if (open) {
    activePluginPanel.value = panelId;
    restoreFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    void nextTick(() => panel.value?.querySelector<HTMLElement>("[data-plugin-picker-trigger]")?.focus());
  } else {
    if (activePluginPanel.value === panelId) activePluginPanel.value = null;
    pickerOpen.value = false;
    if (restoreFocus?.isConnected) restoreFocus.focus();
  }
}, { immediate: true });
watch(activePluginPanel, (owner) => { if (props.modelValue && owner !== panelId) emit("update:modelValue", false); });
watch(pickerOpen, (open) => {
  if (open) void nextTick(() => picker.value?.querySelector<HTMLInputElement>("input")?.focus());
});
onBeforeUnmount(() => { resizing = null; if (activePluginPanel.value === panelId) activePluginPanel.value = null; });
</script>

<template>
  <div
    v-if="modelValue"
    class="plugin-tool-panel__scrim"
    :class="{ 'is-floating': floating }"
    @click="close"
  />
  <aside
    v-show="modelValue"
    ref="panel"
    class="plugin-tool-panel"
    :class="{ 'is-floating': floating }"
    :style="{ '--plugin-panel-width': `${width}px` }"
    :aria-label="t('plugins.tools.title')"
    @keydown="keydown"
  >
    <div
      v-if="modelValue && !floating"
      class="plugin-tool-panel__resize"
      role="separator"
      aria-orientation="vertical"
      :aria-label="t('plugins.tools.resize')"
      :aria-valuenow="width"
      :aria-valuemin="360"
      :aria-valuemax="720"
      tabindex="0"
      @pointerdown="beginResize"
      @pointermove="moveResize"
      @pointerup="endResize"
      @pointercancel="endResize"
      @keydown="resizeByKey"
    >
      <NvxIcon
        :icon="GripVertical"
        :size="16"
      />
    </div>
    <header
      class="plugin-tool-panel__header"
      data-plugin-protected
    >
      <div><strong>{{ t('plugins.tools.title') }}</strong><span :title="contextLabel">{{ contextLabel }}</span></div>
      <NvxIconButton
        :label="t('plugins.tools.close')"
        size="sm"
        @click="close"
      >
        <NvxIcon
          :icon="X"
          :size="16"
        />
      </NvxIconButton>
    </header>
    <div
      class="plugin-tool-panel__picker"
      data-plugin-protected
    >
      <button
        data-plugin-picker-trigger
        type="button"
        :aria-expanded="pickerOpen"
        aria-haspopup="listbox"
        @click="pickerOpen = !pickerOpen"
      >
        <NvxIcon
          :icon="pluginIconMap[selected?.icon ?? ''] ?? Plug"
          :size="16"
        />
        <span>{{ selected?.pluginName ?? t('plugins.tools.choose') }}</span>
        <small>{{ choices.length }}</small><NvxIcon
          :icon="ChevronDown"
          :size="16"
        />
      </button>
      <div
        v-if="pickerOpen"
        ref="picker"
        class="plugin-tool-panel__choices"
      >
        <div class="plugin-tool-panel__search">
          <NvxIcon
            :icon="Search"
            :size="16"
          /><NvxInput
            v-model="query"
            :aria-label="t('plugins.tools.search')"
            :placeholder="t('plugins.tools.search')"
          />
        </div>
        <div
          role="listbox"
          :aria-label="t('plugins.tools.choose')"
        >
          <button
            v-for="item in filtered"
            :key="item.pluginId"
            type="button"
            role="option"
            :aria-selected="item.pluginId === selectedId"
            @click="select(item)"
          >
            <NvxIcon
              :icon="pluginIconMap[item.icon ?? ''] ?? Plug"
              :size="16"
            /><span>{{ item.pluginName }}</span>
          </button>
          <p v-if="!filtered.length">
            {{ t(query ? 'plugins.tools.noMatches' : 'plugins.tools.empty') }}
          </p>
        </div>
      </div>
    </div>
    <div class="plugin-tool-panel__body">
      <NvxInlineNotice
        v-if="!available"
        tone="info"
        :title="t('plugins.tools.sessionRequired')"
      >
        {{ t('plugins.tools.sessionRequiredBody') }}
      </NvxInlineNotice>
      <p
        v-else-if="!choices.length"
        class="plugin-tool-panel__empty"
      >
        {{ t('plugins.tools.empty') }}
      </p>
      <NvxPluginExtensionTarget
        :key="`${targetId}:${instanceKey}`"
        :target-id="targetId"
        :instance-key="instanceKey"
        :display-label="contextLabel"
        :route-path="routePath"
        :selected-plugin-id="modelValue && available ? selectedId : null"
        :disabled="!available || !modelValue"
        :show-identity="false"
        @catalog="setCatalog"
      />
    </div>
  </aside>
</template>

<style scoped>
.plugin-tool-panel { position: relative; z-index: var(--nvx-z-popover); display: grid; grid-template-rows: auto auto minmax(0, 1fr); width: min(var(--plugin-panel-width), calc(100vw - 320px)); min-width: 0; height: 100%; min-height: 0; border-left: 1px solid var(--nvx-color-border); background: var(--nvx-color-bg-surface); color: var(--nvx-color-text-primary); }
.plugin-tool-panel__header { display: flex; align-items: center; justify-content: space-between; gap: 8px; padding: 8px 12px 6px; }
.plugin-tool-panel__header > div { display: grid; min-width: 0; gap: 2px; }
.plugin-tool-panel__header strong { font-size: 14px; line-height: 20px; }
.plugin-tool-panel__header span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-xs); }
.plugin-tool-panel__picker { position: relative; margin: 0 12px 6px; }
.plugin-tool-panel__picker > button { display: flex; width: 100%; min-height: 28px; align-items: center; gap: 6px; padding: 3px 8px; border: 1px solid var(--nvx-color-border-strong); border-radius: var(--nvx-radius-md); background: var(--nvx-color-bg-subtle); color: inherit; font: inherit; font-size: 13px; text-align: left; }
.plugin-tool-panel__picker button > span { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; }
.plugin-tool-panel__picker small { color: var(--nvx-color-text-tertiary); }
.plugin-tool-panel__choices { position: absolute; inset: calc(100% + 4px) 0 auto; z-index: 2; padding: 6px; border: 1px solid var(--nvx-color-border-strong); border-radius: var(--nvx-radius-md); background: var(--nvx-color-bg-surface); box-shadow: var(--nvx-shadow-overlay); }
.plugin-tool-panel__search { display: flex; align-items: center; gap: 8px; padding: 2px 4px 8px; }
.plugin-tool-panel__choices [role=listbox] { max-height: min(300px, 45vh); overflow: auto; overscroll-behavior: contain; }
.plugin-tool-panel__choices [role=option] { display: flex; width: 100%; align-items: center; gap: 8px; min-height: 34px; padding: 6px 8px; border: 0; border-radius: var(--nvx-radius-sm); color: inherit; background: transparent; font: inherit; text-align: left; }
.plugin-tool-panel__choices [aria-selected=true], .plugin-tool-panel__choices [role=option]:hover { background: var(--nvx-color-bg-hover); }
.plugin-tool-panel__choices p, .plugin-tool-panel__empty { margin: 8px; color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-sm); }
.plugin-tool-panel__body { display: grid; grid-auto-rows: max-content; align-content: start; min-width: 0; min-height: 0; gap: 6px; padding: 0 12px 10px; overflow: auto; overscroll-behavior: contain; container: plugin-content / inline-size; }
.plugin-tool-panel__resize { position: absolute; z-index: 2; inset: 0 auto 0 -4px; display: flex; width: 8px; align-items: center; justify-content: center; cursor: col-resize; touch-action: none; color: var(--nvx-color-text-tertiary); }
.plugin-tool-panel__resize:hover, .plugin-tool-panel__resize:focus-visible { background: var(--nvx-color-bg-hover); color: var(--nvx-color-accent); }
.plugin-tool-panel__scrim { display: none; }
.plugin-tool-panel.is-floating { position: absolute; inset: 12px 12px 12px auto; width: min(600px, calc(100% - 24px)); height: auto; max-height: calc(100% - 24px); border: 1px solid var(--nvx-color-border-strong); border-radius: var(--nvx-radius-lg); box-shadow: var(--nvx-shadow-overlay); pointer-events: auto; }
.plugin-tool-panel__scrim.is-floating { display: block; position: absolute; inset: 0; z-index: var(--nvx-z-popover); pointer-events: auto; background: rgb(0 0 0 / 12%); }
button:focus-visible { outline: var(--nvx-focus-ring-width) solid var(--nvx-color-focus-ring); outline-offset: 2px; }
@media (max-width: 1100px) {
  .plugin-tool-panel { position: absolute; inset: 0 0 0 auto; width: min(520px, calc(100% - 24px)); height: 100%; box-shadow: var(--nvx-shadow-overlay); }
  .plugin-tool-panel__resize { display: none; }
  .plugin-tool-panel__scrim { display: block; position: absolute; inset: 0; z-index: var(--nvx-z-popover); background: rgb(0 0 0 / 24%); }
}
</style>
