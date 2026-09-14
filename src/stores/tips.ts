import { defineStore } from "pinia";
import { ref } from "vue";

export type NvxTipTone = "info" | "success" | "warning" | "error";

export interface NvxTipItem {
  id: string;
  scope: string | null;
  tone: NvxTipTone;
  title: string;
  message: string | null;
}

export interface ShowNvxTipInput {
  scope?: string;
  tone?: NvxTipTone;
  title: string;
  message?: string;
  durationMs?: number;
}

const MAX_VISIBLE_TIPS = 3;
const DEFAULT_DURATION_MS: Record<NvxTipTone, number> = {
  info: 4_000,
  success: 4_000,
  warning: 6_000,
  error: 6_000,
};

let tipSequence = 0;

export const useTipsStore = defineStore("tips", () => {
  const items = ref<NvxTipItem[]>([]);
  const dismissTimers = new Map<string, ReturnType<typeof setTimeout>>();

  function clearTimer(id: string) {
    const timer = dismissTimers.get(id);
    if (timer !== undefined) clearTimeout(timer);
    dismissTimers.delete(id);
  }

  function dismiss(id: string) {
    clearTimer(id);
    items.value = items.value.filter((item) => item.id !== id);
  }

  function dismissScope(scope: string) {
    for (const item of items.value) {
      if (item.scope === scope) clearTimer(item.id);
    }
    items.value = items.value.filter((item) => item.scope !== scope);
  }

  function show(input: ShowNvxTipInput) {
    const tone = input.tone ?? "info";
    const scope = input.scope ?? null;
    if (scope) dismissScope(scope);

    tipSequence += 1;
    const item: NvxTipItem = {
      id: `nvx-tip-${Date.now()}-${tipSequence}`,
      scope,
      tone,
      title: input.title,
      message: input.message ?? null,
    };
    items.value = [item, ...items.value];

    for (const hiddenItem of items.value.slice(MAX_VISIBLE_TIPS)) {
      clearTimer(hiddenItem.id);
    }
    items.value = items.value.slice(0, MAX_VISIBLE_TIPS);

    const durationMs = input.durationMs ?? DEFAULT_DURATION_MS[tone];
    if (durationMs > 0) {
      dismissTimers.set(item.id, setTimeout(() => dismiss(item.id), durationMs));
    }
    return item.id;
  }

  function clearAll() {
    for (const item of items.value) clearTimer(item.id);
    items.value = [];
  }

  return { items, show, dismiss, dismissScope, clearAll };
});
