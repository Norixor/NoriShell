<script setup lang="ts">
import { Copy, Square } from "lucide-vue-next";
import { computed, inject, onBeforeUnmount, ref, watch, type CSSProperties } from "vue";

import type { PluginUiContribution, PluginUiNode } from "../../core-api/generated/core-api";
import { registerPluginDialog } from "../../plugins/pluginDialogRegistry";
import { pluginIconMap } from "./pluginIcons";
import NvxPluginSshSyncBrowser from "./NvxPluginSshSyncBrowser.vue";
import NvxPluginUiMenu from "./NvxPluginUiMenu.vue";
import NvxPluginUiTabs from "./NvxPluginUiTabs.vue";
import NvxPluginUiTree from "./NvxPluginUiTree.vue";
import NvxPluginUiChart from "./NvxPluginUiChart.vue";
import NvxPluginUiEditor from "./NvxPluginUiEditor.vue";
import {
  NvxButton,
  NvxCheckbox,
  NvxDialog,
  NvxField,
  NvxIcon,
  NvxInput,
  NvxProgress,
  NvxSelect,
  NvxStatusLabel,
  NvxTextarea,
} from "../ui";

defineOptions({ name: "NvxPluginUiNode" });

const props = defineProps<{
  nodeId: string;
  nodeById: Record<string, PluginUiNode>;
  values: Record<string, string>;
  busy: boolean;
  actionsBlocked?: boolean;
  menuItem?: boolean;
  pageTitleNodeId?: string | null;
  contribution?: PluginUiContribution;
}>();

const emit = defineEmits<{
  action: [actionId: string];
  field: [fieldId: string, value: string];
  menuSelect: [];
}>();

const node = computed(() => props.nodeById[props.nodeId]);
const compact = inject("nvx-plugin-tool-panel-compact", false);
const dialogOpen = ref(false);
const dialogContent = ref<HTMLElement | null>(null);
let disposeDialogRegistration: (() => void) | null = null;
const iconActionRow = computed(() => {
  const current = node.value;
  if (current?.kind !== "stack" || current.direction !== "horizontal") return false;
  const kinds = current.children.map((childId) => props.nodeById[childId]?.kind);
  return kinds.includes("icon") && kinds.includes("text")
    && kinds.some((kind) => kind === "button" || kind === "copyButton");
});

const layoutStyle = computed<CSSProperties>(() => {
  const current = node.value;
  if (!current || (current.kind !== "stack" && current.kind !== "grid")) return {};
  const gap = compact ? Math.min(current.gap, 6) : current.gap;
  if (current.kind === "grid") {
    const columns = current.columnWeights?.length === current.columns
      ? current.columnWeights.map((weight) => `minmax(0, ${weight}fr)`).join(" ")
      : `repeat(${current.columns}, minmax(0, 1fr))`;
    return {
      display: "grid",
      gap: `${gap}px`,
      "--plugin-ui-grid-columns": columns,
      "--plugin-ui-grid-count": current.columns,
      "--plugin-ui-grid-gap": `${gap}px`,
    } as CSSProperties;
  }
  return {
    display: "flex",
    flexDirection: current.direction === "horizontal" ? "row" : "column",
    flexWrap: current.direction === "horizontal" ? "wrap" : undefined,
    alignItems: compact && current.direction === "horizontal"
      && current.children.some((childId) => ["textField", "select"].includes(props.nodeById[childId]?.kind ?? ""))
      ? "flex-end" : current.align === "start" ? "flex-start"
      : current.align === "end" ? "flex-end"
        : current.align,
    gap: `${gap}px`,
  };
});

const tableStyle = computed<CSSProperties>(() => {
  const current = node.value;
  if (current?.kind !== "table") return {};
  const minimumWidth = current.columns.reduce((total, column) => (
    total + (column.width === null ? compact ? 96 : 144 : Math.max(48, Math.min(480, column.width)))
  ), 0);
  return { minWidth: `${minimumWidth}px` };
});

const repeatedTableCaption = computed(() => {
  const current = node.value;
  return current?.kind === "table" && Object.values(props.nodeById).some((parent) => (
    parent.kind === "section" && parent.title === current.label && parent.children.includes(current.nodeId)
  ));
});

function updateBoolean(fieldId: string, value: boolean) {
  emit("field", fieldId, value ? "true" : "false");
}

