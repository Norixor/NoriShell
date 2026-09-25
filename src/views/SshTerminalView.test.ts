import { DOMWrapper, flushPromises, mount } from "@vue/test-utils";
import { createPinia } from "pinia";
import { createMemoryHistory, createRouter, RouterView } from "vue-router";
import { defineComponent, h, KeepAlive, onBeforeUnmount } from "vue";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type {
  HostSummary,
  LocalSessionSummary,
  SshSessionSummary,
  TelnetSessionSummary,
} from "../core-api/generated/core-api";
import NvxWorkspaceTabBar from "../components/layout/NvxWorkspaceTabBar.vue";
import { i18n } from "../locales";
import { useWorkspaceTabsStore } from "../stores/workspaceTabs";
import { useTipsStore } from "../stores/tips";
import {
  focusedTerminalLabel,
  focusTerminalInputTarget,
  registerTerminalInputTarget,
  resetTerminalInputFocusForTests,
  runInFocusedTerminal,
} from "../terminal-input-target";
import { flushTerminalWorkspaceBeforeExit } from "../terminal-workspace-persistence";
import { createSftpTerminalLaunch } from "./sftpTerminalLaunch";

const defaultNavigatorPlatform = navigator.platform;
const nativeEvents = vi.hoisted(() => new Map<string, (event: { payload: unknown }) => void>());
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (name: string, callback: (event: { payload: unknown }) => void) => {
    nativeEvents.set(name, callback);
    return () => nativeEvents.delete(name);
  }),
}));

const client = vi.hoisted(() => ({
  changeTerminalInputFocus: vi.fn(),
  createHost: vi.fn(),
  createIdentity: vi.fn(),
  createVault: vi.fn(),
  fetchVaultStatus: vi.fn(),
  fetchLocalSessionSnapshot: vi.fn(),
  fetchSshSessionSnapshot: vi.fn(),
  fetchTelnetSessionSnapshot: vi.fn(),
  fetchTerminalWorkspaceLayout: vi.fn(),
  fetchTerminalInputFocusSnapshot: vi.fn(),
  getHostConnectionConfig: vi.fn(),
  getLocalSession: vi.fn(),
  getSshSession: vi.fn(),
  importCredential: vi.fn(),
  listHostCatalog: vi.fn(),
  listHosts: vi.fn(),
  prepareTransientCredential: vi.fn(),
  replaceAuthenticationPlan: vi.fn(),
  replaceTerminalWorkspaceLayout: vi.fn(),
  unlockVault: vi.fn(),
  updateHost: vi.fn(),
}));

const secureCredential = vi.hoisted(() => vi.fn());
const secureVault = vi.hoisted(() => vi.fn());

vi.mock("../core-api/secure-credential-client", () => ({
  requestSecureCredential: secureCredential,
}));
vi.mock("../core-api/secure-vault-client", () => ({ requestSecureVault: secureVault }));

vi.mock("../core-api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../core-api/client")>();
  return {
    ...actual,
    canUseDesktopCore: () => true,
    changeTerminalInputFocus: client.changeTerminalInputFocus,
    createHost: client.createHost,
    createIdentity: client.createIdentity,
    createVault: client.createVault,
    fetchVaultStatus: client.fetchVaultStatus,
    fetchLocalSessionSnapshot: client.fetchLocalSessionSnapshot,
    fetchSshSessionSnapshot: client.fetchSshSessionSnapshot,
    fetchTelnetSessionSnapshot: client.fetchTelnetSessionSnapshot,
    fetchTerminalWorkspaceLayout: client.fetchTerminalWorkspaceLayout,
    fetchTerminalInputFocusSnapshot: client.fetchTerminalInputFocusSnapshot,
    getHostConnectionConfig: client.getHostConnectionConfig,
    getLocalSession: client.getLocalSession,
    getSshSession: client.getSshSession,
    importCredential: client.importCredential,
    listHostCatalog: client.listHostCatalog,
    listHosts: client.listHosts,
    prepareTransientCredential: client.prepareTransientCredential,
    replaceAuthenticationPlan: client.replaceAuthenticationPlan,
    replaceTerminalWorkspaceLayout: client.replaceTerminalWorkspaceLayout,
    unlockVault: client.unlockVault,
    updateHost: client.updateHost,
  };
});

const pluginClient = vi.hoisted(() => ({
  fetchPluginTerminalSessionSnapshot: vi.fn(),
  listPluginProtocolLaunches: vi.fn(),
}));
vi.mock("../core-api/plugin-terminal", async (importOriginal) => ({
  ...await importOriginal<typeof import("../core-api/plugin-terminal")>(),
  ...pluginClient,
}));

import SshTerminalView from "./SshTerminalView.vue";

const host: HostSummary = {
  hostId: "019d0000-0000-7000-8000-000000000101",
  label: "Acceptance host",
  address: "127.0.0.1",
  normalizedAddress: "127.0.0.1",
  port: 22222,
  username: "norishell",
  identityId: null,
  favorite: false,
  hasReadyCredential: false,
  stateVersion: "1",
};

const runningSession: SshSessionSummary = {
  sessionId: "019d0000-0000-7000-8000-000000000201",
  openAttemptId: "019d0000-0000-7000-8000-000000000202",
  target: {
    kind: "quickConnect",
    endpoint: { address: "127.0.0.1", port: 22222, username: "norishell" },
  },
  credentialRefId: null,
  endpoint: { address: "127.0.0.1", port: 22222, username: "norishell" },
  generation: "3",
  stateRevision: "8",
  attachmentRevision: "5",
  eventSeq: "13",
  channelId: "019d0000-0000-7000-8000-000000000203",
  negotiatedAlgorithms: [],
  state: "running",
  closeReason: null,
  failureReason: null,
  attachmentCount: 1,
  createdAtUnixMs: 1,
  updatedAtUnixMs: 2,
};

const runningTelnetSession: TelnetSessionSummary = {
  sessionId: "019d0000-0000-7000-8000-000000000401",
  openAttemptId: "019d0000-0000-7000-8000-000000000402",
  endpoint: { address: "telnet.example.test", port: 23 },
  generation: "4",
  stateRevision: "7",
  attachmentRevision: "3",
  eventSeq: "9",
  socketId: "019d0000-0000-7000-8000-000000000403",
  state: "running",
  attachmentCount: 1,
  closeReason: null,
  failureReason: null,
  createdAtUnixMs: 1,
  updatedAtUnixMs: 2,
};

const cleanupFailedLocalSession: LocalSessionSummary = {
  sessionId: "019d0000-0000-7000-8000-000000000301",
  openAttemptId: "019d0000-0000-7000-8000-000000000302",
  shellName: "zsh",
  generation: "1",
  stateRevision: "7",
  attachmentRevision: "4",
  eventSeq: "9",
  ptyId: "019d0000-0000-7000-8000-000000000303",
  state: "failed",
  exit: null,
  failureReason: {
    code: "processCleanupFailed",
    messageKey: "errors.localTerminal.processCleanupFailed",
    diagnosticId: null,
  },
  attachmentCount: 1,
  createdAtUnixMs: 1,
  updatedAtUnixMs: 2,
};

async function openLauncherQuickConnect(
  scopeSelector = ".terminal-launcher",
  target = "example.test",
) {
  const scope = document.querySelector<HTMLElement>(scopeSelector);
  const input = scope?.querySelector<HTMLInputElement>('input[aria-label="SSH 地址"]');
  const form = scope?.querySelector<HTMLFormElement>("form");
  if (!input || !form) throw new Error("Terminal launcher Quick Connect form is unavailable");
  input.value = target;
  input.dispatchEvent(new Event("input", { bubbles: true }));
  await flushPromises();
  form.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
  await flushPromises();
}

const HostsStub = defineComponent({
  name: "HostsStub",
  setup() {
    return () => h("div", "Hosts");
  },
});

const disconnectForClose = vi.fn();
const terminateForClose = vi.fn();
const reconnectWithCredential = vi.fn();
const reconnectSavedCredential = vi.fn();
const reconcileAfterForeground = vi.fn();
const paneControls = new Map<string, {
  activateFromTab: ReturnType<typeof vi.fn>;
  deactivateFromTab: ReturnType<typeof vi.fn>;
  send: ReturnType<typeof vi.fn>;
}>();
const SshPaneContractStub = defineComponent({
  name: "NvxSshTerminalPane",
  props: {
    paneId: { type: String, required: true },
    label: { type: String, required: true },
    target: { type: Object, required: true },
    credentialRefId: { type: String, default: null },
    pluginAuthorizationToken: { type: String, default: null },
    existingSession: { type: Object, default: null },
    deferredStart: Boolean,
    deferredRecovery: { type: String, default: "reconnect" },
    initialDirectory: { type: String, default: null },
    active: Boolean,
    canSplitHorizontal: Boolean,
    canSplitVertical: Boolean,
  },
  emits: [
    "split",
    "close",
    "bellAttention",
    "requestAuthenticationRecovery",
    "requestCredential",
    "requestVaultUnlock",
  ],
  setup(props, { emit, expose }) {
    let unregister: (() => void) | null = null;
    const send = vi.fn().mockResolvedValue(undefined);
    const activateFromTab = vi.fn(() => {
      if (!unregister) {
        unregister = registerTerminalInputTarget({
          id: props.paneId,
          label: () => props.label,
          focusTarget: () => ({
            kind: "ssh",
            target: {
              sessionId: runningSession.sessionId,
              expectedGeneration: runningSession.generation,
              expectedStateRevision: runningSession.stateRevision,
              channelId: runningSession.channelId!,
              attachmentId: "019d0000-0000-7000-8000-000000000204",
              viewId: props.paneId,
            },
          }),
          applyFocusLease: () => undefined,
          canAcceptInput: () => props.active,
          send,
        });
      }
      focusTerminalInputTarget(props.paneId);
    });
    const deactivateFromTab = vi.fn(() => {
      unregister?.();
      unregister = null;
    });
    paneControls.set(props.label, { activateFromTab, deactivateFromTab, send });
    onBeforeUnmount(deactivateFromTab);
    expose({
      disconnectForClose,
      reconcileAfterForeground,
      reconnectWithCredential,
      reconnectSavedCredential,
      activateFromTab,
      deactivateFromTab,
    });
    return () => h("article", {
      class: "ssh-pane-contract-stub",
      "data-active": String(props.active),
      "data-credential-ref": props.credentialRefId ?? "",
      "data-target-kind": (props.target as SshSessionSummary["target"]).kind,
      "data-deferred-start": String(props.deferredStart),
      "data-deferred-recovery": props.deferredRecovery,
      "data-initial-directory": props.initialDirectory ?? "",
    }, [
      props.label,
      props.active
        ? h("button", {
            "aria-label": "向右拆分 Pane",
            disabled: !props.canSplitHorizontal,
            onClick: () => emit("split", "horizontal"),
          })
        : null,
      props.active
        ? h("button", {
            "aria-label": "向下拆分 Pane",
            disabled: !props.canSplitVertical,
            onClick: () => emit("split", "vertical"),
          })
        : null,
      props.active
        ? h("button", {
            "aria-label": "关闭当前 Pane",
            onClick: () => emit("close"),
          })
        : null,
    ]);
  },
});

