import { DOMWrapper, flushPromises, mount } from "@vue/test-utils";
import { defineComponent, h, shallowRef } from "vue";
import NvxHostEditor from "../components/hosts/NvxHostEditor.vue";
import { createPinia } from "pinia";
import { createMemoryHistory, createRouter } from "vue-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { HostSummary, LoginAutomationStepInput } from "../core-api/generated/core-api";
import { i18n } from "../locales";
import { useTipsStore } from "../stores/tips";

const nativeWindows = vi.hoisted(() => ({ open: vi.fn(), listen: vi.fn(), vault: vi.fn() }));
vi.mock("../tool-windows", () => ({ openToolWindow: nativeWindows.open, onToolWindowChanged: nativeWindows.listen }));
vi.mock("../core-api/secure-vault-client", () => ({ requestSecureVault: nativeWindows.vault }));

const client = vi.hoisted(() => ({
  cancelHostCreatePassword: vi.fn(),
  cancelLoginAutomationSecret: vi.fn(),
  createConfiguredHost: vi.fn(),
  createHost: vi.fn(),
  createHostGroup: vi.fn(),
  createHostTag: vi.fn(),
  createVault: vi.fn(),
  createIdentity: vi.fn(),
  createKeyboardInteractiveCredential: vi.fn(),
  createLoginAutomationSecret: vi.fn(),
  createSshAgentCredential: vi.fn(),
  commitOpenSshConfig: vi.fn(),
  deleteHost: vi.fn(),
  deleteHostGroup: vi.fn(),
  deleteHostTag: vi.fn(),
  getAlgorithmPolicyCatalog: vi.fn(),
  getHostConnectionConfig: vi.fn(),
  fetchVaultStatus: vi.fn(),
  importPrivateKeyFile: vi.fn(),
  listCredentialRefs: vi.fn(),
  listHostCatalog: vi.fn(),
  listHostGroups: vi.fn(),
  listHostTags: vi.fn(),
  listIdentities: vi.fn(),
  listSshAgentKeys: vi.fn(),
  previewOpenSshConfig: vi.fn(),
  prepareTransientCredential: vi.fn(),
  testSshConnection: vi.fn(),
  replaceHostOrganization: vi.fn(),
  replaceAlgorithmPolicy: vi.fn(),
  replaceHeartbeatPolicy: vi.fn(),
  replaceMonitoringPolicy: vi.fn(),
  replaceLoginAutomation: vi.fn(),
  replaceRoutePlan: vi.fn(),
  stageHostCreatePassword: vi.fn(),
  confirmLoginAutomation: vi.fn(),
  updateHost: vi.fn(),
  updateHostFavorite: vi.fn(),
  updateHostGroup: vi.fn(),
  updateHostTag: vi.fn(),
  unlockVault: vi.fn(),
}));

vi.mock("../core-api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../core-api/client")>();
  return {
    ...actual,
    canUseDesktopCore: () => true,
    cancelHostCreatePassword: client.cancelHostCreatePassword,
    cancelLoginAutomationSecret: client.cancelLoginAutomationSecret,
    createConfiguredHost: client.createConfiguredHost,
    createHost: client.createHost,
    createHostGroup: client.createHostGroup,
    createHostTag: client.createHostTag,
    createVault: client.createVault,
    createIdentity: client.createIdentity,
    createKeyboardInteractiveCredential: client.createKeyboardInteractiveCredential,
    createLoginAutomationSecret: client.createLoginAutomationSecret,
    createSshAgentCredential: client.createSshAgentCredential,
    commitOpenSshConfig: client.commitOpenSshConfig,
    deleteHost: client.deleteHost,
    deleteHostGroup: client.deleteHostGroup,
    deleteHostTag: client.deleteHostTag,
    getAlgorithmPolicyCatalog: client.getAlgorithmPolicyCatalog,
    getHostConnectionConfig: client.getHostConnectionConfig,
    fetchVaultStatus: client.fetchVaultStatus,
    importPrivateKeyFile: client.importPrivateKeyFile,
    listCredentialRefs: client.listCredentialRefs,
    listHostCatalog: client.listHostCatalog,
    listHostGroups: client.listHostGroups,
    listHostTags: client.listHostTags,
    listIdentities: client.listIdentities,
    listSshAgentKeys: client.listSshAgentKeys,
    previewOpenSshConfig: client.previewOpenSshConfig,
    prepareTransientCredential: client.prepareTransientCredential,
    testSshConnection: client.testSshConnection,
    replaceHostOrganization: client.replaceHostOrganization,
    replaceAlgorithmPolicy: client.replaceAlgorithmPolicy,
    replaceHeartbeatPolicy: client.replaceHeartbeatPolicy,
    replaceMonitoringPolicy: client.replaceMonitoringPolicy,
    replaceLoginAutomation: client.replaceLoginAutomation,
    replaceRoutePlan: client.replaceRoutePlan,
    stageHostCreatePassword: client.stageHostCreatePassword,
    confirmLoginAutomation: client.confirmLoginAutomation,
    updateHost: client.updateHost,
    updateHostFavorite: client.updateHostFavorite,
    updateHostGroup: client.updateHostGroup,
    updateHostTag: client.updateHostTag,
    unlockVault: client.unlockVault,
  };
});

import HostsView from "./HostsView.vue";

const host: HostSummary = {
  hostId: "019d0000-0000-7000-8000-000000000101",
  label: "Production",
  address: "ssh.example.test",
  normalizedAddress: "ssh.example.test",
  port: 22,
  username: "deploy",
  identityId: "019d0000-0000-7000-8000-000000000102",
  favorite: true,
  hasReadyCredential: true,
  stateVersion: "4",
};

const catalogEntry = {
  host,
  group: null,
  tags: [],
  recentConnection: null,
};

function hostConnectionConfig(overrides: Record<string, unknown> = {}) {
  return {
    routePlan: {
      hostId: host.hostId,
      revision: "2",
      ingress: { kind: "directTcp" as const },
      jumpHostIds: [],
    },
    algorithmPolicy: {
      hostId: host.hostId,
      revision: "5",
      policyId: "secure-default",
      compatibilityExceptions: [],
    },
    heartbeatPolicy: {
      hostId: host.hostId,
      revision: "2",
      policy: { mode: "disabled" as const },
    },
    monitoringPolicy: {
      hostId: host.hostId,
      revision: "6",
      policy: {
        enabled: false,
        sampleIntervalSeconds: 15,
        sampleTimeoutSeconds: 5,
        diskMountIds: ["root" as const],
        networkInterfaceIds: ["aggregateNonLoopback" as const],
      },
    },
    loginAutomation: {
      hostId: host.hostId,
      revision: "3",
      confirmedRevision: null,
      enabled: false,
      steps: [],
    },
    ...overrides,
  };
}

async function mountView(initialPath = "/hosts") {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: "/hosts", component: HostsView },
      { path: "/terminal", component: { template: "<div>Terminal</div>" } },
    ],
  });
  await router.push(initialPath);
  await router.isReady();
  const mountHost = document.createElement("div");
  document.body.append(mountHost);
  const pinia = createPinia();
  const target = shallowRef<{ hostId?: string; initialSection?: "connection" | "connectionRoute" | "loginAutomation" } | null>(null);
  let changed: ((kind: string) => void) | undefined;
  nativeWindows.listen.mockImplementation(async (callback) => { changed = callback; return () => { changed = undefined; }; });
  nativeWindows.open.mockImplementation(async (descriptor) => {
    target.value = { hostId: descriptor.hostId ?? undefined, initialSection: descriptor.initialSection };
  });
  // Native IPC is represented only at the window boundary; the real extracted editor runs unchanged.
  const Harness = defineComponent(() => () => h("div", [
    h(HostsView),
    target.value ? h(NvxHostEditor, {
      ...target.value,
      onSaved: () => { target.value = null; changed?.("hostEditor"); },
      onCancel: () => { target.value = null; },
    }) : null,
  ]));
  const wrapper = mount(Harness, {
    attachTo: mountHost,
    global: { plugins: [pinia, router, i18n] },
  });
  await flushPromises();
  return { router, tips: useTipsStore(pinia), wrapper };
}

function button(label: string) {
  const buttons = Array.from(document.body.querySelectorAll<HTMLButtonElement>("button"));
  return buttons.find((candidate) => candidate.textContent?.trim() === label)
    ?? buttons.find((candidate) => candidate.textContent?.trim().includes(label));
}

async function selectHostEditorSection(label: string) {
  const section = Array.from(document.querySelectorAll<HTMLButtonElement>(
    ".hosts-editor__nav-item",
  )).find((candidate) => candidate.textContent?.trim() === label);
  await section?.click();
  await flushPromises();
}

