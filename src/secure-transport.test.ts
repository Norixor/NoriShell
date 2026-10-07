import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const tauri = vi.hoisted(() => ({
  invoke: vi.fn(async () => "desktop"),
  close: vi.fn(async () => undefined),
  onCloseRequested: vi.fn(async () => () => undefined),
  mobile: false,
}));
vi.mock("@tauri-apps/api/core", () => ({
  invoke: tauri.invoke,
  convertFileSrc: (path: string, protocol: string) => `${protocol}://localhost/${path}`,
}));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ close: tauri.close, onCloseRequested: tauri.onCloseRequested }),
}));
vi.mock("./platform-host", () => ({ isMobileLayout: () => tauri.mobile }));

type Listener = (event: MessageEvent) => void;

function fakeChannel() {
  const sent: Record<string, unknown>[] = [];
  let listener: Listener | undefined;
  const channel = {
    postMessage: (text: string) => { sent.push(JSON.parse(text) as Record<string, unknown>); },
    addEventListener: (_type: "message", value: Listener) => { listener = value; },
  };
  const deliver = (message: unknown) => listener?.({ data: JSON.stringify(message) } as MessageEvent);
  return { channel, sent, deliver };
}

describe("secure surface transport", () => {
  beforeEach(() => {
    vi.resetModules();
    tauri.invoke.mockClear();
    tauri.close.mockClear();
    tauri.mobile = false;
  });

  afterEach(() => {
    delete window.__norishellSecure;
    const target = window as unknown as Record<string, unknown>;
    delete target.__TAURI_INTERNALS__;
    delete target.isTauri;
  });

  it("keeps desktop windows on Tauri IPC and window control", async () => {
    const transport = await import("./secure-transport");
    expect(transport.isSecureSurface()).toBe(false);
    await expect(transport.secureInvoke("secure_vault_get", { id: "x" })).resolves.toBe("desktop");
    expect(tauri.invoke).toHaveBeenCalledWith("secure_vault_get", { id: "x" });
    await transport.closeSecureWindow();
    expect(tauri.close).toHaveBeenCalledOnce();
    expect(transport.isolatedDocumentUrl("token")).toBe("norishell-plugin://localhost/token");
  });

  it("ignores an injected channel outside the Android layout", async () => {
    window.__norishellSecure = fakeChannel().channel;
    const transport = await import("./secure-transport");
    expect(transport.isSecureSurface()).toBe(false);
  });

  it("routes Android commands through the surface channel and settles by request id", async () => {
    tauri.mobile = true;
    const { channel, sent, deliver } = fakeChannel();
    window.__norishellSecure = channel;
    const transport = await import("./secure-transport");
    const first = transport.secureInvoke("secure_vault_get", { id: "a" });
    const second = transport.secureInvoke("secure_vault_cancel", { id: "a" });
    expect(sent[0]).toEqual({ type: "ready" });
    expect(sent[1]).toMatchObject({ type: "invoke", id: 1, cmd: "secure_vault_get" });
    expect(JSON.parse(sent[1]?.args as string)).toEqual({ id: "a" });
    deliver({ type: "response", id: 2, result: { ok: false, error: { code: "secureVaultExpired" } } });
    deliver({ type: "response", id: 1, result: { ok: true, value: { id: "a", kind: "unlock" } } });
    await expect(first).resolves.toEqual({ id: "a", kind: "unlock" });
    await expect(second).rejects.toEqual({ code: "secureVaultExpired" });
    expect(tauri.invoke).not.toHaveBeenCalled();
  });

  it("runs the page close path on back and closes by default", async () => {
    tauri.mobile = true;
    const { channel, sent, deliver } = fakeChannel();
    window.__norishellSecure = channel;
    const transport = await import("./secure-transport");
    const handler = vi.fn();
    const dispose = await transport.onSecureWindowCloseRequested(handler);
    deliver({ type: "closeRequested" });
    expect(handler).toHaveBeenCalledOnce();
    expect(sent.some((message) => message.type === "close")).toBe(false);
    dispose();
    deliver({ type: "closeRequested" });
    expect(sent.at(-1)).toEqual({ type: "close" });
  });

  it("runs the same close path for the app bar back button as for the back key", async () => {
    tauri.mobile = true;
    const { channel, sent } = fakeChannel();
    window.__norishellSecure = channel;
    const transport = await import("./secure-transport");
    const handler = vi.fn();
    const dispose = await transport.onSecureWindowCloseRequested(handler);
    transport.requestSecureWindowClose();
    expect(handler).toHaveBeenCalledOnce();
    expect(sent.some((message) => message.type === "close")).toBe(false);
    dispose();
    transport.requestSecureWindowClose();
    expect(sent.at(-1)).toEqual({ type: "close" });
  });

  it("lets shared Tauri API callers listen and invoke inside a surface", async () => {
    tauri.mobile = true;
    const { channel, sent, deliver } = fakeChannel();
    window.__norishellSecure = channel;
    const transport = await import("./secure-transport");
    transport.installSecureSurfaceTransport();
    const internals = (window as unknown as { __TAURI_INTERNALS__: {
      invoke(command: string, args?: Record<string, unknown>): Promise<unknown>;
      transformCallback(callback: (payload: unknown) => void): number;
    } }).__TAURI_INTERNALS__;
    const received = vi.fn();
    const eventId = await internals.invoke("plugin:event|listen", {
      event: "tool-window-exit-requested", target: { kind: "Any" }, handler: internals.transformCallback(received),
    });
    deliver({ type: "event", event: "tool-window-exit-requested", payload: { attemptId: "1" } });
    expect(received).toHaveBeenCalledWith({ event: "tool-window-exit-requested", id: eventId, payload: { attemptId: "1" } });
    await internals.invoke("plugin:event|unlisten", { event: "tool-window-exit-requested", eventId });
    deliver({ type: "event", event: "tool-window-exit-requested", payload: {} });
    expect(received).toHaveBeenCalledOnce();
    await expect(internals.invoke("plugin:webview|set_webview_zoom", {})).rejects.toBe("secure_surface.unsupported");
    void internals.invoke("host_list", { request: { meta: { requestId: "r" } } });
    expect(sent.at(-1)).toMatchObject({ type: "invoke", cmd: "host_list" });
    expect(transport.isolatedDocumentUrl("token")).toBe("https://norishell-plugin.localhost/token");
  });
});