function activateAction(actionId: string) {
  if (props.busy || props.actionsBlocked) return;
  if (props.menuItem) emit("menuSelect");
  emit("action", actionId);
}

function openDialog() {
  if (props.busy) return;
  if (props.menuItem) emit("menuSelect");
  dialogOpen.value = true;
}

function clearDialogRegistration() {
  disposeDialogRegistration?.();
  disposeDialogRegistration = null;
}

function closeDialog() {
  clearDialogRegistration();
  dialogOpen.value = false;
}

function updateDialogOpen(open: boolean) {
  if (!open) clearDialogRegistration();
  dialogOpen.value = open;
}

function synchronizeDialogRegistration() {
  clearDialogRegistration();
  const contribution = props.contribution;
  const content = dialogContent.value;
  if (!dialogOpen.value || !content || !contribution) return;
  disposeDialogRegistration = registerPluginDialog(content, {
    pluginId: contribution.pluginId,
    packageSha256: contribution.packageSha256,
    instanceGeneration: contribution.instanceGeneration,
  }, closeDialog);
}

watch(
  () => [
    dialogOpen.value,
    dialogContent.value,
    props.contribution?.pluginId,
    props.contribution?.packageSha256,
    props.contribution?.instanceGeneration,
  ],
  synchronizeDialogRegistration,
  { flush: "post" },
);

onBeforeUnmount(clearDialogRegistration);
</script>

