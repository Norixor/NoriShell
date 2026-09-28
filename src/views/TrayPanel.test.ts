import { mount, flushPromises } from "@vue/test-utils";
import { beforeEach, afterEach, describe, expect, it, vi } from "vitest";
import { i18n } from "../locales";
import TrayPanel from "./TrayPanel.vue";

const api = vi.hoisted(() => ({
  readTrayPanel: vi.fn(),
  executeTrayPanel: vi.fn(),
  hideTrayPanel: vi.fn(),
  TRAY_PANEL_VISIBILITY_EVENT: "tray-panel-visibility",
}));
vi.mock("../core-api/tray-panel", () => api);
const events = vi.hoisted(() => ({
  handler: null as null | ((event: { payload: { sequence: number; visible: boolean } }) => void),
  unlisten: vi.fn(),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (name: string, handler: typeof events.handler) => {
    if (name === "tray-panel-visibility") events.handler = handler;
    return events.unlisten;
  }),
}));
const reveal = vi.hoisted(() => ({ revealWindowAfterMount: vi.fn(async () => undefined) }));
vi.mock("../window-first-show", () => reveal);
const visibility = (sequence: number, visible: boolean) => events.handler!({ payload: { sequence, visible } });

const snapshot = {
  locale: "en",
  stats: [{ kind: "sessions", count: 1 }, { kind: "tunnels", count: null }],
  notificationState: "active",
  rows: [
    { id: "show-once", label: "Show NoriShell", kind: "show", children: [] },
    { id: "local-once", label: "Local terminal", kind: "newLocalTerminal", children: [] },
    { id: null, label: "1 terminal", kind: "status", children: [] },
    { id: null, label: "Sessions", role: "sessions", kind: "group", children: [{ id: "session-once", label: "Local Shell", kind: "action", children: [] }] },
  ],
};

