<script setup lang="ts">
import { computed, inject, nextTick, onBeforeUnmount, ref, watch } from "vue";

import type {
  PluginUiContribution,
  PluginUiFieldValue,
  PluginUiNode,
} from "../../core-api/generated/core-api";
import NvxPluginUiNode from "./NvxPluginUiNode.vue";

const props = defineProps<{
  contribution: PluginUiContribution;
  busy: boolean;
  actionsBlocked?: boolean;
  requestAction?: (actionId: string, fields: PluginUiFieldValue[]) => Promise<PluginUiContribution | null>;
}>();

const emit = defineEmits<{
  action: [actionId: string, fields: PluginUiFieldValue[]];
}>();

const values = ref<Record<string, string>>({});
const compact = inject("nvx-plugin-tool-panel-compact", false);
const nodeById = computed(() => Object.fromEntries(
  props.contribution.document.nodes.map((node) => [nodeId(node), node]),
));
const parentById = computed(() => {
  const parents: Record<string, string> = {};
  for (const node of props.contribution.document.nodes) {
    for (const childId of childIds(node)) parents[childId] = node.nodeId;
  }
  return parents;
});
const pageTitleNodeId = computed(() => {
  if (props.contribution.target?.surfaceKind !== "page") return null;
  // The page title comes only from the first heading in the root layout; local sections, menus, and dialogs have their own heading levels.
  const findTitle = (identifier: string): string | null | undefined => {
    const node = nodeById.value[identifier];
    if (!node) return undefined;
    if (node.kind === "text" && node.style === "heading") return identifier;
    if (node.kind === "menu" || node.kind === "dialog" || node.kind === "icon") return undefined;
    if (node.kind === "text" && (node.style === "caption" || node.style === "secondary")) return undefined;
    // Once actual content starts, stop searching for a page title so later metrics or section headings cannot become h1.
    if (node.kind !== "stack" && node.kind !== "grid") return null;
    for (const childId of node.children) {
      const title = findTitle(childId);
      if (title !== undefined) return title;
    }
    return undefined;
  };
  return findTitle(props.contribution.document.rootNodeId) ?? null;
});
let previousIdentity = "";
let disposed = false;
const fieldEditVersions = new Map<string, number>();
interface PendingAction {
  identity: string;
  contributionRevision: string;
  fieldVersions: Map<string, number>;
}
let pendingAction: PendingAction | null = null;

function contributionIdentity(contribution: PluginUiContribution) {
  return JSON.stringify([contribution.pluginId, contribution.artifactFingerprintSha256,
    contribution.packageSha256, contribution.instanceGeneration, contribution.stateVersion,
    contribution.target.targetId, contribution.target.contextHandle, contribution.target.targetRevision]);
}
let previousDeclaredValues: Record<string, string> = {};

function nodeId(node: PluginUiNode) {
  return node.nodeId;
}

function childIds(node: PluginUiNode): string[] {
  return node.kind === "stack"
    || node.kind === "grid"
    || node.kind === "section"
    || node.kind === "menu"
    || node.kind === "dialog"
    || node.kind === "disclosure"
    || node.kind === "sshSyncBrowser"
    ? node.children
    : node.kind === "tabs"
      ? node.tabs.flatMap((tab) => tab.children)
    : [];
}

function nearestDialog(nodeIdentifier: string) {
  let current: string | undefined = nodeIdentifier;
  while (current) {
    const currentNode: PluginUiNode | undefined = nodeById.value[current];
    if (currentNode?.kind === "dialog") return current;
    current = parentById.value[current];
  }
  return null;
}

