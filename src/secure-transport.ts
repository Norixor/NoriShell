import { convertFileSrc, invoke, type InvokeArgs } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

import { isMobileLayout } from "./platform-host";

/**
 * Transport for Core-owned Secure Surfaces (secure-*.html, tool-window.html,
 * plugin-isolated.html).
 *
 * Desktop: each surface is a native Tauri window, so this is plain Tauri IPC and
 * window control. Android: the surface is a plain WebView hosted by Kotlin with
 * no Tauri IPC; the only channel is the `__norishellSecure` web message object
 * that Kotlin injects for the secure origin. Kotlin binds every message to the
 * surface that sent it, so a page can never name another surface.
 */

interface SecureChannel {
  postMessage(message: string): void;
  addEventListener(type: "message", listener: (event: MessageEvent) => void): void;
}

type SecureMessage =
  | { type: "response"; id: number; result?: { ok?: boolean; value?: unknown; error?: unknown } }
  | { type: "event"; event: string; payload: unknown }
  | { type: "closeRequested" };

declare global {
  interface Window {
    __norishellSecure?: SecureChannel;
  }
}

const SECURE_ORIGIN = "https://secure.norishell.localhost";
const PLUGIN_DOCUMENT_ORIGIN = "https://norishell-plugin.localhost";

let connected = false;
let nextRequestId = 1;
const pendingRequests = new Map<number, { resolve: (value: unknown) => void; reject: (error: unknown) => void }>();
const eventListeners = new Map<string, Set<(payload: unknown) => void>>();
const closeRequestHandlers = new Set<() => void>();

function surfaceChannel(): SecureChannel | undefined {
  if (typeof window === "undefined" || !isMobileLayout()) return undefined;
  return window.__norishellSecure;
}

/** True only inside an Android Secure Surface. */
export function isSecureSurface(): boolean {
  return surfaceChannel() !== undefined;
}

function connect(channel: SecureChannel): SecureChannel {
  if (connected) return channel;
  connected = true;
  channel.addEventListener("message", (event) => {
    if (typeof event.data === "string") receive(event.data);
  });
  // Kotlin queues Core events and close requests until the page is ready.
  channel.postMessage(JSON.stringify({ type: "ready" }));
  return channel;
}

function receive(text: string) {
  let message: SecureMessage;
  try {
    message = JSON.parse(text) as SecureMessage;
  } catch {
    return;
  }
  if (message.type === "response") {
    const pending = pendingRequests.get(message.id);
    if (!pending) return;
    pendingRequests.delete(message.id);
    if (message.result?.ok === true) pending.resolve(message.result.value);
    else pending.reject(message.result?.error ?? "secure_surface.unavailable");
  } else if (message.type === "event") {
    for (const listener of eventListeners.get(message.event) ?? []) listener(message.payload);
  } else if (message.type === "closeRequested") {
    requestSecureWindowClose();
  }
}

/**
 * Asks this surface to close through the page's own cancel path, exactly as the
 * Android back key does: registered close handlers decide, and a page without one
 * closes directly. The phone app bar's back button uses this so both affordances
 * share one path.
 */
export function requestSecureWindowClose(): void {
  if (closeRequestHandlers.size === 0) {
    void closeSecureWindow();
    return;
  }
  for (const handler of closeRequestHandlers) handler();
}

/** Calls a Core command for this surface; the surface identity is bound by the host, never the page. */
export function secureInvoke<T>(command: string, args: InvokeArgs = {}): Promise<T> {
  const channel = surfaceChannel();
  if (!channel) return invoke<T>(command, args);
  connect(channel);
  return new Promise<T>((resolve, reject) => {
    const id = nextRequestId++;
    pendingRequests.set(id, { resolve: resolve as (value: unknown) => void, reject });
    // Arguments travel pre-serialized so Kotlin forwards them byte for byte.
    channel.postMessage(JSON.stringify({ type: "invoke", id, cmd: command, args: JSON.stringify(args) }));
  });
}

/** Closes this surface; Core cancels anything still pending, exactly like a window close. */
export async function closeSecureWindow(): Promise<void> {
  const channel = surfaceChannel();
  if (!channel) return getCurrentWindow().close();
  connect(channel).postMessage(JSON.stringify({ type: "close" }));
}

/**
 * Runs `handler` instead of closing when the user asks to close the surface
 * (desktop window close button, Android back key). The handler decides whether
 * to call {@link closeSecureWindow}.
 */
