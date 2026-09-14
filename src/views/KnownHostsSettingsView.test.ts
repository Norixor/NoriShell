import { flushPromises, mount } from "@vue/test-utils";
import { createMemoryHistory, createRouter } from "vue-router";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { i18n } from "../locales";

const client = vi.hoisted(() => ({
  deleteKnownHost: vi.fn(),
  listKnownHosts: vi.fn(),
}));

vi.mock("../core-api/client", () => client);

import KnownHostsSettingsView from "./KnownHostsSettingsView.vue";

const knownHost = {
  knownHostId: "known-host-1",
  normalizedAddress: "server.example.com",
  port: 22,
  keyAlgorithm: "ssh-ed25519",
  publicKeyBase64: "must-not-render",
  fingerprintSha256: "SHA256:server-fingerprint",
  firstTrustedAtUnixMs: 1_700_000_000_000,
  lastVerifiedAtUnixMs: 1_700_001_000_000,
  stateVersion: "7",
};

async function mountView() {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: "/known-hosts", component: KnownHostsSettingsView },
      { path: "/settings", component: { template: "<div>Settings</div>" } },
    ],
  });
  await router.push("/known-hosts");
  await router.isReady();
  const wrapper = mount(KnownHostsSettingsView, {
    attachTo: document.body,
    global: { plugins: [router, i18n] },
  });
  await flushPromises();
  return wrapper;
}

describe("KnownHostsSettingsView", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    i18n.global.locale.value = "en";
    client.listKnownHosts.mockResolvedValue([knownHost]);
    client.deleteKnownHost.mockResolvedValue({
      knownHostId: knownHost.knownHostId,
      deleted: true,
      deletedKnownHost: knownHost,
    });
  });

  it("renders the verified fingerprint without exposing the stored public key", async () => {
    const wrapper = await mountView();

    expect(wrapper.text()).toContain("server.example.com:22");
    expect(wrapper.text()).toContain("SHA256:server-fingerprint");
    expect(wrapper.text()).not.toContain("must-not-render");
    wrapper.unmount();
  });

  it("requires confirmation and deletes the exact current-version trust record", async () => {
    const wrapper = await mountView();

    await wrapper.findAll("button").find((button) => button.text() === "Delete trust")!.trigger("click");
    expect(document.body.textContent).toContain("the next connection must confirm the server identity again");
    const confirm = Array.from(document.body.querySelectorAll("button"))
      .filter((button) => button.textContent?.trim() === "Delete trust")
      .at(-1)!;
    confirm.click();
    await flushPromises();

    expect(client.deleteKnownHost).toHaveBeenCalledWith("known-host-1", "7");
    wrapper.unmount();
  });
});