function fieldIdsForAction(actionId: string) {
  const actionNode = props.contribution.document.nodes.find((node) => (
    (node.kind === "button" || node.kind === "copyButton") && node.actionId === actionId
  ) || (
    node.kind === "table" && node.rows.some((row) => row.actionId === actionId)
  ) || (
    node.kind === "tree" && treeHasAction(node.items, actionId)
  ));
  if (!actionNode) return new Set<string>();
  const actionDialog = nearestDialog(actionNode.nodeId);
  return new Set(props.contribution.document.nodes.flatMap((node) => {
    if (
      nearestDialog(node.nodeId) !== actionDialog
      || !(
        node.kind === "textField"
        || node.kind === "select"
        || node.kind === "checkbox"
        || node.kind === "switch"
        || (node.kind === "editor" && !node.readOnly)
      )
      || (node.kind !== "editor" && node.disabled)
    ) return [];
    return [node.fieldId];
  }));
}

function treeHasAction(
  items: Array<{ actionId?: string | null; children?: unknown }>,
  actionId: string,
): boolean {
  return items.some((item) => item.actionId === actionId || (
    Array.isArray(item.children) && treeHasAction(item.children, actionId)
  ));
}

function reconcileFields(preserveNonSecret: boolean, authoritativeFields?: Set<string>) {
  const preserveDraft = (fieldId: string, declaredValue: string, previous: string | undefined) => (
    preserveNonSecret && previous !== undefined && !authoritativeFields?.has(fieldId)
    && (pendingAction !== null || previousDeclaredValues[fieldId] === declaredValue)
  );
  const next: Record<string, string> = {};
  // Defer draft changes while an explicit action is pending. Its exact reply
  // can replace only submitted fields that the user has not edited again.
  const declared: Record<string, string> = {};
  for (const node of props.contribution.document.nodes) {
    if (node.kind === "textField" && !node.disabled) {
      const previous = values.value[node.fieldId];
      if (node.fieldKind === "password") {
        next[node.fieldId] = pendingAction !== null && preserveDraft(node.fieldId, "", previous)
          ? previous! : "";
        declared[node.fieldId] = "";
      } else {
        declared[node.fieldId] = node.value;
        next[node.fieldId] = preserveDraft(node.fieldId, node.value, previous)
          ? previous!
          : node.value;
      }
    }
    else if (node.kind === "editor" && !node.readOnly) {
      const previous = values.value[node.fieldId];
      declared[node.fieldId] = node.value;
      next[node.fieldId] = preserveDraft(node.fieldId, node.value, previous) ? previous! : node.value;
    }
    else if (node.kind === "select" && !node.disabled) {
      const previous = values.value[node.fieldId];
      const defaultValue = node.value ?? "";
      const isAvailable = (value: string | undefined): value is string => (
        value !== undefined && node.options.some((option) => option.value === value && !option.disabled)
      );
      declared[node.fieldId] = defaultValue;
      next[node.fieldId] = preserveDraft(node.fieldId, defaultValue, previous)
        && isAvailable(previous)
        ? previous
        : isAvailable(defaultValue) ? defaultValue : "";
    }
    else if ((node.kind === "checkbox" || node.kind === "switch") && !node.disabled) {
      const previous = values.value[node.fieldId];
      const defaultValue = String(node.checked);
      declared[node.fieldId] = defaultValue;
      next[node.fieldId] = preserveDraft(node.fieldId, defaultValue, previous)
        ? previous!
        : defaultValue;
    }
  }
  values.value = next;
  previousDeclaredValues = declared;
}

function updateField(fieldId: string, value: string) {
  fieldEditVersions.set(fieldId, (fieldEditVersions.get(fieldId) ?? 0) + 1);
  values.value = { ...values.value, [fieldId]: value };
}

function invoke(actionId: string) {
  if (props.busy || props.actionsBlocked || pendingAction) return;
  const scopedFieldIds = fieldIdsForAction(actionId);
  const fields = Object.entries(values.value)
    .filter(([fieldId]) => scopedFieldIds.has(fieldId))
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([fieldId, value]) => ({ fieldId, value }));
  const action: PendingAction | null = props.requestAction ? {
    identity: contributionIdentity(props.contribution),
    contributionRevision: props.contribution.contributionRevision,
    fieldVersions: new Map(fields.map(({ fieldId }) => [fieldId, fieldEditVersions.get(fieldId) ?? 0])),
  } : null;
  pendingAction = action;
  const submittedPasswords = new Set(props.contribution.document.nodes.flatMap((node) => (
    node.kind === "textField"
    && node.fieldKind === "password"
    && scopedFieldIds.has(node.fieldId)
      ? [node.fieldId]
      : []
  )));
  if (submittedPasswords.size > 0) {
    values.value = Object.fromEntries(Object.entries(values.value).map(([fieldId, value]) => (
      [fieldId, submittedPasswords.has(fieldId) ? "" : value]
    )));
  }
  if (!action || !props.requestAction) {
    emit("action", actionId, fields);
    return;
  }
  const requestAction = props.requestAction;
  void completeAction(action, Promise.resolve().then(() => requestAction(actionId, fields)));
}