<template>
  <template v-if="node">
    <div
      v-if="node.kind === 'stack' || node.kind === 'grid'"
      class="plugin-ui-node plugin-ui-node--layout"
      :class="[
        `plugin-ui-node--${node.kind}`,
        { 'plugin-ui-node--horizontal': node.kind === 'stack' && node.direction === 'horizontal',
          'plugin-ui-node--icon-action-row': iconActionRow,
          'plugin-ui-node--grid-equal': node.kind === 'grid' && node.columnWeights?.length !== node.columns },
      ]"
      :style="layoutStyle"
    >
      <NvxPluginUiNode
        v-for="childId in node.children"
        :key="childId"
        :node-id="childId"
        :node-by-id="nodeById"
        :page-title-node-id="pageTitleNodeId"
        :contribution="contribution"
        :values="values"
        :busy="busy"
        :actions-blocked="actionsBlocked"
        :menu-item="menuItem"
        @menu-select="emit('menuSelect')"
        @action="emit('action', $event)"
        @field="(fieldId, value) => emit('field', fieldId, value)"
      />
    </div>

    <section
      v-else-if="node.kind === 'section'"
      class="plugin-ui-node plugin-ui-node--section"
    >
      <h3 v-if="node.title">
        {{ node.title }}
      </h3>
      <NvxPluginUiNode
        v-for="childId in node.children"
        :key="childId"
        :node-id="childId"
        :node-by-id="nodeById"
        :page-title-node-id="pageTitleNodeId"
        :contribution="contribution"
        :values="values"
        :busy="busy"
        :actions-blocked="actionsBlocked"
        :menu-item="menuItem"
        @menu-select="emit('menuSelect')"
        @action="emit('action', $event)"
        @field="(fieldId, value) => emit('field', fieldId, value)"
      />
    </section>

    <NvxPluginSshSyncBrowser
      v-else-if="node.kind === 'sshSyncBrowser' && contribution?.target.surfaceKind === 'page'"
      :contribution="contribution"
      :node-id="node.nodeId"
      :profile-id="node.profileId"
    >
      <NvxPluginUiNode
        v-for="childId in node.children"
        :key="childId"
        :node-id="childId"
        :node-by-id="nodeById"
        :page-title-node-id="pageTitleNodeId"
        :contribution="contribution"
        :values="values"
        :busy="busy"
        :actions-blocked="actionsBlocked"
        @action="emit('action', $event)"
        @field="(fieldId, value) => emit('field', fieldId, value)"
      />
    </NvxPluginSshSyncBrowser>

    <NvxPluginUiTabs
      v-else-if="node.kind === 'tabs'"
      :label="node.label"
      :tabs="node.tabs"
      :disabled="busy || actionsBlocked"
    >
      <template #default="{ tab }">
        <NvxPluginUiNode
          v-for="childId in tab.children"
          :key="childId"
          :node-id="childId"
          :node-by-id="nodeById"
          :page-title-node-id="pageTitleNodeId"
          :contribution="contribution"
          :values="values"
          :busy="busy"
          :actions-blocked="actionsBlocked"
          @action="emit('action', $event)"
          @field="(fieldId, value) => emit('field', fieldId, value)"
        />
      </template>
    </NvxPluginUiTabs>

    <NvxPluginUiTree
      v-else-if="node.kind === 'tree'"
      :label="node.label"
      :items="node.items"
      :disabled="busy || actionsBlocked"
      @action="activateAction"
    />

    <NvxPluginUiChart
      v-else-if="node.kind === 'chart'"
      :label="node.label"
      :chart-kind="node.chartKind"
      :labels="node.labels"
      :series="node.series"
    />

    <NvxPluginUiEditor
      v-else-if="node.kind === 'editor'"
      :field-id="node.fieldId"
      :label="node.label"
      :value="values[node.fieldId] ?? node.value"
      :language="node.language"
      :read-only="node.readOnly"
      :disabled="busy"
      @field="(fieldId, value) => emit('field', fieldId, value)"
    />

    <hr
      v-else-if="node.kind === 'divider'"
      class="plugin-ui-node__divider"
    >

    <component
      :is="node.nodeId === pageTitleNodeId ? 'h1' : 'p'"
      v-else-if="node.kind === 'text'"
      class="plugin-ui-node__text"
      :class="[
        `plugin-ui-node__text--${node.style}`,
        `plugin-ui-node--${node.tone}`,
        { 'plugin-ui-node__text--page-title': node.nodeId === pageTitleNodeId },
      ]"
    >
      {{ node.text }}
    </component>

    <pre
      v-else-if="node.kind === 'code'"
      class="plugin-ui-node__code"
      tabindex="0"
      :class="{ 'plugin-ui-node__code--wrap': node.wrap }"
    ><code>{{ node.text }}</code></pre>

    <NvxIcon
      v-else-if="node.kind === 'icon'"
      :icon="pluginIconMap[node.icon] ?? Square"
      :label="node.accessibleLabel"
      :size="20"
      :class="`plugin-ui-node--${node.tone}`"
    />

    <NvxStatusLabel
      v-else-if="node.kind === 'status'"
      class="plugin-ui-node__status"
      :tone="node.tone"
    >
      {{ node.label }}
    </NvxStatusLabel>

    <NvxProgress
      v-else-if="node.kind === 'progress'"
      :label="node.label ?? ''"
      :value="node.valuePercent"
      :status="node.valuePercent === null ? 'loading' : 'available'"
      size="sm"
    />

    <NvxButton
      v-else-if="node.kind === 'button' || node.kind === 'copyButton'"
      size="sm"
      :variant="menuItem ? 'ghost' : node.kind === 'button' ? node.variant : 'secondary'"
      :role="menuItem ? 'menuitem' : undefined"
      :class="{ 'plugin-ui-node--danger': menuItem && node.kind === 'button' && node.variant === 'danger' }"
      :loading="busy"
      :disabled="busy || actionsBlocked || node.disabled"
      @click="activateAction(node.actionId)"
    >
      <span class="plugin-ui-node__button-content">
        <NvxIcon
          v-if="node.kind === 'copyButton' || node.icon"
          :icon="node.kind === 'copyButton' ? Copy : pluginIconMap[node.icon ?? ''] ?? Square"
          :size="16"
        />
        {{ node.label }}
      </span>
    </NvxButton>

    <NvxField
      v-else-if="node.kind === 'textField'"
      class="plugin-ui-node__field"
      :for-id="`plugin-field-${node.fieldId}`"
      :label="node.label"
    >
      <NvxTextarea
        v-if="node.fieldKind === 'multiline'"
        :id="`plugin-field-${node.fieldId}`"
        :rows="compact ? 3 : 5"
        :model-value="values[node.fieldId] ?? node.value"
        :placeholder="node.placeholder ?? undefined"
        :disabled="busy || node.disabled"
        :maxlength="16384"
        @update:model-value="emit('field', node.fieldId, $event)"
      />
      <NvxInput
        v-else
        :id="`plugin-field-${node.fieldId}`"
        :model-value="values[node.fieldId] ?? node.value"
        :placeholder="node.placeholder ?? undefined"
        :disabled="busy || node.disabled"
        :type="node.fieldKind === 'number' ? 'number' : node.fieldKind === 'password' ? 'password' : 'text'"
        :autocomplete="node.fieldKind === 'password' ? 'off' : undefined"
        :spellcheck="node.fieldKind === 'password' ? false : undefined"
        :autocapitalize="node.fieldKind === 'password' ? 'none' : undefined"
        :maxlength="node.fieldKind === 'password' ? 1024 : 16384"
        @update:model-value="emit('field', node.fieldId, $event)"
      />
    </NvxField>

    <NvxField
      v-else-if="node.kind === 'select'"
      class="plugin-ui-node__field"
      :for-id="`plugin-field-${node.fieldId}`"
      :label="node.label"
    >
      <NvxSelect
        :id="`plugin-field-${node.fieldId}`"
        :model-value="values[node.fieldId] ?? node.value ?? ''"
        :options="node.options"
        :disabled="busy || node.disabled"
        @update:model-value="emit('field', node.fieldId, $event)"
      />
    </NvxField>

    <NvxCheckbox
      v-else-if="node.kind === 'checkbox' || node.kind === 'switch'"
      class="plugin-ui-node__checkbox"
      :model-value="(values[node.fieldId] ?? String(node.checked)) === 'true'"
      :disabled="busy || node.disabled"
      @update:model-value="updateBoolean(node.fieldId, $event)"
    >
      {{ node.label }}
    </NvxCheckbox>

    <div
      v-else-if="node.kind === 'table'"
      class="plugin-ui-node__table-wrap"
      role="region"
      :aria-label="node.label"
      tabindex="0"
    >
      <table
        class="plugin-ui-node__table"
        :style="tableStyle"
      >
        <caption :class="{ 'plugin-ui-node__table-caption--repeated': repeatedTableCaption }">
          {{ node.label }}
        </caption>
        <colgroup>
          <col
            v-for="column in node.columns"
            :key="column.columnId"
            :style="column.width === null ? undefined : {
              width: `${Math.max(48, Math.min(480, column.width))}px`,
            }"
          >
        </colgroup>
        <thead>
          <tr>
            <th
              v-for="column in node.columns"
              :key="column.columnId"
              scope="col"
            >
              {{ column.label }}
            </th>
          </tr>
        </thead>
        <tbody v-if="node.rows.length">
          <tr
            v-for="row in node.rows"
            :key="row.rowId"
            :class="{ 'plugin-ui-node__table-row--action': row.actionId }"
            :tabindex="row.actionId && !busy && !actionsBlocked ? 0 : undefined"
            :aria-disabled="row.actionId ? busy || actionsBlocked : undefined"
            @click="row.actionId && activateAction(row.actionId)"
            @keydown.enter.prevent="row.actionId && activateAction(row.actionId)"
          >
            <td
              v-for="(cell, index) in row.cells"
              :key="node.columns[index]?.columnId"
            >
              {{ cell }}
            </td>
          </tr>
        </tbody>
      </table>
      <p
        v-if="!node.rows.length && node.emptyText"
        class="plugin-ui-node__empty"
      >
        {{ node.emptyText }}
      </p>
    </div>

    <div
      v-else-if="node.kind === 'dialog'"
      class="plugin-ui-node plugin-ui-node--dialog"
    >
      <NvxButton
        size="sm"
        :variant="menuItem ? 'ghost' : 'secondary'"
        :role="menuItem ? 'menuitem' : undefined"
        :disabled="busy"
        @click="openDialog"
      >
        {{ node.triggerLabel }}
      </NvxButton>
      <NvxDialog
        :model-value="dialogOpen"
        :title="node.title"
        :description="node.description ?? undefined"
        :close-label="node.closeLabel"
        plugin-protected
        :theme-protected="false"
        @update:model-value="updateDialogOpen"
      >
        <div
          ref="dialogContent"
          class="plugin-ui-node__dialog-content"
        >
          <NvxPluginUiNode
            v-for="childId in node.children"
            :key="childId"
            :node-id="childId"
            :node-by-id="nodeById"
            :page-title-node-id="pageTitleNodeId"
            :contribution="contribution"
            :values="values"
            :busy="busy"
            :actions-blocked="actionsBlocked"
            @action="emit('action', $event)"
            @field="(fieldId, value) => emit('field', fieldId, value)"
          />
        </div>
      </NvxDialog>
    </div>

    <NvxPluginUiMenu
      v-else-if="node.kind === 'menu'"
      :label="node.label"
      :disabled="busy"
    >
      <template #default="{ selectItem }">
        <NvxPluginUiNode
          v-for="childId in node.children"
          :key="childId"
          :node-id="childId"
          :node-by-id="nodeById"
          :page-title-node-id="pageTitleNodeId"
          :contribution="contribution"
          :values="values"
          :busy="busy"
          :actions-blocked="actionsBlocked"
          menu-item
          @menu-select="selectItem"
          @action="emit('action', $event)"
          @field="(fieldId, value) => emit('field', fieldId, value)"
        />
      </template>
    </NvxPluginUiMenu>

    <details
      v-else-if="node.kind === 'disclosure'"
      class="plugin-ui-node__disclosure"
      :open="node.open"
    >
      <summary>{{ node.label }}</summary>
      <div class="plugin-ui-node__disclosure-content">
        <NvxPluginUiNode
          v-for="childId in node.children"
          :key="childId"
          :node-id="childId"
          :node-by-id="nodeById"
          :page-title-node-id="pageTitleNodeId"
          :contribution="contribution"
          :values="values"
          :busy="busy"
          :actions-blocked="actionsBlocked"
          :menu-item="menuItem"
          @menu-select="emit('menuSelect')"
          @action="emit('action', $event)"
          @field="(fieldId, value) => emit('field', fieldId, value)"
        />
      </div>
    </details>
  </template>