describe("TrayPanel", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.clearAllMocks();
    events.handler = null;
    vi.spyOn(document, "hasFocus").mockReturnValue(true);
    api.readTrayPanel.mockResolvedValue(snapshot);
    api.executeTrayPanel.mockResolvedValue(undefined);
    api.hideTrayPanel.mockResolvedValue(undefined);
  });
  afterEach(() => { vi.restoreAllMocks(); vi.useRealTimers(); });

  it("reads status without opening the app and sends only the clicked opaque token", async () => {
    const wrapper = mount(TrayPanel, { global: { plugins: [i18n] } });
    await flushPromises();
    expect(wrapper.text()).toContain("1 terminal");
    expect(api.executeTrayPanel).not.toHaveBeenCalled();
    await wrapper.findAll("button").find((button) => button.text() === "Local shell")!.trigger("click");
    await flushPromises();
    expect(api.executeTrayPanel).toHaveBeenCalledExactlyOnceWith("local-once");
    wrapper.unmount();
  });

  it("clears private labels on blur and ignores a late snapshot", async () => {
    let resolve!: (value: typeof snapshot) => void;
    api.readTrayPanel.mockReturnValueOnce(new Promise((done) => { resolve = done; }));
    const wrapper = mount(TrayPanel, { global: { plugins: [i18n] } });
    await flushPromises();
    expect(api.readTrayPanel).toHaveBeenCalledOnce();
    window.dispatchEvent(new Event("blur"));
    resolve(snapshot);
    await flushPromises();
    expect(wrapper.text()).not.toContain("Local Shell");
    expect(wrapper.find("nav").exists()).toBe(false);
    wrapper.unmount();
  });

  it("does not rebind an existing resource button to another snapshot target", async () => {
    const wrapper = mount(TrayPanel, { global: { plugins: [i18n] } });
    await flushPromises();
    await wrapper.get("nav button").trigger("click");
    const previous = wrapper.findAll("button").find((button) => button.text() === "Local Shell")!.element;
    api.readTrayPanel.mockResolvedValue({
      locale: "en",
      rows: [{ id: null, label: "Sessions", role: "sessions", kind: "group", children: [{ id: "another-session", label: "Other Shell", kind: "action", children: [] }] }],
    });
    await vi.advanceTimersByTimeAsync(2_000);
    await flushPromises();
    const current = wrapper.findAll("button").find((button) => button.text() === "Other Shell")!.element;
    expect(current).not.toBe(previous);
    expect(api.executeTrayPanel).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("expands resource controls locally, distinguishes unknown counts, and resets disclosure on blur", async () => {
    const wrapper = mount(TrayPanel, { global: { plugins: [i18n] } });
    await flushPromises();
    expect(wrapper.find('[aria-label="Unavailable"]').text()).toBe("—");
    expect(wrapper.text()).not.toContain("Local Shell");
    await wrapper.get("nav button").trigger("click");
    expect(wrapper.text()).toContain("Local Shell");
    expect(api.executeTrayPanel).not.toHaveBeenCalled();
    window.dispatchEvent(new Event("blur"));
    window.dispatchEvent(new Event("focus"));
    await flushPromises();
    expect(wrapper.text()).not.toContain("Local Shell");
    expect(wrapper.get("nav button").attributes("aria-expanded")).toBe("false");
    wrapper.unmount();
  });

  it("opens notification choices without changing policy and executes only the selected pause token", async () => {
    api.readTrayPanel.mockResolvedValue({
      ...snapshot,
      rows: [{ id: null, role: "notifications", kind: "group", label: "Pause notifications", children: [
        { id: "pause-once", role: null, kind: "action", label: "Pause 30 minutes", children: [] },
      ] }],
    });
    const wrapper = mount(TrayPanel, { global: { plugins: [i18n] } });
    await flushPromises();
    expect(wrapper.text()).toContain("Receiving");
    await wrapper.findAll("button").find((button) => button.text().includes("Notifications"))!.trigger("click");
    expect(api.executeTrayPanel).not.toHaveBeenCalled();
    await wrapper.findAll("button").find((button) => button.text() === "Pause 30 minutes")!.trigger("click");
    expect(api.executeTrayPanel).toHaveBeenCalledExactlyOnceWith("pause-once");
    wrapper.unmount();
  });

  it("prevents duplicate actions, removes failed tokens and dismisses with Escape", async () => {
    let reject!: (error: Error) => void;
    api.executeTrayPanel.mockReturnValueOnce(new Promise((_done, fail) => { reject = fail; }));
    const wrapper = mount(TrayPanel, { global: { plugins: [i18n] } });
    await flushPromises();
    const button = wrapper.findAll("button").find((item) => item.text() === "Local shell")!;
    await button.trigger("click");
    await button.trigger("click");
    expect(api.executeTrayPanel).toHaveBeenCalledOnce();
    reject(new Error("stale token"));
    await flushPromises();
    expect(wrapper.text()).not.toContain("Local Shell");
    expect(wrapper.get('[role="alert"]').text()).toContain("did not complete");
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    await flushPromises();
    expect(api.hideTrayPanel).toHaveBeenCalledOnce();
    wrapper.unmount();
  });

  it("reopens from the Core visibility event even when the re-shown WebView never gains DOM focus", async () => {
    const wrapper = mount(TrayPanel, { global: { plugins: [i18n] } });
    await flushPromises();
    expect(reveal.revealWindowAfterMount).toHaveBeenCalledOnce();
    // Dismissing with the close button leaves it as document.activeElement across hide/show.
    (wrapper.get(".tray-panel__close").element as HTMLButtonElement).focus();
    window.dispatchEvent(new Event("blur"));
    visibility(1, false);
    await flushPromises();
    expect(wrapper.text()).toContain("Reading status");
    vi.mocked(document.hasFocus).mockReturnValue(false);
    api.readTrayPanel.mockClear();
    visibility(2, true);
    await flushPromises();
    expect(api.readTrayPanel).toHaveBeenCalledOnce();
    expect(wrapper.text()).toContain("1 terminal");
    expect(wrapper.text()).not.toContain("Reading status");
    wrapper.unmount();
    expect(events.unlisten).toHaveBeenCalledOnce();
  });

  it("rereads after a spurious blur discards the in-flight read of a still-open panel", async () => {
    const wrapper = mount(TrayPanel, { global: { plugins: [i18n] } });
    await flushPromises();
    let resolve!: (value: typeof snapshot) => void;
    api.readTrayPanel.mockReturnValueOnce(new Promise((done) => { resolve = done; }));
    visibility(1, true);
    window.dispatchEvent(new Event("blur"));
    resolve(snapshot);
    await flushPromises();
    expect(wrapper.text()).toContain("Reading status");
    await vi.advanceTimersByTimeAsync(2_000);
    await flushPromises();
    expect(wrapper.text()).toContain("1 terminal");
    wrapper.unmount();
  });

  it("ignores reordered visibility notifications", async () => {
    const wrapper = mount(TrayPanel, { global: { plugins: [i18n] } });
    await flushPromises();
    visibility(3, true);
    await flushPromises();
    visibility(2, false);
    await flushPromises();
    expect(wrapper.text()).toContain("1 terminal");
    wrapper.unmount();
  });

  it("waits silently while a prewarmed panel is hidden and reads when Core shows it", async () => {
    api.readTrayPanel.mockRejectedValueOnce({ code: "tray.panel_hidden" });
    const wrapper = mount(TrayPanel, { global: { plugins: [i18n] } });
    await flushPromises();
    expect(wrapper.find('[role="alert"]').exists()).toBe(false);
    await vi.advanceTimersByTimeAsync(6_000);
    expect(api.readTrayPanel).toHaveBeenCalledOnce();
    visibility(1, true);
    await flushPromises();
    expect(wrapper.text()).toContain("1 terminal");
    wrapper.unmount();
  });

  it("replaces an unanswered read with a timeout code instead of loading forever", async () => {
    api.readTrayPanel.mockReturnValueOnce(new Promise(() => undefined));
    const wrapper = mount(TrayPanel, { global: { plugins: [i18n] } });
    await flushPromises();
    expect(wrapper.text()).toContain("Reading status");
    await vi.advanceTimersByTimeAsync(5_000);
    await flushPromises();
    expect(wrapper.get('[role="alert"]').text()).toContain("tray.panel_read_timeout");
    await wrapper.get('[role="alert"] button').trigger("click");
    await flushPromises();
    expect(wrapper.text()).toContain("1 terminal");
    wrapper.unmount();
  });

  it("shows the Core refusal code for a failed read", async () => {
    api.readTrayPanel.mockRejectedValueOnce({ code: "tray.panel_binding_limit" });
    const wrapper = mount(TrayPanel, { global: { plugins: [i18n] } });
    await flushPromises();
    expect(wrapper.get('[role="alert"]').text()).toContain("Status is temporarily unavailable.");
    expect(wrapper.get('[role="alert"]').text()).toContain("tray.panel_binding_limit");
    wrapper.unmount();
  });
});
