import { flushPromises, mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { createI18n } from "vue-i18n";
import SecureDesktop from "../../views/SecureDesktop.vue";
import { desktopEn } from "../../locales/desktop";
const mocks = vi.hoisted(() => ({ prompt: vi.fn(), decide: vi.fn(), close: vi.fn() }));
vi.mock("../../core-api/desktop-client", () => ({ desktopClient: mocks }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({ close: mocks.close }) }));
beforeEach(() => { vi.clearAllMocks(); mocks.decide.mockReturnValue(new Promise(() => undefined)); });
describe("isolated desktop credentials", () => {
  it("clears the Vault password before decision acknowledgement", async () => {
    mocks.prompt.mockResolvedValue({ id: "prompt", sessionId: "session", label: "Fixture", prompt: { kind: "vaultUnlock" } });
    const wrapper = mount(SecureDesktop, { global: { plugins: [createI18n({ legacy: false, locale: "en", messages: { en: { desktop: desktopEn, window: { protected: "Protected" } } } })] } });
    await flushPromises();
    const field = wrapper.find('input[type="password"]'); await field.setValue("synthetic-fixture-value");
    const approve = wrapper.findAll("button").find((button) => button.text() === desktopEn.approve)!;
    await approve.trigger("click");
    expect((field.element as HTMLInputElement).value).toBe("");
    expect(mocks.decide).toHaveBeenCalledWith(expect.objectContaining({ approved: true, username: null, domain: null, answers: [] }));
    expect(mocks.close).not.toHaveBeenCalled(); wrapper.unmount();
  });

  it("creates a missing Vault with a confirmed UTF-8 password and clears both fields before Core replies", async () => {
    mocks.prompt.mockResolvedValue({ id: "prompt", sessionId: "session", label: "Fixture", prompt: { kind: "vaultCreate" } });
    const wrapper = mount(SecureDesktop, { global: { plugins: [createI18n({ legacy: false, locale: "en", messages: { en: { desktop: desktopEn, window: { protected: "Protected" } } } })] } });
    await flushPromises();

    const fields = wrapper.findAll('input[type="password"]');
    expect(fields).toHaveLength(2);
    await fields[0]!.setValue("密码密码");
    await fields[1]!.setValue("different-password");
    const approve = wrapper.findAll("button").find((button) => button.text() === desktopEn.approve)!;
    expect(approve.attributes("disabled")).toBeDefined();
    await fields[1]!.setValue("密码密码");
    expect(approve.attributes("disabled")).toBeUndefined();
    await approve.trigger("click");

    expect(mocks.decide).toHaveBeenCalledWith(expect.objectContaining({
      approved: true,
      password: "密码密码",
      passwordConfirmation: "密码密码",
    }));
    expect(fields.every((field) => (field.element as HTMLInputElement).value === "")).toBe(true);
    wrapper.unmount();
  });
});
