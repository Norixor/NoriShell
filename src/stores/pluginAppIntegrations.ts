import { invoke, isTauri } from "@tauri-apps/api/core";
import { defineStore } from "pinia";
import { nextTick, ref, watch } from "vue";
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

function startPluginAppShortcutListener(store: ReturnType<typeof usePluginAppIntegrationsStore>): () => void {
  let active = true;
  const onKey = (event: KeyboardEvent) => {
    if (!active || event.defaultPrevented || !store.enabledBindings || store.busy) return;
    const matches = store.snapshots.flatMap((snapshot) => snapshot.registration.commands
      .filter((command) => appShortcutMatches(event, command.shortcut))
      .map((command) => ({ snapshot, command })));
    const selected = matches.length === 1 ? matches[0] : undefined;
    if (!selected) return;
    event.preventDefault();
    event.stopImmediatePropagation();
    void store.run(selected.snapshot, selected.command).catch(() => {
      if (active) useTipsStore().show({ tone: "error", title: i18n.global.t("pluginAppIntegrations.error") });
    });
  };
  let refreshing = false;
  const refreshBindings = async () => {
    if (!active || refreshing || !store.enabledBindings) return;
    refreshing = true;
    try { await store.refresh(); } catch { /* The next focused poll retries the projection. */ }
    finally { refreshing = false; }
  };
  // Core emits no change event for registrations, so only the focused WebView polls: shortcuts can only reach the
  // document that holds keyboard focus, and regaining focus refreshes immediately.
  let timer: ReturnType<typeof setInterval> | undefined;
  const stopPolling = () => {
    if (timer !== undefined) clearInterval(timer);
    timer = undefined;
  };
  const startPolling = () => {
    if (!active) return;
    timer ??= setInterval(() => { void refreshBindings(); }, 3000);
    void refreshBindings();
  };
  document.addEventListener("keydown", onKey);
  window.addEventListener("focus", startPolling);
  window.addEventListener("blur", stopPolling);
  if (document.hasFocus()) startPolling();
  return () => {
    active = false;
    stopPolling();
    document.removeEventListener("keydown", onKey);
    window.removeEventListener("focus", startPolling);
    window.removeEventListener("blur", stopPolling);
  };
}

/**
 * A child WebView has its own Pinia store. The opt-in is session state of the main window only, so the child asks for
 * it once and then follows the main window's change events before using shortcuts.
 */
export async function startPluginAppShortcuts(): Promise<() => void> {
  if (!isTauri()) return () => {};
  const { emit, listen } = await import("@tauri-apps/api/event");
  const store = usePluginAppIntegrationsStore();
  const stopShortcuts = startPluginAppShortcutListener(store);
  let active = true;
  try {
    const unlisten = await listen<{ enabled: boolean }>("norishell://plugin-app-bindings-changed", ({ payload }) => {
      if (!active || typeof payload?.enabled !== "boolean") return;
      store.enabledBindings = payload.enabled;
      if (payload.enabled) void store.refresh().catch(() => undefined);
    });
    await emit("norishell://plugin-app-bindings-request").catch(() => undefined);
    return () => { active = false; unlisten(); stopShortcuts(); };
  } catch (error) {
    active = false;
    stopShortcuts();
    throw error;
  }
}

/** Main-window integration; the Core event carries an exact current plugin fence. */
export async function startPluginAppNavigation(router: import("vue-router").Router): Promise<() => void> {
  if (!isTauri()) return () => {};
  const { emit, listen } = await import("@tauri-apps/api/event");
  const store = usePluginAppIntegrationsStore();
  let active = true;
  const stopShortcuts = startPluginAppShortcutListener(store);
  const stopBindingWatch = watch(() => store.enabledBindings, (enabled) => {
    if (active) void emit("norishell://plugin-app-bindings-changed", { enabled }).catch(() => undefined);
  }, { immediate: true });
  const stopBindingRequests = await listen("norishell://plugin-app-bindings-request", () => {
    if (active) void emit("norishell://plugin-app-bindings-changed", { enabled: store.enabledBindings }).catch(() => undefined);
  });
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
  return () => {
    active = false;
    stopBindingWatch();
    stopBindingRequests();
    stopShortcuts();
    unlisten();
    stopNotifications();
  };
}
