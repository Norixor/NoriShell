import { afterEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import type { Router } from "vue-router";
import type { PluginAppIntegrationSnapshot } from "../core-api/generated/core-api";
import { listPluginDialogs, registerPluginDialog } from "../plugins/pluginDialogRegistry";
import { appShortcutMatches, startPluginAppNavigation, usePluginAppIntegrationsStore } from "./pluginAppIntegrations";

const eventHarness = vi.hoisted(() => ({
  listeners: new Map<string, (event: { payload: unknown }) => unknown>(),
}));

const owner = {
  pluginId: "com.example.commands",
  packageSha256: "a".repeat(64),
  instanceGeneration: "1",
};

type NavigationPayload = typeof owner & { path: string };

function snapshot(currentOwner = owner): PluginAppIntegrationSnapshot {
  return {
    ...currentOwner,
    pluginName: "Commands",
    registration: { commands: [{ id: "inspect", label: "Inspect", targetId: "app.header.actions", actionId: "inspect", shortcut: "Alt+Shift+P", fileExtensions: [] }], statuses: [] },
    notifications: [],
  } as PluginAppIntegrationSnapshot;
}

function appendUnknownDialog() {
  const dialog = document.createElement("section");
  dialog.setAttribute("role", "dialog");
  document.body.append(dialog);
  return dialog;
}

function appendOwnedDialog(currentOwner = owner, afterClose?: () => void) {
  const dialog = appendUnknownDialog();
  const content = document.createElement("div");
  dialog.append(content);
  let dispose = () => {};
  const close = vi.fn(() => {
    dispose();
    dialog.remove();
    afterClose?.();
  });
  dispose = registerPluginDialog(content, currentOwner, close);
  return { dialog, close, dispose };
}

async function dispatchNavigation(payload: NavigationPayload) {
  const listener = eventHarness.listeners.get("norishell://plugin-app-navigation");
  if (!listener) throw new Error("navigation listener missing");
  await listener({ payload });
}

async function startNavigationTest() {
  setActivePinia(createPinia());
  const store = usePluginAppIntegrationsStore();
  store.snapshots = [snapshot()];
  const refresh = vi.spyOn(store, "refresh").mockResolvedValue();
  const push = vi.fn().mockResolvedValue(undefined);
  const stop = await startPluginAppNavigation({ push } as unknown as Router);
  return { store, refresh, push, stop };
}

describe("plugin app shortcuts", () => {
  it("never consumes terminal, dialog, reserved or composing input", () => {
    const event = new KeyboardEvent("keydown", { code: "KeyP", altKey: true, shiftKey: true });
    expect(appShortcutMatches(event, "Alt+Shift+P")).toBe(true);
    expect(appShortcutMatches(new KeyboardEvent("keydown", { code: "Digit1", metaKey: true }), "Meta+1")).toBe(false);
    expect(appShortcutMatches(new KeyboardEvent("keydown", { code: "KeyP", altKey: true, shiftKey: true, isComposing: true }), "Alt+Shift+P")).toBe(false);
    const dialog = document.createElement("div"); dialog.setAttribute("role", "dialog"); document.body.append(dialog);
    expect(appShortcutMatches(event, "Alt+Shift+P")).toBe(false); dialog.remove();
    const terminal = document.createElement("div"); terminal.className = "xterm"; document.body.append(terminal);
    let matched = true; terminal.addEventListener("keydown", (input) => { matched = appShortcutMatches(input, "Alt+Shift+P"); });
    terminal.dispatchEvent(event); expect(matched).toBe(false); terminal.remove();
  });
});

vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true, invoke: vi.fn().mockResolvedValue([]) }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (eventName: string, listener: (event: { payload: unknown }) => unknown) => {
    eventHarness.listeners.set(eventName, listener);
    return () => {
      if (eventHarness.listeners.get(eventName) === listener) eventHarness.listeners.delete(eventName);
    };
  }),
}));
vi.mock("./pluginExtensions", () => ({ usePluginExtensionsStore: () => ({}) }));

