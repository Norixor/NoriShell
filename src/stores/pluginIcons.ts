import { isTauri } from "@tauri-apps/api/core";
import { defineStore } from "pinia";
import { ref } from "vue";

import { readPluginIcon } from "../core-api/client";
import type { PluginIconReadScope } from "../core-api/generated/core-api";

export const usePluginIconsStore = defineStore("pluginIcons", () => {
  const images = ref<Record<string, string | null>>({});
  const revisions = new Map<string, string>();
  const checkedAt = new Map<string, number>();
  const pending = new Map<string, Promise<void>>();
  const generations = new Map<string, number>();

  function imageFor(pluginId: string, scope: PluginIconReadScope): string | undefined {
    return images.value[`${scope}:${pluginId}`] ?? undefined;
  }

  function discardImage(pluginId: string, scope: PluginIconReadScope, failedSource: string) {
    const key = `${scope}:${pluginId}`;
    // A late error for an old image cannot remove a refreshed image or affect another source.
    if (images.value[key] !== failedSource) return;
    images.value[key] = null;
    revisions.delete(key);
  }

  function load(pluginId: string, scope: PluginIconReadScope, revision: string, refresh = false): Promise<void> {
    if (!isTauri()) return Promise.resolve();
    const key = `${scope}:${pluginId}`;
    if (!refresh && revisions.get(key) === revision) {
      const current = pending.get(key);
      if (current) return current;
      if (Date.now() - (checkedAt.get(key) ?? 0) < 300_000) return Promise.resolve();
    }
    const generation = (generations.get(key) ?? 0) + 1;
    generations.set(key, generation);
    // Remove the old icon as soon as its package changes; Core rechecks the installation source.
    if (scope === "installed" && revisions.get(key) !== revision) delete images.value[key];
    revisions.set(key, revision);
    const task = readPluginIcon(pluginId, scope, refresh).then((result) => {
      if (generations.get(key) === generation) {
        images.value[key] = result.dataUrl ?? null;
        checkedAt.set(key, Date.now());
      }
    }).catch(() => {
      // An icon failure does not affect plugin operations; a later read may retry.
      if (generations.get(key) === generation) revisions.delete(key);
    }).finally(() => {
      if (generations.get(key) === generation) pending.delete(key);
    });
    pending.set(key, task);
    return task;
  }

  function retain(pluginIds: string[], scope: PluginIconReadScope) {
    const keys = new Set(pluginIds.map((id) => `${scope}:${id}`));
    for (const key of revisions.keys()) {
      if (!key.startsWith(`${scope}:`) || keys.has(key)) continue;
      generations.set(key, (generations.get(key) ?? 0) + 1);
      revisions.delete(key);
      checkedAt.delete(key);
      pending.delete(key);
      delete images.value[key];
    }
  }

  return { imageFor, discardImage, load, retain };
});
