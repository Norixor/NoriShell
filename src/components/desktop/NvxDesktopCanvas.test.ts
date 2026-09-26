import { mount, flushPromises } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createI18n } from "vue-i18n";
import NvxDesktopCanvas from "./NvxDesktopCanvas.vue";
import type { DesktopSessionSummary } from "../../core-api/generated/core-api";
const mocks = vi.hoisted(() => ({ resolution: vi.fn().mockResolvedValue(undefined), prepare: vi.fn(), frame: vi.fn(), focus: vi.fn().mockResolvedValue("1"), input: vi.fn().mockResolvedValue(undefined) }));
vi.mock("../../plugins/hostDomBroker", () => ({ preparePluginProtectedMount: mocks.prepare }));
vi.mock("../../core-api/desktop-client", () => ({ desktopClient: mocks }));
const session = (id: string) => ({ id, generation: "1", state: "running", width: 1280, height: 800, profile: { protocol: "rdp", rdpResolutionMode: "fixed" } }) as unknown as DesktopSessionSummary;
function frame(sequence: bigint, base: bigint, width: number, height: number, x = 0, y = 0, rectWidth = width, rectHeight = height) {
  const buffer = new ArrayBuffer(40 + rectWidth * rectHeight * 4), view = new DataView(buffer);
  view.setBigUint64(0, sequence, true); view.setBigUint64(8, base, true);
  for (const [offset, value] of [[16, width], [20, height], [24, x], [28, y], [32, rectWidth], [36, rectHeight]] as const) view.setUint32(offset, value, true);
  return buffer;
}
function fixture(props = {}) { return mount(NvxDesktopCanvas, { props: { session: session("a"), active: true, fit: true, panning: false, ...props }, global: { plugins: [createI18n({ legacy: false, locale: "en", messages: { en: { desktop: { focus: "Focus", panFocus: "Pan" } } } })] } }); }
beforeEach(() => { vi.useFakeTimers(); vi.clearAllMocks(); vi.spyOn(document, "hasFocus").mockReturnValue(true); vi.spyOn(document, "hidden", "get").mockReturnValue(false); mocks.frame.mockResolvedValue(new ArrayBuffer(0)); });
afterEach(() => { vi.restoreAllMocks(); vi.unstubAllGlobals(); vi.useRealTimers(); });
describe("protected desktop display", () => {
  it("prepares an empty protected shell before rendering canvas", async () => {
    mocks.prepare.mockImplementation((root: Element) => { expect(root.hasAttribute("data-plugin-protected")).toBe(true); expect(root.querySelector("canvas")).toBeNull(); });
    const wrapper = fixture(); await flushPromises(); expect(mocks.prepare).toHaveBeenCalledOnce(); expect(wrapper.find("canvas").exists()).toBe(true); wrapper.unmount();
  });
  it("keeps a single frame request in flight and discards a previous session reply", async () => {
    let resolve!: (buffer: ArrayBuffer) => void;
    mocks.frame.mockReturnValueOnce(new Promise<ArrayBuffer>((done) => { resolve = done; }));
    const put = vi.fn(); vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue({ putImageData: put, clearRect: vi.fn() } as unknown as CanvasRenderingContext2D);
    const wrapper = fixture(); await flushPromises(); await vi.advanceTimersByTimeAsync(200); expect(mocks.frame).toHaveBeenCalledTimes(1);
    await wrapper.setProps({ session: session("b") });
    resolve(frame(1n, 0n, 1, 1));
    await flushPromises(); expect(put).not.toHaveBeenCalled();
    await wrapper.setProps({ active: false }); await vi.advanceTimersByTimeAsync(100); expect(mocks.frame).toHaveBeenCalledTimes(1); wrapper.unmount();
  });
  it("draws a continuous patch without resetting the canvas size", async () => {
    vi.stubGlobal("ImageData", class { constructor(readonly rgba: Uint8ClampedArray, readonly width: number, readonly height: number) {} });
    const put = vi.fn(); vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue({ putImageData: put, clearRect: vi.fn() } as unknown as CanvasRenderingContext2D);
    mocks.frame.mockResolvedValueOnce(frame(1n, 0n, 2, 2)).mockResolvedValueOnce(frame(2n, 1n, 2, 2, 1, 1, 1, 1));
    const wrapper = fixture(); await flushPromises();
    const canvas = wrapper.find("canvas").element as HTMLCanvasElement;
    const setWidth = vi.fn(), setHeight = vi.fn();
    Object.defineProperty(canvas, "width", { configurable: true, get: () => 2, set: setWidth });
    Object.defineProperty(canvas, "height", { configurable: true, get: () => 2, set: setHeight });
    await vi.advanceTimersByTimeAsync(50); await flushPromises();
    expect(put).toHaveBeenCalledTimes(2);
    expect(put.mock.calls[1]?.slice(1)).toEqual([1, 1]);
    expect(setWidth).not.toHaveBeenCalled(); expect(setHeight).not.toHaveBeenCalled();
    expect(mocks.frame.mock.calls[1]?.[1]).toBe("1");
    wrapper.unmount();
  });
  it("scales a small remote frame to the available viewport and follows fullscreen size changes", async () => {
    vi.stubGlobal("ImageData", class { constructor(readonly rgba: Uint8ClampedArray, readonly width: number, readonly height: number) {} });
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue({ putImageData: vi.fn(), clearRect: vi.fn() } as unknown as CanvasRenderingContext2D);
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
  it("requests a full image after a patch with an unknown base", async () => {
    vi.stubGlobal("ImageData", class { constructor(readonly rgba: Uint8ClampedArray, readonly width: number, readonly height: number) {} });
    const put = vi.fn(); vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue({ putImageData: put, clearRect: vi.fn() } as unknown as CanvasRenderingContext2D);
    mocks.frame.mockResolvedValueOnce(frame(1n, 0n, 2, 2))
      .mockResolvedValueOnce(frame(3n, 2n, 2, 2, 1, 1, 1, 1))
      .mockResolvedValueOnce(frame(3n, 0n, 2, 2));
    const wrapper = fixture(); await flushPromises(); await vi.advanceTimersByTimeAsync(50); await vi.advanceTimersByTimeAsync(50);
    expect(put).toHaveBeenCalledTimes(2);
    expect(mocks.frame.mock.calls[2]?.[1]).toBe("0");
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
    await flushPromises();
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
  async function sizedFixture(width = 1501, height = 901) {
    const wrapper = fixture({ session: adaptive() });
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
    vi.stubGlobal("ImageData", class { constructor(readonly rgba: Uint8ClampedArray, readonly width: number, readonly height: number) {} });
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue({ putImageData: vi.fn(), clearRect: vi.fn() } as unknown as CanvasRenderingContext2D);
    mocks.frame.mockResolvedValueOnce(frame(1n, 0n, 1500, 900));
    const { wrapper, resize } = await sizedFixture();
    await vi.advanceTimersByTimeAsync(250);
    mocks.resolution.mockClear();
    await resize(1280, 800);
    await vi.advanceTimersByTimeAsync(250);
    expect(mocks.resolution).toHaveBeenCalledWith(expect.objectContaining({ id: "a", generation: "1" }), 1280, 800);
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
    expect(mocks.resolution).toHaveBeenCalledWith(expect.objectContaining({ id: "a" }), 1701, 901);
    await wrapper.setProps({ session: { ...vnc, profile: { ...vnc.profile, vncResolutionMode: "server" } } });
    mocks.resolution.mockClear();
    window.dispatchEvent(new Event("resize")); await vi.advanceTimersByTimeAsync(250);
    expect(mocks.resolution).not.toHaveBeenCalled();
    wrapper.unmount();
  });
  it("debounces viewport changes, sends even bounded dimensions without input ownership and deduplicates them", async () => {
    const { wrapper, resize } = await sizedFixture();
    await vi.advanceTimersByTimeAsync(150);
    await resize(1701, 1001);
    await vi.advanceTimersByTimeAsync(199);
    expect(mocks.resolution).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(mocks.resolution).toHaveBeenCalledWith(expect.objectContaining({ id: "a", generation: "1" }), 1700, 1001);
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
    expect(mocks.resolution).toHaveBeenLastCalledWith(expect.objectContaining({ generation: "2" }), 1800, 1000);
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