describe("application lifetime plugin bindings", () => {
  afterEach(() => {
    vi.useRealTimers();
    document.body.replaceChildren();
    listPluginDialogs();
    eventHarness.listeners.clear();
    vi.clearAllMocks();
  });

  it("runs without a plugin page mounted and removes the binding on application cleanup", async () => {
    vi.useFakeTimers();
    setActivePinia(createPinia());
    const store = usePluginAppIntegrationsStore();
    const current = snapshot();
    store.snapshots = [current];
    store.enabledBindings = true;
    const run = vi.spyOn(store, "run").mockResolvedValue();
    const stop = await startPluginAppNavigation({ push: vi.fn() } as unknown as Router);
    const key = () => new KeyboardEvent("keydown", { code: "KeyP", altKey: true, shiftKey: true, cancelable: true });
    const event = key();
    document.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
    expect(run).toHaveBeenCalledOnce();
    // Conflicting plugin bindings must not arbitrarily select an action.
    store.snapshots = [current, { ...current, pluginId: "com.example.other" }];
    const conflict = key();
    document.dispatchEvent(conflict);
    expect(conflict.defaultPrevented).toBe(false);
    expect(run).toHaveBeenCalledOnce();
    store.snapshots = [current];
    stop();
    document.dispatchEvent(key());
    expect(run).toHaveBeenCalledOnce();
    expect(vi.getTimerCount()).toBe(0);
  });

  it("navigates for the current owner when no dialog is open", async () => {
    const { refresh, push, stop } = await startNavigationTest();

    await dispatchNavigation({ ...owner, path: "/plugins" });

    expect(refresh).toHaveBeenCalledTimes(2);
    expect(push).toHaveBeenCalledWith("/plugins");
    stop();
  });

  it("closes only the originating registered dialog before navigating", async () => {
    const owned = appendOwnedDialog();
    const { push, stop } = await startNavigationTest();

    await dispatchNavigation({ ...owner, path: "/plugin/com.example.commands/settings" });

    expect(owned.close).toHaveBeenCalledOnce();
    expect(listPluginDialogs()).toEqual([]);
    expect(push).toHaveBeenCalledWith("/plugin/com.example.commands/settings");
    stop();
  });

  it.each(["unknown", "other owner", "nested"])("immediately discards navigation blocked by a %s dialog", async (kind) => {
    const { refresh, push, stop } = await startNavigationTest();
    let dispose = () => {};
    if (kind === "unknown") {
      appendUnknownDialog().setAttribute("data-plugin-protected", "");
    } else if (kind === "other owner") {
      ({ dispose } = appendOwnedDialog({ ...owner, pluginId: "com.example.other" }));
    } else {
      const owned = appendOwnedDialog();
      dispose = owned.dispose;
      const nested = appendUnknownDialog();
      owned.dialog.append(nested);
    }

    await dispatchNavigation({ ...owner, path: "/plugins" });

    expect(refresh).not.toHaveBeenCalled();
    expect(push).not.toHaveBeenCalled();
    dispose();
    stop();
  });

  it("drops navigation when the owner is revoked during the final Core refresh", async () => {
    const owned = appendOwnedDialog();
    const { store, refresh, push, stop } = await startNavigationTest();
    let calls = 0;
    refresh.mockImplementation(async () => {
      calls += 1;
      if (calls === 2) store.snapshots = [];
    });

    await dispatchNavigation({ ...owner, path: "/plugins" });

    expect(owned.close).toHaveBeenCalledOnce();
    expect(push).not.toHaveBeenCalled();
    stop();
  });

  it("drops navigation when its listener stops during a refresh", async () => {
    const { refresh, push, stop } = await startNavigationTest();
    let resolveRefresh: (() => void) | undefined;
    refresh.mockImplementation(() => new Promise<void>((resolve) => { resolveRefresh = resolve; }));
    const listener = eventHarness.listeners.get("norishell://plugin-app-navigation");
    if (!listener) throw new Error("navigation listener missing");

    const pending = Promise.resolve(listener({ payload: { ...owner, path: "/plugins" } }));
    await Promise.resolve();
    stop();
    resolveRefresh?.();
    await pending;

    expect(push).not.toHaveBeenCalled();
  });

  it("drops navigation if a blocker appears during refresh or while closing its own dialog", async () => {
    const { refresh, push, stop } = await startNavigationTest();
    let calls = 0;
    refresh.mockImplementation(async () => {
      calls += 1;
      if (calls === 1) appendUnknownDialog();
    });

    await dispatchNavigation({ ...owner, path: "/plugins" });
    expect(push).not.toHaveBeenCalled();

    document.body.replaceChildren();
    listPluginDialogs();
    refresh.mockReset();
    refresh.mockImplementation(async () => undefined);
    const owned = appendOwnedDialog(owner, () => { appendUnknownDialog(); });
    await dispatchNavigation({ ...owner, path: "/plugins" });

    expect(owned.close).toHaveBeenCalledOnce();
    expect(push).not.toHaveBeenCalled();
    stop();
  });
});
