<script lang="ts">
/** Fixed secondary routes behind "More"; plugin pages (`/plugin/<id>/<page>`) join at runtime. */
export const MORE_SHEET_ROUTES = ["/desktop", "/tunnels", "/plugins", "/settings"] as const;

export function isMoreSheetRoute(path: string): boolean {
  return (MORE_SHEET_ROUTES as readonly string[]).includes(path) || /^\/plugin\/[^/]+\/[^/]+$/.test(path);
}
</script>

<script setup lang="ts">
import { Monitor, Package, Plug, Settings, Waypoints } from "lucide-vue-next";
import { computed, type Component } from "vue";
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";

import type { PluginNavigationItem } from "../../core-api/generated/core-api";
import { usePluginExtensionsStore } from "../../stores/pluginExtensions";
import { NvxDialog, NvxIcon } from "../ui";

/** Secondary destinations that do not fit the bottom bar: the same routes the desktop rail offers, nothing extra. */
defineProps<{ modelValue: boolean }>();
const emit = defineEmits<{ "update:modelValue": [value: boolean] }>();
const { t } = useI18n();
const router = useRouter();
const extensions = usePluginExtensionsStore();

interface MoreItem { to: string; label: string; icon: Component; plugin?: PluginNavigationItem }
const items = computed<MoreItem[]>(() => [
  { to: "/desktop", label: t("desktop.navigation"), icon: Monitor },
  { to: "/tunnels", label: t("navigation.tunnels"), icon: Waypoints },
  { to: "/plugins", label: t("navigation.plugins"), icon: Package },
  ...extensions.navigation.map((item) => ({
    to: `/plugin/${encodeURIComponent(item.pluginId)}/${encodeURIComponent(item.navigation.pageId)}`,
    label: item.navigation.label,
    icon: Plug,
    plugin: item,
  })),
  { to: "/settings", label: t("navigation.settings"), icon: Settings },
]);

function open(item: MoreItem) {
  emit("update:modelValue", false);
  void router.push(item.to);
}
</script>

<template>
  <NvxDialog
    :model-value="modelValue"
    :title="t('mobile.moreTitle')"
    :close-label="t('mobile.closeSheet')"
    @update:model-value="emit('update:modelValue', $event)"
  >
    <ul class="nvx-mobile-more">
      <li
        v-for="item in items"
        :key="item.to"
      >
        <button
          class="nvx-mobile-more__item"
          type="button"
          :aria-current="router.currentRoute.value.path === item.to ? 'page' : undefined"
          @click="open(item)"
        >
          <span class="nvx-mobile-more__icon">
            <NvxIcon
              :icon="item.icon"
              :size="20"
            />
          </span>
          <span>{{ item.label }}</span>
        </button>
      </li>
    </ul>
  </NvxDialog>
</template>

<style scoped>
.nvx-mobile-more {
  display: grid;
  align-content: start;
  gap: var(--nvx-space-1);
  margin: 0 calc(-1 * var(--nvx-space-2));
  padding: 0;
  list-style: none;
}

.nvx-mobile-more__item {
  display: flex;
  gap: var(--nvx-space-3);
  align-items: center;
  width: 100%;
  min-height: 52px;
  padding: 0 var(--nvx-space-3);
  border: 0;
  border-radius: var(--nvx-radius-md);
  background: transparent;
  color: var(--nvx-color-text-primary);
  font: inherit;
  text-align: left;
  cursor: pointer;
  -webkit-tap-highlight-color: transparent;
}

.nvx-mobile-more__icon {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 40px;
  height: 40px;
  border-radius: var(--nvx-radius-md);
  background: var(--nvx-color-bg-subtle);
  color: var(--nvx-color-text-secondary);
}

.nvx-mobile-more__item:active { background: var(--nvx-color-bg-hover); }
.nvx-mobile-more__item[aria-current="page"] { color: var(--nvx-color-accent); background: var(--nvx-color-accent-soft); }
.nvx-mobile-more__item[aria-current="page"] .nvx-mobile-more__icon { color: var(--nvx-color-accent); background: var(--nvx-color-bg-surface); }
.nvx-mobile-more__item:focus-visible { outline: var(--nvx-focus-ring-width) solid var(--nvx-color-focus-ring); outline-offset: -2px; }
</style>
