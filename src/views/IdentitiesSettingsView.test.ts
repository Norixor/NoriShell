import { flushPromises, mount } from "@vue/test-utils";
import { createMemoryHistory, createRouter } from "vue-router";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { i18n } from "../locales";

const client = vi.hoisted(() => ({
  createIdentity: vi.fn(),
  deleteIdentity: vi.fn(),
  fetchIdentityDeleteImpact: vi.fn(),
  listCredentialRefs: vi.fn(),
  listIdentities: vi.fn(),
  updateIdentity: vi.fn(),
}));

vi.mock("../core-api/client", () => client);

import IdentitiesSettingsView from "./IdentitiesSettingsView.vue";

const identity = {
  identityId: "identity-1",
  label: "Production deploy",
  username: "deploy",
  stateVersion: "4",
};
const credential = {
  credentialRefId: "credential-1",
  identityId: identity.identityId,
  method: "sshAgent" as const,
  priority: 10,
  label: "Agent key",
  details: {
    kind: "sshAgent" as const,
    publicKeyAlgorithm: "ssh-ed25519",
    publicKeyFingerprint: "SHA256:credential",
    publicKeyBlob: [],
    scope: "defaultEnvironment" as const,
  },
  stateVersion: "2",
};

async function mountView() {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: "/settings/identities", component: IdentitiesSettingsView },
      { path: "/settings", component: { template: "<div>Settings</div>" } },
    ],
  });
  await router.push("/settings/identities");
  await router.isReady();
  const wrapper = mount(IdentitiesSettingsView, {
    attachTo: document.body,
    global: { plugins: [router, i18n] },
  });
  await flushPromises();
  return wrapper;
}

describe("IdentitiesSettingsView", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    i18n.global.locale.value = "en";
    client.listIdentities.mockResolvedValue([identity]);
    client.listCredentialRefs.mockResolvedValue([credential]);
  });

  it("shows safe credential summaries and blocks deletion while owners still reference an identity", async () => {
    client.fetchIdentityDeleteImpact.mockResolvedValue({
      identity,
      referencingHosts: [{
        hostId: "host-1",
        label: "Production",
        address: "prod.example.com",
        normalizedAddress: "prod.example.com",
        port: 22,
        username: "deploy",
        identityId: identity.identityId,
        favorite: false,
        hasReadyCredential: true,
        stateVersion: "3",
      }],
      referencingHostCount: 1,
      referencingCredentialRefs: [credential],
      referencingCredentialRefCount: 1,
    });
    const wrapper = await mountView();

    expect(wrapper.text()).toContain("Production deploy");
    expect(wrapper.text()).toContain("Agent key · SSH Agent · priority 10");
    expect(wrapper.text()).not.toContain("credential-1");

    await wrapper.findAll("button").find((button) => button.text() === "Delete")!.trigger("click");
    await flushPromises();

    expect(document.body.textContent).toContain("still referenced by 1 Host(s) and 1 CredentialRef(s)");
    expect(document.body.textContent).toContain("Host: Production");
    expect(client.deleteIdentity).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("updates only the non-secret identity fields with the current state version", async () => {
    client.updateIdentity.mockResolvedValue({ ...identity, label: "Production ops", stateVersion: "5" });
    const wrapper = await mountView();

    await wrapper.findAll("button").find((button) => button.text() === "Edit")!.trigger("click");
    const label = document.body.querySelector<HTMLInputElement>("#identity-label")!;
    label.value = "Production ops";
    label.dispatchEvent(new Event("input"));
    await flushPromises();
    const save = Array.from(document.body.querySelectorAll("button"))
      .find((button) => button.textContent?.trim() === "Save")!;
    save.click();
    await flushPromises();

    expect(client.updateIdentity).toHaveBeenCalledWith({
      identityId: identity.identityId,
      expectedStateVersion: "4",
      label: "Production ops",
      username: "deploy",
    });
    wrapper.unmount();
  });
});