export async function onSecureWindowCloseRequested(handler: () => void): Promise<() => void> {
  const channel = surfaceChannel();
  if (!channel) {
    return getCurrentWindow().onCloseRequested((event) => {
      event.preventDefault();
      handler();
    });
  }
  connect(channel);
  closeRequestHandlers.add(handler);
  return () => { closeRequestHandlers.delete(handler); };
}

/** URL of a plugin guest document for the sandboxed iframe of this surface. */
export function isolatedDocumentUrl(documentToken: string): string {
  if (!isSecureSurface()) return convertFileSrc(documentToken, "norishell-plugin");
  return `${PLUGIN_DOCUMENT_ORIGIN}/${encodeURIComponent(documentToken)}`;
}

/** Android: non-secret appearance Core appended to the surface URL (other origin than the main WebView). */
export function secureSurfaceAppearance(): { locale?: string; themePreference?: string } | null {
  if (!isSecureSurface()) return null;
  const query = new URLSearchParams(window.location.search);
  return { locale: query.get("locale") ?? undefined, themePreference: query.get("theme") ?? undefined };
}

interface TauriInternalsShim {
  invoke(command: string, args?: InvokeArgs): Promise<unknown>;
  transformCallback(callback?: (payload: unknown) => void, once?: boolean): number;
  unregisterCallback(id: number): void;
  convertFileSrc(path: string, protocol?: string): string;
  metadata: { currentWindow: { label: string }; currentWebview: { windowLabel: string; label: string } };
}

/**
 * Android only: lets shared modules that call `@tauri-apps/api` (the Host
 * editor's Core client, `listen`) run inside a surface. Core commands go through
 * {@link secureInvoke} and stay subject to both whitelists; window and WebView
 * chrome commands have no surface equivalent and are rejected locally.
 */
export function installSecureSurfaceTransport(): void {
  const channel = surfaceChannel();
  if (!channel) return;
  connect(channel);
  const callbacks = new Map<number, (payload: unknown) => void>();
  const listeners = new Map<number, { event: string; listener: (payload: unknown) => void }>();
  let nextCallbackId = 1;
  let nextListenerId = 1;
  const removeListener = (eventId: number) => {
    const entry = listeners.get(eventId);
    if (!entry) return;
    listeners.delete(eventId);
    eventListeners.get(entry.event)?.delete(entry.listener);
  };
  const shim: TauriInternalsShim = {
    invoke(command, args = {}) {
      const values = args as Record<string, unknown>;
      if (command === "plugin:event|listen") {
        const event = String(values.event);
        const handler = callbacks.get(Number(values.handler));
        const eventId = nextListenerId++;
        const listener = (payload: unknown) => handler?.({ event, id: eventId, payload });
        listeners.set(eventId, { event, listener });
        if (!eventListeners.has(event)) eventListeners.set(event, new Set());
        eventListeners.get(event)?.add(listener);
        return Promise.resolve(eventId);
      }
      if (command === "plugin:event|unlisten") {
        removeListener(Number(values.eventId));
        return Promise.resolve();
      }
      if (command === "plugin:window|close" || command === "plugin:window|destroy") return closeSecureWindow();
      if (command.startsWith("plugin:") || command === "window_renderer_ready" || command === "window_standalone_action") {
        return Promise.reject("secure_surface.unsupported");
      }
      return secureInvoke(command, args);
    },
    transformCallback(callback, once = false) {
      const id = nextCallbackId++;
      callbacks.set(id, (payload) => {
        if (once) callbacks.delete(id);
        callback?.(payload);
      });
      return id;
    },
    unregisterCallback(id) { callbacks.delete(id); },
    convertFileSrc(path, protocol = "asset") {
      return protocol === "norishell-plugin"
        ? `${PLUGIN_DOCUMENT_ORIGIN}/${encodeURIComponent(path)}`
        : `${SECURE_ORIGIN}/${encodeURIComponent(path)}`;
    },
    metadata: {
      currentWindow: { label: "secure-surface" },
      currentWebview: { windowLabel: "secure-surface", label: "secure-surface" },
    },
  };
  const target = window as unknown as Record<string, unknown>;
  target.__TAURI_INTERNALS__ = shim;
  target.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: (_event: string, eventId: number) => removeListener(eventId) };
  target.isTauri = true;
}
