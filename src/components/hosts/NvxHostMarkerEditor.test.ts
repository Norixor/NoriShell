import { mount } from "@vue/test-utils";
import { createI18n } from "vue-i18n";
import { describe, expect, it } from "vitest";
import { hostMarkersEn, hostMarkersZhCN } from "../../locales/host-markers";
import NvxHostMarker from "./NvxHostMarker.vue";
import NvxHostMarkerEditor from "./NvxHostMarkerEditor.vue";
import { NvxButton, NvxSelect } from "../ui";
const locale = () => createI18n({ legacy: false, locale: "en", messages: { en: { hostMarkers: hostMarkersEn }, "zh-CN": { hostMarkers: hostMarkersZhCN } } });
describe("host marker presentation", () => {
  it("uses translated preset text and renders custom content as text", async () => {
    const i18n = locale();
    const badge = mount(NvxHostMarker, { props: { marker: { kind: "production", color: "red" } }, global: { plugins: [i18n] } });
    expect(badge.text()).toBe("Production");
    i18n.global.locale.value = "zh-CN";
    await badge.vm.$nextTick();
    expect(badge.text()).toBe("生产");
    await badge.setProps({ marker: { kind: "custom", label: "<b>API</b>", color: "blue" } });
    expect(badge.find("b").exists()).toBe(false);
    expect(badge.attributes("title")).toBe("<b>API</b>");
  });
  it("emits isolated draft changes and clear without mutating the saved input", async () => {
    const marker = { kind: "production" as const, color: "red" as const };
    const editor = mount(NvxHostMarkerEditor, { props: { modelValue: marker }, global: { plugins: [locale()] } });
    editor.findComponent(NvxSelect).vm.$emit("update:modelValue", "testing");
    expect(editor.emitted("update:modelValue")?.[0]).toEqual([{ kind: "testing", color: "amber" }]);
    expect(marker.kind).toBe("production");
    await editor.findComponent(NvxButton).trigger("click");
    expect(editor.emitted("update:modelValue")?.[1]).toEqual([null]);
  });
});