const LocalPaneContractStub = defineComponent({
  name: "NvxLocalTerminalPane",
  props: {
    paneId: { type: String, required: true },
    label: { type: String, required: true },
    existingSession: { type: Object, default: null },
    deferredStart: Boolean,
    active: Boolean,
    canSplitHorizontal: Boolean,
    canSplitVertical: Boolean,
  },
  emits: ["split", "close"],
  setup(props, { emit, expose }) {
    const activateFromTab = vi.fn();
    const deactivateFromTab = vi.fn();
    expose({ terminateForClose, activateFromTab, deactivateFromTab });
    return () => h("article", {
      class: "local-pane-contract-stub",
      "data-active": String(props.active),
      "data-deferred-start": String(props.deferredStart),
    }, [
      props.label,
      props.active
        ? h("button", {
            "aria-label": "向右拆分 Pane",
            disabled: !props.canSplitHorizontal,
            onClick: () => emit("split", "horizontal"),
          })
        : null,
      props.active
        ? h("button", {
            "aria-label": "向下拆分 Pane",
            disabled: !props.canSplitVertical,
            onClick: () => emit("split", "vertical"),
          })
        : null,
      props.active
        ? h("button", {
            "aria-label": "关闭当前 Pane",
            onClick: () => emit("close"),
          })
        : null,
    ]);
  },
});

const TelnetPaneContractStub = defineComponent({
  name: "NvxTelnetTerminalPane",
  props: {
    paneId: { type: String, required: true },
    label: { type: String, required: true },
    endpoint: { type: Object, required: true },
    existingSession: { type: Object, default: null },
    deferredStart: Boolean,
    active: Boolean,
    canSplitHorizontal: Boolean,
    canSplitVertical: Boolean,
  },
  emits: ["split", "close", "state", "bellAttention"],
  setup(props, { expose }) {
    const activateFromTab = vi.fn();
    const deactivateFromTab = vi.fn();
    expose({ activateFromTab, deactivateFromTab });
    return () => h("article", {
      class: "telnet-pane-contract-stub",
      "data-active": String(props.active),
      "data-deferred-start": String(props.deferredStart),
    }, props.label);
  },
});

const Shell = defineComponent({
  setup() {
    return () => h("div", [
      h("header", { id: "nvx-terminal-tabs-host" }, [h(NvxWorkspaceTabBar)]),
      h(RouterView, null, {
        default: ({ Component }: { Component: unknown }) => h(
          KeepAlive,
          { include: "SshTerminalView" },
          () => h(Component as never),
        ),
      }),
    ]);
  },
});

async function mountShell(initialLocation = "/terminal") {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: "/terminal", component: SshTerminalView },
      { path: "/hosts", component: HostsStub },
    ],
  });
  await router.push(initialLocation);
  await router.isReady();

  const mountHost = document.createElement("div");
  document.body.append(mountHost);
  const pinia = createPinia();
  const wrapper = mount(Shell, {
    attachTo: mountHost,
    global: {
      plugins: [pinia, router, i18n],
      stubs: {
        NvxSshTerminalPane: SshPaneContractStub,
        NvxLocalTerminalPane: LocalPaneContractStub,
        NvxTelnetTerminalPane: TelnetPaneContractStub,
        NvxPluginTerminalPane: true,
      },
    },
  });
  await flushPromises();
  return { router, wrapper, pinia };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((promiseResolve) => {
    resolve = promiseResolve;
  });
  return { promise, resolve };
}

