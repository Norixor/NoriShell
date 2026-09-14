import { mount, flushPromises } from "@vue/test-utils";
import { beforeEach, afterEach, describe, expect, it, vi } from "vitest";
import { i18n } from "../locales";
import TrayPanel from "./TrayPanel.vue";

const api = vi.hoisted(() => ({ readTrayPanel: vi.fn(), executeTrayPanel: vi.fn(), hideTrayPanel: vi.fn() }));
vi.mock("../core-api/tray-panel", () => api);

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
});
