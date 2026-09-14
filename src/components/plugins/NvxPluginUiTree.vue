<script setup lang="ts">
import { ChevronDown, ChevronRight } from "lucide-vue-next";
import { ref } from "vue";

import { NvxButton, NvxIcon, NvxIconButton } from "../ui";

defineOptions({ name: "NvxPluginUiTree" });

interface PluginUiTreeItem {
  id: string;
  label: string;
  children?: PluginUiTreeItem[] | null;
  actionId?: string | null;
}

const props = withDefaults(defineProps<{
  label: string;
  items: PluginUiTreeItem[];
  disabled?: boolean;
  nested?: boolean;
}>(), {
  disabled: false,
  nested: false,
});

const emit = defineEmits<{
  action: [id: string];
}>();

const expanded = ref(new Set<string>());

function toggle(id: string) {
  const next = new Set(expanded.value);
  if (next.has(id)) next.delete(id);
  else next.add(id);
  expanded.value = next;
}

function activate(actionId: string | null | undefined) {
  if (!props.disabled && actionId) emit("action", actionId);
}
</script>

<template>
  <section
    class="plugin-ui-tree"
    :aria-label="label"
  >
    <ul :role="nested ? 'group' : 'tree'">
      <li
        v-for="item in items"
        :key="item.id"
        role="none"
        class="plugin-ui-tree__entry"
      >
        <div class="plugin-ui-tree__row">
          <NvxIconButton
            v-if="item.children?.length"
            class="plugin-ui-tree__toggle"
            :label="item.label"
            :aria-expanded="expanded.has(item.id)"
            @click="toggle(item.id)"
          >
            <NvxIcon
              :icon="expanded.has(item.id) ? ChevronDown : ChevronRight"
              :size="16"
            />
          </NvxIconButton>
          <span
            v-else
            class="plugin-ui-tree__indent"
            aria-hidden="true"
          />
          <NvxButton
            v-if="item.actionId"
            size="sm"
            variant="ghost"
            class="plugin-ui-tree__item"
            role="treeitem"
            :aria-expanded="item.children?.length ? expanded.has(item.id) : undefined"
            :disabled="disabled"
            @click="activate(item.actionId)"
          >
            {{ item.label }}
          </NvxButton>
          <span
            v-else
            class="plugin-ui-tree__item plugin-ui-tree__item--static"
            role="treeitem"
            :aria-expanded="item.children?.length ? expanded.has(item.id) : undefined"
          >{{ item.label }}</span>
        </div>
        <NvxPluginUiTree
          v-if="item.children?.length && expanded.has(item.id)"
          nested
          :label="item.label"
          :items="item.children"
          :disabled="disabled"
          @action="emit('action', $event)"
        />
      </li>
    </ul>
  </section>
</template>

<style scoped>
.plugin-ui-tree ul { display: grid; margin: 0; padding: 0; list-style: none; }
.plugin-ui-tree[aria-label] { min-width: 0; }
.plugin-ui-tree__entry > .plugin-ui-tree { margin-left: var(--nvx-space-4); }
.plugin-ui-tree__row { display: grid; grid-template-columns: 28px minmax(0, 1fr); align-items: center; min-width: 0; }
.plugin-ui-tree__toggle { justify-self: center; min-width: 28px; padding-inline: var(--nvx-space-1); }
.plugin-ui-tree__indent { width: 28px; }
.plugin-ui-tree__item { width: 100%; justify-content: flex-start; text-align: start; white-space: normal; }
.plugin-ui-tree__item--static { padding: 4px 8px; color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-sm); line-height: var(--nvx-line-height-sm); }
</style>
