<script setup lang="ts">
import { Eye, EyeOff, MoreHorizontal, Plug, RotateCcw } from "lucide-vue-next";
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import type { PluginUiContribution } from "../../core-api/generated/core-api";
import { NvxButton, NvxIcon, NvxIconButton } from "../ui";
import NvxPluginToolPanel from "./NvxPluginToolPanel.vue";
import { pluginIconMap } from "./pluginIcons";
import { floatingButtonSize, placePluginFloatingButtons, readPluginFloatingPreferences, type PluginFloatingPreference } from "./pluginFloatingLayout";

type Choice = Pick<PluginUiContribution, "pluginId" | "pluginName" | "icon">;
const props = withDefaults(defineProps<{
  targetId: "terminal.floating" | "app.content.floating";
  instanceKey: string;
  contextLabel: string;
  preferenceScope: string;
  available?: boolean;
  routePath: string;
}>(), { available: true });
const { t } = useI18n();
const root = ref<HTMLElement | null>(null);
const choices = ref<Choice[]>([]);
const selectedId = ref<string | null>(null);
const panelOpen = ref(false);
const menuOpen = ref(false);
const size = ref({ width: 1200, height: 800 });
const preferenceKey = computed(() => `norishell.pluginFloating.${props.preferenceScope}.v1`);
const preferences = ref<Record<string, PluginFloatingPreference>>(readPluginFloatingPreferences(preferenceKey.value));
const visibleChoices = computed(() => choices.value.filter((item) => !preferences.value[item.pluginId]?.hidden));
const maximumButtons = computed(() => Math.max(1, Math.min(5, Math.floor((size.value.height - 24) / 52))));
const needsMenuButton = computed(() => choices.value.length > 1 || visibleChoices.value.length !== choices.value.length);
const shown = computed(() => visibleChoices.value.slice(0, Math.max(0, maximumButtons.value - (needsMenuButton.value ? 1 : 0))));
const positions = computed(() => placePluginFloatingButtons(
  [...shown.value.map((item) => item.pluginId), "__menu"],
  props.targetId === "app.content.floating"
    ? { ...Object.fromEntries([...shown.value.map((item) => item.pluginId), "__menu"].map((id) => [id, { x: 0, y: .45, hidden: false }])), ...preferences.value }
    : preferences.value,
  size.value.width, size.value.height,
));
let observer: ResizeObserver | null = null;
let drag: { id: string; pointerId: number; startX: number; startY: number; x: number; y: number; moved: boolean; target: HTMLElement } | null = null;
let suppressClick: string | null = null;
function savePreferences() {
  const entries = Object.entries(preferences.value).filter(([id]) => id === "__menu" || choices.value.some((item) => item.pluginId === id)).slice(0, 64);
  try { localStorage.setItem(preferenceKey.value, JSON.stringify(Object.fromEntries(entries))); } catch { /* Optional UI preference. */ }
}
function open(item: Choice) {
  if (suppressClick === item.pluginId) { suppressClick = null; return; }
  if (!props.available) return;
  selectedId.value = item.pluginId;
  panelOpen.value = true;
  menuOpen.value = false;
}
function toggleMenu() {
  if (suppressClick === "__menu") { suppressClick = null; return; }
  menuOpen.value = !menuOpen.value;
}
function toggleHidden(item: Choice) {
  const previous = preferences.value[item.pluginId] ?? { x: props.targetId === "app.content.floating" ? 0 : 1, y: .45, hidden: false };
  preferences.value = { ...preferences.value, [item.pluginId]: { ...previous, hidden: !previous.hidden } };
  savePreferences();
}
function reset() { preferences.value = {}; savePreferences(); menuOpen.value = false; }
function beginDrag(event: PointerEvent, id: string) {
  if (event.button !== 0) return;
  const target = event.currentTarget as HTMLElement;
  const position = positions.value[id];
  if (!position) return;
  target.setPointerCapture(event.pointerId);
  drag = { id, pointerId: event.pointerId, startX: event.clientX, startY: event.clientY, ...position, moved: false, target };
}
function moveDrag(event: PointerEvent) {
  if (!drag || drag.pointerId !== event.pointerId) return;
  const dx = event.clientX - drag.startX; const dy = event.clientY - drag.startY;
  if (!drag.moved && Math.hypot(dx, dy) < 6) return;
  drag.moved = true;
  const maxX = Math.max(1, size.value.width - floatingButtonSize - 12);
  const maxY = Math.max(1, size.value.height - floatingButtonSize - 12);
  preferences.value = { ...preferences.value, [drag.id]: { x: Math.max(0, Math.min(1, (drag.x + dx) / maxX)), y: Math.max(0, Math.min(1, (drag.y + dy) / maxY)), hidden: false } };
  event.preventDefault();
}
function finishDrag(event: PointerEvent) {
  if (!drag || drag.pointerId !== event.pointerId) return;
  if (drag.target.hasPointerCapture(event.pointerId)) drag.target.releasePointerCapture(event.pointerId);
  if (drag.moved) { suppressClick = drag.id; savePreferences(); }
  drag = null;
}
function onKeydown(event: KeyboardEvent) {
  if (event.key === "Escape") menuOpen.value = false;
  if (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10")) { event.preventDefault(); menuOpen.value = true; }
}
watch(preferenceKey, (key) => { preferences.value = readPluginFloatingPreferences(key); menuOpen.value = false; panelOpen.value = false; });
watch(() => props.instanceKey, () => { menuOpen.value = false; });
onMounted(() => {
  if (!root.value) return;
  observer = new ResizeObserver(([entry]) => {
    if (entry) size.value = { width: entry.contentRect.width, height: entry.contentRect.height };
  });
  observer.observe(root.value);
});
onBeforeUnmount(() => { observer?.disconnect(); observer = null; drag = null; });
</script>
<template>
  <div
    ref="root"
    class="plugin-floating"
    :data-floating-target="targetId"
    @keydown="onKeydown"
  >
    <template v-if="choices.length && !panelOpen">
      <NvxIconButton
        v-for="item in shown"
        :key="item.pluginId"
        class="plugin-floating__button"
        :data-floating-plugin="item.pluginId"
        :style="{ left: `${positions[item.pluginId]?.x ?? 0}px`, top: `${positions[item.pluginId]?.y ?? 0}px` }"
        :label="item.pluginName"
        :title="available ? t('plugins.floating.dragHint', { plugin: item.pluginName }) : t('plugins.tools.sessionRequired')"
        :disabled="!available"
        @pointerdown="beginDrag($event, item.pluginId)"
        @pointermove="moveDrag"
        @pointerup="finishDrag"
        @pointercancel="finishDrag"
        @contextmenu.prevent="menuOpen = true"
        @click="open(item)"
      >
        <NvxIcon
          :icon="pluginIconMap[item.icon ?? ''] ?? Plug"
          :size="20"
        />
      </NvxIconButton>
      <NvxIconButton
        v-if="needsMenuButton"
        class="plugin-floating__button plugin-floating__more"
        :style="{ left: `${positions.__menu?.x ?? 0}px`, top: `${positions.__menu?.y ?? 0}px` }"
        :label="t('plugins.floating.manage')"
        :aria-expanded="menuOpen"
        @pointerdown="beginDrag($event, '__menu')"
        @pointermove="moveDrag"
        @pointerup="finishDrag"
        @pointercancel="finishDrag"
        @click="toggleMenu"
      >
        <NvxIcon
          :icon="MoreHorizontal"
          :size="16"
        />
      </NvxIconButton>
      <div
        v-if="menuOpen"
        class="plugin-floating__menu"
        role="region"
        :aria-label="t('plugins.floating.manage')"
      >
        <strong>{{ t('plugins.floating.manage') }}</strong>
        <div class="plugin-floating__menu-items">
          <div
            v-for="item in choices"
            :key="item.pluginId"
          >
            <button
              type="button"
              :disabled="!available"
              @click="open(item)"
            >
              {{ item.pluginName }}
            </button><NvxIconButton
              size="sm"
              :label="t(preferences[item.pluginId]?.hidden ? 'plugins.floating.show' : 'plugins.floating.hide', { plugin: item.pluginName })"
              @click="toggleHidden(item)"
            >
              <NvxIcon
                :icon="preferences[item.pluginId]?.hidden ? EyeOff : Eye"
                :size="16"
              />
            </NvxIconButton>
          </div>
        </div>
        <NvxButton
          size="sm"
          variant="ghost"
          @click="reset"
        >
          <NvxIcon
            :icon="RotateCcw"
            :size="16"
          />{{ t('plugins.floating.reset') }}
        </NvxButton>
      </div>
    </template>
    <NvxPluginToolPanel
      v-model="panelOpen"
      floating
      :target-id="targetId"
      :instance-key="instanceKey"
      :context-label="contextLabel"
      :available="available"
      :initial-plugin-id="selectedId"
      :route-path="routePath"
      @catalog="choices = $event"
    />
  </div>
</template>
<style scoped>
.plugin-floating { position: absolute; inset: 0; z-index: var(--nvx-z-popover); pointer-events: none; }
.plugin-floating__button { position: absolute; width: 44px; height: 44px; border: 1px solid var(--nvx-color-border-strong); border-radius: 50%; background: var(--nvx-color-bg-surface); color: var(--nvx-color-text-primary); box-shadow: var(--nvx-shadow-overlay); pointer-events: auto; touch-action: none; user-select: none; }
.plugin-floating__button:hover { background: var(--nvx-color-bg-hover); }
.plugin-floating__more { color: var(--nvx-color-text-secondary); }
.plugin-floating__menu { position: absolute; z-index: 2; top: 12px; right: 64px; display: grid; width: min(320px, calc(100% - 88px)); max-height: calc(100% - 24px); gap: 8px; padding: 12px; border: 1px solid var(--nvx-color-border-strong); border-radius: var(--nvx-radius-md); background: var(--nvx-color-bg-surface); color: var(--nvx-color-text-primary); box-shadow: var(--nvx-shadow-overlay); pointer-events: auto; }
.plugin-floating__menu > strong { font-size: var(--nvx-font-size-sm); }
.plugin-floating__menu-items { min-height: 0; max-height: 45vh; overflow: auto; overscroll-behavior: contain; }
.plugin-floating__menu-items > div { display: flex; align-items: center; gap: 4px; }
.plugin-floating__menu-items > div > button:first-child { flex: 1; min-width: 0; min-height: 34px; padding: 6px; border: 0; background: transparent; color: inherit; font: inherit; text-align: left; }
.plugin-floating__menu-items > div > button:first-child:hover { background: var(--nvx-color-bg-hover); }
.plugin-floating__menu-items > div > button:disabled { opacity: .5; }
</style>