async function chooseSelectOption(selectId: string, label: string) {
  document.querySelector<HTMLButtonElement>(`#${selectId}`)?.click();
  await flushPromises();
  const option = Array.from(document.body.querySelectorAll<HTMLElement>('[role="option"]'))
    .find((candidate) => candidate.textContent?.trim() === label);
  if (!option) throw new Error(`Missing Select option: ${label}`);
  option.click();
  await flushPromises();
}

const body = () => new DOMWrapper(document.body);

describe("HostsView single-Host management contract", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    nativeWindows.vault.mockResolvedValue(true);
    i18n.global.locale.value = "en";
    client.listHostCatalog.mockResolvedValue([catalogEntry]);
    client.listHostGroups.mockResolvedValue([]);
    client.listHostTags.mockResolvedValue([]);
    client.listIdentities.mockResolvedValue([{
      identityId: host.identityId,
      label: "Production deploy",
      username: "deploy",
      stateVersion: "1",
    }]);
    client.listCredentialRefs.mockResolvedValue([{
      credentialRefId: "019d0000-0000-7000-8000-000000000103",
      identityId: host.identityId,
      method: "password",
      priority: 10,
      label: "Proxy password",
      details: { kind: "password" },
      stateVersion: "1",
    }]);
    client.fetchVaultStatus.mockResolvedValue({ state: "unlocked" });
    client.prepareTransientCredential.mockResolvedValue({
      credentialRefId: "019d0000-0000-7000-8000-000000000104",
    });
    client.testSshConnection.mockResolvedValue({ verified: true });
    client.createVault.mockResolvedValue({ state: "unlocked" });
    client.unlockVault.mockResolvedValue({ state: "unlocked" });
    client.getHostConnectionConfig.mockResolvedValue(hostConnectionConfig());
    client.getAlgorithmPolicyCatalog.mockResolvedValue({
      catalogVersion: "2026.08.29.1",
      defaultPolicyId: "secure-default",
      entries: [
        {
          stableId: "kex-curve25519-sha256",
          category: "keyExchange",
          algorithmName: "curve25519-sha256",
          available: true,
          enabledByDefault: true,
          selectableException: false,
          risk: "modern",
          riskMessageKey: null,
        },
        {
          stableId: "compat-kex-dh-group14-sha1",
          category: "keyExchange",
          algorithmName: "diffie-hellman-group14-sha1",
          available: true,
          enabledByDefault: false,
          selectableException: true,
          risk: "weak",
          riskMessageKey: "sshHosts.algorithms.riskWeak",
        },
      ],
    });
    client.replaceRoutePlan.mockImplementation(async (input) => ({
      hostId: input.hostId,
      revision: "3",
      ingress: input.ingress,
      jumpHostIds: input.jumpHostIds,
    }));
    client.replaceAlgorithmPolicy.mockImplementation(async (input) => ({
      hostId: input.hostId,
      revision: "6",
      policyId: input.policyId,
      compatibilityExceptions: input.compatibilityExceptions,
    }));
    client.replaceHeartbeatPolicy.mockImplementation(async (input) => ({
      hostId: input.hostId,
      revision: "3",
      policy: input.policy,
    }));
    client.replaceMonitoringPolicy.mockImplementation(async (input) => ({
      policy: {
        hostId: input.hostId,
        revision: "7",
        policy: input.policy,
      },
      runtimeReconciled: true,
    }));
    client.createLoginAutomationSecret.mockImplementation(async (input) => ({
      stagedSecretId: input.operationId,
      label: input.label,
      expiresAtUnixMs: Date.now() + 60_000,
    }));
    client.cancelLoginAutomationSecret.mockImplementation(async () => ({
      cancelled: true,
    }));
    client.cancelHostCreatePassword.mockResolvedValue({ cancelled: true });
    client.stageHostCreatePassword.mockImplementation(async (input) => ({
      stagedPasswordId: input.operationId,
      identityLabel: input.identityLabel,
      credentialLabel: input.credentialLabel,
      expiresAtUnixMs: Date.now() + 60_000,
    }));
    client.replaceLoginAutomation.mockImplementation(async (input) => ({
      hostId: input.hostId,
      revision: "4",
      confirmedRevision: null,
      enabled: input.enabled,
      steps: input.steps.map((step: LoginAutomationStepInput) => {
        if (step.kind === "expect") return step;
        if (step.kind === "sendText") return step;
        return {
          kind: "sendSecret" as const,
          secretLabel: step.kind === "sendSecret" ? step.secretLabel : "Existing secret",
          appendEnter: step.appendEnter,
          timeoutSeconds: step.timeoutSeconds,
        };
      }),
    }));
    client.confirmLoginAutomation.mockResolvedValue({
      hostId: host.hostId,
      revision: "4",
      confirmedRevision: "4",
      enabled: true,
      steps: [],
    });
    client.listSshAgentKeys.mockResolvedValue([{
      keyHandle: "one-use-agent-handle",
      publicKeyAlgorithm: "ssh-ed25519",
      publicKeyFingerprint: "SHA256:agent-test",
      identityKind: "ordinary",
      certificate: null,
      hardwareKeyApplication: null,
      comment: "deploy key",
      expiresAtUnixMs: Date.now() + 60_000,
    }]);
    client.createIdentity.mockResolvedValue({
      identityId: "019d0000-0000-7000-8000-000000000131",
      label: "Agent identity",
      username: "deploy",
      stateVersion: "1",
    });
    client.createSshAgentCredential.mockResolvedValue({
      credentialRefId: "019d0000-0000-7000-8000-000000000132",
      identityId: host.identityId,
      method: "sshAgent",
      priority: 100,
      label: "deploy key",
      details: {
        kind: "sshAgent",
        publicKeyAlgorithm: "ssh-ed25519",
        publicKeyFingerprint: "SHA256:agent-test",
        publicKeyBlob: [1, 2, 3],
        scope: "defaultEnvironment",
      },
      stateVersion: "1",
    });
    client.importPrivateKeyFile.mockResolvedValue({
      credentialRefId: "019d0000-0000-7000-8000-000000000134",
      identityId: host.identityId,
      method: "privateKey",
      priority: 100,
      label: "Imported SSH private key",
      details: {
        kind: "privateKey",
        publicKeyAlgorithm: "ssh-rsa",
        publicKeyFingerprint: "SHA256:aws-key",
      },
      stateVersion: "1",
    });
    client.createKeyboardInteractiveCredential.mockResolvedValue({
      credentialRefId: "019d0000-0000-7000-8000-000000000133",
      identityId: host.identityId,
      method: "keyboardInteractive",
      priority: 110,
      label: "Server interactive authentication",
      details: { kind: "keyboardInteractive", maxRounds: 8 },
      stateVersion: "1",
    });
    client.previewOpenSshConfig.mockResolvedValue({
      snapshotId: "openssh-preview-1",
      expiresAtUnixMs: Date.now() + 300_000,
      candidates: [{
        candidateId: "openssh-candidate-1",
        alias: "staging",
        endpoint: { address: "staging.example.test", port: 22 },
        username: "ops",
        identityFileHints: ["~/.ssh/id_ed25519"],
        route: { kind: "direct" },
        diagnostics: [],
        importable: true,
      }],
      diagnostics: [],
    });
    client.commitOpenSshConfig.mockResolvedValue({
      hosts: [{
        ...host,
        hostId: "019d0000-0000-7000-8000-000000000141",
        label: "staging",
        address: "staging.example.test",
        normalizedAddress: "staging.example.test",
        username: "ops",
        identityId: null,
        favorite: false,
        hasReadyCredential: false,
        stateVersion: "1",
      }],
    });
    client.createHost.mockImplementation(async (input) => ({
      ...host,
      ...input,
      hostId: "019d0000-0000-7000-8000-000000000111",
      normalizedAddress: input.address.toLowerCase(),
      identityId: null,
      favorite: false,
      hasReadyCredential: false,
      stateVersion: "1",
    }));
    client.createConfiguredHost.mockImplementation(async (input) => {
      const createdHost = {
        ...host,
        hostId: "019d0000-0000-7000-8000-000000000111",
        label: input.label,
        address: input.address,
        normalizedAddress: input.address.toLowerCase(),
        port: input.port,
        username: input.username,
        identityId: input.identityId,
        favorite: input.favorite,
        hasReadyCredential: Boolean(input.stagedPasswordId),
        stateVersion: "1",
      };
      return {
        host: createdHost,
        organization: {
          hostId: createdHost.hostId,
          groupId: input.groupId,
          tagIds: input.tagIds,
          hostStateVersion: "1",
        },
        connectionConfig: hostConnectionConfig(),
      };
    });
    client.updateHost.mockImplementation(async (input) => ({
      ...host,
      ...input,
      normalizedAddress: input.address.toLowerCase(),
      hasReadyCredential: true,
      stateVersion: "5",
    }));
    client.deleteHost.mockResolvedValue(undefined);
    client.deleteHostGroup.mockResolvedValue(undefined);
    client.deleteHostTag.mockResolvedValue(undefined);
    client.replaceHostOrganization.mockResolvedValue({
      hostId: host.hostId,
      groupId: null,
      tagIds: [],
      hostStateVersion: "6",
    });
    client.updateHostFavorite.mockImplementation(async (input) => ({
      ...host,
      favorite: input.favorite,
      stateVersion: "5",
    }));
    client.createHostGroup.mockResolvedValue({
      groupId: "019d0000-0000-7000-8000-000000000121",
      label: "Production",
      stateVersion: "1",
    });
    client.createHostTag.mockResolvedValue({
      tagId: "019d0000-0000-7000-8000-000000000122",
      label: "Critical",
      stateVersion: "1",
    });
    client.updateHostGroup.mockImplementation(async (input) => ({
      groupId: input.groupId,
      label: input.label,
      stateVersion: "2",
    }));
    client.updateHostTag.mockImplementation(async (input) => ({
      tagId: input.tagId,
      label: input.label,
      stateVersion: "2",
    }));
  });

  afterEach(() => {
    document.body.innerHTML = "";
  });

  it("keeps dirty native-window drafts until discard is explicitly confirmed", async () => {
    const { wrapper } = await mountView("/hosts?create=1");
    const editor = wrapper.getComponent(NvxHostEditor);
    expect(nativeWindows.open).toHaveBeenCalledWith({ kind: "hostEditor", hostId: null, title: "Add Host", initialSection: "connection" });
    await body().get("#host-address").setValue("unsaved.example.test");
    expect(await (editor.vm.$.exposed as { requestClose(): Promise<boolean> }).requestClose()).toBe(false);
    await flushPromises();
    expect(editor.emitted("cancel")).toBeUndefined();
    await button("Keep editing")?.click();
    await flushPromises();
    expect((body().get("#host-address").element as HTMLInputElement).value).toBe("unsaved.example.test");
    expect(await (editor.vm.$.exposed as { requestClose(): Promise<boolean> }).requestClose()).toBe(false);
    await flushPromises();
    await button("Discard and close")?.click();
    await flushPromises();
    expect(editor.emitted("cancel")).toEqual([[]]);
    expect(document.querySelector("#host-address")).toBeNull();
    expect(client.createConfiguredHost).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("blocks native close during the independent connection probe", async () => {
    let finishProbe!: (result: { verified: boolean }) => void;
    client.testSshConnection.mockImplementationOnce(() => new Promise(resolve => { finishProbe = resolve; }));
    const { wrapper } = await mountView("/hosts?create=1");
    const editor = wrapper.getComponent(NvxHostEditor);
    await chooseSelectOption("host-authentication-mode", "Save password");
    await body().get("#host-address").setValue("probe.example.test");
    await body().get("#host-username").setValue("deploy");
    await body().get("#host-password").setValue("temporary-password");
    await button("Test connection")?.click();
    await flushPromises();
    expect(await (editor.vm.$.exposed as { requestClose(): Promise<boolean> }).requestClose()).toBe(false);
    expect(editor.emitted("cancel")).toBeUndefined();
    expect(body().text()).not.toContain("Discard the changes and close this window?");
    finishProbe({ verified: true });
    await flushPromises();
    wrapper.unmount();
  });

  it("keeps a failed password-create draft open until staged-secret cleanup succeeds", async () => {
    client.createConfiguredHost.mockRejectedValueOnce(new Error("save failed"));
    client.cancelHostCreatePassword.mockRejectedValueOnce(new Error("cleanup unavailable"));
    const { wrapper } = await mountView("/hosts?create=1");
    const editor = wrapper.getComponent(NvxHostEditor);
    await chooseSelectOption("host-authentication-mode", "Save password");
    await body().get("#host-address").setValue("staged.example.test");
    await body().get("#host-password").setValue("temporary-password");
    await button("Save Host")?.click();
    await flushPromises();
    expect(client.stageHostCreatePassword).toHaveBeenCalledTimes(1);
    expect(await (editor.vm.$.exposed as { requestClose(): Promise<boolean> }).requestClose()).toBe(false);
    await flushPromises();
    await button("Discard and close")?.click();
    await flushPromises();
    expect(editor.emitted("cancel")).toBeUndefined();
    expect(document.querySelector("#host-address")).not.toBeNull();
    await button("Discard and close")?.click();
    await flushPromises();
    expect(editor.emitted("cancel")).toEqual([[]]);
    expect(client.cancelHostCreatePassword).toHaveBeenCalledTimes(2);
    expect(client.cancelHostCreatePassword.mock.calls[1]?.[0]).toEqual(client.cancelHostCreatePassword.mock.calls[0]?.[0]);
    wrapper.unmount();
  });

  it("atomically creates a Host using only non-secret endpoint metadata", async () => {
    const createdHost = {
      ...host,
      hostId: "019d0000-0000-7000-8000-000000000111",
      label: "Staging",
      address: "STAGING.example.test",
      normalizedAddress: "staging.example.test",
      port: 2222,
      username: "ops",
      identityId: null,
      favorite: false,
      hasReadyCredential: false,
      stateVersion: "1",
    };
    client.listHostCatalog
      .mockResolvedValueOnce([])
      .mockResolvedValueOnce([])
      .mockResolvedValue([{ ...catalogEntry, host: createdHost }]);
    const { wrapper } = await mountView("/hosts?create=1");

    await body().get("#host-label").setValue("Staging");
    await body().get("#host-address").setValue("STAGING.example.test");
    await body().get("#host-port").setValue("2222");
    await body().get("#host-username").setValue("ops");
    await button("Save Host")?.click();
    await flushPromises();

    expect(client.createConfiguredHost).toHaveBeenCalledWith(expect.objectContaining({
      operationId: expect.any(String),
      idempotencyKey: expect.stringMatching(/^host-configured-create-/),
      label: "Staging",
      address: "STAGING.example.test",
      port: 2222,
      username: "ops",
      identityId: null,
      favorite: false,
      groupId: null,
      tagIds: [],
      ingress: { kind: "directTcp" },
      jumpHostIds: [],
      authenticationMode: "identity",
      credentialRefIds: [],
      algorithmPolicyId: "secure-default",
      compatibilityExceptions: [],
      heartbeatPolicy: { mode: "disabled" },
      loginAutomationEnabled: false,
      loginAutomationConfirmed: false,
      loginAutomationSteps: [],
      stagedPasswordId: null,
    }));
    expect(client.createConfiguredHost.mock.calls[0]?.[0]).not.toHaveProperty("password");
    expect(client.createHost).not.toHaveBeenCalled();
    expect(wrapper.text()).toContain("Staging");
    wrapper.unmount();
  });

  it("requires a successful private-key import before binding its Identity to a new Host", async () => {
    client.listHostCatalog
      .mockResolvedValueOnce([])
      .mockResolvedValueOnce([])
      .mockResolvedValueOnce([]);
    const { wrapper } = await mountView("/hosts?create=1");

    await body().get("#host-label").setValue("AWS key Host");
    await body().get("#host-address").setValue("ec2.example.test");
    await body().get("#host-username").setValue("ec2-user");
    await chooseSelectOption("host-authentication-mode", "Private key");

    expect(body().text()).toContain("Choose an SSH private key");
    await button("Save Host")?.click();
    await flushPromises();
    expect(client.createConfiguredHost).not.toHaveBeenCalled();

    await button("Import private key file")?.click();
    await flushPromises();
    await button("Choose and import file")?.click();
    await flushPromises();

    expect(client.importPrivateKeyFile).toHaveBeenCalledWith(expect.objectContaining({
      identityId: host.identityId,
      passphrase: null,
      priority: 100,
      label: "Imported SSH private key",
    }));
    expect(client.importPrivateKeyFile.mock.calls[0]?.[0]).not.toHaveProperty("path");
    expect(client.importPrivateKeyFile.mock.calls[0]?.[0]).not.toHaveProperty("secret");
    expect(body().text()).toContain("Imported: ssh-rsa · SHA256:aws-key");

    const privateKeyDialog = Array.from(document.querySelectorAll<HTMLElement>('[role="dialog"]'))
      .find((candidate) => candidate.querySelector(".nvx-dialog__title")?.textContent?.trim()
        === "Import SSH private key");
    const closeImport = Array.from(privateKeyDialog?.querySelectorAll<HTMLButtonElement>("button") ?? [])
      .find((candidate) => candidate.textContent?.trim() === "Cancel");
    await closeImport?.click();
    await flushPromises();

    expect(body().text()).toContain("Private key encrypted into the Vault");
    await button("Save Host")?.click();
    await flushPromises();

    expect(client.createConfiguredHost).toHaveBeenCalledWith(expect.objectContaining({
      label: "AWS key Host",
      address: "ec2.example.test",
      username: "ec2-user",
      identityId: host.identityId,
      authenticationMode: "identity",
      credentialRefIds: [],
      stagedPasswordId: null,
    }));
    expect(client.stageHostCreatePassword).not.toHaveBeenCalled();
    expect(client.createConfiguredHost.mock.calls[0]?.[0]).not.toHaveProperty("password");
    expect(client.createConfiguredHost.mock.calls[0]?.[0]).not.toHaveProperty("privateKey");
    wrapper.unmount();
  });

  it("stages a directly entered password in the Vault and consumes only its opaque handle", async () => {
    client.listHostCatalog.mockResolvedValueOnce([]).mockResolvedValueOnce([]);
    const { wrapper } = await mountView("/hosts?create=1");

    await body().get("#host-label").setValue("Password Host");
    await body().get("#host-address").setValue("password.example.test");
    await chooseSelectOption("host-authentication-mode", "Save password");
    await body().get("#host-password").setValue("never-in-final-request");
    await button("Save Host")?.click();
    await flushPromises();

    const stagedRequest = client.stageHostCreatePassword.mock.calls[0]?.[0];
    expect(stagedRequest).toMatchObject({
      operationId: expect.any(String),
      idempotencyKey: expect.stringMatching(/^host-configured-create-/),
      identityLabel: "Password Host · Host login",
      credentialLabel: "Login password",
      password: "never-in-final-request",
    });
    expect(client.createConfiguredHost).toHaveBeenCalledWith(expect.objectContaining({
      operationId: stagedRequest.operationId,
      idempotencyKey: stagedRequest.idempotencyKey,
      identityId: null,
      authenticationMode: "identity",
      credentialRefIds: [],
      stagedPasswordId: stagedRequest.operationId,
    }));
    expect(client.createConfiguredHost.mock.calls[0]?.[0]).not.toHaveProperty("password");
    expect(client.cancelHostCreatePassword).not.toHaveBeenCalled();
    expect(document.querySelector("#host-password")).toBeNull();
    wrapper.unmount();
  });

  it("accepts a short server password and reports a serialized Vault failure without exposing it", async () => {
    client.listHostCatalog.mockResolvedValueOnce([]);
    client.stageHostCreatePassword.mockRejectedValueOnce(JSON.stringify({
      code: "vault.locked",
      messageKey: "errors.vault.locked",
    }));
    const { wrapper } = await mountView("/hosts?create=1");

    await body().get("#host-label").setValue("Short password Host");
    await body().get("#host-address").setValue("short-password.example.test");
    await chooseSelectOption("host-authentication-mode", "Save password");
    await body().get("#host-password").setValue("sen");
    await button("Save Host")?.click();
    await flushPromises();

    expect(client.stageHostCreatePassword).toHaveBeenCalledWith(expect.objectContaining({
      password: "sen",
    }));
    expect(client.createConfiguredHost).not.toHaveBeenCalled();
    expect(body().text()).toContain("Unlock the encrypted Vault before saving a password");
    expect(body().text()).not.toContain("sen");
    wrapper.unmount();
  });

  it("keeps the Host password field available for a temporary test while the Vault is locked", async () => {
    client.fetchVaultStatus.mockResolvedValueOnce({ state: "locked" });
    client.listHostCatalog.mockResolvedValueOnce([]);
    const { wrapper } = await mountView("/hosts?create=1");

    await chooseSelectOption("host-authentication-mode", "Save password");

    expect(document.querySelector("#host-password")).not.toBeNull();
    expect(body().text()).toContain("Unlock the Vault only when saving the Host password");
    expect(button("Unlock Vault")).toBeDefined();
    wrapper.unmount();
  });

  it("uses the independent secure Vault flow without rendering a Vault password in the editor", async () => {
    client.fetchVaultStatus.mockResolvedValue({ state: "missing" });
    const { wrapper } = await mountView("/hosts?create=1");
    await chooseSelectOption("host-authentication-mode", "Save password");
    await button("Create Vault")?.click();
    await flushPromises();
    expect(nativeWindows.vault).toHaveBeenCalledWith("ensureUnlocked");
    expect(document.querySelector("#host-vault-password")).toBeNull();
    expect(client.createVault).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("saves heartbeat and monitoring from the Add Host form", async () => {
    const createdHost = {
      ...host,
      hostId: "019d0000-0000-7000-8000-000000000111",
      stateVersion: "1",
      favorite: false,
      identityId: null,
      hasReadyCredential: false,
    };
    client.listHostCatalog
      .mockResolvedValueOnce([])
      .mockResolvedValueOnce([{ ...catalogEntry, host: createdHost }]);
    const { wrapper } = await mountView("/hosts?create=1");

    await body().get("#host-label").setValue("Staging");
    await body().get("#host-address").setValue("staging.example.test");
    await selectHostEditorSection("Connection Health");
    await body().get("#heartbeat-transport-enabled").setValue(true);
    await body().get("#heartbeat-interval").setValue("45");
    await body().get("#heartbeat-reply-timeout").setValue("12");
    await body().get("#heartbeat-failure-threshold").setValue("4");
    await body().get("#monitoring-enabled").setValue(true);
    await body().get("#monitoring-interval").setValue("20");
    await body().get("#monitoring-timeout").setValue("4");
    await button("Save Host")?.click();
    await flushPromises();

    expect(client.getHostConnectionConfig).not.toHaveBeenCalled();
    expect(client.createConfiguredHost).toHaveBeenCalledWith(expect.objectContaining({
      heartbeatPolicy: {
        mode: "transportKeepalive",
        intervalSeconds: 45,
        replyTimeoutSeconds: 12,
        failureThreshold: 4,
      },
      monitoringPolicy: {
        enabled: true,
        sampleIntervalSeconds: 20,
        sampleTimeoutSeconds: 4,
        diskMountIds: ["root"],
        networkInterfaceIds: ["aggregateNonLoopback"],
      },
    }));
    expect(client.replaceHeartbeatPolicy).not.toHaveBeenCalled();
    expect(client.replaceMonitoringPolicy).not.toHaveBeenCalled();
    expect(document.querySelector("#host-address")).toBeNull();
    wrapper.unmount();
  });

  it("includes route and confirmed non-secret login automation in the atomic create request", async () => {
    const jumpHost = {
      ...host,
      hostId: "019d0000-0000-7000-8000-000000000201",
      label: "Bastion",
      address: "bastion.example.test",
      normalizedAddress: "bastion.example.test",
    };
    client.listHostCatalog
      .mockResolvedValueOnce([{ ...catalogEntry, host: jumpHost }])
      .mockResolvedValueOnce([{ ...catalogEntry, host: jumpHost }])
      .mockResolvedValueOnce([]);
    const { wrapper } = await mountView("/hosts?create=1");

    await body().get("#host-label").setValue("Routed Host");
    await body().get("#host-address").setValue("routed.example.test");
    await selectHostEditorSection("Connection Route");
    await chooseSelectOption("route-ingress", "SOCKS5 proxy");
    await body().get("#route-proxy-address").setValue("proxy.example.test");
    await body().get("#route-proxy-port").setValue("1080");
    await button("Add Jump Host")?.click();
    await flushPromises();

    await selectHostEditorSection("Post-login Commands");
    await body().get("#login-automation-enabled").setValue(true);
    await button("Add step")?.click();
    const stepTypeId = document.querySelector<HTMLElement>(
      '[id^="login-automation-type-"]',
    )?.id;
    if (!stepTypeId) throw new Error("Missing create automation step type");
    await chooseSelectOption(stepTypeId, "Send regular text");
    await body().get('textarea[id^="login-automation-value-"]').setValue("cd /srv/app");
    await body().get("#login-automation-confirm-on-save").setValue(true);
    await button("Save Host")?.click();
    await flushPromises();

    expect(client.createConfiguredHost).toHaveBeenCalledWith(expect.objectContaining({
      ingress: {
        kind: "socks5Proxy",
        endpoint: {
          address: "proxy.example.test",
          normalizedAddress: "proxy.example.test",
          port: 1080,
        },
        dnsMode: "proxy",
        proxyAuthCredentialRefId: null,
      },
      jumpHostIds: [jumpHost.hostId],
      loginAutomationEnabled: true,
      loginAutomationConfirmed: true,
      loginAutomationSteps: [{
        kind: "sendText",
        text: "cd /srv/app",
        appendEnter: true,
        timeoutSeconds: 10,
      }],
    }));
    expect(client.replaceRoutePlan).not.toHaveBeenCalled();
    expect(client.replaceLoginAutomation).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("reloads the current state fence in the independent editor and preserves the Identity reference", async () => {
    client.listHostCatalog
      .mockResolvedValueOnce([catalogEntry])
      .mockResolvedValue([{
        ...catalogEntry,
        host: { ...host, label: "Production primary", port: 2200, stateVersion: "5" },
      }]);
    const { wrapper } = await mountView();

    await button("Edit")?.click();
    await flushPromises();
    await body().get("#host-label").setValue("Production primary");
    await body().get("#host-port").setValue("2200");
    await button("Save Changes")?.click();
    await flushPromises();

    expect(client.updateHost).toHaveBeenCalledWith({
      hostId: host.hostId,
      expectedStateVersion: "5",
      label: "Production primary",
      address: host.address,
      port: 2200,
      username: host.username,
      identityId: host.identityId,
      favorite: true,
    });
    expect(wrapper.text()).toContain("Production primary");
    wrapper.unmount();
  });

  it("saves a SOCKS5 ingress and ordered Jump Hosts without mixing descriptions into the control grid", async () => {
    const jumpOne = {
      ...host,
      hostId: "019d0000-0000-7000-8000-000000000201",
      label: "Bastion one",
      address: "bastion-one.example.test",
      normalizedAddress: "bastion-one.example.test",
      stateVersion: "1",
    };
    const jumpTwo = {
      ...host,
      hostId: "019d0000-0000-7000-8000-000000000202",
      label: "Bastion two",
      address: "bastion-two.example.test",
      normalizedAddress: "bastion-two.example.test",
      stateVersion: "1",
    };
    client.listHostCatalog.mockResolvedValue([
      catalogEntry,
      { ...catalogEntry, host: jumpOne },
      { ...catalogEntry, host: jumpTwo },
    ]);
    const { wrapper } = await mountView();

    await button("Connection Route")?.click();
    await flushPromises();
    expect(client.getHostConnectionConfig).toHaveBeenCalledWith(host.hostId);
    expect(document.querySelector(".hosts-route__control-grid .hosts-route__hint")).toBeNull();

    await chooseSelectOption("route-ingress", "SOCKS5 proxy");
    await body().get("#route-proxy-address").setValue("proxy.example.test");
    await body().get("#route-proxy-port").setValue("1080");
    await chooseSelectOption("route-proxy-credential", "deploy · Proxy password");
    await button("Add Jump Host")?.click();
    await flushPromises();
    await button("Add Jump Host")?.click();
    await flushPromises();
    await button("Save Changes")?.click();
    await flushPromises();

    expect(client.replaceRoutePlan).toHaveBeenCalledWith({
      hostId: host.hostId,
      expectedRevision: "2",
      ingress: {
        kind: "socks5Proxy",
        endpoint: {
          address: "proxy.example.test",
          normalizedAddress: "proxy.example.test",
          port: 1080,
        },
        dnsMode: "proxy",
        proxyAuthCredentialRefId: "019d0000-0000-7000-8000-000000000103",
      },
      jumpHostIds: [jumpOne.hostId, jumpTwo.hostId],
    });
    expect(document.querySelector("#route-ingress")).toBeNull();
    wrapper.unmount();
  });

  it("loads the Core algorithm catalog and saves only a catalog-owned Host exception", async () => {
    const { wrapper } = await mountView();

    await button("Edit")?.click();
    await flushPromises();
    await selectHostEditorSection("Algorithms");
    expect(client.getAlgorithmPolicyCatalog).toHaveBeenCalledTimes(1);
    expect(client.getHostConnectionConfig).toHaveBeenCalledWith(host.hostId);
    expect(document.body.textContent).toContain("curve25519-sha256");
    expect(document.body.textContent).toContain("diffie-hellman-group14-sha1");

    await body().get("#algorithm-compat-kex-dh-group14-sha1").setValue(true);
    await button("Save Changes")?.click();
    await flushPromises();

    expect(client.replaceAlgorithmPolicy).toHaveBeenCalledWith({
      hostId: host.hostId,
      expectedRevision: "5",
      policyId: "secure-default",
      compatibilityExceptions: [{
        category: "keyExchange",
        exceptionId: "compat-kex-dh-group14-sha1",
        reason: null,
      }],
    });
    expect(document.querySelector("#algorithm-compat-kex-dh-group14-sha1")).toBeNull();
    wrapper.unmount();
  });

  it("loads and saves bounded SSH transport keepalive fields with descriptions outside the Select grid", async () => {
    const { wrapper } = await mountView();

    await button("Edit")?.click();
    await flushPromises();
    await selectHostEditorSection("Connection Health");
    expect(client.getHostConnectionConfig).toHaveBeenCalledWith(host.hostId);
    expect(document.querySelector(".hosts-health__three-column-grid .hosts-heartbeat__hint")).toBeNull();

    await body().get("#heartbeat-transport-enabled").setValue(true);
    await body().get("#heartbeat-interval").setValue("45");
    await body().get("#heartbeat-reply-timeout").setValue("12");
    await body().get("#heartbeat-failure-threshold").setValue("4");
    await button("Save Changes")?.click();
    await flushPromises();

    expect(client.replaceHeartbeatPolicy).toHaveBeenCalledWith({
      hostId: host.hostId,
      expectedRevision: "2",
      policy: {
        mode: "transportKeepalive",
        intervalSeconds: 45,
        replyTimeoutSeconds: 12,
        failureThreshold: 4,
      },
    });
    wrapper.unmount();
  });

  it("shows an escaped shell heartbeat preview and saves one fixed non-secret payload", async () => {
    client.getHostConnectionConfig.mockResolvedValue(hostConnectionConfig({
      heartbeatPolicy: {
        hostId: host.hostId,
        revision: "8",
        policy: {
          mode: "shellHeartbeat",
          payloadText: "printf ready",
          lineEnding: "lf",
          intervalSeconds: 60,
          userIdleSeconds: 15,
        },
      },
    }));
    const { wrapper } = await mountView();

    await button("Edit")?.click();
    await flushPromises();
    await selectHostEditorSection("Advanced");
    expect(body().text()).toContain("printf ready\\n");
    await body().get("#heartbeat-payload").setValue("echo alive");
    await chooseSelectOption("heartbeat-line-ending", "CRLF (\\r\\n)");
    await button("Save Changes")?.click();
    await flushPromises();

    expect(client.replaceHeartbeatPolicy).toHaveBeenCalledWith({
      hostId: host.hostId,
      expectedRevision: "8",
      policy: {
        mode: "shellHeartbeat",
        payloadText: "echo alive",
        lineEnding: "crlf",
        intervalSeconds: 60,
        userIdleSeconds: 15,
      },
    });
    wrapper.unmount();
  });

  it("enables bounded independent server monitoring without free-form resource identifiers", async () => {
    const { wrapper } = await mountView();

    await button("Edit")?.click();
    await flushPromises();
    await selectHostEditorSection("Connection Health");
    expect(client.getHostConnectionConfig).toHaveBeenCalledWith(host.hostId);
    expect(document.querySelector(".hosts-monitoring__control-grid .hosts-monitoring__hint")).toBeNull();

    await body().get("#monitoring-enabled").setValue(true);
    await body().get("#monitoring-interval").setValue("20");
    await body().get("#monitoring-timeout").setValue("4");
    await button("Save Changes")?.click();
    await flushPromises();

    expect(client.replaceMonitoringPolicy).toHaveBeenCalledWith({
      hostId: host.hostId,
      expectedRevision: "6",
      policy: {
        enabled: true,
        sampleIntervalSeconds: 20,
        sampleTimeoutSeconds: 4,
        diskMountIds: ["root"],
        networkInterfaceIds: ["aggregateNonLoopback"],
      },
    });
    wrapper.unmount();
  });

  it("keeps the monitoring dialog open when persistence succeeds before runtime reconciliation", async () => {
    client.replaceMonitoringPolicy.mockImplementationOnce(async (input) => ({
      policy: {
        hostId: input.hostId,
        revision: "8",
        policy: input.policy,
      },
      runtimeReconciled: false,
    }));
    const { wrapper } = await mountView();

    await button("Edit")?.click();
    await flushPromises();
    await selectHostEditorSection("Connection Health");
    await body().get("#monitoring-enabled").setValue(true);
    await button("Save Changes")?.click();
    await flushPromises();

    expect(document.querySelector("#monitoring-enabled")).not.toBeNull();
    expect(document.body.textContent).toContain(
      "The policy was saved, but the active monitoring resources have not reconciled yet.",
    );
    wrapper.unmount();
  });

  it("lets the user change the favorite state from the Host form", async () => {
    const { wrapper } = await mountView();

    await button("Edit")?.click();
    await flushPromises();
    await body().get("#host-favorite").setValue(false);
    await button("Save Changes")?.click();
    await flushPromises();

    expect(client.updateHost).toHaveBeenCalledWith(expect.objectContaining({
      hostId: host.hostId,
      expectedStateVersion: host.stateVersion,
      identityId: host.identityId,
      favorite: false,
    }));
    wrapper.unmount();
  });

  it("renders a filled star for a favorite Host", async () => {
    const { wrapper } = await mountView();

    expect(wrapper.get(".hosts-list__favorite").attributes("aria-pressed")).toBe("true");
    expect(wrapper.get(".hosts-list__favorite svg").classes()).toContain(
      "hosts-list__favorite-icon--filled",
    );

    wrapper.unmount();
  });

  it("groups the Host list and keeps classifications beside the Host name", async () => {
    const productionGroup = {
      groupId: "019d0000-0000-7000-8000-000000000201",
      label: "Production",
      stateVersion: "1",
    };
    const personalGroup = {
      groupId: "019d0000-0000-7000-8000-000000000202",
      label: "Personal",
      stateVersion: "1",
    };
    client.listHostCatalog.mockResolvedValue([
      {
        ...catalogEntry,
        group: productionGroup,
        tags: [{
          tagId: "019d0000-0000-7000-8000-000000000203",
          label: "Critical",
          stateVersion: "1",
        }],
      },
      {
        ...catalogEntry,
        host: { ...host, hostId: "019d0000-0000-7000-8000-000000000204", label: "Laptop" },
        group: personalGroup,
      },
    ]);
    const { wrapper } = await mountView();

    const sections = wrapper.findAll(".hosts-list__section");
    expect(sections).toHaveLength(2);
    expect(wrapper.text()).toContain("Personal · 1 hosts");
    expect(wrapper.text()).toContain("Production · 1 hosts");
    const productionRow = sections.find((section) => section.text().includes("Production"))!;
    expect(productionRow.get(".hosts-list__title-row").text()).toContain("Production");
    expect(productionRow.get(".hosts-list__title-row").text()).toContain("Critical");
    expect(productionRow.find(".hosts-list__identity > .hosts-list__classification").exists()).toBe(false);

    wrapper.unmount();
  });

  it("reads the standard Agent only on explicit action and saves the selected public key", async () => {
    const { tips, wrapper } = await mountView();

    await button("SSH Agent")?.click();
    expect(client.listSshAgentKeys).not.toHaveBeenCalled();
    await button("Read Agent")?.click();
    await flushPromises();
    await body().get(".hosts-agent__key").trigger("click");
    await button("Save Agent credential")?.click();
    await flushPromises();

    expect(client.listSshAgentKeys).toHaveBeenCalledTimes(1);
    expect(client.createSshAgentCredential).toHaveBeenCalledWith({
      identityId: host.identityId,
      keyHandle: "one-use-agent-handle",
      expectedIdentityKind: "ordinary",
      priority: 100,
      label: "SSH Agent ssh-ed25519 key",
    });
    expect(tips.items).toEqual(expect.arrayContaining([
      expect.objectContaining({
        scope: "hosts-agent",
        tone: "success",
        title: "SSH Agent credential saved",
      }),
    ]));
    wrapper.unmount();
  });

  it("imports a selected private-key file through Core and shows its derived fingerprint", async () => {
    const { tips, wrapper } = await mountView();

    await button("Edit")?.click();
    await flushPromises();
    await button("Import private key file")?.click();
    await flushPromises();
    await button("Choose and import file")?.click();
    await flushPromises();

    expect(client.importPrivateKeyFile).toHaveBeenCalledWith(expect.objectContaining({
      identityId: host.identityId,
      passphrase: null,
      priority: 100,
      label: "Imported SSH private key",
    }));
    expect(client.importPrivateKeyFile.mock.calls[0]?.[0]).not.toHaveProperty("path");
    expect(client.importPrivateKeyFile.mock.calls[0]?.[0]).not.toHaveProperty("secret");
    expect(body().text()).toContain("Imported: ssh-rsa · SHA256:aws-key");
    expect(tips.items).toEqual(expect.arrayContaining([
      expect.objectContaining({
        scope: "host-editor-private-key",
        tone: "success",
        title: "Private key encrypted into the Vault",
      }),
    ]));
    wrapper.unmount();
  });

  it("identifies Agent certificates and binds the saved credential to that exact kind", async () => {
    client.listSshAgentKeys.mockResolvedValueOnce([{
      keyHandle: "certificate-handle",
      publicKeyAlgorithm: "ssh-ed25519",
      publicKeyFingerprint: "SHA256:subject",
      identityKind: "certificate",
      hardwareKeyApplication: null,
      comment: null,
      expiresAtUnixMs: Date.now() + 60_000,
      certificate: {
        source: "systemSshAgent",
        certificateBlob: [1, 2, 3],
        certificateAlgorithm: "ssh-ed25519-cert-v01@openssh.com",
        certificateFingerprint: "SHA256:certificate",
        serial: "42",
        subjectPublicKeyBlob: [4, 5, 6],
        subjectPublicKeyAlgorithm: "ssh-ed25519",
        subjectPublicKeyFingerprint: "SHA256:subject",
        caPublicKeyFingerprint: "SHA256:ca",
        keyId: "release-operator",
        validPrincipals: [],
        certificateType: "user",
        validAfterUnixSeconds: 1,
        validBeforeUnixSeconds: null,
        criticalOptions: [],
        extensions: [],
      },
    }]);
    const { wrapper } = await mountView();

    await button("SSH Agent")?.click();
    await button("Read Agent")?.click();
    await flushPromises();
    expect(body().text()).toContain("OpenSSH user certificate");
    expect(body().text()).toContain("Certificate serial 42");
    expect(body().text()).toContain("Any principal (still limited by server policy)");
    await body().get(".hosts-agent__key").trigger("click");
    await button("Save Agent credential")?.click();
    await flushPromises();

    expect(client.createSshAgentCredential).toHaveBeenCalledWith(expect.objectContaining({
      keyHandle: "certificate-handle",
      expectedIdentityKind: "certificate",
    }));
    wrapper.unmount();
  });

  it("identifies FIDO2 Agent keys and preserves the hardware-key kind and application", async () => {
    client.listSshAgentKeys.mockResolvedValueOnce([{
      keyHandle: "hardware-key-handle",
      publicKeyAlgorithm: "sk-ssh-ed25519@openssh.com",
      publicKeyFingerprint: "SHA256:hardware-key",
      identityKind: "hardwareKey",
      hardwareKeyApplication: "ssh:norishell",
      comment: null,
      expiresAtUnixMs: Date.now() + 60_000,
      certificate: null,
    }]);
    const { wrapper } = await mountView();

    await button("SSH Agent")?.click();
    await button("Read Agent")?.click();
    await flushPromises();
    expect(body().text()).toContain("FIDO2 hardware key");
    expect(body().text()).toContain("FIDO2 application: ssh:norishell");
    await body().get(".hosts-agent__key").trigger("click");
    await button("Save Agent credential")?.click();
    await flushPromises();

    expect(client.createSshAgentCredential).toHaveBeenCalledWith(expect.objectContaining({
      keyHandle: "hardware-key-handle",
      expectedIdentityKind: "hardwareKey",
    }));
    wrapper.unmount();
  });

  it("creates a bounded keyboard-interactive policy without collecting an answer", async () => {
    const { tips, wrapper } = await mountView();

    await button("Interactive auth")?.click();
    await body().get("#keyboard-interactive-max-rounds").setValue("12");
    await body().get("#keyboard-interactive-priority").setValue("115");
    await button("Save interactive credential")?.click();
    await flushPromises();

    expect(client.createKeyboardInteractiveCredential).toHaveBeenCalledWith({
      identityId: host.identityId,
      maxRounds: 12,
      priority: 115,
      label: "Server interactive authentication",
    });
    expect(tips.items).toEqual(expect.arrayContaining([
      expect.objectContaining({
        scope: "hosts-keyboard-interactive",
        tone: "success",
        title: "Interactive credential saved",
      }),
    ]));
    expect(document.querySelector<HTMLInputElement>("input[type='password']")).toBeNull();
    wrapper.unmount();
  });

  it("previews pasted OpenSSH text before atomically importing selected direct Hosts", async () => {
    const { tips, wrapper } = await mountView();

    await button("Import SSH Config")?.click();
    expect(client.previewOpenSshConfig).not.toHaveBeenCalled();
    const configText = "Host staging\n  HostName staging.example.test\n  User ops";
    await body().get("#openssh-config-text").setValue(configText);
    await button("Safe preview")?.click();
    await flushPromises();
    expect(client.previewOpenSshConfig).toHaveBeenCalledWith(configText);
    expect(body().text()).toContain("staging.example.test:22");

    await button("Import 1 selected")?.click();
    await flushPromises();
    expect(client.commitOpenSshConfig).toHaveBeenCalledWith(
      "openssh-preview-1",
      ["openssh-candidate-1"],
    );
    expect(tips.items).toEqual(expect.arrayContaining([
      expect.objectContaining({
        scope: "hosts-openssh-import",
        tone: "success",
        title: "Atomically imported 1 host(s)",
      }),
    ]));
    wrapper.unmount();
  });

  it("opens the OpenSSH import dialog from the terminal launcher route intent", async () => {
    const { wrapper } = await mountView("/hosts?importSshConfig=1");

    expect(document.querySelector("#openssh-config-text")).not.toBeNull();
    expect(body().text()).toContain("Import SSH Config");
    wrapper.unmount();
  });

  it("shows the exact safely mapped non-direct route before selection is committed", async () => {
    client.previewOpenSshConfig.mockResolvedValueOnce({
      snapshotId: "openssh-preview-route",
      expiresAtUnixMs: Date.now() + 300_000,
      candidates: [
        {
          candidateId: "openssh-candidate-route",
          alias: "proxied",
          endpoint: { address: "target.example.test", port: 22 },
          username: null,
          identityFileHints: [],
          route: {
            kind: "socks5",
            proxy: { address: "2001:db8::10", port: 1080 },
          },
          diagnostics: [],
          importable: true,
        },
        {
          candidateId: "openssh-candidate-jump",
          alias: "jumped",
          endpoint: { address: "jumped.example.test", port: 22 },
          username: "deploy",
          identityFileHints: [],
          route: {
            kind: "jumpChain",
            hops: [{
              endpoint: { address: "jump.example.test", port: 2222 },
              username: "jump-user",
            }],
          },
          diagnostics: [],
          importable: true,
        },
      ],
      diagnostics: [],
    });
    const { wrapper } = await mountView();

    await button("Import SSH Config")?.click();
    await body().get("#openssh-config-text").setValue(
      "Host proxied\nProxyCommand /usr/bin/nc -X 5 -x [2001:db8::10]:1080 %h %p",
    );
    await button("Safe preview")?.click();
    await flushPromises();

    expect(body().text()).toContain("Route: SOCKS5 via [2001:db8::10]:1080 (proxy DNS)");
    expect(body().text()).toContain("Route: Jump Chain: jump-user@jump.example.test:2222");
    expect(button("Import 2 selected")?.disabled).toBe(false);
    wrapper.unmount();
  });

  it("creates and assigns non-secret Group and Tag metadata", async () => {
    const { wrapper } = await mountView();

    await button("Edit")?.click();
    await flushPromises();
    await selectHostEditorSection("Classification");
    await body().get('input[placeholder="Group name"]').setValue("Production");
    await button("New group")?.click();
    await flushPromises();
    await body().get('input[placeholder="Tag name"]').setValue("Critical");
    await button("New tag")?.click();
    await flushPromises();
    await button("Save Changes")?.click();
    await flushPromises();

    expect(client.createHostGroup).toHaveBeenCalledWith("Production");
    expect(client.createHostTag).toHaveBeenCalledWith("Critical");
    expect(client.replaceHostOrganization).toHaveBeenCalledWith({
      hostId: host.hostId,
      expectedHostStateVersion: "5",
      groupId: "019d0000-0000-7000-8000-000000000121",
      tagIds: ["019d0000-0000-7000-8000-000000000122"],
    });
    wrapper.unmount();
  });

  it("renames and deletes classification metadata through explicit actions", async () => {
    const group = {
      groupId: "019d0000-0000-7000-8000-000000000121",
      label: "Production",
      stateVersion: "1",
    };
    client.listHostGroups.mockResolvedValue([group]);
    const { wrapper } = await mountView();

    await button("Manage groups and tags")?.click();
    await flushPromises();
    await button("Rename")?.click();
    await body().get("#classification-label").setValue("Core production");
    await button("Save name")?.click();
    await flushPromises();

    expect(client.updateHostGroup).toHaveBeenCalledWith({
      groupId: group.groupId,
      expectedStateVersion: group.stateVersion,
      label: "Core production",
    });

    const classificationRow = body().findAll(".hosts-classification-list__row")[0];
    await classificationRow?.findAll("button")[1]?.trigger("click");
    await button("Delete classification")?.click();
    await flushPromises();

    expect(client.deleteHostGroup).toHaveBeenCalledWith(group.groupId, group.stateVersion);
    wrapper.unmount();
  });

  it("creates and renames Groups in the classification manager", async () => {
    client.createHostGroup.mockImplementation(async (label) => ({
      groupId: "019d0000-0000-7000-8000-000000000121",
      label,
      stateVersion: "1",
    }));
    const { wrapper } = await mountView();

    await button("Manage groups and tags")?.click();
    await flushPromises();
    await body().get("#classification-new-group-label").setValue("Staging");
    await button("New group")?.click();
    await flushPromises();

    expect(client.createHostGroup).toHaveBeenCalledWith("Staging");
    expect(body().text()).toContain("Staging");

    await button("Rename")?.click();
    await body().get("#classification-label").setValue("Pre-production");
    await button("Save name")?.click();
    await flushPromises();

    expect(client.updateHostGroup).toHaveBeenCalledWith({
      groupId: "019d0000-0000-7000-8000-000000000121",
      expectedStateVersion: "1",
      label: "Pre-production",
    });
    wrapper.unmount();
  });

  it("toggles a favorite without resending endpoint fields", async () => {
    const { wrapper } = await mountView();

    await body().get('button[aria-label="Remove from favorites"]').trigger("click");
    await flushPromises();

    expect(client.updateHostFavorite).toHaveBeenCalledWith({
      hostId: host.hostId,
      expectedStateVersion: host.stateVersion,
      favorite: false,
    });
    expect(client.updateHost).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("saves a secret-safe login plan through the unified Host editor", async () => {
    const { wrapper } = await mountView();

    await button("Post-login commands")?.click();
    await flushPromises();
    await body().get("#login-automation-enabled").setValue(true);
    await button("Add step")?.click();
    const firstValue = document.querySelector<HTMLInputElement>(
      'input[id^="login-automation-value-"]',
    );
    if (!firstValue) throw new Error("Missing Expect value input");
    await new DOMWrapper(firstValue).setValue("Password:");

    await button("Add step")?.click();
    const typeSelects = Array.from(document.querySelectorAll<HTMLElement>(
      '[id^="login-automation-type-"]',
    ));
    const secondTypeId = typeSelects[1]?.id;
    if (!secondTypeId) throw new Error("Missing second step type Select");
    await chooseSelectOption(secondTypeId, "Send Vault secret");
    await body().get('input[id^="login-automation-secret-label-"]').setValue("Deploy password");
    await body().get('input[id^="login-automation-secret-value-"]').setValue("one-time-test-secret");

    await button("Save Changes")?.click();
    await flushPromises();
    expect(client.createLoginAutomationSecret).toHaveBeenCalledWith({
      operationId: expect.any(String),
      idempotencyKey: expect.stringMatching(/^login-automation-secret-/),
      hostId: host.hostId,
      expectedAutomationRevision: "3",
      label: "Deploy password",
      value: "one-time-test-secret",
    });
    expect(client.replaceLoginAutomation).toHaveBeenCalledWith({
      hostId: host.hostId,
      expectedRevision: "3",
      enabled: true,
      steps: [
        { kind: "expect", literalText: "Password:", timeoutSeconds: 10 },
        {
          kind: "sendSecret",
          stagedSecretId: expect.any(String),
          secretLabel: "Deploy password",
          appendEnter: true,
          timeoutSeconds: 10,
        },
      ],
    });
    expect(client.confirmLoginAutomation).not.toHaveBeenCalled();
    expect(document.querySelector("#host-address")).toBeNull();
    wrapper.unmount();
  });

  it("closes after a lost replace response reports the staged secret already consumed", async () => {
    client.replaceLoginAutomation.mockRejectedValueOnce(new Error("revision changed"));
    client.cancelLoginAutomationSecret.mockResolvedValueOnce({ cancelled: false });
    const { wrapper } = await mountView();

    await button("Post-login commands")?.click();
    await flushPromises();
    await body().get("#login-automation-enabled").setValue(true);
    await button("Add step")?.click();
    const typeId = document.querySelector<HTMLElement>('[id^="login-automation-type-"]')?.id;
    if (!typeId) throw new Error("Missing step type Select");
    await chooseSelectOption(typeId, "Send Vault secret");
    await body().get('input[id^="login-automation-secret-label-"]').setValue("Deploy password");
    await body().get('input[id^="login-automation-secret-value-"]').setValue("temporary-secret");
    client.cancelLoginAutomationSecret.mockClear();

    await button("Save Changes")?.click();
    await flushPromises();
    const operationId = client.createLoginAutomationSecret.mock.calls[0]?.[0].operationId;
    expect(operationId).toEqual(expect.any(String));
    expect(client.cancelLoginAutomationSecret).not.toHaveBeenCalled();

    await button("Cancel")?.click();
    await flushPromises();
    await button("Discard and close")?.click();
    await flushPromises();
    expect(client.cancelLoginAutomationSecret).toHaveBeenCalledWith({
      operationId,
      idempotencyKey: `login-automation-secret-${operationId}`,
    });
    expect(document.querySelector('[role="dialog"]')).toBeNull();
    wrapper.unmount();
  });

  it("cleans a failed create identity before rotating it for an explicit retry", async () => {
    client.createLoginAutomationSecret
      .mockRejectedValueOnce(new Error("create response lost"))
      .mockImplementationOnce(async (input) => ({
        stagedSecretId: input.operationId,
        label: input.label,
        expiresAtUnixMs: Date.now() + 60_000,
      }));
    const { wrapper } = await mountView();

    await button("Post-login commands")?.click();
    await flushPromises();
    await body().get("#login-automation-enabled").setValue(true);
    await button("Add step")?.click();
    const typeId = document.querySelector<HTMLElement>('[id^="login-automation-type-"]')?.id;
    if (!typeId) throw new Error("Missing step type Select");
    await chooseSelectOption(typeId, "Send Vault secret");
    await body().get('input[id^="login-automation-secret-label-"]').setValue("Deploy password");
    await body().get('input[id^="login-automation-secret-value-"]').setValue("retry-secret");

    await button("Save Changes")?.click();
    await flushPromises();
    const firstOperationId = client.createLoginAutomationSecret.mock.calls[0]?.[0].operationId;
    expect(firstOperationId).toEqual(expect.any(String));
    expect(client.cancelLoginAutomationSecret).toHaveBeenCalledWith({
      operationId: firstOperationId,
      idempotencyKey: `login-automation-secret-${firstOperationId}`,
    });
    expect(document.querySelector<HTMLInputElement>(
      'input[id^="login-automation-secret-value-"]',
    )?.value).toBe("retry-secret");
    expect(client.createLoginAutomationSecret).toHaveBeenCalledTimes(1);

    await button("Save Changes")?.click();
    await flushPromises();
    const retryRequest = client.createLoginAutomationSecret.mock.calls[1]?.[0];
    expect(retryRequest.operationId).not.toBe(firstOperationId);
    expect(retryRequest).toMatchObject({
      idempotencyKey: `login-automation-secret-${retryRequest.operationId}`,
      value: "retry-secret",
    });
    expect(client.replaceLoginAutomation).toHaveBeenCalledTimes(1);
    wrapper.unmount();
  });

  it("keeps the failed create identity when cleanup is uncertain", async () => {
    client.createLoginAutomationSecret.mockRejectedValue(new Error("needs reconciliation"));
    const { wrapper } = await mountView();

    await button("Post-login commands")?.click();
    await flushPromises();
    await body().get("#login-automation-enabled").setValue(true);
    await button("Add step")?.click();
    const typeId = document.querySelector<HTMLElement>('[id^="login-automation-type-"]')?.id;
    if (!typeId) throw new Error("Missing step type Select");
    await chooseSelectOption(typeId, "Send Vault secret");
    await body().get('input[id^="login-automation-secret-label-"]').setValue("Deploy password");
    await body().get('input[id^="login-automation-secret-value-"]').setValue("retry-secret");
    client.cancelLoginAutomationSecret.mockRejectedValueOnce(new Error("cleanup uncertain"));

    await button("Save Changes")?.click();
    await flushPromises();
    const firstOperationId = client.createLoginAutomationSecret.mock.calls[0]?.[0].operationId;

    await button("Save Changes")?.click();
    await flushPromises();
    expect(client.createLoginAutomationSecret.mock.calls[1]?.[0].operationId)
      .toBe(firstOperationId);
    expect(document.querySelector<HTMLInputElement>(
      'input[id^="login-automation-secret-value-"]',
    )?.value).toBe("retry-secret");
    wrapper.unmount();
  });

  it("deletes only after confirmation and removes the matching Host projection", async () => {
    const { tips, wrapper } = await mountView();
    await button("Delete")?.click();
    expect(client.deleteHost).not.toHaveBeenCalled();
    await button("Delete Host")?.click();
    await flushPromises();

    expect(client.deleteHost).toHaveBeenCalledWith(host.hostId, host.stateVersion);
    expect(wrapper.text()).not.toContain("Production");
    expect(tips.items).toEqual(expect.arrayContaining([
      expect.objectContaining({ tone: "success", title: "Host deleted" }),
    ]));
    wrapper.unmount();
  });

  it("navigates to the kept-alive Terminal route with an explicit hostId intent", async () => {
    const { router, wrapper } = await mountView();
    await button("Connect")?.click();
    await flushPromises();

    expect(router.currentRoute.value.path).toBe("/terminal");
    expect(router.currentRoute.value.query).toEqual({ hostId: host.hostId });
    wrapper.unmount();
  });

  it("tests the unsaved password form in place without creating a session", async () => {
    const { router, tips, wrapper } = await mountView();
    await button("Add Host")?.click();
    await flushPromises();
    await body().get("#host-address").setValue("test.example");
    await body().get("#host-username").setValue("tester");
    await chooseSelectOption("host-authentication-mode", "Save password");
    await body().get("#host-password").setValue("temporary-password");
    await button("Test connection")?.click();
    await flushPromises();

    expect(client.prepareTransientCredential).toHaveBeenCalledWith({
      kind: "password",
      secret: "temporary-password",
    });
    expect(client.testSshConnection).toHaveBeenCalledWith({
      endpoint: { address: "test.example", port: 22, username: "tester" },
      credentialRefId: "019d0000-0000-7000-8000-000000000104",
    });
    expect(router.currentRoute.value.path).toBe("/hosts");
    expect(document.body.textContent).toContain("Add Host");
    expect(tips.items).toEqual(expect.arrayContaining([
      expect.objectContaining({ tone: "success", title: "Connection test succeeded" }),
    ]));
    expect(client.createConfiguredHost).not.toHaveBeenCalled();
    expect(client.updateHost).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("keeps the Host editor open and shows the Core failure reason", async () => {
    client.testSshConnection.mockRejectedValue({
      code: "ssh_connection_test.authentication_rejected",
      messageKey: "errors.sshSession.authenticationRejected",
    });
    const { router, tips, wrapper } = await mountView();
    await button("Add Host")?.click();
    await flushPromises();
    await body().get("#host-address").setValue("test.example");
    await body().get("#host-username").setValue("tester");
    await chooseSelectOption("host-authentication-mode", "Save password");
    await body().get("#host-password").setValue("wrong-password");
    await button("Test connection")?.click();
    await flushPromises();

    expect(router.currentRoute.value.path).toBe("/hosts");
    expect(document.body.textContent).toContain("Add Host");
    expect(document.querySelector<HTMLInputElement>("#host-password")?.value)
      .toBe("wrong-password");
    expect(tips.items).toEqual(expect.arrayContaining([
      expect.objectContaining({
        tone: "error",
        title: "Connection test failed",
        message: "The server rejected the credential. Check the authentication details.",
      }),
    ]));
    wrapper.unmount();
  });

  it("allows a temporary password test while the Vault is locked", async () => {
    client.fetchVaultStatus.mockResolvedValue({ state: "locked" });
    const { wrapper } = await mountView();
    await button("Add Host")?.click();
    await flushPromises();
    await chooseSelectOption("host-authentication-mode", "Save password");

    expect(document.querySelector<HTMLInputElement>("#host-password")).not.toBeNull();
    expect(document.body.textContent).toContain("Unlock the encrypted Vault");
    wrapper.unmount();
  });
});