describe("SshTerminalView route and Header behavior", () => {
  it("opens each SFTP directory request in a new Host tab and consumes its path only once", async () => {
    const readyHost = { ...host, hasReadyCredential: true };
    client.listHosts.mockResolvedValue([readyHost]);
    client.fetchVaultStatus.mockResolvedValue({ state: "unlocked", vaultId: "vault", revision: "2", entryCount: 1 });
    const { wrapper, router } = await mountShell();
    const first = createSftpTerminalLaunch(host.hostId, Array.from(new TextEncoder().encode("/srv/first")))!;
    await router.push({ path: "/terminal", query: { hostId: host.hostId, source: "sftpDirectory", connectOperationId: first } });
    await flushPromises();
    expect(wrapper.findAll(".ssh-pane-contract-stub")).toHaveLength(1);
    expect(wrapper.find(".ssh-pane-contract-stub").attributes("data-initial-directory")).toBe("/srv/first");
    expect(router.currentRoute.value.query.connectOperationId).toBeUndefined();

    const second = createSftpTerminalLaunch(host.hostId, Array.from(new TextEncoder().encode("/srv/second")))!;
    await router.push({ path: "/terminal", query: { hostId: host.hostId, source: "sftpDirectory", connectOperationId: second } });
    await flushPromises();
    expect(wrapper.findAll(".ssh-pane-contract-stub")).toHaveLength(2);
    expect(wrapper.findAll(".ssh-pane-contract-stub").map((pane) => pane.attributes("data-initial-directory")))
      .toEqual(["/srv/first", "/srv/second"]);
    wrapper.unmount();
  });

  it("does not carry a pending SFTP directory into a later ordinary Host launch", async () => {
    const noCredentialHost = { ...host, hasReadyCredential: false };
    const readyHost = { ...host, hasReadyCredential: true };
    client.fetchVaultStatus.mockResolvedValue({ state: "unlocked", vaultId: "vault", revision: "2", entryCount: 1 });
    const { wrapper, router } = await mountShell();
    client.listHosts.mockResolvedValue([noCredentialHost]);
    const operationId = createSftpTerminalLaunch(host.hostId, Array.from(new TextEncoder().encode("/srv/old")))!;
    await router.push({ path: "/terminal", query: { hostId: host.hostId, source: "sftpDirectory", connectOperationId: operationId } });
    await flushPromises();
    expect(wrapper.findAll(".ssh-pane-contract-stub")).toHaveLength(0);

    client.listHosts.mockResolvedValue([readyHost]);
    await router.push({ path: "/terminal", query: { hostId: host.hostId, source: "overview", connectOperationId: crypto.randomUUID() } });
    await flushPromises();
    expect(wrapper.findAll(".ssh-pane-contract-stub")).toHaveLength(1);
    expect(wrapper.find(".ssh-pane-contract-stub").attributes("data-initial-directory")).toBe("");
    wrapper.unmount();
  });

  it("shows an error and opens no Host tab for an expired SFTP directory request", async () => {
    const { wrapper, router, pinia } = await mountShell();
    await router.push({ path: "/terminal", query: { hostId: host.hostId, source: "sftpDirectory", connectOperationId: crypto.randomUUID() } });
    await flushPromises();
    expect(wrapper.findAll(".ssh-pane-contract-stub")).toHaveLength(0);
    expect(useTipsStore(pinia).items).toEqual(expect.arrayContaining([
      expect.objectContaining({ scope: "ssh-sftp-directory-launch", title: i18n.global.t("sshTerminal.sftpDirectoryLaunchExpired") }),
    ]));
    wrapper.unmount();
  });
  it("recovers a pending plugin launch even when its navigation event was lost and deduplicates later events", async () => {
    const launch = { launchId: "launch", pluginId: "provider.test", providerId: "serial", label: "Serial device",
      tabId: "provider-tab", paneId: "provider-pane", revision: "1", claimed: false, expiresAtUnixMs: Date.now() + 60_000 };
    pluginClient.listPluginProtocolLaunches.mockResolvedValue([launch]);
    const { wrapper } = await mountShell();
    expect(wrapper.findAll("nvx-plugin-terminal-pane-stub")).toHaveLength(1);
    expect(wrapper.findComponent({ name: "NvxPluginTerminalPane" }).props("paneId")).toBe("provider-pane");
    nativeEvents.get("plugin-protocol-launch")?.({ payload: launch });
    await flushPromises();
    expect(wrapper.findAll("nvx-plugin-terminal-pane-stub")).toHaveLength(1);
    wrapper.unmount();
  });

  it("restores provider history as deferred without opening a protocol connection", async () => {
    localStorage.setItem("norishell.ui.preferences.v1", JSON.stringify({ terminalStartupBehavior: "restoreHistory" }));
    client.fetchTerminalWorkspaceLayout.mockResolvedValue({ revision: "1", updatedAtUnixMs: 0,
      layout: { schemaVersion: 1, activeTabId: "provider-tab", tabs: [{ tabId: "provider-tab", activePaneId: "provider-pane",
        layout: { kind: "pane", paneId: "provider-pane", terminalId: "provider-pane" },
        panes: [{ kind: "plugin", paneId: "provider-pane", label: "Serial device", pluginId: "provider.test", providerId: "serial", schemaHash: "hash", configuration: { baud: 115200 } }],
      }] } });
    // A live provider session forces layout recovery independent of startup preference.
    pluginClient.fetchPluginTerminalSessionSnapshot.mockResolvedValue({ snapshotRevision: "1", sessions: [{
      sessionId: "other-session", tabId: "other-tab", paneId: "other-pane", label: "Other device",
      pluginId: "provider.test", providerId: "serial", schemaHash: "hash", configuration: {},
      state: "running", generation: "1", stateRevision: "1", attachmentRevision: "1", eventSeq: "1", streamId: "stream",
      attachmentCount: 0, cleanupBlocked: false, failureReason: null,
    }] });
    const { wrapper } = await mountShell();
    const history = wrapper.findAllComponents({ name: "NvxPluginTerminalPane" }).find((pane) => pane.props("paneId") === "provider-pane");
    expect(history?.props("deferredStart")).toBe(true);
    expect(wrapper.findAll("nvx-plugin-terminal-pane-stub")).toHaveLength(2);
    wrapper.unmount();
  });

  it("runs layout shortcuts through the registered controller and exposes the active saved Host id", async () => {
    const savedHostSession: SshSessionSummary = {
      ...runningSession,
      target: { kind: "host", hostId: host.hostId, expectedHostStateVersion: host.stateVersion },
    };
    client.fetchSshSessionSnapshot.mockResolvedValue({
      snapshotRevision: "1",
      sessions: [savedHostSession],
    });
    const { wrapper, pinia } = await mountShell();
    const workspaceTabs = useWorkspaceTabsStore(pinia);
    const controller = workspaceTabs.terminalController;
    expect(workspaceTabs.terminalTabs[0]?.hostId).toBe(host.hostId);

    controller?.runShortcut?.("terminal.split-right");
    await flushPromises();
    expect(document.querySelectorAll(".nvx-terminal-split-tree__pane")).toHaveLength(2);

    controller?.runShortcut?.("terminal.focus-previous-pane");
    controller?.runShortcut?.("workspace.new-local");
    await flushPromises();
    expect(document.querySelectorAll('[role="tab"]')).toHaveLength(2);
    expect(document.querySelectorAll(".local-pane-contract-stub")).toHaveLength(1);
    wrapper.unmount();
  });

  it("places a new full-height Pane to the right of a vertical stack", async () => {
    client.fetchSshSessionSnapshot.mockResolvedValue({ snapshotRevision: "1", sessions: [runningSession] });
    const { wrapper } = await mountShell();

    await wrapper.get('button[aria-label="向下拆分 Pane"]').trigger("click");
    await flushPromises();
    expect(document.querySelectorAll(".nvx-terminal-split-tree__pane")).toHaveLength(2);

    await wrapper.get('button[aria-label="在右侧新建完整高度 Pane"]').trigger("click");
    await flushPromises();
    const panes = Array.from(document.querySelectorAll<HTMLElement>(".nvx-terminal-split-tree__pane"));
    expect(panes).toHaveLength(3);
    expect(panes[0]?.style.height).toBe("50%");
    expect(panes[1]?.style.top).toBe("50%");
    expect(panes[2]?.style.height).toBe("100%");
    expect(panes[2]?.style.left).toBe("50%");
    wrapper.unmount();
  });

  it("aggregates a background pane BEL into the Header tab marker and clears it on focus", async () => {
    client.fetchSshSessionSnapshot.mockResolvedValue({ snapshotRevision: "1", sessions: [runningSession] });
    const { wrapper, pinia } = await mountShell();
    const workspaceTabs = useWorkspaceTabsStore(pinia);
    const pane = wrapper.findComponent(SshPaneContractStub);
    pane.vm.$emit("bellAttention", true);
    await flushPromises();
    expect(workspaceTabs.terminalTabs[0]?.bellAttention).toBe(true);

    workspaceTabs.terminalController!.activate(workspaceTabs.terminalTabs[0]!.groupId);
    await flushPromises();
    expect(workspaceTabs.terminalTabs[0]?.bellAttention).toBe(false);
    wrapper.unmount();
  });

  it("creates a local terminal directly and opens Quick Connect in an explicit Launcher", async () => {
    const { wrapper, pinia } = await mountShell();
    const controller = useWorkspaceTabsStore(pinia).terminalController!;

    expect(controller.createLocal()).toBe(true);
    await flushPromises();
    expect(wrapper.findAll(".local-pane-contract-stub")).toHaveLength(1);

    expect(controller.quickConnect()).toBe(true);
    await flushPromises();
    expect(document.querySelector("#quick-address")).not.toBeNull();
    // Do not create and discard a default local PTY while the authentication form is already open.
    expect(controller.createLocal()).toBe(false);
    wrapper.unmount();
  });

  it("focuses only an existing Telnet pane with an exact session, generation, and socket", async () => {
    const noSocketSession: TelnetSessionSummary = {
      ...runningTelnetSession,
      sessionId: "019d0000-0000-7000-8000-000000000404",
      openAttemptId: "019d0000-0000-7000-8000-000000000405",
      generation: "5",
      socketId: null,
      state: "connecting",
    };
    client.fetchTelnetSessionSnapshot.mockResolvedValue({
      snapshotRevision: "1",
      sessions: [runningTelnetSession, noSocketSession],
    });
    const { wrapper, router, pinia } = await mountShell();
    const controller = useWorkspaceTabsStore(pinia).terminalController!;
    const snapshotCalls = client.fetchTelnetSessionSnapshot.mock.calls.length;

    await router.push("/hosts");
    await flushPromises();
    expect(controller.focusTelnetSession(runningTelnetSession.sessionId, "999", runningTelnetSession.socketId!)).toBe(false);
    expect(controller.focusTelnetSession(runningTelnetSession.sessionId, runningTelnetSession.generation, null)).toBe(false);
    expect(controller.focusTelnetSession(runningTelnetSession.sessionId, runningTelnetSession.generation, "019d0000-0000-7000-8000-000000000499")).toBe(false);
    expect(router.currentRoute.value.path).toBe("/hosts");

    expect(controller.focusTelnetSession(
      runningTelnetSession.sessionId,
      runningTelnetSession.generation,
      runningTelnetSession.socketId!,
    )).toBe(true);
    await flushPromises();
    expect(router.currentRoute.value.path).toBe("/terminal");
    expect(client.fetchTelnetSessionSnapshot).toHaveBeenCalledTimes(snapshotCalls);

    await router.push("/hosts");
    expect(controller.focusTelnetSession(noSocketSession.sessionId, noSocketSession.generation, runningTelnetSession.socketId!)).toBe(false);
    expect(controller.focusTelnetSession(noSocketSession.sessionId, noSocketSession.generation, null)).toBe(true);
    await flushPromises();
    expect(router.currentRoute.value.path).toBe("/terminal");

    await router.push("/hosts");
    const dialog = document.createElement("div");
    dialog.setAttribute("role", "dialog");
    dialog.setAttribute("aria-modal", "true");
    document.body.append(dialog);
    expect(controller.focusTelnetSession(
      runningTelnetSession.sessionId,
      runningTelnetSession.generation,
      runningTelnetSession.socketId!,
    )).toBe(false);
    expect(router.currentRoute.value.path).toBe("/hosts");
    dialog.remove();
    wrapper.unmount();
  });

  it("opens an approved extra terminal channel once without looking up Hosts or credentials", async () => {
    const { wrapper } = await mountShell();
    const listCalls = client.listHosts.mock.calls.length;
    const event = { operationId: "019d0000-0000-7000-8000-000000000951", authorizationToken: "019d0000-0000-7000-8000-000000000952", target: runningSession.target, label: "Docker · web" };
    nativeEvents.get("plugin-terminal-channel-approved")?.({ payload: event });
    await flushPromises();
    const panes = wrapper.findAllComponents(SshPaneContractStub);
    expect(panes).toHaveLength(1);
    expect(panes[0]!.props("pluginAuthorizationToken")).toBe(event.authorizationToken);
    expect(panes[0]!.props("target")).toEqual(event.target);
    expect(panes[0]!.props("label")).toBe("Docker · web");
    expect(panes[0]!.props("credentialRefId")).toBeNull();
    expect(client.listHosts).toHaveBeenCalledTimes(listCalls);
    expect(client.getHostConnectionConfig).not.toHaveBeenCalled();
    expect(client.prepareTransientCredential).not.toHaveBeenCalled();
    nativeEvents.get("plugin-terminal-channel-approved")?.({ payload: event });
    await flushPromises();
    expect(wrapper.findAllComponents(SshPaneContractStub)).toHaveLength(1);
    wrapper.unmount();
  });

  beforeEach(() => {
    pluginClient.fetchPluginTerminalSessionSnapshot.mockResolvedValue({ snapshotRevision: "0", sessions: [] });
    pluginClient.listPluginProtocolLaunches.mockResolvedValue([]);
    vi.clearAllMocks();
    nativeEvents.clear();
    localStorage.clear();
    resetTerminalInputFocusForTests();
    paneControls.clear();
    i18n.global.locale.value = "zh-CN";
    client.listHosts.mockResolvedValue([host]);
    client.listHostCatalog.mockResolvedValue([{
      host,
      group: null,
      tags: [],
      recentConnection: null,
    }]);
    client.fetchSshSessionSnapshot.mockResolvedValue({
      snapshotRevision: "0",
      sessions: [],
    });
    client.fetchLocalSessionSnapshot.mockResolvedValue({
      snapshotRevision: "0",
      sessions: [],
    });
    client.fetchTelnetSessionSnapshot.mockResolvedValue({
      snapshotRevision: "0",
      sessions: [],
    });
    client.fetchTerminalWorkspaceLayout.mockResolvedValue({
      revision: "1",
      layout: { schemaVersion: 1, activeTabId: null, tabs: [] },
      updatedAtUnixMs: 0,
    });
    client.getSshSession.mockResolvedValue({
      session: runningSession,
      attachments: [],
      activeHostKeyChallenge: null,
      inputLease: null,
    });
    client.getLocalSession.mockResolvedValue({
      session: cleanupFailedLocalSession,
      attachments: [],
      inputLease: null,
    });
    client.replaceTerminalWorkspaceLayout.mockImplementation(async ({ layout }) => ({
      revision: "2",
      layout,
      updatedAtUnixMs: 1,
    }));
    client.fetchTerminalInputFocusSnapshot.mockResolvedValue({
      focusEpoch: "0",
      target: null,
      lease: null,
    });
    client.changeTerminalInputFocus.mockImplementation(async ({ expectedFocusEpoch, target }) => ({
      focusEpoch: (BigInt(expectedFocusEpoch) + 1n).toString(),
      target,
      lease: null,
    }));
    client.fetchVaultStatus.mockResolvedValue({
      state: "missing",
      vaultId: null,
      revision: null,
      entryCount: null,
    });
    client.createHost.mockResolvedValue({ ...host, hasReadyCredential: false, identityId: null });
    client.createIdentity.mockResolvedValue({
      identityId: "019d0000-0000-7000-8000-000000000501",
      label: host.label,
      username: host.username,
      stateVersion: "1",
    });
    client.importCredential.mockResolvedValue({
      credentialRefId: "019d0000-0000-7000-8000-000000000502",
      identityId: "019d0000-0000-7000-8000-000000000501",
      kind: "password",
      priority: 100,
      label: host.label,
      publicKeyAlgorithm: null,
      publicKeyFingerprint: null,
      stateVersion: "1",
    });
    client.prepareTransientCredential.mockResolvedValue({
      credentialRefId: "019d0000-0000-7000-8000-000000000503",
      expiresAtUnixMs: Date.now() + 180_000,
    });
    client.getHostConnectionConfig.mockResolvedValue({
      authenticationPlan: {
        hostId: host.hostId,
        revision: "1",
        mode: "identity",
        credentialRefIds: [],
      },
    });
    client.replaceAuthenticationPlan.mockResolvedValue({
      hostId: host.hostId,
      revision: "2",
      mode: "hostOverride",
      credentialRefIds: ["019d0000-0000-7000-8000-000000000502"],
    });
    client.updateHost.mockResolvedValue({ ...host, hasReadyCredential: true });
    client.createVault.mockResolvedValue({
      state: "unlocked",
      vaultId: "019d0000-0000-7000-8000-000000000504",
      revision: "1",
      entryCount: 0,
    });
    client.unlockVault.mockResolvedValue({
      state: "unlocked",
      vaultId: "019d0000-0000-7000-8000-000000000504",
      revision: "1",
      entryCount: 1,
    });
    secureCredential.mockResolvedValue("019d0000-0000-7000-8000-000000000503");
    secureVault.mockResolvedValue(true);
    disconnectForClose.mockResolvedValue(undefined);
    terminateForClose.mockResolvedValue(undefined);
    reconnectWithCredential.mockResolvedValue(undefined);
    reconnectSavedCredential.mockResolvedValue(undefined);
  });

  afterEach(() => {
    vi.useRealTimers();
    Object.defineProperty(navigator, "platform", {
      configurable: true,
      value: defaultNavigatorPlatform,
    });
    document.body.innerHTML = "";
  });

  it("keeps both Header actions visible while Quick Commands starts collapsed", async () => {
    const { wrapper } = await mountShell();

    expect(document.querySelector('.quick-commands')).toBeNull();
    expect(document.querySelectorAll("#nvx-workspace-tab-bar")).toHaveLength(1);
    expect(document.querySelectorAll(".nvx-terminal-tab-bar")).toHaveLength(1);
    expect(document.querySelectorAll('button[aria-label="新建连接"]')).toHaveLength(1);
    expect(document.querySelectorAll('button[aria-label="显示快捷命令"]')).toHaveLength(1);
    expect(document.body.textContent).toContain("最近连接");
    expect(document.body.textContent).toContain(host.label);
    wrapper.unmount();
  });

  it("returns to Terminal when a persisted Header tab is clicked from another route", async () => {
    client.fetchSshSessionSnapshot.mockResolvedValue({
      snapshotRevision: "1",
      sessions: [runningSession],
    });
    const { router, wrapper } = await mountShell();

    await router.push("/hosts");
    await flushPromises();
    expect(router.currentRoute.value.path).toBe("/hosts");

    document.querySelector<HTMLButtonElement>('[role="tab"]')?.click();
    await flushPromises();
    expect(router.currentRoute.value.path).toBe("/terminal");
    expect(paneControls.get("norishell@127.0.0.1")?.activateFromTab).toHaveBeenCalled();
    wrapper.unmount();
  });

  it("debounces a secret-free workspace projection into the SQLite command", async () => {
    const { wrapper } = await mountShell();
    vi.useFakeTimers();
    document.querySelector<HTMLButtonElement>('button[aria-label="新建连接"]')?.click();
    await flushPromises();

    await vi.advanceTimersByTimeAsync(399);
    expect(client.replaceTerminalWorkspaceLayout).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    await flushPromises();

    expect(client.replaceTerminalWorkspaceLayout).toHaveBeenCalledTimes(1);
    const request = client.replaceTerminalWorkspaceLayout.mock.calls[0]?.[0];
    expect(request.expectedRevision).toBe("1");
    expect(request.layout.tabs).toHaveLength(1);
    expect(JSON.stringify(request.layout)).not.toMatch(/sessionId|credentialRefId|scrollback/);
    wrapper.unmount();
  });

  it("waits for an in-flight write and the latest projection before exit", async () => {
    const firstWrite = deferred<{
      revision: string;
      layout: unknown;
      updatedAtUnixMs: number;
    }>();
    client.replaceTerminalWorkspaceLayout
      .mockImplementationOnce(() => firstWrite.promise)
      .mockImplementationOnce(async ({ layout }) => ({
        revision: "3",
        layout,
        updatedAtUnixMs: 2,
      }));
    const { wrapper } = await mountShell();
    vi.useFakeTimers();

    document.querySelector<HTMLButtonElement>('button[aria-label="新建连接"]')?.click();
    await flushPromises();
    await vi.advanceTimersByTimeAsync(400);
    expect(client.replaceTerminalWorkspaceLayout).toHaveBeenCalledTimes(1);

    document.querySelector<HTMLButtonElement>('button[aria-label="新建连接"]')?.click();
    await flushPromises();
    let exitBarrierResolved = false;
    const exitBarrier = flushTerminalWorkspaceBeforeExit().then(() => {
      exitBarrierResolved = true;
    });
    await Promise.resolve();
    expect(exitBarrierResolved).toBe(false);

    const firstLayout = client.replaceTerminalWorkspaceLayout.mock.calls[0]?.[0].layout;
    firstWrite.resolve({ revision: "2", layout: firstLayout, updatedAtUnixMs: 1 });
    await exitBarrier;

    expect(client.replaceTerminalWorkspaceLayout).toHaveBeenCalledTimes(2);
    expect(client.replaceTerminalWorkspaceLayout.mock.calls[1]?.[0].expectedRevision).toBe("2");
    expect(client.replaceTerminalWorkspaceLayout.mock.calls[1]?.[0].layout.tabs).toHaveLength(2);
    wrapper.unmount();
  });

  it("rejects the exit barrier when the latest projection is not durable", async () => {
    client.replaceTerminalWorkspaceLayout.mockRejectedValueOnce(
      new Error("SQLite write failed"),
    );
    const { wrapper } = await mountShell();
    vi.useFakeTimers();

    document.querySelector<HTMLButtonElement>('button[aria-label="新建连接"]')?.click();
    await flushPromises();

    await expect(flushTerminalWorkspaceBeforeExit()).rejects.toThrow(
      "terminal workspace layout is not durable",
    );
    expect(client.replaceTerminalWorkspaceLayout).toHaveBeenCalledTimes(1);
    wrapper.unmount();
  });

  it("consumes a Host connection request after the cached Terminal route is activated", async () => {
    const { router, wrapper } = await mountShell();
    await router.push("/hosts");
    await flushPromises();

    await router.push({ path: "/terminal", query: { hostId: host.hostId } });
    await flushPromises();

    expect(client.listHosts).toHaveBeenCalled();
    expect((document.querySelector("#quick-address") as HTMLInputElement | null)?.value)
      .toBe(host.address);
    expect((document.querySelector("#quick-port") as HTMLInputElement | null)?.value)
      .toBe(String(host.port));
    expect((document.querySelector("#quick-username") as HTMLInputElement | null)?.value)
      .toBe(host.username);
    expect(router.currentRoute.value.fullPath).toBe("/terminal");
    wrapper.unmount();
  });

  it("creates one new Tab for one Overview operation and rejects an exact replay", async () => {
    localStorage.setItem("norishell.ui.preferences.v1", JSON.stringify({
      newTerminalBehavior: "localTerminal",
    }));
    const { router, wrapper } = await mountShell();
    const initialTabs = document.querySelectorAll('[role="tab"]').length;
    const operationId = "019d0000-0000-7000-8000-000000000099";

    await router.push({
      path: "/terminal",
      query: { hostId: host.hostId, source: "overview", connectOperationId: operationId },
    });
    await flushPromises();
    expect(document.querySelectorAll('[role="tab"]')).toHaveLength(initialTabs + 1);
    expect(document.querySelector(".local-pane-contract-stub")).toBeNull();
    expect(router.currentRoute.value.fullPath).toBe("/terminal");

    await router.push({
      path: "/terminal",
      query: { hostId: host.hostId, source: "overview", connectOperationId: operationId },
    });
    await flushPromises();
    expect(document.querySelectorAll('[role="tab"]')).toHaveLength(initialTabs + 1);
    expect(router.currentRoute.value.fullPath).toBe("/terminal");
    wrapper.unmount();
  });

  it("creates one new Tab for one approved plugin operation and rejects an exact replay", async () => {
    localStorage.setItem("norishell.ui.preferences.v1", JSON.stringify({
      newTerminalBehavior: "localTerminal",
    }));
    const { router, wrapper } = await mountShell();
    const initialTabs = document.querySelectorAll('[role="tab"]').length;
    const operationId = "019d0000-0000-7000-8000-000000000099";

    await router.push({
      path: "/terminal",
      query: { hostId: host.hostId, source: "plugin", pluginAuthorizationToken: "019d0000-0000-7000-8000-000000000100", terminalLabel: "Docker · aaaaaaaaaaaa", connectOperationId: operationId },
    });
    await flushPromises();
    expect(document.querySelectorAll('[role="tab"]')).toHaveLength(initialTabs + 1);
    expect(document.querySelector(".local-pane-contract-stub")).toBeNull();
    expect(router.currentRoute.value.fullPath).toBe("/terminal");

    await router.push({
      path: "/terminal",
      query: { hostId: host.hostId, source: "plugin", pluginAuthorizationToken: "019d0000-0000-7000-8000-000000000100", terminalLabel: "Docker · aaaaaaaaaaaa", connectOperationId: operationId },
    });
    await flushPromises();
    expect(document.querySelectorAll('[role="tab"]')).toHaveLength(initialTabs + 1);
    expect(router.currentRoute.value.fullPath).toBe("/terminal");
    wrapper.unmount();
  });

  it("initializes persisted/live state before consuming a first-mount Host query", async () => {
    client.fetchTerminalWorkspaceLayout.mockResolvedValue({
      revision: "7",
      layout: {
        schemaVersion: 1,
        activeTabId: "tab-1",
        tabs: [{
          tabId: "tab-1",
          layout: { kind: "pane", paneId: "pane-local", terminalId: "pane-local" },
          activePaneId: "pane-local",
          panes: [{ kind: "local", paneId: "pane-local", label: "zsh" }],
        }],
      },
      updatedAtUnixMs: 10,
    });
    const { wrapper } = await mountShell(`/terminal?hostId=${host.hostId}`);

    expect(client.fetchTerminalWorkspaceLayout).toHaveBeenCalledTimes(1);
    expect(document.querySelectorAll('[role="tab"]')).toHaveLength(2);
    expect((document.querySelector("#quick-address") as HTMLInputElement | null)?.value)
      .toBe(host.address);
    wrapper.unmount();
  });

  it("restores an active Core session even when startup prefers the welcome page", async () => {
    localStorage.setItem("norishell.ui.preferences.v1", JSON.stringify({
      terminalStartupBehavior: "welcome",
    }));
    client.fetchSshSessionSnapshot.mockResolvedValue({
      snapshotRevision: "9",
      sessions: [runningSession],
    });

    const { wrapper } = await mountShell();

    expect(client.fetchSshSessionSnapshot).toHaveBeenCalledTimes(1);
    expect(document.body.textContent).toContain("norishell@127.0.0.1");
    expect(document.querySelector(".ssh-pane-contract-stub")).not.toBeNull();
    wrapper.unmount();
  });

  it("reattaches a live Core session into its persisted Pane instead of flattening the layout", async () => {
    client.fetchSshSessionSnapshot.mockResolvedValue({
      snapshotRevision: "9",
      sessions: [runningSession],
    });
    client.fetchTerminalWorkspaceLayout.mockResolvedValue({
      revision: "7",
      layout: {
        schemaVersion: 1,
        activeTabId: "tab-1",
        tabs: [{
          tabId: "tab-1",
          layout: {
            kind: "split",
            splitId: "split-1",
            direction: "horizontal",
            ratio: 0.6,
            first: { kind: "pane", paneId: "pane-live", terminalId: "pane-live" },
            second: { kind: "pane", paneId: "pane-local", terminalId: "pane-local" },
          },
          activePaneId: "pane-live",
          panes: [{
            kind: "sshQuickConnect",
            paneId: "pane-live",
            label: "norishell@127.0.0.1",
            endpoint: runningSession.endpoint,
          }, {
            kind: "local",
            paneId: "pane-local",
            label: "zsh",
          }],
        }],
      },
      updatedAtUnixMs: 10,
    });
    client.getSshSession.mockResolvedValue({
      session: runningSession,
      attachments: [{
        attachmentId: "019d0000-0000-7000-8000-000000000204",
        attachAttemptId: "019d0000-0000-7000-8000-000000000205",
        sessionId: runningSession.sessionId,
        generation: runningSession.generation,
        channelId: runningSession.channelId,
        viewId: "pane-live",
        stateRevision: runningSession.stateRevision,
        attachmentRevision: runningSession.attachmentRevision,
        attachedAtUnixMs: 2,
      }],
      activeHostKeyChallenge: null,
      inputLease: null,
    });

    const { wrapper } = await mountShell();

    expect(document.querySelectorAll('[role="tab"]')).toHaveLength(1);
    expect(document.querySelectorAll(".nvx-terminal-split-tree__pane")).toHaveLength(2);
    expect(document.querySelector(".ssh-pane-contract-stub")?.getAttribute("data-deferred-start"))
      .toBe("false");
    expect(document.querySelector(".local-pane-contract-stub")?.getAttribute("data-deferred-start"))
      .toBe("false");
    expect(client.replaceTerminalWorkspaceLayout).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("keeps missing-credential Host history actionable while starting a fresh local PTY", async () => {
    client.fetchTerminalWorkspaceLayout.mockResolvedValue({
      revision: "7",
      layout: {
        schemaVersion: 1,
        activeTabId: "tab-1",
        tabs: [{
          tabId: "tab-1",
          layout: {
            kind: "split",
            splitId: "split-1",
            direction: "horizontal",
            ratio: 0.6,
            first: { kind: "pane", paneId: "pane-1", terminalId: "pane-1" },
            second: { kind: "pane", paneId: "pane-2", terminalId: "pane-2" },
          },
          activePaneId: "pane-2",
          panes: [{
            kind: "sshHost",
            paneId: "pane-1",
            label: "Acceptance host",
            hostId: host.hostId,
          }, {
            kind: "local",
            paneId: "pane-2",
            label: "zsh",
          }],
        }],
      },
      updatedAtUnixMs: 10,
    });
    const { wrapper } = await mountShell();

    expect(document.querySelectorAll('[role="tab"]')).toHaveLength(1);
    expect(document.querySelectorAll(".nvx-terminal-split-tree__pane")).toHaveLength(2);
    expect(document.querySelector(".ssh-pane-contract-stub")?.getAttribute("data-deferred-start"))
      .toBe("true");
    expect(document.querySelector(".ssh-pane-contract-stub")?.getAttribute("data-deferred-recovery"))
      .toBe("credential");
    expect(document.querySelector(".local-pane-contract-stub")?.getAttribute("data-deferred-start"))
      .toBe("false");
    expect(client.replaceTerminalWorkspaceLayout).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("starts a fresh saved-Host connection when history restore can unlock its credential", async () => {
    const readyHost = {
      ...host,
      identityId: "019d0000-0000-7000-8000-000000000501",
      hasReadyCredential: true,
    };
    client.listHosts.mockResolvedValue([readyHost]);
    client.fetchVaultStatus.mockResolvedValue({
      state: "unlocked",
      vaultId: "019d0000-0000-7000-8000-000000000504",
      revision: "2",
      entryCount: 1,
    });
    client.fetchTerminalWorkspaceLayout.mockResolvedValue({
      revision: "7",
      layout: {
        schemaVersion: 1,
        activeTabId: "tab-1",
        tabs: [{
          tabId: "tab-1",
          layout: { kind: "pane", paneId: "pane-1", terminalId: "pane-1" },
          activePaneId: "pane-1",
          panes: [{
            kind: "sshHost",
            paneId: "pane-1",
            label: readyHost.label,
            hostId: readyHost.hostId,
          }],
        }],
      },
      updatedAtUnixMs: 10,
    });

    const { wrapper } = await mountShell();

    expect(client.fetchVaultStatus).not.toHaveBeenCalled();
    expect(document.querySelector(".ssh-pane-contract-stub")?.getAttribute("data-deferred-start"))
      .toBe("false");
    expect(document.querySelector(".ssh-pane-contract-stub")?.getAttribute("data-deferred-recovery"))
      .toBe("reconnect");
    wrapper.unmount();
  });

  it("shows the welcome page without overwriting history until the user creates a terminal", async () => {
    localStorage.setItem("norishell.ui.preferences.v1", JSON.stringify({
      terminalStartupBehavior: "welcome",
    }));
    client.fetchTerminalWorkspaceLayout.mockResolvedValue({
      revision: "7",
      layout: {
        schemaVersion: 1,
        activeTabId: "tab-1",
        tabs: [{
          tabId: "tab-1",
          layout: { kind: "pane", paneId: "pane-1", terminalId: "pane-1" },
          activePaneId: "pane-1",
          panes: [{
            kind: "sshHost",
            paneId: "pane-1",
            label: host.label,
            hostId: host.hostId,
          }],
        }],
      },
      updatedAtUnixMs: 10,
    });
    const { wrapper } = await mountShell();

    expect(document.querySelectorAll('[role="tab"]')).toHaveLength(0);
    expect(document.body.textContent).toContain(i18n.global.t("sshTerminal.launcherTitle"));
    expect(client.replaceTerminalWorkspaceLayout).not.toHaveBeenCalled();

    vi.useFakeTimers();
    document.querySelector<HTMLButtonElement>('button[aria-label="新建连接"]')?.click();
    await flushPromises();
    await vi.advanceTimersByTimeAsync(400);
    await flushPromises();

    expect(document.querySelectorAll('[role="tab"]')).toHaveLength(1);
    expect(client.replaceTerminalWorkspaceLayout).toHaveBeenCalledTimes(1);
    expect(client.replaceTerminalWorkspaceLayout.mock.calls[0]?.[0].layout.tabs[0]?.panes[0]?.kind)
      .toBe("launcher");
    wrapper.unmount();
  });

  it("prefills Quick Connect from the inline launcher target", async () => {
    const { wrapper } = await mountShell();

    await openLauncherQuickConnect(".terminal-launcher", "ops@edge.example.test:2200");

    expect(document.querySelector<HTMLInputElement>("#quick-address")?.value)
      .toBe("edge.example.test");
    expect(document.querySelector<HTMLInputElement>("#quick-port")?.value).toBe("2200");
    expect(document.querySelector<HTMLInputElement>("#quick-username")?.value).toBe("ops");
    wrapper.unmount();
  });

  it("routes the launcher SSH Config action to the import flow", async () => {
    const { router, wrapper } = await mountShell();

    Array.from(document.querySelectorAll<HTMLButtonElement>(".terminal-launcher__method"))
      .find((button) => button.textContent?.includes("导入 SSH Config"))
      ?.click();
    await flushPromises();

    expect(router.currentRoute.value.path).toBe("/hosts");
    expect(router.currentRoute.value.query.importSshConfig).toBe("1");
    wrapper.unmount();
  });

  it("opens a fresh local terminal when the new-terminal preference requests it", async () => {
    localStorage.setItem("norishell.ui.preferences.v1", JSON.stringify({
      newTerminalBehavior: "localTerminal",
    }));
    const { wrapper } = await mountShell();

    document.querySelector<HTMLButtonElement>('button[aria-label="新建连接"]')?.click();
    await flushPromises();

    expect(document.querySelectorAll('[role="tab"]')).toHaveLength(1);
    expect(document.querySelector(".local-pane-contract-stub")?.getAttribute("data-deferred-start"))
      .toBe("false");
    expect(document.body.textContent).not.toContain(i18n.global.t("sshTerminal.newTabState"));
    wrapper.unmount();
  });

  it("keeps restored Quick Connect history credential-gated because secrets are not persisted", async () => {
    client.fetchTerminalWorkspaceLayout.mockResolvedValue({
      revision: "7",
      layout: {
        schemaVersion: 1,
        activeTabId: "tab-1",
        tabs: [{
          tabId: "tab-1",
          layout: { kind: "pane", paneId: "pane-1", terminalId: "pane-1" },
          activePaneId: "pane-1",
          panes: [{
            kind: "sshQuickConnect",
            paneId: "pane-1",
            label: "temporary@example.test",
            endpoint: { address: "example.test", port: 22, username: "temporary" },
          }],
        }],
      },
      updatedAtUnixMs: 10,
    });
    const { wrapper } = await mountShell();

    expect(document.querySelector(".ssh-pane-contract-stub")?.getAttribute("data-deferred-start"))
      .toBe("true");
    expect(document.querySelector(".ssh-pane-contract-stub")?.getAttribute("data-target-kind"))
      .toBe("quickConnect");
    expect(document.querySelector(".ssh-pane-contract-stub")?.getAttribute("data-deferred-recovery"))
      .toBe("credential");
    wrapper.unmount();
  });

  it("unlocks Vault and retries the same Pane after Core reports a locked credential", async () => {
    const readyHost = { ...host, identityId: "019d0000-0000-7000-8000-000000000501", hasReadyCredential: true };
    client.listHosts.mockResolvedValue([readyHost]);
    client.fetchVaultStatus.mockResolvedValue({
      state: "locked",
      vaultId: "019d0000-0000-7000-8000-000000000504",
      revision: "2",
      entryCount: 1,
    });
    client.fetchTerminalWorkspaceLayout.mockResolvedValue({
      revision: "7",
      layout: {
        schemaVersion: 1,
        activeTabId: "tab-1",
        tabs: [{
          tabId: "tab-1",
          layout: { kind: "pane", paneId: "pane-1", terminalId: "pane-1" },
          activePaneId: "pane-1",
          panes: [{
            kind: "sshHost",
            paneId: "pane-1",
            label: readyHost.label,
            hostId: readyHost.hostId,
          }],
        }],
      },
      updatedAtUnixMs: 10,
    });
    const { wrapper } = await mountShell();
    const pane = wrapper.findComponent(SshPaneContractStub);

    pane.vm.$emit("requestAuthenticationRecovery", "pane-1", {
      kind: "host",
      hostId: readyHost.hostId,
      expectedHostStateVersion: readyHost.stateVersion,
    });
    await flushPromises();

    await flushPromises();

    expect(secureVault).toHaveBeenCalledWith("ensureUnlocked");
    expect(reconnectSavedCredential).toHaveBeenCalledTimes(1);
    wrapper.unmount();
  });

  it.each(["requestCredential", "requestVaultUnlock"])("deduplicates %s while waiting for Host metadata before showing a prompt", async (eventName) => {
    const readyHost = { ...host, hasReadyCredential: true };
    client.listHosts.mockResolvedValue([readyHost]);
    client.fetchVaultStatus.mockResolvedValue({ state: "locked", vaultId: "vault-1", revision: "1", entryCount: 1 });
    client.fetchTerminalWorkspaceLayout.mockResolvedValue({
      revision: "7",
      layout: { schemaVersion: 1, activeTabId: "tab-1", tabs: [{
        tabId: "tab-1", layout: { kind: "pane", paneId: "pane-1", terminalId: "pane-1" },
        activePaneId: "pane-1", panes: [{ kind: "sshHost", paneId: "pane-1", label: readyHost.label, hostId: readyHost.hostId }],
      }] }, updatedAtUnixMs: 10,
    });
    const { wrapper } = await mountShell();
    let resolveHosts!: (hosts: HostSummary[]) => void;
    client.listHosts.mockReturnValue(new Promise<HostSummary[]>((resolve) => { resolveHosts = resolve; }));
    const callsBefore = client.listHosts.mock.calls.length;
    const pane = wrapper.findComponent(SshPaneContractStub);
    const target = { kind: "host", hostId: readyHost.hostId, expectedHostStateVersion: readyHost.stateVersion };
    pane.vm.$emit(eventName, "pane-1", target);
    await flushPromises();
    pane.vm.$emit(eventName, "pane-1", target);
    await flushPromises();
    expect(client.listHosts).toHaveBeenCalledTimes(callsBefore + 1);
    resolveHosts([readyHost]);
    await flushPromises();
    if (eventName === "requestCredential") {
      expect(document.querySelector("#quick-address")).not.toBeNull();
      expect(secureCredential).not.toHaveBeenCalled();
    } else {
      expect(secureVault).toHaveBeenCalledTimes(1);
    }
    wrapper.unmount();
  });

  it("does not offer Vault creation as a recovery path for a saved credential when its local Vault is missing", async () => {
    const readyHost = { ...host, identityId: "019d0000-0000-7000-8000-000000000501", hasReadyCredential: true };
    client.listHosts.mockResolvedValue([readyHost]);
    client.fetchVaultStatus.mockResolvedValue({ state: "missing", vaultId: null, revision: null, entryCount: null });
    client.fetchTerminalWorkspaceLayout.mockResolvedValue({
      revision: "7",
      layout: {
        schemaVersion: 1,
        activeTabId: "tab-1",
        tabs: [{
          tabId: "tab-1",
          layout: { kind: "pane", paneId: "pane-1", terminalId: "pane-1" },
          activePaneId: "pane-1",
          panes: [{ kind: "sshHost", paneId: "pane-1", label: readyHost.label, hostId: readyHost.hostId }],
        }],
      },
      updatedAtUnixMs: 10,
    });
    const { wrapper } = await mountShell();
    const pane = wrapper.findComponent(SshPaneContractStub);

    pane.vm.$emit("requestVaultUnlock", "pane-1", {
      kind: "host",
      hostId: readyHost.hostId,
      expectedHostStateVersion: readyHost.stateVersion,
    });
    await flushPromises();

    expect(document.querySelector("#vault-flow-password")).toBeNull();
    expect(secureVault).not.toHaveBeenCalled();
    expect(client.createVault).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("keeps restored Telnet history closed until the user accepts all risks", async () => {
    client.fetchTerminalWorkspaceLayout.mockResolvedValue({
      revision: "7",
      layout: {
        schemaVersion: 1,
        activeTabId: "tab-1",
        tabs: [{
          tabId: "tab-1",
          layout: { kind: "pane", paneId: "pane-telnet", terminalId: "pane-telnet" },
          activePaneId: "pane-telnet",
          panes: [{
            kind: "telnet",
            paneId: "pane-telnet",
            label: "legacy.example.test:23",
            address: "legacy.example.test",
            port: 23,
          }],
        }],
      },
      updatedAtUnixMs: 10,
    });

    const { wrapper } = await mountShell();

    expect(wrapper.findComponent({ name: "NvxTelnetTerminalPane" }).props("deferredStart"))
      .toBe(true);
    wrapper.unmount();
  });

  it("does not treat a failed Core session snapshot as proof that restart restore is safe", async () => {
    client.fetchSshSessionSnapshot.mockRejectedValue(new Error("Core unavailable"));
    client.fetchTerminalWorkspaceLayout.mockResolvedValue({
      revision: "7",
      layout: {
        schemaVersion: 1,
        activeTabId: "tab-1",
        tabs: [{
          tabId: "tab-1",
          layout: { kind: "pane", paneId: "pane-1", terminalId: "" },
          activePaneId: "pane-1",
          panes: [{ kind: "local", paneId: "pane-1", label: "zsh" }],
        }],
      },
      updatedAtUnixMs: 10,
    });
    const { wrapper } = await mountShell();

    expect(document.querySelectorAll('[role="tab"]')).toHaveLength(0);
    expect(document.body.textContent).toContain("终端布局暂时无法读取或保存");
    expect(client.replaceTerminalWorkspaceLayout).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("restores cleanup-failed Local sessions and closes the UI before termination retry completes", async () => {
    client.fetchLocalSessionSnapshot.mockResolvedValue({
      snapshotRevision: "10",
      sessions: [cleanupFailedLocalSession],
    });
    const { wrapper } = await mountShell();

    expect(document.querySelector(".local-pane-contract-stub")).not.toBeNull();
    document.querySelector<HTMLButtonElement>(
      'button[aria-label^="关闭标签页："]',
    )?.click();
    await flushPromises();
    expect(document.body.textContent).toContain("1 个仍在连接或运行的终端会话");
    Array.from(document.querySelectorAll<HTMLButtonElement>(".nvx-dialog__actions button"))
      .find((candidate) => candidate.textContent?.includes("断开 1 个会话"))?.click();
    await flushPromises();

    expect(terminateForClose).toHaveBeenCalledTimes(1);
    expect(document.querySelectorAll('[role="tab"]')).toHaveLength(0);
    wrapper.unmount();
  });

  it("focuses only the exact notification session across routes and respects a blocking dialog", async () => {
    client.fetchSshSessionSnapshot.mockResolvedValue({ snapshotRevision: "9", sessions: [runningSession] });
    const { wrapper, router, pinia } = await mountShell();
    const scope = { kind: "ssh" as const, sessionId: runningSession.sessionId, generation: runningSession.generation, channelId: runningSession.channelId!, paneId: wrapper.findComponent(SshPaneContractStub).props("paneId") };
    const controller = useWorkspaceTabsStore(pinia).terminalController!;
    await router.push("/hosts"); await flushPromises();
    expect(controller.focusNativeSession({ ...scope, generation: "999" })).toBe(false); await flushPromises();
    expect(router.currentRoute.value.path).toBe("/hosts");
    const dialog = document.createElement("div"); dialog.setAttribute("role", "dialog"); dialog.setAttribute("aria-modal", "true"); document.body.append(dialog);
    expect(controller.focusNativeSession(scope)).toBe(false); await flushPromises();
    expect(router.currentRoute.value.path).toBe("/hosts"); dialog.remove();
    expect(controller.focusNativeSession(scope)).toBe(true); await flushPromises();
    expect(router.currentRoute.value.path).toBe("/terminal");
    expect(wrapper.findAllComponents(SshPaneContractStub)).toHaveLength(1);
    expect(disconnectForClose).not.toHaveBeenCalled(); expect(reconnectWithCredential).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("focuses existing SSH and Local failure panes by session and generation without requiring Channel or PTY", async () => {
    client.fetchSshSessionSnapshot.mockResolvedValue({ snapshotRevision: "9", sessions: [runningSession] });
    client.fetchLocalSessionSnapshot.mockResolvedValue({ snapshotRevision: "9", sessions: [cleanupFailedLocalSession] });
    const { wrapper, router, pinia } = await mountShell();
    const controller = useWorkspaceTabsStore(pinia).terminalController!;
    const sshSnapshotCalls = client.fetchSshSessionSnapshot.mock.calls.length;
    const localSnapshotCalls = client.fetchLocalSessionSnapshot.mock.calls.length;

    await router.push("/hosts");
    expect(controller.focusSshSession(runningSession.sessionId, "999")).toBe(false);
    expect(controller.focusSshSession(runningSession.sessionId, runningSession.generation)).toBe(true);
    await flushPromises();
    expect(router.currentRoute.value.path).toBe("/terminal");

    await router.push("/hosts");
    expect(controller.focusLocalSession(cleanupFailedLocalSession.sessionId, "999")).toBe(false);
    expect(controller.focusLocalSession(
      cleanupFailedLocalSession.sessionId,
      cleanupFailedLocalSession.generation,
    )).toBe(true);
    await flushPromises();
    expect(router.currentRoute.value.path).toBe("/terminal");
    expect(client.fetchSshSessionSnapshot).toHaveBeenCalledTimes(sshSnapshotCalls);
    expect(client.fetchLocalSessionSnapshot).toHaveBeenCalledTimes(localSnapshotCalls);
    wrapper.unmount();
  });

  it("routes Header click and Arrow activation through Pane focus ownership, then clears it for a blank Tab", async () => {
    const secondSession: SshSessionSummary = {
      ...runningSession,
      sessionId: "019d0000-0000-7000-8000-000000000211",
      openAttemptId: "019d0000-0000-7000-8000-000000000212",
      channelId: "019d0000-0000-7000-8000-000000000213",
      endpoint: { address: "second.example.test", port: 22, username: "root" },
      target: {
        kind: "quickConnect",
        endpoint: { address: "second.example.test", port: 22, username: "root" },
      },
    };
    client.fetchSshSessionSnapshot.mockResolvedValue({
      snapshotRevision: "9",
      sessions: [runningSession, secondSession],
    });
    const { wrapper } = await mountShell();
    const firstLabel = "norishell@127.0.0.1";
    const secondLabel = "root@second.example.test";
    const tabs = Array.from(document.querySelectorAll<HTMLButtonElement>('[role="tab"]'));

    expect(focusedTerminalLabel.value).toBe(firstLabel);
    tabs.find((tab) => tab.textContent?.includes(secondLabel))?.click();
    await flushPromises();
    expect(paneControls.get(firstLabel)?.deactivateFromTab).toHaveBeenCalled();
    expect(paneControls.get(secondLabel)?.activateFromTab).toHaveBeenCalled();
    expect(focusedTerminalLabel.value).toBe(secondLabel);
    expect(await runInFocusedTerminal("uptime")).toBe("sent");
    expect(paneControls.get(secondLabel)?.send).toHaveBeenCalledWith("uptime\r");
    expect(paneControls.get(firstLabel)?.send).not.toHaveBeenCalled();

    tabs.find((tab) => tab.textContent?.includes(secondLabel))?.dispatchEvent(new KeyboardEvent(
      "keydown",
      { key: "ArrowLeft", bubbles: true },
    ));
    await flushPromises();
    expect(paneControls.get(secondLabel)?.deactivateFromTab).toHaveBeenCalled();
    expect(focusedTerminalLabel.value).toBe(firstLabel);

    document.querySelector<HTMLButtonElement>(
      `button[aria-label="${i18n.global.t("sshTerminal.closeTab")}：${secondLabel}"]`,
    )?.click();
    await flushPromises();
    expect(focusedTerminalLabel.value).toBeNull();
    Array.from(document.querySelectorAll<HTMLButtonElement>(".nvx-dialog__actions button"))
      .find((button) => button.textContent?.includes(i18n.global.t("sshTerminal.cancel")))
      ?.click();
    await flushPromises();
    expect(focusedTerminalLabel.value).toBe(secondLabel);

    document.querySelector<HTMLButtonElement>(
      `button[aria-label="${i18n.global.t("sshTerminal.newConnection")}"]`,
    )?.click();
    await flushPromises();
    expect(focusedTerminalLabel.value).toBeNull();
    expect(await runInFocusedTerminal("whoami")).toBe("unavailable");
    wrapper.unmount();
  });

  it("reconciles SSH Pane state when the Terminal window returns to the foreground", async () => {
    client.fetchSshSessionSnapshot.mockResolvedValue({
      snapshotRevision: "9",
      sessions: [runningSession],
    });
    const { wrapper } = await mountShell();
    reconcileAfterForeground.mockClear();

    window.dispatchEvent(new Event("focus"));
    await flushPromises();

    expect(reconcileAfterForeground).toHaveBeenCalled();
    wrapper.unmount();
  });

  it.each([
    { platform: "MacIntel", modifier: { metaKey: true }, wrongModifier: { ctrlKey: true } },
    { platform: "Win32", modifier: { ctrlKey: true }, wrongModifier: { metaKey: true } },
  ])("reserves numeric Tab shortcuts on $platform before terminal input", async ({
    platform,
    modifier,
    wrongModifier,
  }) => {
    Object.defineProperty(navigator, "platform", { configurable: true, value: platform });
    const secondSession: SshSessionSummary = {
      ...runningSession,
      sessionId: "019d0000-0000-7000-8000-000000000221",
      openAttemptId: "019d0000-0000-7000-8000-000000000222",
      channelId: "019d0000-0000-7000-8000-000000000223",
      endpoint: { address: "shortcut.example.test", port: 22, username: "root" },
      target: {
        kind: "quickConnect",
        endpoint: { address: "shortcut.example.test", port: 22, username: "root" },
      },
    };
    client.fetchSshSessionSnapshot.mockResolvedValue({
      snapshotRevision: "9",
      sessions: [runningSession, secondSession],
    });
    const { wrapper } = await mountShell();
    const terminalSurface = document.querySelector<HTMLElement>(".ssh-pane-contract-stub");

    const wrongPlatformEvent = new KeyboardEvent("keydown", {
      key: "2",
      ...wrongModifier,
      bubbles: true,
      cancelable: true,
    });
    terminalSurface?.dispatchEvent(wrongPlatformEvent);
    await flushPromises();
    expect(wrongPlatformEvent.defaultPrevented).toBe(false);
    expect(focusedTerminalLabel.value).toBe("norishell@127.0.0.1");

    const selectSecond = new KeyboardEvent("keydown", {
      key: "2",
      ...modifier,
      bubbles: true,
      cancelable: true,
    });
    terminalSurface?.dispatchEvent(selectSecond);
    await flushPromises();
    expect(selectSecond.defaultPrevented).toBe(true);
    expect(focusedTerminalLabel.value).toBe("root@shortcut.example.test");
    expect(paneControls.get("norishell@127.0.0.1")?.deactivateFromTab).toHaveBeenCalled();
    expect(paneControls.get("root@shortcut.example.test")?.activateFromTab).toHaveBeenCalled();

    const missingTarget = new KeyboardEvent("keydown", {
      key: "9",
      ...modifier,
      bubbles: true,
      cancelable: true,
    });
    terminalSurface?.dispatchEvent(missingTarget);
    await flushPromises();
    expect(missingTarget.defaultPrevented).toBe(true);
    expect(focusedTerminalLabel.value).toBe("root@shortcut.example.test");
    wrapper.unmount();
  });

  it("consumes a numeric Tab shortcut without switching behind a blocking dialog", async () => {
    Object.defineProperty(navigator, "platform", { configurable: true, value: "MacIntel" });
    client.fetchSshSessionSnapshot.mockResolvedValue({
      snapshotRevision: "9",
      sessions: [runningSession],
    });
    const { wrapper } = await mountShell();

    document.querySelector<HTMLButtonElement>('button[aria-label="新建连接"]')?.click();
    await flushPromises();
    await openLauncherQuickConnect(".terminal-pane-launcher", "dialog.example.test");
    expect(document.querySelector('[role="dialog"]')).not.toBeNull();
    expect(focusedTerminalLabel.value).toBeNull();

    const blockedShortcut = new KeyboardEvent("keydown", {
      key: "1",
      metaKey: true,
      bubbles: true,
      cancelable: true,
    });
    document.querySelector<HTMLInputElement>("#quick-address")?.dispatchEvent(blockedShortcut);
    await flushPromises();
    expect(blockedShortcut.defaultPrevented).toBe(true);
    expect(focusedTerminalLabel.value).toBeNull();
    expect(document.querySelectorAll<HTMLElement>('[role="tab"]')[0]?.getAttribute("aria-selected"))
      .toBe("false");
    wrapper.unmount();
  });

  it("renders every Telnet risk acknowledgement as visible checkbox text", async () => {
    const { wrapper } = await mountShell();

    document.querySelector<HTMLButtonElement>('button[aria-label="新建连接"]')?.click();
    await flushPromises();
    Array.from(document.querySelectorAll<HTMLButtonElement>(".terminal-pane-launcher button"))
      .find((button) => button.textContent?.includes(i18n.global.t("sshTerminal.telnetAction")))
      ?.click();
    await flushPromises();

    const dialog = document.querySelector<HTMLElement>('[role="dialog"]');
    expect(dialog?.textContent).toContain(i18n.global.t("telnetSession.acceptCleartext"));
    expect(dialog?.textContent).toContain(i18n.global.t("telnetSession.acceptMissingIdentity"));
    expect(dialog?.textContent).toContain(i18n.global.t("telnetSession.acceptTampering"));
    expect(dialog?.querySelectorAll(".nvx-checkbox__label")).toHaveLength(3);
    wrapper.unmount();
  });

  it("requires explicit confirmation, then hides the Tab before disconnect completes", async () => {
    client.fetchSshSessionSnapshot.mockResolvedValue({
      snapshotRevision: "9",
      sessions: [runningSession],
    });
    const { wrapper } = await mountShell();

    document.querySelector<HTMLButtonElement>(
      `button[aria-label="关闭标签页：norishell@127.0.0.1"]`,
    )?.click();
    await flushPromises();
    expect(disconnectForClose).not.toHaveBeenCalled();
    expect(document.body.textContent).toContain("不再提示单 Pane 标签页");
    Array.from(document.querySelectorAll<HTMLButtonElement>(".nvx-dialog__actions button"))
      .find((candidate) => candidate.textContent?.includes("关闭标签页"))?.click();
    await flushPromises();

    expect(disconnectForClose).toHaveBeenCalledTimes(1);
    expect(document.querySelector(".ssh-pane-contract-stub")).toBeNull();
    expect(document.querySelectorAll('[role="tab"]')).toHaveLength(0);
    wrapper.unmount();
  });

  it("skips the prompt only for a single-Pane Tab when the preference is enabled", async () => {
    localStorage.setItem("norishell.ui.preferences.v1", JSON.stringify({
      singlePaneTabCloseBehavior: "closeDirectly",
    }));
    client.fetchSshSessionSnapshot.mockResolvedValue({
      snapshotRevision: "9",
      sessions: [runningSession],
    });
    const { wrapper } = await mountShell();

    document.querySelector<HTMLButtonElement>(
      `button[aria-label="关闭标签页：norishell@127.0.0.1"]`,
    )?.click();
    await flushPromises();

    expect(disconnectForClose).toHaveBeenCalledTimes(1);
    expect(document.querySelector('[role="dialog"]')).toBeNull();
    expect(document.querySelector(".ssh-pane-contract-stub")).toBeNull();
    wrapper.unmount();
  });

  it("persists the single-Pane don't-ask-again choice from the confirmation", async () => {
    client.fetchSshSessionSnapshot.mockResolvedValue({
      snapshotRevision: "9",
      sessions: [runningSession],
    });
    const { wrapper } = await mountShell();

    document.querySelector<HTMLButtonElement>(
      `button[aria-label="关闭标签页：norishell@127.0.0.1"]`,
    )?.click();
    await flushPromises();
    const checkbox = document.querySelector<HTMLInputElement>(
      '.nvx-dialog input[type="checkbox"]',
    );
    checkbox?.click();
    Array.from(document.querySelectorAll<HTMLButtonElement>(".nvx-dialog__actions button"))
      .find((candidate) => candidate.textContent?.includes("关闭标签页"))?.click();
    await flushPromises();

    expect(JSON.parse(localStorage.getItem("norishell.ui.preferences.v1") ?? "{}"))
      .toMatchObject({ singlePaneTabCloseBehavior: "closeDirectly" });
    wrapper.unmount();
  });

  it("splits the active Tab into a real Pane Launcher and reuses it for a Local terminal", async () => {
    client.fetchSshSessionSnapshot.mockResolvedValue({
      snapshotRevision: "9",
      sessions: [runningSession],
    });
    const { wrapper } = await mountShell();

    document.querySelector<HTMLButtonElement>('button[aria-label="向右拆分 Pane"]')?.click();
    await flushPromises();

    expect(focusedTerminalLabel.value).toBeNull();
    expect(document.querySelectorAll(".nvx-terminal-split-tree__pane")).toHaveLength(2);
    expect(document.querySelectorAll('[role="separator"]')).toHaveLength(1);
    expect(document.querySelector(".terminal-pane-launcher")).not.toBeNull();
    expect(document.querySelectorAll('[role="tab"]')).toHaveLength(1);
    expect(document.querySelector('[role="tab"]')?.textContent).toContain("2 个 Pane");

    const separator = document.querySelector<HTMLElement>('[role="separator"]');
    separator?.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
    await flushPromises();
    expect(separator?.getAttribute("aria-valuenow")).toBe("55");

    Array.from(document.querySelectorAll<HTMLButtonElement>(".terminal-pane-launcher button"))
      .find((candidate) => candidate.textContent?.includes("本地终端"))?.click();
    await flushPromises();

    expect(document.querySelector(".terminal-pane-launcher")).toBeNull();
    expect(document.querySelector(".local-pane-contract-stub")).not.toBeNull();
    expect(document.querySelectorAll('[role="tab"]')).toHaveLength(1);
    expect(document.querySelector('[role="tab"]')?.textContent).toContain("本机 Shell");
    wrapper.unmount();
  });

  it("reuses a split Launcher for Quick Connect without adding another Header Tab", async () => {
    client.fetchSshSessionSnapshot.mockResolvedValue({
      snapshotRevision: "9",
      sessions: [runningSession],
    });
    const { wrapper } = await mountShell();
    const body = new DOMWrapper(document.body);
    document.querySelector<HTMLButtonElement>('button[aria-label="向右拆分 Pane"]')?.click();
    await flushPromises();
    await openLauncherQuickConnect(".terminal-pane-launcher", "split.example.test");
    await body.get("#quick-address").setValue("split.example.test");
    await body.get("#quick-username").setValue("ops");
    document.querySelector<HTMLButtonElement>(".nvx-dialog__actions .nvx-button--primary")?.click();
    await flushPromises();

    expect(document.querySelectorAll('[role="tab"]')).toHaveLength(1);
    expect(document.querySelectorAll(".ssh-pane-contract-stub")).toHaveLength(2);
    expect(document.querySelector(".terminal-pane-launcher")).toBeNull();
    expect(document.querySelector('[role="tab"]')?.textContent).toContain("ops@split.example.test");
    expect(document.querySelector('[role="tab"]')?.textContent).toContain("2 个 Pane");
    wrapper.unmount();
  });

  it("always confirms a multi-Pane Tab, then closes its UI before background disconnects", async () => {
    localStorage.setItem("norishell.ui.preferences.v1", JSON.stringify({
      singlePaneTabCloseBehavior: "closeDirectly",
    }));
    client.fetchSshSessionSnapshot.mockResolvedValue({
      snapshotRevision: "9",
      sessions: [runningSession],
    });
    const { wrapper } = await mountShell();
    document.querySelector<HTMLButtonElement>('button[aria-label="向右拆分 Pane"]')?.click();
    await flushPromises();
    Array.from(document.querySelectorAll<HTMLButtonElement>(".terminal-pane-launcher button"))
      .find((candidate) => candidate.textContent?.includes("本地终端"))?.click();
    await flushPromises();

    document.querySelector<HTMLButtonElement>(
      'button[aria-label^="关闭标签页："]',
    )?.click();
    await flushPromises();
    expect(document.body.textContent).toContain("2 个仍在连接或运行的终端会话");
    expect(document.querySelector('[role="dialog"]')).not.toBeNull();
    expect(document.body.textContent).not.toContain("不再提示单 Pane 标签页");
    Array.from(document.querySelectorAll<HTMLButtonElement>(".nvx-dialog__actions button"))
      .find((candidate) => candidate.textContent?.includes("断开 2 个会话"))?.click();
    await flushPromises();

    expect(disconnectForClose).toHaveBeenCalledTimes(1);
    expect(terminateForClose).toHaveBeenCalledTimes(1);
    expect(document.querySelectorAll('[role="tab"]')).toHaveLength(0);
    wrapper.unmount();
  });

  it("restores the optimistically hidden Tab and retry controls when disconnect times out", async () => {
    client.fetchSshSessionSnapshot.mockResolvedValue({
      snapshotRevision: "9",
      sessions: [runningSession],
    });
    const { wrapper } = await mountShell();
    const firstDisconnect = deferred<void>();
    disconnectForClose
      .mockImplementationOnce(() => firstDisconnect.promise)
      .mockImplementationOnce(() => new Promise<void>(() => undefined));
    vi.useFakeTimers();

    document.querySelector<HTMLButtonElement>(
      'button[aria-label^="关闭标签页："]',
    )?.click();
    await flushPromises();
    Array.from(document.querySelectorAll<HTMLButtonElement>(".nvx-dialog__actions button"))
      .find((candidate) => candidate.textContent?.includes("断开 1 个会话"))?.click();
    await flushPromises();
    expect(document.querySelectorAll('[role="tab"]')).toHaveLength(0);

    await vi.advanceTimersByTimeAsync(15_000);
    await flushPromises();
    expect(document.querySelectorAll('[role="tab"]')).toHaveLength(1);
    expect(document.body.textContent).toContain("至少一个终端未能关闭");
    expect(Array.from(document.querySelectorAll<HTMLButtonElement>(".nvx-dialog__actions button"))
      .find((candidate) => candidate.textContent?.includes("断开 1 个会话"))?.disabled).toBe(false);

    Array.from(document.querySelectorAll<HTMLButtonElement>(".nvx-dialog__actions button"))
      .find((candidate) => candidate.textContent?.includes("断开 1 个会话"))?.click();
    await flushPromises();
    expect(document.querySelectorAll('[role="tab"]')).toHaveLength(0);

    firstDisconnect.resolve(undefined);
    await flushPromises();
    await vi.advanceTimersByTimeAsync(15_000);
    await flushPromises();
    expect(document.querySelectorAll('[role="tab"]')).toHaveLength(1);
    expect(document.body.textContent).toContain("至少一个终端未能关闭");
    wrapper.unmount();
  });

  it("closes only the confirmed active Pane and keeps the sibling session visible", async () => {
    client.fetchSshSessionSnapshot.mockResolvedValue({
      snapshotRevision: "9",
      sessions: [runningSession],
    });
    const { wrapper } = await mountShell();
    document.querySelector<HTMLButtonElement>('button[aria-label="向右拆分 Pane"]')?.click();
    await flushPromises();
    Array.from(document.querySelectorAll<HTMLButtonElement>(".terminal-pane-launcher button"))
      .find((candidate) => candidate.textContent?.includes("本地终端"))?.click();
    await flushPromises();

    document.querySelectorAll<HTMLElement>(".nvx-terminal-split-tree__pane")[0]?.dispatchEvent(
      new PointerEvent("pointerdown", { bubbles: true }),
    );
    await flushPromises();
    document.querySelector<HTMLButtonElement>('button[aria-label="关闭当前 Pane"]')?.click();
    await flushPromises();
    expect(document.body.textContent).toContain("断开并关闭此 Pane");
    Array.from(document.querySelectorAll<HTMLButtonElement>(".nvx-dialog__actions button"))
      .find((candidate) => candidate.textContent?.includes("断开并关闭 Pane"))?.click();
    await flushPromises();
    expect(disconnectForClose).toHaveBeenCalledTimes(1);
    expect(document.querySelectorAll(".nvx-terminal-split-tree__pane")).toHaveLength(2);

    wrapper.findComponent(SshPaneContractStub).vm.$emit("state", "closed", {
      ...runningSession,
      state: "closed",
    });
    await flushPromises();
    expect(document.querySelectorAll(".nvx-terminal-split-tree__pane")).toHaveLength(1);
    expect(document.querySelector(".local-pane-contract-stub")).not.toBeNull();
    expect(document.querySelectorAll('[role="tab"]')).toHaveLength(1);
    wrapper.unmount();
  });

  it("preserves a recent Host target when using a transient credential", async () => {
    client.listHostCatalog.mockResolvedValue([{
      host,
      group: null,
      tags: [],
      recentConnection: {
        hostId: host.hostId,
        connectedAtUnixMs: 1_000,
        recencySequence: "1",
        successfulConnectionCount: "1",
      },
    }]);
    const { wrapper } = await mountShell();
    const body = new DOMWrapper(document.body);

    document.querySelector<HTMLButtonElement>('button[aria-label="新建连接"]')?.click();
    await flushPromises();
    expect(document.querySelectorAll('[role="tab"]')).toHaveLength(1);
    expect(document.body.textContent).toContain(host.label);
    expect(document.body.textContent).toContain("最近连接");

    Array.from(document.querySelectorAll<HTMLButtonElement>(".ssh-terminal-recent__item"))
      .find((candidate) => candidate.textContent?.includes(host.label))?.click();
    await flushPromises();
    expect(body.get("#quick-address").element).toHaveProperty("value", host.address);
    expect(document.querySelectorAll('[role="tab"]')).toHaveLength(1);
    expect(document.querySelector(".ssh-pane-contract-stub")).toBeNull();

    document.querySelector<HTMLButtonElement>(".nvx-dialog__actions .nvx-button--primary")?.click();
    await flushPromises();

    expect(secureCredential).toHaveBeenCalledWith({
      kind: "password",
      label: host.label,
      identityId: null,
    });
    expect(document.querySelectorAll('[role="tab"]')).toHaveLength(1);
    expect(document.querySelector(".ssh-pane-contract-stub")?.getAttribute("data-target-kind"))
      .toBe("host");
    expect(document.querySelector(".ssh-pane-contract-stub")?.getAttribute("data-credential-ref"))
      .toBe("019d0000-0000-7000-8000-000000000503");

    wrapper.findComponent(SshPaneContractStub).vm.$emit(
      "requestCredential",
      "pane-id",
      {
        kind: "host",
        hostId: host.hostId,
        expectedHostStateVersion: host.stateVersion,
      },
    );
    await flushPromises();
    expect(document.body.textContent).toContain("重新输入 SSH 认证");
    expect(document.querySelector<HTMLInputElement>("#quick-address")?.disabled).toBe(true);
    expect(document.querySelector<HTMLInputElement>("#quick-username")?.disabled).toBe(true);
    document.querySelector<HTMLButtonElement>(".nvx-dialog__actions .nvx-button--primary")?.click();
    await flushPromises();
    expect(secureCredential).toHaveBeenLastCalledWith(expect.objectContaining({
      identityId: null,
    }));
    expect(reconnectWithCredential).toHaveBeenCalledWith(
      "019d0000-0000-7000-8000-000000000503",
    );
    wrapper.unmount();
  });

  it("uses an independent Quick Connect when the requested Host endpoint is edited", async () => {
    const { wrapper } = await mountShell(`/terminal?hostId=${host.hostId}`);
    const body = new DOMWrapper(document.body);
    await body.get("#quick-address").setValue("different.example.com");
    document.querySelector<HTMLButtonElement>(".nvx-dialog__actions .nvx-button--primary")?.click();
    await flushPromises();
    expect(wrapper.findComponent(SshPaneContractStub).props("target")).toEqual({
      kind: "quickConnect",
      endpoint: { address: "different.example.com", port: host.port, username: host.username },
    });
    expect(client.updateHost).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("saves a credential through Identity/Vault references without placing the secret on Host", async () => {
    client.fetchVaultStatus.mockResolvedValue({
      state: "unlocked",
      vaultId: "019d0000-0000-7000-8000-000000000504",
      revision: "1",
      entryCount: 0,
    });
    const { wrapper } = await mountShell();
    const body = new DOMWrapper(document.body);

    await openLauncherQuickConnect(".terminal-launcher", "saved.example.test");
    await body.get("#quick-address").setValue("saved.example.test");
    await body.get("#quick-port").setValue("2222");
    await body.get("#quick-username").setValue("ops");
    await body.get("#quick-save-credential").setValue(true);
    secureCredential.mockResolvedValueOnce("019d0000-0000-7000-8000-000000000502");
    document.querySelector<HTMLButtonElement>(".nvx-dialog__actions .nvx-button--primary")?.click();
    await flushPromises();

    expect(client.createHost).toHaveBeenCalledWith({
      label: "ops@saved.example.test",
      address: "saved.example.test",
      port: 2222,
      username: "ops",
    });
    expect(client.createHost.mock.calls[0]?.[0]).not.toHaveProperty("secret");
    expect(secureCredential).toHaveBeenCalledWith(expect.objectContaining({
      identityId: "019d0000-0000-7000-8000-000000000501",
      kind: "password",
    }));
    expect(client.updateHost).toHaveBeenCalledWith(expect.objectContaining({
      hostId: host.hostId,
      identityId: "019d0000-0000-7000-8000-000000000501",
    }));
    expect(document.querySelector(".ssh-pane-contract-stub")?.getAttribute("data-credential-ref"))
      .toBe("019d0000-0000-7000-8000-000000000502");
    wrapper.unmount();
  });

  it("keeps a no-secret Host when Vault is skipped and asks again on the next connection", async () => {
    const noCredentialHost = { ...host, identityId: null, hasReadyCredential: false };
    client.createHost.mockImplementation(async () => {
      client.listHosts.mockResolvedValue([noCredentialHost]);
      client.listHostCatalog.mockResolvedValue([{
        host: noCredentialHost,
        group: null,
        tags: [],
        recentConnection: null,
      }]);
      return noCredentialHost;
    });
    const { wrapper } = await mountShell();
    const body = new DOMWrapper(document.body);

    await openLauncherQuickConnect(".terminal-launcher", host.address);
    await body.get("#quick-address").setValue(host.address);
    await body.get("#quick-username").setValue(host.username ?? "");
    await body.get("#quick-save-credential").setValue(true);
    secureCredential.mockResolvedValueOnce(null);
    document.querySelector<HTMLButtonElement>(".nvx-dialog__actions .nvx-button--primary")?.click();
    await flushPromises();

    expect(client.createHost).toHaveBeenCalledTimes(1);
    expect(secureCredential).toHaveBeenCalledWith(expect.objectContaining({ identityId: "019d0000-0000-7000-8000-000000000501" }));
    expect(document.body.textContent).toContain("开始新的终端");
    expect(document.querySelector("#quick-password")).toBeNull();
    expect(secureCredential).toHaveBeenCalledTimes(1);
    wrapper.unmount();
  });

  it("connects a Ready saved Host directly only when Vault is already unlocked", async () => {
    const readyHost = { ...host, hasReadyCredential: true };
    client.listHosts.mockResolvedValue([readyHost]);
    client.listHostCatalog.mockResolvedValue([{
      host: readyHost,
      group: null,
      tags: [],
      recentConnection: null,
    }]);
    client.fetchVaultStatus.mockResolvedValue({
      state: "unlocked",
      vaultId: "019d0000-0000-7000-8000-000000000504",
      revision: "2",
      entryCount: 1,
    });
    const { wrapper } = await mountShell();

    document.querySelector<HTMLButtonElement>('button[aria-label="新建连接"]')?.click();
    await flushPromises();
    document.querySelector<HTMLButtonElement>(".ssh-terminal-recent__item")?.click();
    await flushPromises();

    expect(document.querySelector("#quick-password")).toBeNull();
    expect(document.querySelector(".ssh-pane-contract-stub")?.getAttribute("data-target-kind"))
      .toBe("host");
    expect(secureCredential).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("binds a newly saved reauthentication credential to a Host override before reconnecting", async () => {
    const readyHost = {
      ...host,
      identityId: "019d0000-0000-7000-8000-000000000501",
      hasReadyCredential: true,
    };
    const previousCredentialRefId = "019d0000-0000-7000-8000-000000000599";
    client.listHosts.mockResolvedValue([readyHost]);
    client.listHostCatalog.mockResolvedValue([{
      host: readyHost,
      group: null,
      tags: [],
      recentConnection: {
        hostId: readyHost.hostId,
        connectedAtUnixMs: 1_000,
        recencySequence: "1",
        successfulConnectionCount: "1",
      },
    }]);
    client.fetchVaultStatus.mockResolvedValue({
      state: "unlocked",
      vaultId: "019d0000-0000-7000-8000-000000000504",
      revision: "2",
      entryCount: 1,
    });
    client.getHostConnectionConfig.mockResolvedValue({
      authenticationPlan: {
        hostId: readyHost.hostId,
        revision: "7",
        mode: "hostOverride",
        credentialRefIds: [previousCredentialRefId],
      },
    });
    reconnectWithCredential.mockRejectedValueOnce(new Error("transport stopped"));
    const { wrapper } = await mountShell();
    const body = new DOMWrapper(document.body);

    document.querySelector<HTMLButtonElement>('button[aria-label="新建连接"]')?.click();
    await flushPromises();
    document.querySelector<HTMLButtonElement>(".ssh-terminal-recent__item")?.click();
    await flushPromises();
    wrapper.findComponent(SshPaneContractStub).vm.$emit(
      "requestCredential",
      "pane-id",
      {
        kind: "host",
        hostId: readyHost.hostId,
        expectedHostStateVersion: readyHost.stateVersion,
      },
    );
    await flushPromises();
    await body.get("#quick-save-credential").setValue(true);
    secureCredential.mockResolvedValueOnce("019d0000-0000-7000-8000-000000000502");
    document.querySelector<HTMLButtonElement>(".nvx-dialog__actions .nvx-button--primary")?.click();
    await flushPromises();

    expect(client.replaceAuthenticationPlan).toHaveBeenCalledWith({
      hostId: readyHost.hostId,
      expectedRevision: "7",
      mode: "hostOverride",
      credentialRefIds: [
        "019d0000-0000-7000-8000-000000000502",
        previousCredentialRefId,
      ],
    });
    expect(reconnectWithCredential).toHaveBeenCalledWith(
      "019d0000-0000-7000-8000-000000000502",
    );
    expect(document.body.textContent).toContain("凭据已保存，但 SSH 连接未建立");
    expect(document.body.textContent).toContain("新凭据已安全保存到 Vault");
    wrapper.unmount();
  });
});