async function completeAction(action: PendingAction, request: Promise<PluginUiContribution | null>) {
  try {
    const result = await request;
    await nextTick();
    if (disposed || pendingAction !== action || !result
      || contributionIdentity(result) !== action.identity
      || contributionIdentity(props.contribution) !== action.identity
      || result.contributionRevision !== props.contribution.contributionRevision
      || BigInt(result.contributionRevision) !== BigInt(action.contributionRevision) + 1n) return;
    const unchangedSubmittedFields = new Set([...action.fieldVersions].flatMap(([fieldId, version]) => (
      (fieldEditVersions.get(fieldId) ?? 0) === version ? [fieldId] : []
    )));
    reconcileFields(true, unchangedSubmittedFields);
  } catch {
    // The host owns action feedback. A failed or stale reply keeps the draft.
  } finally {
    if (pendingAction === action) pendingAction = null;
  }
}

watch(
  () => [contributionIdentity(props.contribution), props.contribution.contributionRevision],
  ([identity]) => {
    if (pendingAction && pendingAction.identity !== identity) pendingAction = null;
    const preserveNonSecret = previousIdentity === identity;
    reconcileFields(preserveNonSecret);
    previousIdentity = identity ?? "";
  },
  { immediate: true },
);
onBeforeUnmount(() => { disposed = true; pendingAction = null; });
</script>

<template>
  <div
    class="plugin-ui-document"
    :class="{
      'plugin-ui-document--page': contribution.target?.surfaceKind === 'page',
      'plugin-ui-document--compact': compact,
    }"
  >
    <NvxPluginUiNode
      :node-id="contribution.document.rootNodeId"
      :node-by-id="nodeById"
      :page-title-node-id="pageTitleNodeId"
      :contribution="contribution"
      :values="values"
      :busy="busy"
      :actions-blocked="actionsBlocked"
      @field="updateField"
      @action="invoke"
    />
  </div>
</template>