</template>

<style scoped>
.plugin-ui-node--layout,
.plugin-ui-node__field,
.plugin-ui-node__disclosure,
.plugin-ui-node__dialog-content { min-width: 0; max-width: 100%; }
.plugin-ui-node--layout > * { min-width: 0; }
.plugin-ui-node--grid { grid-template-columns: var(--plugin-ui-grid-columns); }
.plugin-ui-node--grid-equal { grid-template-columns: repeat(auto-fit, minmax(min(100%, max(12rem, calc((100% - (var(--plugin-ui-grid-count) - 1) * var(--plugin-ui-grid-gap)) / var(--plugin-ui-grid-count)))), 1fr)); }
.plugin-ui-node--horizontal > .plugin-ui-node__field { flex: 1 1 12rem; }
.plugin-ui-node--horizontal > .plugin-ui-node--section { flex: 1 1 16rem; }
.plugin-ui-node__field { align-content: start; overflow-wrap: anywhere; }
.plugin-ui-node__field :deep(input),
.plugin-ui-node__field :deep(textarea),
.plugin-ui-node__field :deep(.nvx-select) { min-width: 0; max-width: 100%; }
.plugin-ui-node__status,
.plugin-ui-node__checkbox { max-width: 100%; overflow-wrap: anywhere; }
.plugin-ui-node__status :deep(svg) { flex-shrink: 0; }
.plugin-ui-node__status :deep(span),
.plugin-ui-node__checkbox :deep(.nvx-checkbox__content) { min-width: 0; }
.plugin-ui-node__checkbox :deep(input) { flex-shrink: 0; }
.plugin-ui-node--section { display: grid; grid-template-columns: minmax(0, 1fr); min-width: 0; max-width: 100%; container: plugin-content / inline-size; gap: var(--nvx-space-4); padding: var(--nvx-space-5); border: var(--nvx-border-width) solid var(--nvx-color-border); border-radius: var(--nvx-radius-md); background: var(--nvx-color-bg-surface); }
.plugin-ui-node--section h3 { overflow-wrap: anywhere; }
.plugin-ui-node--section h3 { margin: 0; font-size: var(--nvx-font-size-md); font-weight: var(--nvx-font-weight-semibold); }
.plugin-ui-node__divider { width: 100%; margin: var(--nvx-space-2) 0; border: 0; border-top: var(--nvx-border-width) solid var(--nvx-color-border); }
.plugin-ui-node__text { margin: 0; white-space: pre-wrap; overflow-wrap: anywhere; }
.plugin-ui-node__text--secondary, .plugin-ui-node__text--caption { color: var(--nvx-color-text-secondary); }
.plugin-ui-node__text--caption { font-size: var(--nvx-font-size-xs); }
.plugin-ui-node__text--heading { font-size: var(--nvx-font-size-lg); font-weight: var(--nvx-font-weight-semibold); }
.plugin-ui-node__text--page-title { font-size: var(--nvx-font-size-xl); line-height: var(--nvx-line-height-xl); }
.plugin-ui-node__text--monospace { font-family: var(--nvx-font-mono); }
.plugin-ui-node--info { color: var(--nvx-color-accent); }
.plugin-ui-node--success { color: var(--nvx-color-success); }
.plugin-ui-node--warning { color: var(--nvx-color-warning); }
.plugin-ui-node--danger { color: var(--nvx-color-danger); }
.plugin-ui-node__code { min-width: 0; max-width: 100%; max-height: min(32rem, 60vh); margin: 0; padding: var(--nvx-space-3); overflow: auto; overscroll-behavior: contain; border: var(--nvx-border-width) solid var(--nvx-color-border); border-radius: var(--nvx-radius-md); background: var(--nvx-color-bg-subtle); color: var(--nvx-color-text-primary); font-family: var(--nvx-font-mono); }
.plugin-ui-node__code:focus-visible,
.plugin-ui-node__table-wrap:focus-visible { outline: var(--nvx-focus-ring-width) solid var(--nvx-color-focus-ring); outline-offset: 2px; }
.plugin-ui-node__code--wrap { white-space: pre-wrap; overflow-wrap: anywhere; }
.plugin-ui-node__button-content { display: inline-flex; align-items: center; gap: var(--nvx-space-2); }
.plugin-ui-node__table-wrap { min-width: 0; max-width: 100%; overflow: auto; overscroll-behavior-x: contain; }
.plugin-ui-node__table { width: 100%; table-layout: fixed; border-collapse: collapse; text-align: left; }
.plugin-ui-node__table caption { padding-bottom: var(--nvx-space-3); color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-sm); text-align: left; }
.plugin-ui-node__table th, .plugin-ui-node__table td { padding: var(--nvx-space-3); border-bottom: var(--nvx-border-width) solid var(--nvx-color-border); overflow-wrap: anywhere; }
.plugin-ui-node__table th { height: 38px; padding-block: var(--nvx-space-2); background: var(--nvx-color-bg-subtle); color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-xs); font-weight: var(--nvx-font-weight-medium); }
.plugin-ui-node__table td { height: 48px; white-space: pre-wrap; font-weight: var(--nvx-font-weight-medium); font-variant-numeric: tabular-nums; }
.plugin-ui-node__table tbody tr:last-child td { border-bottom: 0; }
.plugin-ui-node__table-row--action { cursor: pointer; }
.plugin-ui-node__table-row--action:hover { background: var(--nvx-color-bg-hover); }
.plugin-ui-node__table-row--action:focus-visible { outline: var(--nvx-focus-ring-width) solid var(--nvx-color-focus-ring); outline-offset: calc(-1 * var(--nvx-focus-ring-width)); }
.plugin-ui-node__empty { margin: var(--nvx-space-3) 0 0; overflow-wrap: anywhere; color: var(--nvx-color-text-tertiary); }
.plugin-ui-node__disclosure { border-top: var(--nvx-border-width) solid var(--nvx-color-border); }
.plugin-ui-node__disclosure summary { padding: var(--nvx-space-3) 0; overflow-wrap: anywhere; cursor: pointer; color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-sm); font-weight: var(--nvx-font-weight-medium); }
.plugin-ui-node__disclosure summary:focus-visible { outline: var(--nvx-focus-ring-width) solid var(--nvx-color-focus-ring); outline-offset: 2px; }
.plugin-ui-node__disclosure-content { display: grid; grid-template-columns: minmax(0, 1fr); gap: var(--nvx-space-3); padding-bottom: var(--nvx-space-3); }
.plugin-ui-node__dialog-content { display: grid; grid-template-columns: minmax(0, 1fr); container: plugin-content / inline-size; gap: var(--nvx-space-3); }
@container plugin-content (max-width: 760px) {
  .plugin-ui-node--grid:not(.plugin-ui-node--grid-equal) { grid-template-columns: minmax(0, 1fr); }
}
@container plugin-page (max-width: 760px) {
  .plugin-ui-node--grid:not(.plugin-ui-node--grid-equal) { grid-template-columns: minmax(0, 1fr); }
}
@media (max-width: 760px) {
  .plugin-ui-node--grid:not(.plugin-ui-node--grid-equal) { grid-template-columns: minmax(0, 1fr); }
}
</style>
