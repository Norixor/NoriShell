import { flushPromises, mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { i18n } from "../locales";
import SecureSshChallenge from "./SecureSshChallenge.vue";
const client = vi.hoisted(() => ({ get: vi.fn(), submit: vi.fn(), cancel: vi.fn() }));
vi.mock("../core-api/secure-ssh-challenge-client", () => ({ secureSshChallengeClient: client }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({ startDragging: vi.fn() }) }));
function render() { return mount(SecureSshChallenge, { global: { plugins: [i18n] } }); }
beforeEach(() => { vi.clearAllMocks(); i18n.global.locale.value = "en"; client.submit.mockReturnValue(new Promise(() => undefined)); });
describe("isolated SSH challenges", () => {
  it("renders Core fingerprints and submits only the exact prompt decision", async () => {
    client.get.mockResolvedValue({ id: "fixture", content: { kind: "sshHostKey", challenge: { endpoint: { address: "fixture.example", port: 22 }, keyAlgorithm: "ssh-ed25519", fingerprintSha256: "SHA256:fixture" } } });
    const wrapper = render(); await flushPromises();
    expect(wrapper.text()).toContain("fixture.example:22");
    expect(wrapper.text()).toContain("SHA256:fixture");
    await wrapper.findAll("button").find((button) => button.text() === i18n.global.t("sshSession.hostKey.accept"))!.trigger("click");
    expect(client.submit).toHaveBeenCalledWith("", true, []);
    wrapper.unmount();
  });
  it("keeps mixed keyboard answers isolated and clears them before Core acknowledges", async () => {
    client.get.mockResolvedValue({ id: "fixture", content: { kind: "sshKeyboard", challenge: { name: "Login", instructions: "Fixture", prompts: [{ promptIndex: 0, text: "Account", echo: true, sensitive: false }, { promptIndex: 1, text: "Code", echo: false, sensitive: true }] } } });
    const wrapper = render(); await flushPromises();
    const fields = wrapper.findAll("input");
    expect(fields[0]!.attributes("type")).toBe("text");
    expect(fields[1]!.attributes("type")).toBe("password");
    await fields[0]!.setValue("fixture-user"); await fields[1]!.setValue("fixture-secret");
    await wrapper.find("form").trigger("submit");
    expect(client.submit).toHaveBeenCalledWith("", true, ["fixture-user", "fixture-secret"]);
    expect(fields.every((field) => (field.element as HTMLInputElement).value === "")).toBe(true);
    wrapper.unmount();
  });
  it("never offers approval for a changed Metrics fingerprint", async () => {
    client.get.mockResolvedValue({ id: "fixture", content: { kind: "metricsHostKey", challenge: { hostId: "fixture-host", algorithm: "ssh-ed25519", fingerprintSha256: "SHA256:new", trustedFingerprintSha256: "SHA256:old" } } });
    const wrapper = render(); await flushPromises();
    const approve = wrapper.findAll("button").find((button) => button.text() === i18n.global.t("sshSession.hostKey.accept"))!;
    expect(approve.attributes("disabled")).toBeDefined();
    await approve.trigger("click"); expect(client.submit).not.toHaveBeenCalled();
    await wrapper.findAll("button").find((button) => button.text() === i18n.global.t("sshSession.hostKey.reject"))!.trigger("click");
    expect(client.submit).toHaveBeenCalledWith("", false, []);
    wrapper.unmount();
  });
});
