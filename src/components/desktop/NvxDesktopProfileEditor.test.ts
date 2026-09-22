import { flushPromises, mount } from "@vue/test-utils";
import { defineComponent } from "vue";
import { createI18n } from "vue-i18n";
import { beforeEach, describe, expect, it, vi } from "vitest";

import NvxDesktopProfileEditor from "./NvxDesktopProfileEditor.vue";
import { desktopEn } from "../../locales/desktop";

const desktop = vi.hoisted(() => ({ profiles: vi.fn(), save: vi.fn() }));
const core = vi.hoisted(() => ({ listHosts: vi.fn(), listIdentities: vi.fn(), listCredentialRefs: vi.fn() }));
vi.mock("../../core-api/desktop-client", () => ({ desktopClient: desktop }));
vi.mock("../../core-api/client", () => core);

const profile = () => ({
  id: "desktop-fixture", label: " Existing desktop ", protocol: "rdp" as const, address: " desktop.example ", port: 3389,
  username: "user", domain: "DOMAIN", hostId: null, gatewayHostId: null, credentialRefId: null,
  width: 1280, height: 800, clipboardEnabled: false, audioPlaybackEnabled: true, revision: "4",
});

const PasswordFields = defineComponent({
  name: "NvxDesktopPasswordFields",
  setup(_, { expose }) {
    expose({ prepare: () => null, acceptSaved: vi.fn(), discard: () => Promise.resolve(true) });
    return () => null;
  },
});

function mountEditor(profileId?: string) {
  return mount(NvxDesktopProfileEditor, {
    props: { profileId },
    global: {
      plugins: [createI18n({
        legacy: false,
        locale: "en",
        messages: { en: { desktop: desktopEn, toolWindows: {
          unsavedTitle: "Unsaved changes", unsavedDescription: "Discard the changes and close this window?",
          keepEditing: "Keep editing", discard: "Discard and close",
        } } },
      })],
      stubs: {
        Teleport: true,
        NvxDesktopPasswordFields: PasswordFields,
        NvxSelect: {
          props: ["modelValue", "options", "disabled"], emits: ["update:modelValue"],
          template: '<select :value="modelValue" :disabled="disabled" @change="$emit(\'update:modelValue\', $event.target.value)"><option v-for="option in options" :key="option.value" :value="option.value">{{ option.label }}</option></select>',
        },
      },
    },
  });
}

beforeEach(() => {
  vi.resetAllMocks();
  desktop.profiles.mockResolvedValue([profile()]);
  desktop.save.mockImplementation(async (value) => ({ ...value, revision: "5" }));
  core.listHosts.mockResolvedValue([{ hostId: "gateway", label: "Gateway" }]);
  core.listIdentities.mockResolvedValue([{ identityId: "identity" }]);
  core.listCredentialRefs.mockResolvedValue([]);
});

describe("NvxDesktopProfileEditor", () => {
  it("loads its profile, host and credential choices independently, then validates and saves the normalized draft", async () => {
    const wrapper = mountEditor("desktop-fixture");
    await flushPromises();

    expect((wrapper.get("#desktop-label").element as HTMLInputElement).value).toBe(" Existing desktop ");
    expect(core.listHosts).toHaveBeenCalledTimes(1);
    expect(core.listCredentialRefs).toHaveBeenCalledWith("identity");

    await wrapper.find("select").setValue("vnc");
    expect((wrapper.get("#desktop-port").element as HTMLInputElement).value).toBe("5900");
    expect(wrapper.find("#desktop-domain").exists()).toBe(false);
    await wrapper.get("form").trigger("submit");
    await flushPromises();

    expect(desktop.save).toHaveBeenCalledWith(expect.objectContaining({
      id: "desktop-fixture", label: "Existing desktop", address: "desktop.example", protocol: "vnc", port: 5900, audioPlaybackEnabled: false,
    }), null);
    expect(wrapper.emitted("saved")?.[0]).toEqual([expect.objectContaining({ revision: "5" })]);
  });

  it("keeps an invalid new draft local and exposes a close promise that requires an explicit discard", async () => {
    const wrapper = mountEditor();
    await flushPromises();

    await wrapper.get("form").trigger("submit");
    expect(wrapper.text()).toContain(desktopEn.invalid);

    await wrapper.get("#desktop-label").setValue("Unsaved");
    const requestClose = (wrapper.vm as unknown as { requestClose(): Promise<boolean> }).requestClose();
    await flushPromises();
    expect(wrapper.text()).toContain("Discard the changes and close this window?");
    await wrapper.findAll("button").find((button) => button.text() === "Keep editing")!.trigger("click");
    expect(await requestClose).toBe(false);

    const discard = (wrapper.vm as unknown as { requestClose(): Promise<boolean> }).requestClose();
    await flushPromises();
    await wrapper.findAll("button").find((button) => button.text() === "Discard and close")!.trigger("click");
    expect(await discard).toBe(true);
  });
});
