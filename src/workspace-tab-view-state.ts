import { computed, ref } from "vue";

import type { WorkspaceTabKind } from "./workspace-tab-windows";

export interface WorkspaceTabViewSummary {
  id: string;
  viewLabel: string;
  kind: WorkspaceTabKind;
  label: string;
  stateLabel: string;
  /** The page route this Tab presents; the Navigation Rail highlights it while the Tab is active. */
  route?: string;
  hostId?: string | null;
  bellAttention?: boolean;
  /** Whether this Terminal Tab's Quick Commands sidebar is open. */
  quickCommandsOpen?: boolean;
}

const summaries = ref<WorkspaceTabViewSummary[]>([]);
const activeId = ref<string | null>(null);
const pendingId = ref<string | null>(null);

export const workspaceTabViewSummaries = computed(() => summaries.value);
export const activeWorkspaceTabViewId = computed(() => activeId.value);
/** The active Tab whose content has not rendered yet; the shell shows a placeholder for it. */
export const pendingWorkspaceTabViewId = computed(() => pendingId.value);

export function setPendingWorkspaceTabView(id: string | null): void {
  pendingId.value = id;
}

export function setWorkspaceTabViewSummary(summary: WorkspaceTabViewSummary): void {
  const index = summaries.value.findIndex((item) => item.id === summary.id);
  if (index < 0) summaries.value = [...summaries.value, summary];
  else summaries.value = summaries.value.map((item, position) => position === index ? summary : item);
}

/** Puts `summary` in the Header position of `targetId`, or appends it when absent. */
export function replaceWorkspaceTabViewSummary(targetId: string, summary: WorkspaceTabViewSummary): void {
  const retained = summaries.value.filter((item) => item.id !== summary.id);
  const index = retained.findIndex((item) => item.id === targetId);
  summaries.value = index < 0 ? [...retained, summary]
    : retained.map((item, position) => position === index ? summary : item);
}

export function retainWorkspaceTabViewSummaries(ids: ReadonlySet<string>): void {
  summaries.value = summaries.value.filter((summary) => ids.has(summary.id));
  if (activeId.value && !ids.has(activeId.value)) activeId.value = null;
}

export function workspaceTabViewFallbackAfterRemoval(id: string, remaining: ReadonlySet<string>): string | null {
  const index = summaries.value.findIndex((summary) => summary.id === id);
  if (index < 0) return null;
  for (let position = index - 1; position >= 0; position--) {
    const candidate = summaries.value[position];
    if (candidate && remaining.has(candidate.id)) return candidate.id;
  }
  for (let position = index + 1; position < summaries.value.length; position++) {
    const candidate = summaries.value[position];
    if (candidate && remaining.has(candidate.id)) return candidate.id;
  }
  return null;
}

export function setActiveWorkspaceTabView(id: string | null): void {
  activeId.value = id;
}

export function workspaceTabViewSummary(id: string): WorkspaceTabViewSummary | null {
  return summaries.value.find((summary) => summary.id === id) ?? null;
}
