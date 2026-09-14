import { invoke, isTauri } from "@tauri-apps/api/core";
import { defineStore } from "pinia";
import { nextTick, ref } from "vue";
import type { PluginAppIntegrationSnapshot, PluginAppCommand } from "../core-api/generated/core-api";
import { useTipsStore } from "./tips";
import { usePluginExtensionsStore } from "./pluginExtensions";
import { i18n } from "../locales";
import {
  listPluginDialogs,
  pluginDialogOwnerMatches,
  type PluginDialogOwner,
} from "../plugins/pluginDialogRegistry";

export function appShortcutMatches(event: KeyboardEvent, shortcut: string | null): boolean {
  if (!shortcut || event.repeat || event.isComposing || event.ctrlKey || event.metaKey || !event.altKey || !event.shiftKey) return false;
  if (document.querySelector('[role="dialog"], dialog[open]')) return false;
  const target = event.target;
  if (target instanceof Element && target.closest('input, textarea, select, [contenteditable="true"], .xterm, [data-terminal-surface]')) return false;
  return shortcut === `Alt+Shift+${event.code.replace(/^Key/, "")}` && /^Key[A-Z]$/.test(event.code);
}
export const usePluginAppIntegrationsStore = defineStore("pluginAppIntegrations", () => {
  const snapshots = ref<PluginAppIntegrationSnapshot[]>([]);
  const busy = ref(false);
  const enabledBindings = ref(false);
  const extensions = usePluginExtensionsStore();
  async function refresh() {
    if (!isTauri()) return;
    snapshots.value = await invoke<PluginAppIntegrationSnapshot[]>("plugin_app_integration_list", { request: { meta: { requestId: crypto.randomUUID() } } });
  }
  async function run(snapshot: PluginAppIntegrationSnapshot, command: PluginAppCommand) {
    if (busy.value) return;
    busy.value = true;
    let lease: Awaited<ReturnType<typeof extensions.acquireTarget>> | undefined;
    try {
      await refresh();
      const current = snapshots.value.find((item) => item.pluginId === snapshot.pluginId && item.packageSha256 === snapshot.packageSha256 && item.instanceGeneration === snapshot.instanceGeneration);
      if (!current || !current.registration.commands.some((item) => JSON.stringify(item) === JSON.stringify(command))) throw new Error("stale app command");
      lease = await extensions.acquireTarget(command.targetId, command.pageId ? `${snapshot.pluginId}|${command.pageId}` : "global");
      const contributions = await extensions.loadTargetContributions(lease.context);
      const contribution = contributions.find((item) => item.pluginId === snapshot.pluginId && item.packageSha256 === snapshot.packageSha256 && item.instanceGeneration === snapshot.instanceGeneration);
      if (!contribution) throw new Error("stale app command target");
      await extensions.invokeAction(contribution, command.actionId, [], null);
    } finally {
      try {
        if (lease) await extensions.releaseTarget(lease);
      } finally {
        busy.value = false;
      }
    }
  }
  return { snapshots, busy, enabledBindings, refresh, run };
});

type PluginAppNavigationPayload = PluginDialogOwner & { path: string };

function isAllowedPluginAppNavigationPath(payload: PluginAppNavigationPayload): boolean {
  const allowed = ["/terminal", "/overview", "/hosts", "/sftp", "/tunnels", "/plugins", "/settings"];
  const segments = payload.path.split("/");
  const ownPage = segments.length === 4 && segments[1] === "plugin" && segments[2] === payload.pluginId
    && /^[a-zA-Z0-9._:-]+$/.test(segments[3] ?? "");
  return allowed.includes(payload.path) || ownPage;
}

function hasCurrentPluginAppNavigationOwner(
  snapshots: PluginAppIntegrationSnapshot[],
  owner: PluginDialogOwner,
): boolean {
  return snapshots.some((snapshot) => pluginDialogOwnerMatches(snapshot, owner));
}

function onlyOwnPluginDialogs(owner: PluginDialogOwner): boolean {
  return listPluginDialogs().every((state) => (
    state.registration !== null && pluginDialogOwnerMatches(state.registration.owner, owner)
  ));
}

function closeOwnPluginDialogs(owner: PluginDialogOwner) {
  for (const state of listPluginDialogs()) {
    if (state.registration && pluginDialogOwnerMatches(state.registration.owner, owner)) {
      state.registration.close();
    }
  }
}

/** Main-window integration; the Core event carries an exact current plugin fence. */
export async function startPluginAppNavigation(router: import("vue-router").Router): Promise<() => void> {
  if (!isTauri()) return () => {};
  const { listen } = await import("@tauri-apps/api/event");
  const store = usePluginAppIntegrationsStore();
  let active = true;
  const unlisten = await listen<PluginAppNavigationPayload>("norishell://plugin-app-navigation", async ({ payload }) => {
    // Unknown or foreign dialog owners block immediately and never replay; only the host renderer can recognize
    // the owner of this plugin's declarative modal.
    if (!active || !onlyOwnPluginDialogs(payload)) return;
    try {
      await store.refresh();
      if (!active || !hasCurrentPluginAppNavigationOwner(store.snapshots, payload)
        || !isAllowedPluginAppNavigationPath(payload)) return;
      if (!onlyOwnPluginDialogs(payload)) return;
      closeOwnPluginDialogs(payload);
      await nextTick();
      await store.refresh();
      if (!active || !hasCurrentPluginAppNavigationOwner(store.snapshots, payload)
        || listPluginDialogs().length > 0) return;
      await router.push(payload.path);
    } catch { /* Revoked or unavailable owner: navigation is discarded. */ }
  });
  const stopNotifications = await listen<PluginAppIntegrationSnapshot>("norishell://plugin-app-notification", async ({ payload }) => {
    if (!active) return;
    try {
      await store.refresh();
      const current = store.snapshots.find((snapshot) => snapshot.pluginId === payload.pluginId && snapshot.packageSha256 === payload.packageSha256 && snapshot.instanceGeneration === payload.instanceGeneration);
      const notification = payload.notifications.at(-1);
      if (!active || !current || !notification || !current.notifications.some((item) => item.id === notification.id && item.text === notification.text)) return;
      useTipsStore().show({ scope: `plugin-app:${payload.pluginId}`, tone: "info", title: current.pluginName, message: notification.text });
    } catch { /* Revocation discards queued notifications. */ }
  });
  const onKey = (event: KeyboardEvent) => {
    if (!active || !store.enabledBindings || store.busy) return;
    const matches = store.snapshots.flatMap((snapshot) => snapshot.registration.commands
      .filter((command) => appShortcutMatches(event, command.shortcut))
      .map((command) => ({ snapshot, command })));
    const selected = matches.length === 1 ? matches[0] : undefined;
    if (!selected) return;
    event.preventDefault();
    event.stopPropagation();
    void store.run(selected.snapshot, selected.command).catch(() => {
      if (active) useTipsStore().show({ tone: "error", title: i18n.global.t("pluginAppIntegrations.error") });
    });
  };
  let refreshing = false;
  const refreshBindings = async () => {
    if (!active || refreshing || !store.enabledBindings) return;
    refreshing = true;
    try { await store.refresh(); } catch { /* The next bounded poll retries the projection. */ }
    finally { refreshing = false; }
  };
  const timer = setInterval(() => { void refreshBindings(); }, 3000);
  document.addEventListener("keydown", onKey);
  return () => {
    active = false;
    clearInterval(timer);
    document.removeEventListener("keydown", onKey);
    unlisten();
    stopNotifications();
  };
}
