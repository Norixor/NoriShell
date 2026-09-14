import { mount, flushPromises } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createI18n } from "vue-i18n";
import NvxDesktopCanvas from "./NvxDesktopCanvas.vue";
import type { DesktopSessionSummary } from "../../core-api/generated/core-api";
const mocks = vi.hoisted(() => ({ prepare: vi.fn(), frame: vi.fn(), focus: vi.fn().mockResolvedValue("1"), input: vi.fn().mockResolvedValue(undefined) }));
vi.mock("../../plugins/hostDomBroker", () => ({ preparePluginProtectedMount: mocks.prepare }));
vi.mock("../../core-api/desktop-client", () => ({ desktopClient: mocks }));
const session = (id: string) => ({ id, generation: "1", state: "running" }) as DesktopSessionSummary;
function fixture(props = {}) { return mount(NvxDesktopCanvas, { props: { session: session("a"), active: true, fit: true, panning: false, ...props }, global: { plugins: [createI18n({ legacy: false, locale: "en", messages: { en: { desktop: { focus: "Focus", panFocus: "Pan" } } } })] } }); }
beforeEach(() => { vi.useFakeTimers(); vi.clearAllMocks(); vi.spyOn(document, "hasFocus").mockReturnValue(true); vi.spyOn(document, "hidden", "get").mockReturnValue(false); mocks.frame.mockResolvedValue(new ArrayBuffer(0)); });
afterEach(() => { vi.restoreAllMocks(); vi.useRealTimers(); });
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
    const buffer = new ArrayBuffer(20), view = new DataView(buffer); view.setBigUint64(0, 1n, true); view.setUint32(8, 1, true); view.setUint32(12, 1, true); resolve(buffer);
    await flushPromises(); expect(put).not.toHaveBeenCalled();
    await wrapper.setProps({ active: false }); await vi.advanceTimersByTimeAsync(100); expect(mocks.frame).toHaveBeenCalledTimes(1); wrapper.unmount();
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
