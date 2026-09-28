import { mount, flushPromises } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createI18n } from "vue-i18n";
import NvxDesktopCanvas from "./NvxDesktopCanvas.vue";
import { frameV2 } from "./input.test";
import type { DesktopSessionSummary } from "../../core-api/generated/core-api";
const mocks = vi.hoisted(() => ({ resolution: vi.fn().mockResolvedValue(undefined), prepare: vi.fn(), frame: vi.fn(), focus: vi.fn().mockResolvedValue("1"), input: vi.fn().mockResolvedValue(undefined) }));
vi.mock("../../plugins/hostDomBroker", () => ({ preparePluginProtectedMount: mocks.prepare }));
vi.mock("../../core-api/desktop-client", () => ({ desktopClient: mocks }));
const session = (id: string) => ({ id, generation: "1", state: "running", width: 1280, height: 800, profile: { protocol: "rdp", rdpResolutionMode: "fixed" } }) as unknown as DesktopSessionSummary;
/** Full frame (base 0) or single-rect patch in the version 2 wire format. */
function frame(sequence: bigint, base: bigint, width: number, height: number, x = 0, y = 0, rectWidth = width, rectHeight = height) {
  return frameV2({ frameSequence: sequence, frame: { base, width, height, rects: [[x, y, rectWidth, rectHeight]] } });
}
function context2d(extra: Partial<CanvasRenderingContext2D> = {}) {
  const put = vi.fn();
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue({ putImageData: put, clearRect: vi.fn(), drawImage: vi.fn(), ...extra } as unknown as CanvasRenderingContext2D);
  return put;
}
function fixture(props = {}) { return mount(NvxDesktopCanvas, { props: { session: session("a"), active: true, fit: true, panning: false, ...props }, global: { plugins: [createI18n({ legacy: false, locale: "en", messages: { en: { desktop: { focus: "Focus", panFocus: "Pan" } } } })] } }); }
/** Gives the canvas a 100 × 100 remote size mapped onto a 100 × 100 CSS box so pointer coordinates are predictable. */
function sizeCanvas(canvas: HTMLCanvasElement) {
  Object.defineProperty(canvas, "width", { configurable: true, get: () => 100, set: () => undefined });
  Object.defineProperty(canvas, "height", { configurable: true, get: () => 100, set: () => undefined });
  canvas.getBoundingClientRect = () => ({ left: 0, top: 0, width: 100, height: 100, right: 100, bottom: 100, x: 0, y: 0, toJSON: () => undefined });
  Object.assign(canvas, { setPointerCapture: vi.fn(), hasPointerCapture: vi.fn().mockReturnValue(false), releasePointerCapture: vi.fn() });
}
beforeEach(() => {
  vi.useFakeTimers();
  vi.clearAllMocks();
  vi.spyOn(document, "hasFocus").mockReturnValue(true);
  vi.spyOn(document, "hidden", "get").mockReturnValue(false);
  mocks.frame.mockResolvedValue(new ArrayBuffer(0));
  // Animation frames run on the fake clock so painting cadence is observable.
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => setTimeout(() => callback(performance.now()), 16));
  vi.stubGlobal("cancelAnimationFrame", (handle: number) => clearTimeout(handle));
  vi.stubGlobal("ImageData", class { constructor(readonly rgba: Uint8ClampedArray, readonly width: number, readonly height: number) {} });
});
afterEach(() => { vi.restoreAllMocks(); vi.unstubAllGlobals(); vi.useRealTimers(); });
describe("protected desktop display", () => {
  it("prepares an empty protected shell before rendering canvas", async () => {
    mocks.prepare.mockImplementation((root: Element) => { expect(root.hasAttribute("data-plugin-protected")).toBe(true); expect(root.querySelector("canvas")).toBeNull(); });
    const wrapper = fixture(); await flushPromises(); expect(mocks.prepare).toHaveBeenCalledOnce(); expect(wrapper.find("canvas").exists()).toBe(true); wrapper.unmount();
  });
  it("keeps a single frame request in flight and discards a previous session reply", async () => {
    let resolve!: (buffer: ArrayBuffer) => void;
    mocks.frame.mockReturnValueOnce(new Promise<ArrayBuffer>((done) => { resolve = done; }));
    const put = context2d();
    const wrapper = fixture(); await flushPromises(); await vi.advanceTimersByTimeAsync(200); expect(mocks.frame).toHaveBeenCalledTimes(1);
    await wrapper.setProps({ session: session("b") });
    resolve(frame(1n, 0n, 1, 1));
    await flushPromises(); expect(put).not.toHaveBeenCalled();
    await wrapper.setProps({ active: false }); await vi.advanceTimersByTimeAsync(100); expect(mocks.frame).toHaveBeenCalledTimes(1); wrapper.unmount();
  });
  it("stops frame requests while inactive or hidden and resumes on activation", async () => {
    const hidden = vi.spyOn(document, "hidden", "get");
    const wrapper = fixture(); await flushPromises();
    expect(mocks.frame).toHaveBeenCalledTimes(1);

    await wrapper.setProps({ active: false });
    expect(vi.getTimerCount()).toBe(0);
    await vi.advanceTimersByTimeAsync(160);
    expect(mocks.frame).toHaveBeenCalledTimes(1);
    await wrapper.setProps({ active: true }); await flushPromises();
    expect(mocks.frame).toHaveBeenCalledTimes(2);

    hidden.mockReturnValue(true);
    document.dispatchEvent(new Event("visibilitychange"));
    expect(vi.getTimerCount()).toBe(0);
    await vi.advanceTimersByTimeAsync(160);
    expect(mocks.frame).toHaveBeenCalledTimes(2);
    hidden.mockReturnValue(false);
    document.dispatchEvent(new Event("visibilitychange"));
    await flushPromises();
    expect(mocks.frame).toHaveBeenCalledTimes(3);
    wrapper.unmount();
  });
  it("only pulls frames while the session is running", async () => {
    const wrapper = fixture({ session: { ...session("a"), state: "connecting" } }); await flushPromises();
    await vi.advanceTimersByTimeAsync(300);
    expect(mocks.frame).not.toHaveBeenCalled();
    await wrapper.setProps({ session: session("a") }); await flushPromises();
    expect(mocks.frame).toHaveBeenCalledTimes(1);
    await wrapper.setProps({ session: { ...session("a"), state: "closed" } });
    expect(vi.getTimerCount()).toBe(0);
    await vi.advanceTimersByTimeAsync(300);
    expect(mocks.frame).toHaveBeenCalledTimes(1);
    wrapper.unmount();
  });
  it("paints every patch rect, waits an animation frame between painted replies and advances only delivered sequences", async () => {
    const put = context2d();
    mocks.frame
      .mockResolvedValueOnce(frameV2({ frameSequence: 1n, cursorSequence: 0n, frame: { base: 0n, width: 2, height: 2 } }))
      .mockResolvedValueOnce(frameV2({ frameSequence: 3n, cursorSequence: 9n, frame: { base: 1n, width: 2, height: 2, rects: [[1, 1, 1, 1], [0, 0, 1, 1]] } }))
      .mockResolvedValueOnce(frameV2({ frameSequence: 5n, cursorSequence: 9n, cursor: { kind: 1 } }));
    const wrapper = fixture(); await flushPromises();
    const canvas = wrapper.find("canvas").element as HTMLCanvasElement;
    const setWidth = vi.fn(), setHeight = vi.fn();
    Object.defineProperty(canvas, "width", { configurable: true, get: () => 2, set: setWidth });
    Object.defineProperty(canvas, "height", { configurable: true, get: () => 2, set: setHeight });
    expect(put).toHaveBeenCalledTimes(1);
    expect(mocks.frame).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(15);
    expect(mocks.frame).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(1); await flushPromises();
    expect(mocks.frame).toHaveBeenCalledTimes(2);
    expect(mocks.frame.mock.calls[1]?.slice(1)).toEqual(["1", "0"]);
    expect(put).toHaveBeenCalledTimes(3);
    expect(put.mock.calls[1]?.slice(1)).toEqual([1, 1]);
    expect(put.mock.calls[2]?.slice(1)).toEqual([0, 0]);
    expect(setWidth).not.toHaveBeenCalled(); expect(setHeight).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(16); await flushPromises();
    expect(mocks.frame.mock.calls[2]?.slice(1)).toEqual(["3", "0"]);
    await vi.advanceTimersByTimeAsync(16); await flushPromises();
    // The cursor-only reply advances the cursor sequence; the frame header value alone never moves afterSequence.
    expect(mocks.frame.mock.calls[3]?.slice(1)).toEqual(["3", "9"]);
    expect(canvas.style.cursor).toBe("none");
    wrapper.unmount();
  });
  it("re-requests at once after an empty long-poll reply and backs off when Core answered immediately", async () => {
    vi.spyOn(performance, "now").mockReturnValueOnce(0).mockReturnValueOnce(300).mockReturnValue(1000);
    const wrapper = fixture(); await flushPromises();
    expect(mocks.frame).toHaveBeenCalledTimes(2);
    await vi.advanceTimersByTimeAsync(49);
    expect(mocks.frame).toHaveBeenCalledTimes(2);
    await vi.advanceTimersByTimeAsync(1); await flushPromises();
    expect(mocks.frame).toHaveBeenCalledTimes(3);
    wrapper.unmount();
  });
  it("retries a pending Tab ownership error quietly and reports frame failures", async () => {
    mocks.frame.mockRejectedValueOnce({ code: "workspace_tab.wrong_owner", messageKey: "workspace_tab.wrong_owner" }).mockRejectedValueOnce(new Error("boom"));
    const wrapper = fixture(); await flushPromises();
    expect(wrapper.emitted("frameError")).toBeUndefined();
    await vi.advanceTimersByTimeAsync(249);
    expect(mocks.frame).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(1); await flushPromises();
    expect(mocks.frame).toHaveBeenCalledTimes(2);
    expect(wrapper.emitted("frameError")).toHaveLength(1);
    expect(wrapper.emitted("error")).toBeUndefined();
    await vi.advanceTimersByTimeAsync(250); await flushPromises();
    expect(mocks.frame).toHaveBeenCalledTimes(3);
    wrapper.unmount();
  });
  it("reports a Tab ownership failure that persists instead of staying black", async () => {
    mocks.frame.mockRejectedValue({ code: "workspace_tab.wrong_owner", messageKey: "workspace_tab.wrong_owner" });
    const wrapper = fixture(); await flushPromises();
    await vi.advanceTimersByTimeAsync(250 * 6); await flushPromises();
    expect(wrapper.emitted("frameError")).toBeUndefined();
    await vi.advanceTimersByTimeAsync(250 * 2); await flushPromises();
    expect(wrapper.emitted("frameError")).toHaveLength(1);
    wrapper.unmount();
  });
  it("shows the remote cursor bitmap scaled to the display and resets it per session", async () => {
    context2d({ drawImage: vi.fn() });
    const toDataURL = vi.spyOn(HTMLCanvasElement.prototype, "toDataURL").mockReturnValue("data:image/png;base64,QUJD");
    mocks.frame.mockResolvedValueOnce(frameV2({ frameSequence: 1n, cursorSequence: 2n, frame: { base: 0n, width: 100, height: 50 }, cursor: { kind: 2, width: 8, height: 8, hotspotX: 4, hotspotY: 2 } }));
    const wrapper = fixture(); await flushPromises();
    const viewport = wrapper.get(".desktop-display__viewport").element;
    Object.defineProperties(viewport, { clientWidth: { configurable: true, get: () => 200 }, clientHeight: { configurable: true, get: () => 200 } });
    window.dispatchEvent(new Event("resize")); await flushPromises();
    const canvas = wrapper.get("canvas").element as HTMLCanvasElement;
    expect(canvas.style.width).toBe("200px");
    expect(canvas.style.cursor).toBe('url("data:image/png;base64,QUJD") 8 4, default');
    expect(toDataURL).toHaveBeenCalled();
    await wrapper.setProps({ panning: true, fit: false }); await flushPromises();
    expect(canvas.style.cursor).toBe("");
    await wrapper.setProps({ panning: false, session: session("b") }); await flushPromises();
    expect(canvas.style.cursor).toBe("default");
    await vi.advanceTimersByTimeAsync(60); await flushPromises();
    expect(mocks.frame).toHaveBeenLastCalledWith(expect.objectContaining({ id: "b" }), "0", "0");
    wrapper.unmount();
  });
  it("coalesces pointer moves per animation frame and flushes them before button events", async () => {
    const wrapper = fixture(); await flushPromises();
    const canvas = wrapper.find("canvas"); sizeCanvas(canvas.element as HTMLCanvasElement);
    await canvas.trigger("pointerdown", { button: 0, pointerId: 1, clientX: 10, clientY: 10 }); await flushPromises();
    expect(mocks.input).not.toHaveBeenCalled();
    await canvas.trigger("pointermove", { pointerId: 1, buttons: 1, clientX: 20, clientY: 20 });
    await canvas.trigger("pointermove", { pointerId: 1, buttons: 1, clientX: 30, clientY: 40 });
    await canvas.trigger("pointermove", { pointerId: 1, buttons: 1, clientX: 50, clientY: 60 });
    await flushPromises();
    expect(mocks.input).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(16); await flushPromises();
    expect(mocks.input).toHaveBeenCalledTimes(1);
    expect(mocks.input.mock.calls[0]?.[0].input).toEqual({ kind: "pointer", x: 50, y: 60, buttons: 1 });
    await canvas.trigger("pointermove", { pointerId: 1, buttons: 1, clientX: 70, clientY: 70 });
    await canvas.trigger("pointerup", { button: 0, pointerId: 1, buttons: 0, clientX: 70, clientY: 70 }); await flushPromises();
    expect(mocks.input.mock.calls.slice(1).map((call) => call[0].input)).toEqual([
      { kind: "pointer", x: 70, y: 70, buttons: 1 },
      { kind: "pointer", x: 70, y: 70, buttons: 0 },
    ]);
    expect(mocks.input.mock.calls.map((call) => call[0].sequence)).toEqual(["1", "2", "3"]);
    wrapper.unmount();
  });
  it("accumulates wheel deltas within an animation frame and flushes them before keys", async () => {
    const wrapper = fixture(); await flushPromises();
    const canvas = wrapper.find("canvas"); sizeCanvas(canvas.element as HTMLCanvasElement);
    await canvas.trigger("pointerdown", { button: 0, pointerId: 1, clientX: 10, clientY: 10 }); await flushPromises();
    await canvas.trigger("wheel", { deltaX: 0, deltaY: 10, clientX: 10, clientY: 10 });
    await canvas.trigger("wheel", { deltaX: 3, deltaY: 20, clientX: 12, clientY: 14 });
    await canvas.trigger("wheel", { deltaMode: 1, deltaX: 0, deltaY: 1, clientX: 12, clientY: 14 });
    await canvas.trigger("keydown", { code: "KeyA", key: "a", location: 0 }); await flushPromises();
    expect(mocks.input.mock.calls.map((call) => call[0].input)).toEqual([
      { kind: "wheel", x: 12, y: 14, deltaX: 3, deltaY: 46 },
      { kind: "key", scanCode: 30, keysym: 0x61, down: true },
    ]);
    await vi.advanceTimersByTimeAsync(16); await flushPromises();
    expect(mocks.input).toHaveBeenCalledTimes(2);
    wrapper.unmount();
  });
  it("drops a pointer event rejected by the queue limit without revoking control", async () => {
    const wrapper = fixture(); await flushPromises();
    const canvas = wrapper.find("canvas"); sizeCanvas(canvas.element as HTMLCanvasElement);
    await canvas.trigger("pointerdown", { button: 0, pointerId: 1, clientX: 10, clientY: 10 }); await flushPromises();
    mocks.focus.mockClear();
    mocks.input.mockRejectedValueOnce({ code: "desktop.resourceLimit", messageKey: "desktop.errors.resourceLimit" });
    await canvas.trigger("pointermove", { pointerId: 1, buttons: 0, clientX: 20, clientY: 20 });
    await vi.advanceTimersByTimeAsync(16); await flushPromises();
    expect(mocks.focus).not.toHaveBeenCalled();
    expect(wrapper.emitted("error")).toBeUndefined();
    await canvas.trigger("pointerup", { button: 0, pointerId: 1, buttons: 0, clientX: 20, clientY: 20 }); await flushPromises();
    expect(mocks.input).toHaveBeenCalledTimes(2);
    mocks.input.mockRejectedValueOnce({ code: "desktop.resourceLimit", messageKey: "desktop.errors.resourceLimit" });
    await canvas.trigger("keydown", { code: "KeyA", key: "a", location: 0 }); await flushPromises();
    expect(mocks.focus).toHaveBeenCalledWith(null);
    expect(wrapper.emitted("error")).toHaveLength(1);
    wrapper.unmount();
  });
  it("sends Command as Control when the device preference is enabled", async () => {
    const wrapper = fixture({ commandAsControl: true }); await flushPromises();
    const canvas = wrapper.find("canvas"); sizeCanvas(canvas.element as HTMLCanvasElement);
    await canvas.trigger("pointerdown", { button: 0, pointerId: 1, clientX: 10, clientY: 10 }); await flushPromises();
    await canvas.trigger("keydown", { code: "MetaLeft", key: "Meta", location: 1, metaKey: true }); await flushPromises();
    expect(mocks.input.mock.lastCall?.[0].input).toEqual({ kind: "key", scanCode: 29, keysym: 0xffe3, down: true });
    wrapper.unmount();
  });
  it("releases keys pressed with Command at once because macOS WebKit drops their keyup", async () => {
    vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0)");
    const wrapper = fixture({ commandAsControl: true }); await flushPromises();
    const canvas = wrapper.find("canvas"); sizeCanvas(canvas.element as HTMLCanvasElement);
    await canvas.trigger("pointerdown", { button: 0, pointerId: 1, clientX: 10, clientY: 10 }); await flushPromises();
    mocks.input.mockClear();
    await canvas.trigger("keydown", { code: "MetaLeft", key: "Meta", location: 1, metaKey: true }); await flushPromises();
    await canvas.trigger("keydown", { code: "KeyC", key: "c", location: 0, metaKey: true }); await flushPromises();
    await canvas.trigger("keyup", { code: "KeyC", key: "c", location: 0, metaKey: true }); await flushPromises();
    await canvas.trigger("keyup", { code: "MetaLeft", key: "Meta", location: 1 }); await flushPromises();
    expect(mocks.input.mock.calls.map((call) => [(call[0].input as { scanCode: number }).scanCode, (call[0].input as { down: boolean }).down])).toEqual([
      [29, true], [46, true], [46, false], [29, false],
    ]);
    wrapper.unmount();
  });
  it("sends an exposed key sequence in order after acquiring control", async () => {
    const wrapper = fixture(); await flushPromises();
    const ok = await (wrapper.vm as unknown as { sendKeys: (events: unknown[]) => Promise<boolean> }).sendKeys([
      { kind: "key", scanCode: 29, keysym: 0xffe3, down: true }, { kind: "key", scanCode: 29, keysym: 0xffe3, down: false },
    ]);
    expect(ok).toBe(true);
    expect(mocks.focus).toHaveBeenCalledWith(expect.objectContaining({ id: "a" }));
    expect(mocks.input.mock.calls.map((call) => [call[0].sequence, (call[0].input as { down: boolean }).down])).toEqual([["1", true], ["2", false]]);
    await wrapper.setProps({ panning: true, fit: false });
    expect(await (wrapper.vm as unknown as { sendKeys: (events: unknown[]) => Promise<boolean> }).sendKeys([{ kind: "key", scanCode: 29, keysym: 0xffe3, down: true }])).toBe(false);
    wrapper.unmount();
  });
  it("scales a small remote frame to the available viewport and follows fullscreen size changes", async () => {
    context2d();
    mocks.frame.mockResolvedValueOnce(frame(1n, 0n, 100, 50));
    const wrapper = fixture(); await flushPromises();
    const viewport = wrapper.get(".desktop-display__viewport").element;
    let width = 400, height = 300;
    Object.defineProperties(viewport, {
      clientWidth: { configurable: true, get: () => width },
      clientHeight: { configurable: true, get: () => height },
    });
    window.dispatchEvent(new Event("resize")); await flushPromises();
    const canvas = wrapper.get("canvas").element as HTMLCanvasElement;
    expect(canvas.style.width).toBe("400px"); expect(canvas.style.height).toBe("200px");
    width = 400; height = 100;
    window.dispatchEvent(new Event("resize")); await flushPromises();
    expect(canvas.style.width).toBe("200px"); expect(canvas.style.height).toBe("100px");
    await wrapper.setProps({ fit: false });
    expect(canvas.style.width).toBe(""); expect(canvas.style.height).toBe("");
    wrapper.unmount();
  });
  it("shows actual size at one device pixel per remote pixel in HiDPI mode", async () => {
    context2d();
    vi.spyOn(window, "devicePixelRatio", "get").mockReturnValue(2);
    mocks.frame.mockResolvedValueOnce(frame(1n, 0n, 100, 50));
    const wrapper = fixture({ fit: false, hiDpi: true }); await flushPromises();
    window.dispatchEvent(new Event("resize")); await flushPromises();
    const canvas = wrapper.get("canvas").element as HTMLCanvasElement;
    expect(canvas.style.width).toBe("50px"); expect(canvas.style.height).toBe("25px");
    await wrapper.setProps({ hiDpi: false });
    expect(canvas.style.width).toBe("");
    wrapper.unmount();
  });
  it("requests a full image after a patch with an unknown base", async () => {
    const put = context2d();
    mocks.frame.mockResolvedValueOnce(frame(1n, 0n, 2, 2))
      .mockResolvedValueOnce(frame(3n, 2n, 2, 2, 1, 1, 1, 1))
      .mockResolvedValueOnce(frame(3n, 0n, 2, 2));
    const wrapper = fixture(); await flushPromises(); await vi.advanceTimersByTimeAsync(50); await vi.advanceTimersByTimeAsync(50);
    expect(put).toHaveBeenCalledTimes(2);
    expect(mocks.frame.mock.calls[2]?.[1]).toBe("0");
    expect(mocks.frame.mock.calls[3]?.[1]).toBe("3");
    wrapper.unmount();
  });
  it("pans the local viewport without sending desktop input", async () => {
    const wrapper = fixture(); await flushPromises();
    const canvas = wrapper.find("canvas"), viewport = wrapper.find(".desktop-display__viewport").element as HTMLElement;
    Object.defineProperties(viewport, { scrollLeft: { value: 40, writable: true }, scrollTop: { value: 30, writable: true } });
    const capture = vi.fn(), release = vi.fn(), focus = vi.spyOn(canvas.element, "focus");
    Object.assign(canvas.element, { setPointerCapture: capture, hasPointerCapture: vi.fn().mockReturnValue(true), releasePointerCapture: release });
    await wrapper.setProps({ fit: false, panning: true }); await flushPromises();
    mocks.input.mockClear(); mocks.focus.mockClear();
    await canvas.trigger("pointerdown", { button: 0, pointerId: 4, clientX: 120, clientY: 100 });
    await canvas.trigger("pointermove", { pointerId: 4, clientX: 85, clientY: 70 });
    const unrelated = document.createElement("div"); document.body.append(unrelated); await flushPromises();
    await canvas.trigger("pointermove", { pointerId: 4, clientX: 70, clientY: 60 });
    await canvas.trigger("wheel", { deltaX: 10, deltaY: 20 });
    await vi.advanceTimersByTimeAsync(20); await flushPromises();
    expect(viewport.scrollLeft).toBe(90); expect(viewport.scrollTop).toBe(70); expect(capture).toHaveBeenCalledWith(4); expect(focus).toHaveBeenCalledWith({ preventScroll: true }); expect(release).not.toHaveBeenCalled(); expect(mocks.input).not.toHaveBeenCalled(); expect(mocks.focus).not.toHaveBeenCalled();
    unrelated.remove();
    await wrapper.setProps({ panning: false }); await flushPromises();
    expect(release).toHaveBeenCalledWith(4); expect(mocks.focus).toHaveBeenCalledWith(null); wrapper.unmount();
  });
  it("cancels a pan for a blocking dialog and keeps normal pointer input available", async () => {
    const wrapper = fixture({ fit: false, panning: true }); await flushPromises();
    const canvas = wrapper.find("canvas");
    const release = vi.fn();
    Object.assign(canvas.element, { setPointerCapture: vi.fn(), hasPointerCapture: vi.fn().mockReturnValue(true), releasePointerCapture: release });
    await canvas.trigger("pointerdown", { button: 0, pointerId: 8, clientX: 20, clientY: 20 });
    const dialog = document.createElement("div"); dialog.setAttribute("role", "dialog"); dialog.setAttribute("aria-modal", "true"); document.body.append(dialog); await flushPromises();
    expect(release).toHaveBeenCalledWith(8); expect(mocks.input).not.toHaveBeenCalled();
    dialog.remove(); await flushPromises();
    await wrapper.setProps({ panning: false }); await flushPromises();
    mocks.input.mockClear();
    await canvas.trigger("pointerdown", { button: 0, pointerId: 9, clientX: 20, clientY: 20 }); await flushPromises();
    await canvas.trigger("pointerup", { button: 0, pointerId: 9, clientX: 20, clientY: 20 }); await flushPromises();
    expect(mocks.input).toHaveBeenCalledTimes(1); wrapper.unmount();
  });
});