<style scoped>
.plugin-ui-document { display: grid; grid-template-columns: minmax(0, 1fr); min-width: 0; max-width: 100%; gap: var(--nvx-space-3); }
.plugin-ui-document--page { container: plugin-page / inline-size; }
.plugin-ui-document--compact {
  --nvx-control-height-sm: 26px;
  --nvx-control-height-md: 28px;
  --nvx-font-size-sm: 13px;
  gap: 6px;
  font-size: 13px;
  line-height: 1.4;
}
.plugin-ui-document--compact :deep(.plugin-ui-node--section) { gap: 6px; padding: 6px 8px; border: var(--nvx-border-width) solid var(--nvx-color-border); border-radius: var(--nvx-radius-md); background: var(--nvx-color-bg-canvas); }
.plugin-ui-document--compact :deep(.plugin-ui-node--section > h3) { margin: -6px -8px 0; padding: 4px 8px; border-bottom: var(--nvx-border-width) solid var(--nvx-color-border); border-radius: var(--nvx-radius-md) var(--nvx-radius-md) 0 0; background: var(--nvx-color-bg-subtle); font-size: 14px; line-height: 20px; }
.plugin-ui-document--compact :deep(.plugin-ui-node__text--heading) { font-size: 15px; line-height: 21px; }
.plugin-ui-document--compact :deep(.plugin-ui-node__field) { gap: 3px; }
.plugin-ui-document--compact :deep(.plugin-ui-node--horizontal > .plugin-ui-node__field) { flex-basis: 9rem; }
.plugin-ui-document--compact :deep(.plugin-ui-node--grid-equal) { grid-template-columns: repeat(auto-fit, minmax(min(100%, max(9rem, calc((100% - (var(--plugin-ui-grid-count) - 1) * var(--plugin-ui-grid-gap)) / var(--plugin-ui-grid-count)))), 1fr)); }
.plugin-ui-document--compact :deep(.plugin-ui-node--section > .nvx-button) { justify-self: start; }
.plugin-ui-document--compact :deep(.nvx-field__label) { color: var(--nvx-color-text-secondary); font-size: 12px; line-height: 17px; }
.plugin-ui-document--compact :deep(.plugin-ui-node--icon-action-row) { padding: 4px 6px; border: var(--nvx-border-width) solid var(--nvx-color-border); border-radius: var(--nvx-radius-sm); background: var(--nvx-color-bg-surface); }
.plugin-ui-document--compact :deep(.plugin-ui-node--icon-action-row > .nvx-icon) { flex-shrink: 0; }
.plugin-ui-document--compact :deep(.plugin-ui-node--icon-action-row > .plugin-ui-node__text) { flex: 1 1 7rem; }
.plugin-ui-document--compact :deep(.plugin-ui-node--icon-action-row > .nvx-button) { margin-inline-start: auto; }
.plugin-ui-document--compact :deep(.nvx-input),
.plugin-ui-document--compact :deep(.nvx-select__trigger),
.plugin-ui-document--compact :deep(.nvx-button) { padding-inline: 8px; }
.plugin-ui-document--compact :deep(.nvx-textarea) { min-height: 76px; padding: 6px 8px; font-size: 12px; line-height: 18px; }
.plugin-ui-document--compact :deep(.plugin-ui-node__button-content) { gap: 4px; }
.plugin-ui-document--compact :deep(.plugin-ui-node__divider) { margin-block: 3px; }
.plugin-ui-document--compact :deep(.plugin-ui-node__code) { max-height: min(12rem, 30vh); padding: 6px 8px; font-size: 12px; line-height: 18px; }
.plugin-ui-document--compact :deep(.plugin-ui-node__table-wrap) { border: var(--nvx-border-width) solid var(--nvx-color-border); border-radius: var(--nvx-radius-sm); background: var(--nvx-color-bg-surface); }
.plugin-ui-document--compact :deep(.plugin-ui-node__table caption) { padding: 4px 6px; border-bottom: var(--nvx-border-width) solid var(--nvx-color-border); font-size: 12px; }
.plugin-ui-document--compact :deep(.plugin-ui-node__table .plugin-ui-node__table-caption--repeated) { position: absolute; width: 1px; height: 1px; margin: -1px; padding: 0; border: 0; overflow: hidden; clip-path: inset(50%); white-space: nowrap; }
.plugin-ui-document--compact :deep(.plugin-ui-node__table th),
.plugin-ui-document--compact :deep(.plugin-ui-node__table td) { box-sizing: border-box; height: 28px; padding: 4px 6px; font-size: 12px; line-height: 19px; }
.plugin-ui-document--compact :deep(.plugin-ui-node__table td) { font-weight: var(--nvx-font-weight-regular); }
.plugin-ui-document--compact :deep(.plugin-ui-node__table tbody tr:nth-child(even)) { background: var(--nvx-color-bg-canvas); }
.plugin-ui-document--compact :deep(.plugin-ui-node__table .plugin-ui-node__table-row--action:hover) { background: var(--nvx-color-bg-hover); }
.plugin-ui-document--compact :deep(.plugin-ui-node__empty) { margin: 4px 6px; font-size: 12px; }
.plugin-ui-document--compact :deep(.plugin-ui-node__disclosure summary) { padding-block: 6px; font-size: 12px; }
.plugin-ui-document--compact :deep(.plugin-ui-node__disclosure-content) { gap: 6px; padding-bottom: 6px; }
</style>