describe("adaptive desktop resolution", () => {
  const adaptive = () => ({ ...session("a"), profile: { ...session("a").profile, rdpResolutionMode: "adaptive" as const } });
  async function sizedFixture(width = 1501, height = 901, props = {}) {
    const wrapper = fixture({ session: adaptive(), ...props });
    await flushPromises();
    const viewport = wrapper.get(".desktop-display__viewport").element;
    const resize = async (w = width, h = height) => {
      Object.defineProperties(viewport, { clientWidth: { configurable: true, value: w }, clientHeight: { configurable: true, value: h } });
      window.dispatchEvent(new Event("resize"));
      await flushPromises();
    };
    await resize();
    return { wrapper, resize };
  }
  it("returns to the initial profile size after the remote image has changed", async () => {
    context2d();
    mocks.frame.mockResolvedValueOnce(frame(1n, 0n, 1500, 900));
    const { wrapper, resize } = await sizedFixture();
    await vi.advanceTimersByTimeAsync(250);
    mocks.resolution.mockClear();
    await resize(1280, 800);
    await vi.advanceTimersByTimeAsync(250);
    expect(mocks.resolution).toHaveBeenCalledWith(expect.objectContaining({ id: "a", generation: "1" }), 1280, 800, 100);
    wrapper.unmount();
  });
  it("requests an exact VNC viewport size only in adaptive mode", async () => {
    const vnc = { ...session("a"), profile: { ...session("a").profile, protocol: "vnc" as const, vncResolutionMode: "adaptive" as const } };
    const wrapper = fixture({ session: vnc });
    await flushPromises();
    const viewport = wrapper.get(".desktop-display__viewport").element;
    Object.defineProperties(viewport, { clientWidth: { configurable: true, value: 1701 }, clientHeight: { configurable: true, value: 901 } });
    window.dispatchEvent(new Event("resize")); await flushPromises();
    await vi.advanceTimersByTimeAsync(250);
    expect(mocks.resolution).toHaveBeenCalledWith(expect.objectContaining({ id: "a" }), 1701, 901, 100);
    await wrapper.setProps({ session: { ...vnc, profile: { ...vnc.profile, vncResolutionMode: "server" } } });
    mocks.resolution.mockClear();
    window.dispatchEvent(new Event("resize")); await vi.advanceTimersByTimeAsync(250);
    expect(mocks.resolution).not.toHaveBeenCalled();
    wrapper.unmount();
  });
  it("requests device pixels and the matching RDP scale in HiDPI mode, but scale 100 for VNC", async () => {
    vi.spyOn(window, "devicePixelRatio", "get").mockReturnValue(2);
    const { wrapper, resize } = await sizedFixture(801, 501, { hiDpi: true });
    await vi.advanceTimersByTimeAsync(250);
    expect(mocks.resolution).toHaveBeenCalledWith(expect.objectContaining({ id: "a" }), 1602, 1002, 200);
    mocks.resolution.mockClear();
    await wrapper.setProps({ hiDpi: false });
    await vi.advanceTimersByTimeAsync(250);
    expect(mocks.resolution).toHaveBeenCalledWith(expect.objectContaining({ id: "a" }), 800, 501, 100);
    mocks.resolution.mockClear();
    await wrapper.setProps({ hiDpi: true, session: { ...session("a"), profile: { ...session("a").profile, protocol: "vnc", vncResolutionMode: "adaptive" } } });
    await resize(801, 501);
    await vi.advanceTimersByTimeAsync(250);
    expect(mocks.resolution).toHaveBeenCalledWith(expect.objectContaining({ id: "a" }), 1602, 1002, 100);
    wrapper.unmount();
  });
  it("debounces viewport changes, sends even bounded dimensions without input ownership and deduplicates them", async () => {
    const { wrapper, resize } = await sizedFixture();
    await vi.advanceTimersByTimeAsync(150);
    await resize(1701, 1001);
    await vi.advanceTimersByTimeAsync(199);
    expect(mocks.resolution).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(mocks.resolution).toHaveBeenCalledWith(expect.objectContaining({ id: "a", generation: "1" }), 1700, 1001, 100);
    expect(mocks.input).not.toHaveBeenCalled();
    expect(wrapper.get("canvas").element.width).toBe(300);
    await resize(1701, 1001);
    await vi.advanceTimersByTimeAsync(250);
    expect(mocks.resolution).toHaveBeenCalledOnce();
    await resize(9000, 9000);
    await vi.advanceTimersByTimeAsync(250);
    const [, width, height] = mocks.resolution.mock.calls.at(-1)!;
    expect(width % 2).toBe(0); expect(width * height).toBeLessThanOrEqual(16_777_216);
    wrapper.unmount();
  });
  it("cancels pending requests on hide or mode change", async () => {
    const { wrapper, resize } = await sizedFixture();
    await wrapper.setProps({ active: false });
    await vi.advanceTimersByTimeAsync(250);
    expect(mocks.resolution).not.toHaveBeenCalled();
    await wrapper.setProps({ active: true, session: session("a") });
    await resize(1700, 1000);
    await vi.advanceTimersByTimeAsync(250);
    expect(mocks.resolution).not.toHaveBeenCalled();
    wrapper.unmount();
  });
  it("drops stale failures and sends only the latest size after an in-flight request", async () => {
    let fail!: (error: unknown) => void;
    mocks.resolution.mockImplementationOnce(() => new Promise((_, reject) => { fail = reject; }));
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue({ clearRect: vi.fn() } as unknown as CanvasRenderingContext2D);
    const { wrapper, resize } = await sizedFixture();
    await vi.advanceTimersByTimeAsync(250);
    await resize(1800, 1000);
    await wrapper.setProps({ session: { ...adaptive(), generation: "2" } });
    await vi.advanceTimersByTimeAsync(250);
    expect(mocks.resolution).toHaveBeenCalledOnce();
    fail({ code: "old-generation" }); await flushPromises();
    await vi.advanceTimersByTimeAsync(250);
    expect(wrapper.emitted("resolutionError")).toBeUndefined();
    expect(mocks.resolution).toHaveBeenLastCalledWith(expect.objectContaining({ generation: "2" }), 1800, 1000, 100);
    wrapper.unmount();
  });
  it("preserves a concrete resize error and does not retry a rejected size automatically", async () => {
    const error = { code: "desktop_resolution_unsupported", messageKey: "desktop.errors.unsupportedOperation", diagnosticId: "test" };
    mocks.resolution.mockRejectedValueOnce(error);
    const { wrapper, resize } = await sizedFixture();
    await vi.advanceTimersByTimeAsync(250);
    expect(wrapper.emitted("resolutionError")).toEqual([[error]]);
    await resize(); await vi.advanceTimersByTimeAsync(600);
    expect(mocks.resolution).toHaveBeenCalledOnce();
    wrapper.unmount();
  });
});
