import { DOMWrapper, flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { createPinia } from "pinia";
import { createMemoryHistory, createRouter } from "vue-router";
import { defineComponent } from "vue";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { SftpSessionSnapshot, SftpTransferIntentSnapshot } from "../core-api/generated/core-api";
import { ensureHostVault } from "../core-api/secure-vault-client";
import { openToolWindow } from "../tool-windows";
import { i18n } from "../locales";
import { SFTP_PREFERENCES_KEY } from "../stores/sftpPreferences";
import { useTipsStore } from "../stores/tips";
import { useWorkspaceTabsStore } from "../stores/workspaceTabs";
import { acceptSftpPluginNavigation, discardSftpPluginNavigations } from "./sftpPluginNavigation";
import type { FileTabHandoffSnapshot } from "./fileTabHandoffSnapshot";

const dialog = vi.hoisted(() => ({ open: vi.fn(), save: vi.fn() }));
const terminalNavigation = vi.hoisted(() => ({ directory: vi.fn(async () => undefined), host: vi.fn(async () => undefined) }));
vi.mock("../workspace-tab-shell-action", () => ({
  openWorkspaceDirectoryTerminal: terminalNavigation.directory,
  openWorkspaceTerminalHost: terminalNavigation.host,
}));
const pathApi = vi.hoisted(() => ({ homeDir: vi.fn(), sep: vi.fn(() => "/") }));
const webview = vi.hoisted(() => ({
  handler: null as ((event: { payload: unknown }) => void) | null,
  unlisten: vi.fn(),
  onDragDropEvent: vi.fn(async (handler: (event: { payload: unknown }) => void) => {
    webview.handler = handler;
    return webview.unlisten;
  }),
}));
const client = vi.hoisted(() => ({
  cancelSftpDirectoryListing: vi.fn(),
  cancelSftpTransfer: vi.fn(),
  cancelSftpTransferIntent: vi.fn(),
  createSftpLocalDirectoryChild: vi.fn(),
  disconnectSftpSession: vi.fn(),
  enqueueSftpTransfer: vi.fn(),
  enqueueSftpTransferIntent: vi.fn(),
  fetchSftpSessionSnapshot: vi.fn(),
  fetchSftpTransferIntentSnapshot: vi.fn(),
  fetchSshSessionSnapshot: vi.fn(),
  listHosts: vi.fn(),
  listSftpDirectory: vi.fn(),
  listSftpLocalDirectory: vi.fn(),
  mutateSftpFile: vi.fn(),
  openSftpLocalDirectoryChild: vi.fn(),
  openSftpSession: vi.fn(),
  prepareSftpTransferIntent: vi.fn(),
  previewSftpFile: vi.fn(),
  registerSftpLocalBoundary: vi.fn(),
  registerSftpLocalDirectory: vi.fn(),
  releaseSftpLocalDirectory: vi.fn(),
  retainSftpRemoteCleanupForExit: vi.fn(),
  retainSftpTransferIntentCleanupForExit: vi.fn(),
  retrySftpRemoteCleanup: vi.fn(),
  retrySftpTransferIntentCleanup: vi.fn(),
  resumeSftpTransfer: vi.fn(),
  tailSftpFile: vi.fn(),
}));

vi.mock("../core-api/secure-vault-client", () => ({ ensureHostVault: vi.fn() }));
vi.mock("../tool-windows", () => ({ openToolWindow: vi.fn().mockResolvedValue(undefined), onToolWindowChanged: vi.fn().mockResolvedValue(() => undefined) }));
vi.mock("@tauri-apps/plugin-dialog", () => dialog);
vi.mock("@tauri-apps/api/path", () => pathApi);
vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({ onDragDropEvent: webview.onDragDropEvent }),
}));
vi.mock("../core-api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../core-api/client")>();
  return {
    ...actual,
    canUseDesktopCore: () => true,
    ...client,
  };
});

import SftpView from "./SftpView.vue";

const host = {
  hostId: "019d0000-0000-7000-8000-000000000301",
  label: "Files",
  address: "files.example.test",
  normalizedAddress: "files.example.test",
  port: 22,
  username: "deploy",
  identityId: null,
  favorite: false,
  hasReadyCredential: true,
  stateVersion: "7",
};

function readySnapshot(transfers: SftpSessionSnapshot["transfers"] = []): SftpSessionSnapshot {
  return {
    snapshotRevision: "4",
    sessions: [{
      sessionId: "019d0000-0000-7000-8000-000000000302",
      hostId: host.hostId,
      parentSshSession: null,
      generation: "1",
      stateRevision: "4",
      state: "ready",
      transferCount: transfers.length,
      activeTransferCount: transfers.filter((transfer) => transfer.state === "transferring").length,
      failure: null,
    }],
    transfers,
  };
}

function newFileTab(store: ReturnType<typeof useWorkspaceTabsStore>, kind: "local" | "remote"): string {
  const id = `file:${crypto.randomUUID()}`;
  expect(store.createInitialFileTab(id, kind, null, "")).toBe(true);
  return id;
}

async function mountView(
  snapshot: SftpSessionSnapshot = { snapshotRevision: "0", sessions: [], transfers: [] },
  intentSnapshot: SftpTransferIntentSnapshot = { snapshotRevision: "0", transfers: [] },
  remoteEntryName: string | Array<{
    entryRef: string;
    path: { bytes: number[] };
    displayName: string;
    kind: "file" | "directory" | "symlink" | "other";
    size: number | null;
    modifiedAtUnixMs: number;
    permissionBits: number | null;
  }> = "release.bin",
  options: {
    keepAlive?: boolean;
    hosts?: Array<typeof host>;
    workspaceTab?: "local" | "remote";
    initialHostId?: string;
    initialSessionId?: string;
    initialGeneration?: string;
    routeQuery?: Record<string, string>;
    setupStore?: (store: ReturnType<typeof useWorkspaceTabsStore>) => void;
    handoffSnapshot?: (id: string) => FileTabHandoffSnapshot;
  } = {},
) {
  let currentSnapshot = snapshot;
  let currentIntentSnapshot = intentSnapshot;
  client.fetchSftpSessionSnapshot.mockImplementation(() => Promise.resolve(currentSnapshot));
  client.openSftpSession.mockImplementation((input) => {
    const sessionId = input.sessionId ?? readySnapshot().sessions[0]!.sessionId;
    const generation = input.sessionId ? "2" : "1";
    const reconnected = {
      ...readySnapshot(currentSnapshot.transfers).sessions[0]!,
      sessionId,
      hostId: input.hostId,
      generation,
      transferCount: currentSnapshot.transfers.filter((transfer) => transfer.sessionId === sessionId).length,
    };
    currentSnapshot = {
      snapshotRevision: "5",
      sessions: [
        ...currentSnapshot.sessions.filter((item) => item.sessionId !== sessionId),
        reconnected,
      ],
      transfers: currentSnapshot.transfers,
    };
    return Promise.resolve(reconnected);
  });
  client.disconnectSftpSession.mockImplementation((input) => {
    const closed = currentSnapshot.sessions.find((session) => session.sessionId === input.sessionId);
    currentSnapshot = {
      ...currentSnapshot,
      snapshotRevision: String(BigInt(currentSnapshot.snapshotRevision) + 1n),
      sessions: currentSnapshot.sessions.map((session) => session.sessionId === input.sessionId
        ? { ...session, state: "closed" as const }
        : session),
    };
    return Promise.resolve(closed ? { ...closed, state: "closed" as const } : undefined);
  });
  client.listHosts.mockResolvedValue(options.hosts ?? [host]);
  client.fetchSftpTransferIntentSnapshot.mockImplementation(() => Promise.resolve(currentIntentSnapshot));
  client.releaseSftpLocalDirectory.mockResolvedValue(undefined);
  client.cancelSftpDirectoryListing.mockResolvedValue(undefined);
  client.listSftpDirectory.mockImplementation((input) => Promise.resolve({
    sessionId: input.sessionId,
    generation: input.expectedGeneration,
    directoryRef: input.sessionId === readySnapshot().sessions[0]!.sessionId
      ? "remote-directory-root"
      : `remote-directory-${input.sessionId}`,
    path: input.path,
    entries: Array.isArray(remoteEntryName) ? remoteEntryName : [{
      entryRef: "remote-entry-release",
      path: { bytes: Array.from(new TextEncoder().encode(`/${remoteEntryName}`)) },
      displayName: remoteEntryName,
      kind: "file",
      size: 8,
      modifiedAtUnixMs: 1_788_000_000_000,
      permissionBits: null,
    }],
    nextCursor: null,
  }));
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: "/sftp", component: SftpView },
      { path: "/terminal", component: { template: "<div>Terminal</div>" } },
    ],
  });
  await router.push({ path: "/sftp", query: options.routeQuery });
  await router.isReady();
  const pinia = createPinia();
  const workspaceTabs = useWorkspaceTabsStore(pinia);
  options.setupStore?.(workspaceTabs);
  const workspaceTabId = options.workspaceTab ? newFileTab(workspaceTabs, options.workspaceTab) : undefined;
  const cachedRoute = defineComponent({
    template: '<RouterView v-slot="{ Component, route }"><KeepAlive include="SftpView"><component :is="Component" :key="route.path" /></KeepAlive></RouterView>',
  });
  const wrapper = mount(options.keepAlive ? cachedRoute : SftpView, {
    props: workspaceTabId ? {
      workspaceTabId, initialKind: options.workspaceTab, active: true,
      initialHostId: options.initialHostId ?? null,
      initialSessionId: options.initialSessionId ?? null,
      initialGeneration: options.initialGeneration ?? null,
      handoffSnapshot: options.handoffSnapshot?.(workspaceTabId) ?? null,
    } : {},
    global: {
      plugins: [pinia, router, i18n],
      stubs: {
        NvxCodeEditor: {
          props: ["modelValue", "readonly", "label"],
          emits: ["update:modelValue"],
          template: '<textarea data-code-editor :value="modelValue" :readonly="readonly" :aria-label="label" @input="$emit(\'update:modelValue\', $event.target.value)" />',
        },
      },
    },
  });
  await flushPromises();
  return {
    wrapper,
    router,
    workspaceTabs,
    tips: useTipsStore(pinia),
    setSnapshot: (next: SftpSessionSnapshot) => { currentSnapshot = next; },
    setIntentSnapshot: (next: SftpTransferIntentSnapshot) => { currentIntentSnapshot = next; },
  };
}

async function openTransferActivity(wrapper: VueWrapper) {
  const toggle = wrapper.find<HTMLButtonElement>("[data-sftp-transfer-toggle]");
  expect(toggle.exists()).toBe(true);
  await toggle.trigger("click");
  await flushPromises();
}

async function openPaneActions(pane: DOMWrapper<Element>) {
  const paneTrigger = pane.find<HTMLButtonElement>('button[aria-label="More file actions"]');
  const topbarTrigger = pane.element.closest('.sftp-view')?.querySelector<HTMLButtonElement>('.sftp-view__topbar button[aria-label="More file actions"]');
  const trigger = paneTrigger.exists() ? paneTrigger : topbarTrigger ? new DOMWrapper(topbarTrigger) : paneTrigger;
  expect(trigger.exists()).toBe(true);
  await trigger.trigger("click");
  await flushPromises();
}

function paneAction(pane: DOMWrapper<Element>, label: string) {
  const paneId = pane.element.closest('[data-pane-id]')?.getAttribute("data-pane-id");
  const button = Array.from(document.querySelectorAll<HTMLButtonElement>('.sftp-pane-actions-menu__popover [role="menuitem"]'))
    .find((item) => item.closest('.sftp-pane-actions-menu__popover')?.getAttribute("data-pane-id") === paneId
      && item.textContent?.trim() === label);
  return button ? new DOMWrapper(button) : undefined;
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

async function dragEntryToPane(source: DOMWrapper<Element>, target: DOMWrapper<Element>) {
  const elementFromPoint = vi.spyOn(document, "elementFromPoint").mockReturnValue(target.element);
  try {
    await source.trigger("pointerdown", { button: 0, pointerId: 1, clientX: 16, clientY: 16 });
    await source.trigger("pointermove", { pointerId: 1, clientX: 32, clientY: 32 });
    await source.trigger("pointerup", { pointerId: 1, clientX: 32, clientY: 32 });
  } finally {
    elementFromPoint.mockRestore();
  }
}

describe("SftpView production boundaries", () => {
  it("opens a new terminal at the right-clicked directory instead of the previous selection", async () => {
    const entries = [
      { entryRef: "file", path: { bytes: Array.from(new TextEncoder().encode("/old.txt")) }, displayName: "old.txt", kind: "file" as const, size: 1, modifiedAtUnixMs: 1, permissionBits: null },
      { entryRef: "directory", path: { bytes: Array.from(new TextEncoder().encode("/new folder")) }, displayName: "new folder", kind: "directory" as const, size: null, modifiedAtUnixMs: 1, permissionBits: null },
    ];
    const { wrapper, router } = await mountView(readySnapshot(), undefined, entries);
    const rows = wrapper.findAll('[data-pane-id="sftp-remote-pane"] .sftp-view__entries > button');
    await rows.find((row) => row.text().includes("old.txt"))!.trigger("click");
    await rows.find((row) => row.text().includes("new folder"))!.trigger("contextmenu");
    await wrapper.findAll('[role="menuitem"]').find((button) => button.text().includes("Open terminal here"))!.trigger("click");
    await flushPromises();
    expect(router.currentRoute.value.path).toBe("/sftp");
    expect(terminalNavigation.directory).toHaveBeenCalledWith(
      host.hostId, Array.from(new TextEncoder().encode("/new folder")),
    );
    wrapper.unmount();
  });

  it("uses the current directory for a file or list background right-click", async () => {
    const { wrapper } = await mountView(readySnapshot(), undefined, "old.txt");
    const row = wrapper.find('[data-pane-id="sftp-remote-pane"] .sftp-view__entries > button');
    await row.trigger("contextmenu");
    await wrapper.findAll('[role="menuitem"]').find((button) => button.text().includes("Open terminal here"))!.trigger("click");
    await flushPromises();
    expect(terminalNavigation.directory).toHaveBeenCalledWith(host.hostId, [47]);
    wrapper.unmount();

    const blank = await mountView(readySnapshot(), undefined, "old.txt");
    await blank.wrapper.find('[data-pane-id="sftp-remote-pane"] .sftp-view__entries > button').trigger("click");
    await blank.wrapper.find('[data-pane-id="sftp-remote-pane"] .sftp-view__entries').trigger("contextmenu");
    await blank.wrapper.findAll('[role="menuitem"]').find((button) => button.text().includes("Open terminal here"))!.trigger("click");
    await flushPromises();
    expect(terminalNavigation.directory).toHaveBeenCalledTimes(2);
    expect(terminalNavigation.directory).toHaveBeenLastCalledWith(host.hostId, [47]);
    blank.wrapper.unmount();
  });
  it("positions the measured menu inside the viewport and recomputes after resize", async () => {
    const bounds = vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue({ width: 216, height: 520 } as DOMRect);
    const width = vi.spyOn(window, "innerWidth", "get").mockReturnValue(800);
    const height = vi.spyOn(window, "innerHeight", "get").mockReturnValue(600);
    const { wrapper } = await mountView(readySnapshot(), { snapshotRevision: "0", transfers: [] }, "notes.txt");
    try {
      await wrapper.find('[data-pane-id="sftp-remote-pane"] .sftp-view__entries > button')
        .trigger("contextmenu", { clientX: 790, clientY: 590 });
      await flushPromises();
      const menu = wrapper.get<HTMLElement>('[role="menu"]');
      expect(menu.element.style.left).toBe("576px");
      expect(menu.element.style.top).toBe("72px");
      width.mockReturnValue(400);
      height.mockReturnValue(300);
      bounds.mockReturnValue({ width: 216, height: 284 } as DOMRect);
      window.dispatchEvent(new Event("resize"));
      await flushPromises();
      expect(menu.element.style.left).toBe("176px");
      expect(menu.element.style.top).toBe("8px");
    } finally {
      wrapper.unmount();
      bounds.mockRestore(); width.mockRestore(); height.mockRestore();
    }
  });

  beforeEach(() => {
    vi.clearAllMocks();
    dialog.open.mockReset();
    vi.mocked(ensureHostVault).mockResolvedValue(true);
    localStorage.clear();
    webview.handler = null;
    pathApi.homeDir.mockRejectedValue(new Error("desktop path API unavailable in this test"));
    pathApi.sep.mockReturnValue("/");
    i18n.global.locale.value = "en";
    discardSftpPluginNavigations("test.navigation");
  });

  it("opens an independent saved-Host SFTP session and lists protocol entries", async () => {
    const { wrapper } = await mountView();
    const connect = wrapper.findAll("button").find((button) => button.text().includes("Connect SFTP"));
    await connect?.trigger("click");
    await flushPromises();

    expect(client.openSftpSession).toHaveBeenCalledWith({
      hostId: host.hostId,
      expectedHostStateVersion: "7",
    });
    expect(client.listSftpDirectory).toHaveBeenCalledWith(expect.objectContaining({
      expectedGeneration: "1",
      path: { bytes: [47] },
    }));
    expect(wrapper.text()).toContain("release.bin");
    wrapper.unmount();
  });

  it("keeps the connected server, address, response time, and disconnect action in one Pane header", async () => {
    const { wrapper } = await mountView(readySnapshot());
    const topbar = wrapper.get(".sftp-view__topbar");
    const header = wrapper.find('[data-pane-id="sftp-remote-pane"] .sftp-view__pane-header');

    expect(topbar.find(".sftp-view__title strong").exists()).toBe(false);
    expect(topbar.text()).toContain("New file tab");
    expect(topbar.text()).toContain("Transfers 0");
    expect(topbar.findAll(".terminal-pane-controls__separator")).toHaveLength(1);
    expect(header.find(".sftp-view__pane-identity strong").text()).toBe("Files");
    expect(header.find(".sftp-view__pane-address").text()).toBe("files.example.test:22");
    expect(header.find(".sftp-view__pane-status").text()).toMatch(/^\d+ms$/);
    expect(header.find(".nvx-select").exists()).toBe(false);
    expect(header.findAll(".sftp-view__pane-endpoint")).toHaveLength(1);
    expect(header.find('button[aria-label="Disconnect"]').text()).toBe("");
    wrapper.unmount();
  });

  it("uses a single top row for a lone remote Pane with labeled new tab and icon-only disconnect actions", async () => {
    const { wrapper } = await mountView(readySnapshot());
    const localPane = wrapper.find('[data-pane-id="sftp-local-pane"]');
    await openPaneActions(localPane);
    await paneAction(localPane, "Close current file Pane")?.trigger("click");
    await flushPromises();

    const topbar = wrapper.find(".sftp-view__topbar");
    expect(wrapper.findAll(".sftp-view__pane-header")).toHaveLength(0);
    expect(topbar.get(".sftp-view__title strong").text()).toBe("Files");
    expect(topbar.get(".sftp-view__topbar-address").text()).toBe("files.example.test:22");
    expect(topbar.get(".sftp-view__topbar-status").text()).toMatch(/^\d+ms$/);
    expect(topbar.text()).toContain("New file tab");
    expect(topbar.get('button[aria-label="Disconnect"]').text()).toBe("");
    expect(topbar.find('button[aria-label="More file actions"]').exists()).toBe(true);
    await topbar.get('button[aria-label="Disconnect"]').trigger("click");
    await flushPromises();
    expect(client.disconnectSftpSession).toHaveBeenCalledWith({
      sessionId: readySnapshot().sessions[0]!.sessionId,
      expectedGeneration: "1",
    });
    wrapper.unmount();
  });

  it("reattaches an exact initial File session without reopening that Host", async () => {
    const settled = vi.fn();
    window.addEventListener("norishell:initial-resource-settled", settled);
    const existing = readySnapshot();
    const sessionId = existing.sessions[0]!.sessionId;
    const { wrapper, workspaceTabs } = await mountView(existing, undefined, "release.bin", {
      workspaceTab: "remote", initialHostId: host.hostId, initialSessionId: sessionId,
    });
    expect(client.openSftpSession).not.toHaveBeenCalled();
    expect(workspaceTabs.fileSessionOwner(sessionId)).toBeTruthy();
    expect(client.listSftpDirectory).toHaveBeenCalledWith(expect.objectContaining({ sessionId }));
    expect((settled.mock.calls[0]?.[0] as CustomEvent).detail).toEqual({
      kind: "file", tabId: workspaceTabs.activeFileTabId, resourceId: sessionId, safeToProject: true,
    });
    wrapper.unmount();
    window.removeEventListener("norishell:initial-resource-settled", settled);
  });

  it("first opens a missing exact File session with the seed session ID", async () => {
    const sessionId = "019d0000-0000-7000-8000-000000000393";
    const { wrapper } = await mountView(undefined, undefined, "release.bin", {
      workspaceTab: "remote", initialHostId: host.hostId, initialSessionId: sessionId,
    });
    expect(client.openSftpSession).toHaveBeenCalledWith({
      hostId: host.hostId, expectedHostStateVersion: host.stateVersion, sessionId,
    });
    wrapper.unmount();
  });

  it("keeps an exact closed initial File session closed until an explicit connect", async () => {
    const existing = readySnapshot();
    existing.sessions[0]!.state = "closed";
    const sessionId = existing.sessions[0]!.sessionId;
    const { wrapper } = await mountView(existing, undefined, "release.bin", {
      workspaceTab: "remote", initialHostId: host.hostId, initialSessionId: sessionId,
    });
    expect(client.openSftpSession).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("reattaches a plugin File session by exact ID and generation without a saved Host", async () => {
    const settled = vi.fn();
    window.addEventListener("norishell:initial-resource-settled", settled);
    const existing = readySnapshot();
    existing.sessions[0]!.hostId = null;
    const sessionId = existing.sessions[0]!.sessionId;
    const { wrapper, workspaceTabs } = await mountView(existing, undefined, "release.bin", {
      workspaceTab: "remote", initialSessionId: sessionId, initialGeneration: "1",
    });
    expect(client.openSftpSession).not.toHaveBeenCalled();
    expect(workspaceTabs.fileSessionOwner(sessionId)).toBe(workspaceTabs.activeFileTabId);
    expect((settled.mock.calls[0]?.[0] as CustomEvent).detail.safeToProject).toBe(true);
    wrapper.unmount();
    window.removeEventListener("norishell:initial-resource-settled", settled);
  });

  it("retains the plugin File seed when the exact generation is absent", async () => {
    const settled = vi.fn();
    window.addEventListener("norishell:initial-resource-settled", settled);
    const existing = readySnapshot();
    existing.sessions[0]!.hostId = null;
    existing.sessions[0]!.generation = "2";
    const { wrapper, tips } = await mountView(existing, undefined, "release.bin", {
      workspaceTab: "remote", initialSessionId: existing.sessions[0]!.sessionId, initialGeneration: "1",
    });
    expect(client.openSftpSession).not.toHaveBeenCalled();
    expect(client.fetchSftpSessionSnapshot).toHaveBeenCalledTimes(2);
    expect((settled.mock.calls[0]?.[0] as CustomEvent).detail.safeToProject).toBe(false);
    expect(tips.items.some((item) => item.tone === "error")).toBe(true);
    wrapper.unmount();
    window.removeEventListener("norishell:initial-resource-settled", settled);
  });

  it("releases the File projection gate when Core confirms the failed initial ID is absent", async () => {
    const settled = vi.fn();
    window.addEventListener("norishell:initial-resource-settled", settled);
    const sessionId = "019d0000-0000-7000-8000-000000000394";
    const { wrapper, tips } = await mountView(undefined, undefined, "release.bin", {
      workspaceTab: "remote", initialSessionId: sessionId, initialGeneration: "1",
    });
    expect(client.openSftpSession).not.toHaveBeenCalled();
    expect(client.fetchSftpSessionSnapshot).toHaveBeenCalledTimes(2);
    expect((settled.mock.calls[0]?.[0] as CustomEvent).detail.safeToProject).toBe(true);
    expect(tips.items.some((item) => item.tone === "error")).toBe(true);
    wrapper.unmount();
    window.removeEventListener("norishell:initial-resource-settled", settled);
  });

  it("waits for an explicit Vault continuation before connecting and ignores cancellation", async () => {
    let finish!: (allowed: boolean) => void;
    vi.mocked(ensureHostVault).mockReturnValue(new Promise<boolean>((resolve) => { finish = resolve; }));
    const { wrapper } = await mountView();
    expect(ensureHostVault).not.toHaveBeenCalled();
    const connect = wrapper.findAll("button").find((button) => button.text().includes("Connect SFTP"))!;
    await connect.trigger("click");
    await flushPromises();
    expect(ensureHostVault).toHaveBeenCalledWith(host.hostId, "7", false);
    expect(client.openSftpSession).not.toHaveBeenCalled();
    finish(false);
    await flushPromises();
    expect(client.openSftpSession).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("does not resume a Vault continuation after leaving the SFTP view", async () => {
    let finish!: (allowed: boolean) => void;
    vi.mocked(ensureHostVault).mockReturnValue(new Promise<boolean>((resolve) => { finish = resolve; }));
    const { wrapper } = await mountView();
    await wrapper.findAll("button").find((button) => button.text().includes("Connect SFTP"))!.trigger("click");
    await flushPromises();
    wrapper.unmount();
    finish(true);
    await flushPromises();
    expect(client.openSftpSession).not.toHaveBeenCalled();
  });

  it("shows session failure Tips once per failure and keeps the file workspace free of error banners", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    const initial = readySnapshot();
    const { wrapper, tips, setSnapshot } = await mountView(initial);
    try {
      const failed: SftpSessionSnapshot = {
        ...initial, snapshotRevision: "5",
        sessions: initial.sessions.map((session) => ({
          ...session, state: "failed", stateRevision: "5",
          failure: { code: "transportLost", stage: "sftp-session", messageKey: "errors.sftp.sessionFailed" },
        })),
      };
      setSnapshot(failed);
      await vi.advanceTimersByTimeAsync(1000);
      await flushPromises();
      expect(tips.items).toEqual([expect.objectContaining({
        tone: "error", title: "The independent SSH connection for SFTP was lost.", message: "Files",
      })]);
      expect(wrapper.find(".sftp-view > .nvx-inline-notice").exists()).toBe(false);
      expect(wrapper.text()).not.toContain("sftp.sessionFailed");
      expect(wrapper.text()).toContain("Connection failed");
      expect(wrapper.text()).toContain("Connect SFTP");
      const id = tips.items[0]!.id;
      setSnapshot({ ...failed, snapshotRevision: "6", sessions: failed.sessions.map((session) => ({ ...session, stateRevision: "6" })) });
      await vi.advanceTimersByTimeAsync(1000);
      expect(tips.items[0]!.id).toBe(id);
      tips.clearAll();
      await vi.advanceTimersByTimeAsync(1000);
      expect(tips.items).toHaveLength(0);
      setSnapshot({ ...initial, snapshotRevision: "7" });
      await vi.advanceTimersByTimeAsync(1000);
      setSnapshot({ ...failed, snapshotRevision: "8", sessions: failed.sessions.map((session) => ({ ...session, generation: "2" })) });
      await vi.advanceTimersByTimeAsync(1000);
      expect(tips.items).toHaveLength(1);
      expect(tips.items[0]!.id).not.toBe(id);
    } finally {
      wrapper.unmount();
      tips.clearAll();
      vi.useRealTimers();
    }
  });

  it("recovers a Vault-blocked session directly without routing to Terminal", async () => {
    const snapshot = readySnapshot();
    snapshot.sessions[0]!.state = "failed";
    snapshot.sessions[0]!.failure = { code: "vaultLocked", stage: "authentication", messageKey: "errors.sftp.vaultLocked" };
    const { wrapper } = await mountView(snapshot);
    const retry = wrapper.findAll("button").find((button) => button.text() === "Unlock and retry");
    expect(retry).toBeDefined();
    await retry!.trigger("click");
    await flushPromises();
    expect(ensureHostVault).toHaveBeenCalledWith(host.hostId, "7", true);
    expect(client.openSftpSession).toHaveBeenCalledWith({ hostId: host.hostId, expectedHostStateVersion: "7", sessionId: snapshot.sessions[0]!.sessionId });
    wrapper.unmount();
  });

  it("retries an unknown host key in the same SFTP Pane", async () => {
    const snapshot = readySnapshot();
    snapshot.sessions[0]!.state = "failed";
    snapshot.sessions[0]!.failure = { code: "hostKeyRejected", stage: "host-key", messageKey: "errors.sftp.hostKeyRejected" };
    const { wrapper, router } = await mountView(snapshot);
    const retry = wrapper.findAll("button").find((button) => button.text().includes("Connect SFTP"));
    expect(retry).toBeDefined();
    await retry!.trigger("click");
    await flushPromises();
    expect(client.openSftpSession).toHaveBeenCalledWith({ hostId: host.hostId, expectedHostStateVersion: "7", sessionId: snapshot.sessions[0]!.sessionId });
    expect(router.currentRoute.value.path).toBe("/sftp");
    wrapper.unmount();
  });

  it("adds a fresh remote Pane from a local Pane without reusing a failed session", async () => {
    const snapshot = readySnapshot();
    snapshot.sessions[0]!.state = "failed";
    snapshot.sessions[0]!.failure = { code: "hostKeyRejected", stage: "host-key", messageKey: "errors.sftp.hostKeyRejected" };
    const { wrapper } = await mountView(snapshot);
    await openPaneActions(wrapper.find('[data-pane-id="sftp-local-pane"]'));
    await paneAction(wrapper.find('[data-pane-id="sftp-local-pane"]'), "Add remote pane")?.trigger("click");
    await flushPromises();
    const remotePanes = wrapper.findAll(".sftp-view__pane")
      .filter((pane) => pane.element.closest('[data-pane-id]')?.getAttribute("data-pane-id") !== "sftp-local-pane");
    expect(remotePanes).toHaveLength(2);
    const freshPane = remotePanes.find((pane) => pane.element.closest('[data-pane-id]')?.getAttribute("data-pane-id") !== "sftp-remote-pane")!.element;
    const connect = Array.from(freshPane.querySelectorAll<HTMLButtonElement>("button"))
      .find((button) => button.textContent?.includes("Connect SFTP"));
    connect?.click();
    await flushPromises();
    expect(client.openSftpSession).toHaveBeenCalledWith({ hostId: host.hostId, expectedHostStateVersion: "7" });
    wrapper.unmount();
  });

  it("connects from the empty remote Pane and displays files in that same Pane", async () => {
    const { wrapper, router } = await mountView();
    const remotePane = wrapper.get('[data-pane-id="sftp-remote-pane"]');
    expect(remotePane.get(".sftp-view__empty-remote").text()).toContain("Choose a server");
    expect(remotePane.text()).not.toContain("This Pane is not connected");
    expect(remotePane.text()).not.toContain("Disconnect");

    await remotePane.get('[aria-label="SFTP Host for this Pane"]').trigger("click");
    const option = Array.from(document.body.querySelectorAll<HTMLElement>('[role="option"]'))
      .find((item) => item.textContent?.includes(host.label));
    expect(option).toBeDefined();
    option?.click();
    await flushPromises();
    await remotePane.findAll("button").find((button) => button.text().includes("Connect SFTP"))?.trigger("click");
    await flushPromises();

    expect(client.openSftpSession).toHaveBeenCalledWith({
      hostId: host.hostId,
      expectedHostStateVersion: host.stateVersion,
    });
    expect(remotePane.text()).toContain("release.bin");
    expect(router.currentRoute.value.path).toBe("/sftp");
    wrapper.unmount();
  });

  it("does not replay another File Tab's consumed tray focus into a new local Tab", async () => {
    const session = readySnapshot().sessions[0]!;
    const { wrapper, tips } = await mountView(readySnapshot(), undefined, "release.bin", {
      workspaceTab: "local",
      routeQuery: {
        focusOperation: "tray-focus-a",
        focusSessionId: session.sessionId,
        focusGeneration: session.generation,
      },
      setupStore: (store) => {
        const owner = newFileTab(store, "remote");
        expect(store.claimFileSession(session.sessionId, owner)).toBe(true);
        expect(store.consumeFileFocusOperation("tray-focus-a")).toBe(true);
      },
    });

    expect(wrapper.findAll(".nvx-terminal-split-tree__pane")).toHaveLength(1);
    expect(wrapper.find(".sftp-view__empty-remote").exists()).toBe(false);
    expect(tips.items).toHaveLength(0);
    wrapper.unmount();
  });

  it("keeps the right-side split controls within the split tree minimum size", async () => {
    const { wrapper } = await mountView();
    const tree = wrapper.get(".nvx-terminal-split-tree").element as HTMLElement;
    tree.getBoundingClientRect = () => ({
      x: 0, y: 0, top: 0, left: 0, right: 500, bottom: 300, width: 500, height: 300, toJSON: () => ({}),
    });
    const before = wrapper.findAll(".nvx-terminal-split-tree__pane").length;
    const splitRight = wrapper.get<HTMLButtonElement>('.sftp-view__topbar button[aria-label="Split Pane to the right"]');
    await splitRight.trigger("click");
    await flushPromises();
    expect(wrapper.findAll(".nvx-terminal-split-tree__pane")).toHaveLength(before);
    wrapper.unmount();
  });

  it("opens remote panes from the local file tab using the terminal-style controls", async () => {
    const { wrapper } = await mountView({ snapshotRevision: "0", sessions: [], transfers: [] }, undefined, "notes.txt", { workspaceTab: "local" });
    const controls = wrapper.get(".sftp-view__topbar");
    expect(wrapper.findAll(".nvx-terminal-split-tree__pane")).toHaveLength(1);
    expect(wrapper.find('[role="listbox"][aria-label="Local files"]').exists()).toBe(true);
    expect(wrapper.find('[aria-label="Sort local files"]').exists()).toBe(true);
    await controls.get('button[aria-label="Split Pane to the right"]').trigger("click");
    await flushPromises();
    expect(wrapper.findAll(".nvx-terminal-split-tree__pane")).toHaveLength(2);
    expect(wrapper.get(".sftp-view__empty-remote").text()).toContain("Choose a server");
    await controls.get('button[aria-label="Add full-height Pane on the right"]').trigger("click");
    await flushPromises();
    expect(wrapper.findAll(".nvx-terminal-split-tree__pane")).toHaveLength(3);
    wrapper.unmount();
  });

  it("uses terminal Pane shortcuts for the active file tab", async () => {
    const { wrapper, workspaceTabs } = await mountView({ snapshotRevision: "0", sessions: [], transfers: [] }, undefined, "notes.txt", { workspaceTab: "local" });
    expect(workspaceTabs.fileTabs[0]?.paneCount).toBe(1);
    workspaceTabs.runFileShortcut("terminal.split-right");
    await flushPromises();
    expect(wrapper.findAll(".nvx-terminal-split-tree__pane")).toHaveLength(2);
    expect(workspaceTabs.fileTabs[0]?.paneCount).toBe(2);
    expect(wrapper.get(".sftp-view__empty-remote").text()).toContain("Choose a server");
    workspaceTabs.runFileShortcut("terminal.close-pane");
    await flushPromises();
    expect(wrapper.findAll(".nvx-terminal-split-tree__pane")).toHaveLength(1);
    expect(workspaceTabs.fileTabs[0]?.paneCount).toBe(1);
    wrapper.unmount();
  });

  it("keeps a toolbar-created remote Pane disconnected until its own server is chosen", async () => {
    const { wrapper } = await mountView(readySnapshot());
    await wrapper.get('.sftp-view__topbar button[aria-label="Split Pane to the right"]').trigger("click");
    await flushPromises();
    expect(wrapper.findAll(".sftp-view__empty-remote")).toHaveLength(1);
    expect(wrapper.findAll(".nvx-terminal-split-tree__pane")).toHaveLength(3);
    expect(client.openSftpSession).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("restores remembered directories only after a local capability or explicit remote connection is ready", async () => {
    const remotePath = Array.from(new TextEncoder().encode("/var/data"));
    localStorage.setItem(SFTP_PREFERENCES_KEY, JSON.stringify({
      version: 1,
      browser: { showHidden: true, foldersFirst: true, sort: "name" },
      rememberLastDirectory: true,
      directories: {
        local: "/Users/test/Remembered",
        remote: [{ hostId: host.hostId, pathBytes: remotePath }],
      },
    }));
    client.registerSftpLocalDirectory.mockResolvedValue({
      directoryRef: "remembered-local", revision: "1", displayName: "Remembered",
    });
    client.listSftpLocalDirectory.mockResolvedValue({
      directoryRef: "remembered-local", revision: "1", entries: [], nextCursor: null,
    });
    const { wrapper } = await mountView();
    expect(client.registerSftpLocalDirectory).toHaveBeenCalledWith("/Users/test/Remembered");
    expect(client.openSftpSession).not.toHaveBeenCalled();

    const connect = wrapper.findAll("button").find((button) => button.text().includes("Connect SFTP"));
    await connect?.trigger("click");
    await flushPromises();
    expect(client.listSftpDirectory).toHaveBeenCalledWith(expect.objectContaining({ path: { bytes: remotePath } }));
    wrapper.unmount();
  });

  it("falls back to the normal remote root when a remembered directory cannot be listed", async () => {
    const missingPath = Array.from(new TextEncoder().encode("/missing"));
    localStorage.setItem(SFTP_PREFERENCES_KEY, JSON.stringify({
      version: 1,
      browser: { showHidden: true, foldersFirst: true, sort: "name" },
      rememberLastDirectory: true,
      directories: { local: null, remote: [{ hostId: host.hostId, pathBytes: missingPath }] },
    }));
    const { wrapper, tips } = await mountView();
    client.listSftpDirectory.mockImplementation((input) => {
      if (input.path.bytes.join(",") === missingPath.join(",")) return Promise.reject(new Error("missing"));
      return Promise.resolve({
        sessionId: input.sessionId, generation: input.expectedGeneration, directoryRef: "remote-directory-root",
        path: input.path, entries: [], nextCursor: null,
      });
    });
    const connect = wrapper.findAll("button").find((button) => button.text().includes("Connect SFTP"));
    await connect?.trigger("click");
    await flushPromises();

    expect(client.listSftpDirectory).toHaveBeenNthCalledWith(1, expect.objectContaining({ path: { bytes: missingPath } }));
    await vi.waitFor(() => expect(client.listSftpDirectory).toHaveBeenLastCalledWith(expect.objectContaining({ path: { bytes: [47] } })));
    expect(tips.items[0]?.title).toBe("The last directory could not be restored; the default directory is open");
    wrapper.unmount();
  });

  it("does not apply a failed remembered remote fallback after its Pane unmounts", async () => {
    const missingPath = Array.from(new TextEncoder().encode("/missing"));
    localStorage.setItem(SFTP_PREFERENCES_KEY, JSON.stringify({
      version: 1,
      browser: { showHidden: true, foldersFirst: true, sort: "name" },
      rememberLastDirectory: true,
      directories: { local: null, remote: [{ hostId: host.hostId, pathBytes: missingPath }] },
    }));
    const pendingListing = deferred<never>();
    const { wrapper } = await mountView();
    client.listSftpDirectory.mockImplementation(() => pendingListing.promise);
    const connect = wrapper.findAll("button").find((button) => button.text().includes("Connect SFTP"));
    await connect?.trigger("click");
    await vi.waitFor(() => expect(client.listSftpDirectory).toHaveBeenCalledWith(expect.objectContaining({ path: { bytes: missingPath } })));
    wrapper.unmount();
    pendingListing.reject(new Error("missing"));
    await flushPromises();

    expect(client.listSftpDirectory).toHaveBeenCalledTimes(1);
  });

  it("lets an existing Pane hide files and clears selections which are no longer visible", async () => {
    const entries = [".env", "visible.txt"].map((displayName, index) => ({
      entryRef: `entry-${index}`,
      path: { bytes: Array.from(new TextEncoder().encode(`/${displayName}`)) },
      displayName,
      kind: "file" as const,
      size: 1,
      modifiedAtUnixMs: 1,
      permissionBits: null,
    }));
    const { wrapper } = await mountView(readySnapshot(), undefined, entries);
    const remotePane = wrapper.find('[data-pane-id="sftp-remote-pane"]');
    await remotePane.findAll(".sftp-view__entries > button")[0]!.trigger("click");
    await openPaneActions(remotePane);
    const hiddenToggle = Array.from(document.querySelectorAll<HTMLButtonElement>('.sftp-pane-actions-menu__popover [role="menuitemcheckbox"]'))
      .filter((button) => button.closest('.sftp-pane-actions-menu__popover')?.getAttribute("data-pane-id") === "sftp-remote-pane")
      .map((button) => new DOMWrapper(button))
      .find((button) => button.text().trim() === "Show hidden files");
    expect(hiddenToggle?.attributes("aria-checked")).toBe("true");
    await hiddenToggle?.trigger("click");
    await flushPromises();

    expect(remotePane.text()).not.toContain(".env");
    expect(remotePane.findAll(".sftp-view__entries > button")).toHaveLength(1);
    await openPaneActions(remotePane);
    expect(paneAction(remotePane, "Delete")?.attributes("disabled")).toBeDefined();
    wrapper.unmount();
  });

  it("keeps SFTP connection work isolated to its Pane and releases a failed Pane", async () => {
    const { wrapper } = await mountView();
    const originalPane = wrapper.find('[data-pane-id="sftp-remote-pane"]');
    await openPaneActions(originalPane);
    await paneAction(originalPane, "Split file Pane to the right")?.trigger("click");
    await flushPromises();

    const firstOpen = deferred<SftpSessionSnapshot["sessions"][number]>();
    client.openSftpSession
      .mockImplementationOnce(() => firstOpen.promise)
      .mockImplementationOnce(() => Promise.resolve({
        ...readySnapshot().sessions[0]!,
        sessionId: "019d0000-0000-7000-8000-000000000398",
      }));

    const connectButtons = () => wrapper.findAll<HTMLButtonElement>("button")
      .filter((button) => button.text().includes("Connect SFTP"));
    expect(connectButtons()).toHaveLength(2);

    await connectButtons()[0]!.trigger("click");
    await flushPromises();
    expect(client.openSftpSession).toHaveBeenCalledTimes(1);
    expect(connectButtons()[0]!.attributes("disabled")).toBeDefined();
    expect(connectButtons()[1]!.attributes("disabled")).toBeUndefined();

    await connectButtons()[0]!.trigger("click");
    await connectButtons()[1]!.trigger("click");
    await flushPromises();
    expect(client.openSftpSession).toHaveBeenCalledTimes(2);

    firstOpen.resolve(readySnapshot().sessions[0]!);
    await flushPromises();
    wrapper.unmount();

    const failed = deferred<SftpSessionSnapshot["sessions"][number]>();
    client.openSftpSession.mockClear();
    const secondMount = await mountView();
    client.openSftpSession.mockImplementationOnce(() => failed.promise);
    const failedConnect = secondMount.wrapper.findAll<HTMLButtonElement>("button")
      .find((button) => button.text().includes("Connect SFTP"));
    await failedConnect?.trigger("click");
    await flushPromises();
    expect(failedConnect?.attributes("disabled")).toBeDefined();

    failed.reject(new Error("connection refused"));
    await flushPromises();
    const retryConnect = secondMount.wrapper.findAll<HTMLButtonElement>("button")
      .find((button) => button.text().includes("Connect SFTP"));
    expect(retryConnect?.attributes("disabled")).toBeUndefined();
    await retryConnect?.trigger("click");
    await flushPromises();
    expect(client.openSftpSession).toHaveBeenCalledTimes(2);
    secondMount.wrapper.unmount();
  });

  it("releases the local Pane when its folder picker rejects", async () => {
    dialog.open.mockRejectedValueOnce(new Error("picker unavailable"));
    const { wrapper } = await mountView();
    const localPane = wrapper.find('[data-pane-id="sftp-local-pane"]');
    await openPaneActions(localPane);
    await paneAction(localPane, "Change local folder")?.trigger("click");
    await flushPromises();

    const localPath = wrapper.find<HTMLInputElement>('[aria-label="Local path"]');
    expect(localPath.attributes("disabled")).toBeUndefined();
    await openPaneActions(localPane);
    expect(paneAction(localPane, "Change local folder")?.attributes("disabled")).toBeUndefined();
    wrapper.unmount();
  });

  it("projects the exact local directory capability and releases it when the Tab view is destroyed", async () => {
    pathApi.homeDir.mockResolvedValue("/Users/test");
    client.registerSftpLocalDirectory.mockResolvedValue({
      directoryRef: "moved-local", revision: "6", displayName: "Home", rememberablePath: "/Users/test",
    });
    client.listSftpLocalDirectory.mockResolvedValue({
      directoryRef: "moved-local", revision: "6", entries: [], nextCursor: null,
    });
    const { wrapper, workspaceTabs } = await mountView(undefined, undefined, "release.bin", { workspaceTab: "local" });
    const id = workspaceTabs.fileTabs[0]!.groupId;
    const controller = workspaceTabs.fileController(id)!;
    const payload = controller.snapshotHandoff();
    expect(payload.panes[0]?.endpoint).toEqual({
      kind: "local", directoryRef: "moved-local", revision: "6", displayPath: "~/",
    });

    const localPane = wrapper.find(`[data-pane-id="${payload.panes[0]!.paneId}"]`);
    expect(wrapper.find(".sftp-view__topbar--local").exists()).toBe(true);
    expect(localPane.find(".sftp-view__pane-header").exists()).toBe(false);
    expect(localPane.find(".sftp-view__commandbar").exists()).toBe(true);
    workspaceTabs.finishCloseFileTab(id);
    wrapper.unmount();
    await flushPromises();
    expect(client.releaseSftpLocalDirectory).toHaveBeenCalledWith({ directoryRef: "moved-local", expectedRevision: "6" });
    expect(client.disconnectSftpSession).not.toHaveBeenCalled();
  });

  it("imports the same SFTP generation and local capability without reconnecting", async () => {
    const session = readySnapshot().sessions[0]!;
    const pathBytes = Array.from(new TextEncoder().encode("/var/log"));
    client.listSftpLocalDirectory.mockResolvedValue({
      directoryRef: "transferred-local", revision: "11", entries: [], nextCursor: null,
    });
    const { wrapper, workspaceTabs } = await mountView(readySnapshot(), undefined, "release.bin", {
      workspaceTab: "remote",
      handoffSnapshot: (id) => ({
        version: 1,
        tab: { groupId: id, kind: "remote", hostId: host.hostId, label: "Files", paneCount: 2 },
        layout: {
          kind: "split", splitId: "transferred-split", direction: "horizontal", ratio: 0.5,
          first: { kind: "pane", paneId: "transferred-local-pane", terminalId: "local" },
          second: { kind: "pane", paneId: "transferred-remote-pane", terminalId: "remote" },
        },
        activePaneId: "transferred-remote-pane",
        panes: [
          {
            paneId: "transferred-local-pane",
            endpoint: { kind: "local", directoryRef: "transferred-local", revision: "11", displayPath: "Home" },
            directory: "Home", remoteDirectoryPathBytes: null, localTrail: [], localRememberedPath: null,
            search: "", sort: "name", showHidden: false, foldersFirst: true,
          },
          {
            paneId: "transferred-remote-pane",
            endpoint: { kind: "remote", hostId: host.hostId, sessionId: session.sessionId, generation: session.generation },
            directory: "/var/log", remoteDirectoryPathBytes: pathBytes, localTrail: [], localRememberedPath: null,
            search: "", sort: "name", showHidden: false, foldersFirst: true,
          },
        ],
      }),
    });

    expect(client.openSftpSession).not.toHaveBeenCalled();
    expect(client.registerSftpLocalDirectory).not.toHaveBeenCalled();
    expect(client.listSftpLocalDirectory).toHaveBeenCalledWith(expect.objectContaining({
      directoryRef: "transferred-local", expectedRevision: "11",
    }));
    expect(client.listSftpDirectory).toHaveBeenCalledWith(expect.objectContaining({
      sessionId: session.sessionId, expectedGeneration: session.generation, path: { bytes: pathBytes },
    }));
    expect(workspaceTabs.fileSessionOwner(session.sessionId)).toBe(workspaceTabs.fileTabs[0]?.groupId);
    expect(wrapper.find('[data-pane-id="transferred-remote-pane"]').exists()).toBe(true);
    wrapper.unmount();
  });

  it("releases a local capability which arrives after its Pane has unmounted", async () => {
    const home = deferred<string>();
    pathApi.homeDir.mockReturnValue(home.promise);
    client.registerSftpLocalDirectory.mockResolvedValue({
      directoryRef: "late-local", revision: "1", displayName: "Home", rememberablePath: "/Users/test",
    });
    const { wrapper } = await mountView();
    wrapper.unmount();
    home.resolve("/Users/test");
    await flushPromises();

    expect(client.registerSftpLocalDirectory).toHaveBeenCalledWith("/Users/test");
    expect(client.releaseSftpLocalDirectory).toHaveBeenCalledWith({ directoryRef: "late-local", expectedRevision: "1" });
    expect(client.listSftpLocalDirectory).not.toHaveBeenCalled();
  });

  it("attaches the exact shared SFTP child and opens an extensionless Nginx site editor without creating a connection", async () => {
    const snapshot = readySnapshot();
    const shared = { ...snapshot.sessions[0]!, hostId: null, parentSshSession: { sessionId: "019d0000-0000-7000-8000-000000000901", generation: "3" } };
    snapshot.sessions = [shared];
    client.fetchSshSessionSnapshot.mockResolvedValue({ sessions: [{ sessionId: shared.parentSshSession.sessionId, generation: "3", state: "running", target: { kind: "quickConnect", endpoint: { address: "quick.example.test", port: 22, username: "ops" } } }] });
    client.previewSftpFile.mockResolvedValue({ sessionId: shared.sessionId, generation: shared.generation, displayName: "default", content: { kind: "text", text: "worker_processes auto;", endOffset: "22", editable: true, truncated: false, lineEnding: "lf" } });
    const { wrapper, workspaceTabs } = await mountView(snapshot, undefined, [{ entryRef: "nginx-config", path: { bytes: [...new TextEncoder().encode("/etc/nginx/sites-available/default")] }, displayName: "default", kind: "file", size: 22, modifiedAtUnixMs: 1, permissionBits: null }], { workspaceTab: "remote" });
    expect(workspaceTabs.claimFileSession(shared.sessionId, workspaceTabs.fileTabs[0]!.groupId)).toBe(true);
    const event = { operationId: "019d0000-0000-7000-8000-000000000911", pluginId: "test.navigation", sftpSession: shared, request: { kind: "sftp", path: "/etc/nginx/sites-available/default", edit: true } };
    expect(acceptSftpPluginNavigation(event)).toBe(true);
    await flushPromises();
    expect(client.openSftpSession).not.toHaveBeenCalled();
    expect(client.listSftpDirectory).toHaveBeenCalledWith(expect.objectContaining({ sessionId: shared.sessionId, expectedGeneration: "1", path: { bytes: [...new TextEncoder().encode("/etc/nginx/sites-available")] } }));
    expect(openToolWindow).toHaveBeenCalledWith(expect.objectContaining({ kind: "sftpFile", request: expect.objectContaining({ sessionId: shared.sessionId, entryRef: "nginx-config" }), tail: false }));
    expect(wrapper.text()).toContain("ops@quick.example.test:22");
    expect(document.body.querySelector("[data-code-editor]")).toBeNull();
    expect(client.mutateSftpFile).not.toHaveBeenCalled();
    expect(acceptSftpPluginNavigation(event)).toBe(false);
    wrapper.unmount();
    discardSftpPluginNavigations("test.navigation");
  });

  it("rejects stale shared-child navigation without falling back to a saved Host connection", async () => {
    const snapshot = readySnapshot();
    const shared = { ...snapshot.sessions[0]!, hostId: null, parentSshSession: { sessionId: "019d0000-0000-7000-8000-000000000921", generation: "1" } };
    snapshot.sessions = [{ ...shared, generation: "2" }];
    const { tips, wrapper, workspaceTabs } = await mountView(snapshot, undefined, "release.bin", { workspaceTab: "remote" });
    expect(workspaceTabs.claimFileSession(shared.sessionId, workspaceTabs.fileTabs[0]!.groupId)).toBe(true);
    expect(acceptSftpPluginNavigation({ operationId: "019d0000-0000-7000-8000-000000000922", pluginId: "test.navigation", sftpSession: shared, request: { kind: "sftp", path: "/etc", edit: false } })).toBe(true);
    await flushPromises();
    expect(client.openSftpSession).not.toHaveBeenCalled();
    expect(client.listSftpDirectory).not.toHaveBeenCalled();
    expect(tips.items).toEqual(expect.arrayContaining([
      expect.objectContaining({
        scope: "sftp-plugin-navigation",
        tone: "error",
      }),
    ]));
    expect(tips.items[0]?.title).toContain("The requested file or shared SFTP connection is no longer available");
    expect(wrapper.text()).not.toContain("The requested file or shared SFTP connection is no longer available");
    wrapper.unmount();
  });

  it("previews an allowlisted remote text file from its right-click menu", async () => {
    client.previewSftpFile.mockResolvedValue({
      sessionId: readySnapshot().sessions[0]!.sessionId,
      generation: "1",
      displayName: "notes.txt",
      content: {
        kind: "text",
        text: "bounded preview content",
        endOffset: "23",
        editable: true,
        truncated: false,
        lineEnding: "lf",
      },
    });
    const { wrapper } = await mountView(
      readySnapshot(),
      { snapshotRevision: "0", transfers: [] },
      "notes.txt",
    );
    const entry = wrapper.find('[data-pane-id="sftp-remote-pane"] .sftp-view__entries > button');
    await entry.trigger("contextmenu", { clientX: 40, clientY: 40 });
    const preview = wrapper.findAll<HTMLButtonElement>('[role="menuitem"]')
      .find((button) => button.text().trim() === "Preview / edit");
    expect(preview?.element.disabled).toBe(false);
    await preview?.trigger("click");
    await flushPromises();

    expect(openToolWindow).toHaveBeenCalledWith(expect.objectContaining({ kind: "sftpFile", tail: false, request: expect.objectContaining({ sessionId: readySnapshot().sessions[0]!.sessionId, expectedGeneration: "1", directoryRef: "remote-directory-root", entryRef: "remote-entry-release" }) }));
    expect(document.body.querySelector("[data-code-editor]")).toBeNull();
    wrapper.unmount();
  });

  it("follows appended remote bytes without running a shell command", async () => {
    client.previewSftpFile.mockResolvedValue({
      sessionId: readySnapshot().sessions[0]!.sessionId,
      generation: "1",
      displayName: "service.log",
      content: {
        kind: "text",
        text: "first",
        endOffset: "5",
        editable: true,
        truncated: false,
        lineEnding: "lf",
      },
    });
    client.tailSftpFile.mockResolvedValue({
      sessionId: readySnapshot().sessions[0]!.sessionId,
      generation: "1",
      startOffset: "5",
      nextOffset: "12",
      totalSize: "12",
      reset: false,
      bytes: Array.from(new TextEncoder().encode("\nsecond")),
    });
    const { wrapper } = await mountView(
      readySnapshot(),
      { snapshotRevision: "0", transfers: [] },
      "service.log",
    );
    await wrapper.find('[data-pane-id="sftp-remote-pane"] .sftp-view__entries > button')
      .trigger("contextmenu", { clientX: 40, clientY: 40 });
    const contextItems = wrapper.findAll<HTMLButtonElement>('[role="menuitem"]');
    expect(contextItems.some((button) => button.text().trim() === "Preview / edit")).toBe(true);
    await contextItems
      .find((button) => button.text().trim() === "Follow live")?.trigger("click");
    await flushPromises();

    expect(openToolWindow).toHaveBeenCalledWith(expect.objectContaining({ kind: "sftpFile", tail: true, request: expect.objectContaining({ sessionId: readySnapshot().sessions[0]!.sessionId, expectedGeneration: "1", directoryRef: "remote-directory-root", entryRef: "remote-entry-release" }) }));
    expect(client.tailSftpFile).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("downloads the selected remote file from its right-click menu", async () => {
    dialog.save.mockResolvedValue("/tmp/release.bin");
    client.registerSftpLocalBoundary.mockResolvedValue({
      token: "download-target",
      kind: "downloadTarget",
      displayName: "release.bin",
      size: null,
    });
    client.enqueueSftpTransfer.mockResolvedValue({ transferId: "right-click-download" });
    const { wrapper } = await mountView(readySnapshot());
    const entry = wrapper.find('[data-pane-id="sftp-remote-pane"] .sftp-view__entries > button');
    await entry.trigger("contextmenu", { clientX: 40, clientY: 40 });
    const download = wrapper.findAll<HTMLButtonElement>('[role="menuitem"]')
      .find((button) => button.text().trim() === "Download");
    await download?.trigger("click");
    await flushPromises();

    expect(dialog.save).toHaveBeenCalledWith(expect.objectContaining({ defaultPath: "release.bin" }));
    expect(client.enqueueSftpTransfer).toHaveBeenCalledWith(expect.objectContaining({
      direction: "download",
      source: { kind: "remote", path: { bytes: Array.from(new TextEncoder().encode("/release.bin")) } },
      target: { kind: "localBoundaryToken", token: "download-target", displayName: "release.bin" },
    }));
    wrapper.unmount();
  });

  it("uploads Finder or Explorer file drops through the existing transfer queue", async () => {
    client.registerSftpLocalBoundary.mockResolvedValue({
      token: "native-drop-source",
      kind: "uploadSource",
      displayName: "from-finder.txt",
      size: 12,
    });
    client.enqueueSftpTransfer.mockResolvedValue({ transferId: "native-drop-transfer" });
    const { wrapper } = await mountView(readySnapshot());
    const remotePane = wrapper.find('[data-pane-id="sftp-remote-pane"] .sftp-view__pane');
    const elementFromPoint = vi.spyOn(document, "elementFromPoint").mockReturnValue(remotePane.element);
    try {
      webview.handler?.({
        payload: {
          type: "drop",
          paths: ["/Users/test/Desktop/from-finder.txt"],
          position: { x: 80, y: 80 },
        },
      });
      await flushPromises();
    } finally {
      elementFromPoint.mockRestore();
    }

    expect(client.registerSftpLocalBoundary).toHaveBeenCalledWith(
      "uploadSource",
      "/Users/test/Desktop/from-finder.txt",
    );
    expect(client.enqueueSftpTransfer).toHaveBeenCalledWith(expect.objectContaining({
      direction: "upload",
      source: { kind: "localBoundaryToken", token: "native-drop-source", displayName: "from-finder.txt" },
      target: { kind: "remote", path: { bytes: Array.from(new TextEncoder().encode("/from-finder.txt")) } },
      conflictPolicy: "failIfExists",
    }));
    wrapper.unmount();
    expect(webview.unlisten).toHaveBeenCalledOnce();
  });

  it("clears stale Pane state when the session snapshot advances to a new generation", async () => {
    const initial = readySnapshot();
    const { wrapper, setSnapshot } = await mountView(initial);
    expect(wrapper.text()).toContain("release.bin");

    setSnapshot({
      ...initial,
      snapshotRevision: "5",
      sessions: initial.sessions.map((session) => ({ ...session, generation: "2", stateRevision: "5" })),
    });

    await vi.waitFor(() => {
      expect(wrapper.text()).toContain("moved to a new connection generation");
    }, { timeout: 1_500 });
    expect(wrapper.text()).not.toContain("release.bin");
    expect(wrapper.get('[data-pane-id="sftp-remote-pane"] .sftp-view__pane-status').text()).toBe("—");

    const reopen = wrapper.findAll("button").find((button) => button.text().includes("Reopen root"));
    await reopen?.trigger("click");
    await flushPromises();
    expect(client.listSftpDirectory).toHaveBeenLastCalledWith(expect.objectContaining({
      expectedGeneration: "2",
      path: { bytes: [47] },
    }));
    expect(wrapper.text()).toContain("release.bin");
    wrapper.unmount();
  });

  it("opens a typed remote path without replacing the current directory on failure", async () => {
    const { wrapper } = await mountView(readySnapshot());
    const path = wrapper.find<HTMLInputElement>('[aria-label="Remote path"]');
    await path.setValue("/var/www/releases");
    await path.trigger("keydown", { key: "Enter" });
    await flushPromises();

    expect(client.listSftpDirectory).toHaveBeenLastCalledWith(expect.objectContaining({
      path: { bytes: Array.from(new TextEncoder().encode("/var/www/releases")) },
    }));
    wrapper.unmount();
  });

  it("navigates remote breadcrumbs, back, and forward only after a directory loads", async () => {
    const { wrapper } = await mountView(readySnapshot());
    const pane = wrapper.get('[data-pane-id="sftp-remote-pane"]');
    const path = pane.get<HTMLInputElement>('[aria-label="Remote path"]');
    await pane.get('button[aria-label="Edit path"]').trigger("click");
    expect(path.isVisible()).toBe(true);
    await path.setValue("/var");
    await path.trigger("keydown", { key: "Enter" });
    await flushPromises();
    expect(path.element.style.display).toBe("none");
    await pane.get('button[aria-label="Edit path"]').trigger("click");
    await path.setValue("/var/log");
    await path.trigger("keydown", { key: "Enter" });
    await flushPromises();

    expect(pane.get('button[aria-label="Back"]').attributes("disabled")).toBeUndefined();
    expect(pane.findAll(".sftp-view__breadcrumbs button").map((button) => button.text())).toEqual(["/", "var", "log"]);
    await pane.get('button[aria-label="Back"]').trigger("click");
    await flushPromises();
    expect(path.element.value).toBe("/var");

    client.listSftpDirectory.mockRejectedValueOnce(new Error("permission denied"));
    await pane.get('button[aria-label="Forward"]').trigger("click");
    await flushPromises();
    expect(path.element.value).toBe("/var");
    expect(pane.get('button[aria-label="Forward"]').attributes("disabled")).toBeUndefined();

    await pane.get('button[aria-label="Forward"]').trigger("click");
    await flushPromises();
    expect(path.element.value).toBe("/var/log");
    await pane.findAll(".sftp-view__breadcrumbs button")[0]!.trigger("click");
    await flushPromises();
    expect(path.element.value).toBe("/");
    wrapper.unmount();
  });

  it("reopens released local history capabilities without changing history on failure", async () => {
    pathApi.sep.mockReturnValue("\\");
    let registration = 0;
    client.registerSftpLocalDirectory.mockImplementation((path: string) => Promise.resolve({
      directoryRef: `registered-${++registration}`, revision: "1", displayName: path, rememberablePath: path,
    }));
    client.openSftpLocalDirectoryChild.mockResolvedValue({
      directoryRef: "child-1", revision: "1", displayName: "Program Files", rememberablePath: "C:\\Program Files",
    });
    client.listSftpLocalDirectory.mockImplementation((input) => Promise.resolve({
      directoryRef: input.directoryRef, revision: input.expectedRevision,
      entries: input.directoryRef === "registered-1" ? [{
        entryRef: "program-files", displayName: "Program Files", kind: "directory",
        size: null, modifiedAtUnixMs: null,
      }] : [],
      nextCursor: null,
    }));
    const { wrapper } = await mountView();
    const pane = wrapper.get('[data-pane-id="sftp-local-pane"]');
    expect(pane.findAll(".sftp-view__commandbar > button").slice(0, 3).map((button) => button.attributes("aria-label")))
      .toEqual(["Up", "Back", "Forward"]);
    await pane.get(".sftp-view__entries > button").trigger("dblclick");
    await flushPromises();
    await pane.get('button[aria-label="Up"]').trigger("click");
    await flushPromises();
    expect(client.releaseSftpLocalDirectory).toHaveBeenCalledWith({ directoryRef: "child-1", expectedRevision: "1" });
    expect(pane.get<HTMLInputElement>('[aria-label="Local path"]').element.value).toBe("C:\\");

    client.registerSftpLocalDirectory.mockRejectedValueOnce(new Error("directory removed"));
    await pane.get('button[aria-label="Back"]').trigger("click");
    await flushPromises();
    expect(pane.get<HTMLInputElement>('[aria-label="Local path"]').element.value).toBe("C:\\");
    expect(pane.get('button[aria-label="Back"]').attributes("disabled")).toBeUndefined();

    await pane.get('button[aria-label="Back"]').trigger("click");
    await flushPromises();
    expect(client.registerSftpLocalDirectory).toHaveBeenLastCalledWith("C:\\Program Files");
    expect(pane.get<HTMLInputElement>('[aria-label="Local path"]').element.value).toBe("C:\\Program Files");
    expect(client.releaseSftpLocalDirectory).toHaveBeenCalledWith({ directoryRef: "registered-1", expectedRevision: "1" });

    client.registerSftpLocalDirectory.mockRejectedValueOnce(new Error("parent unavailable"));
    await pane.get('button[aria-label="Up"]').trigger("click");
    await flushPromises();
    expect(pane.get<HTMLInputElement>('[aria-label="Local path"]').element.value).toBe("C:\\Program Files");
    await pane.get('button[aria-label="Up"]').trigger("click");
    await flushPromises();
    expect(client.registerSftpLocalDirectory).toHaveBeenLastCalledWith("C:\\");
    expect(pane.get<HTMLInputElement>('[aria-label="Local path"]').element.value).toBe("C:\\");
    expect(client.releaseSftpLocalDirectory).toHaveBeenCalledWith({ directoryRef: "registered-2", expectedRevision: "1" });
    wrapper.unmount();
  });

  it("expands a home-relative typed local path before registering the directory", async () => {
    pathApi.homeDir.mockResolvedValue("/Users/test/");
    client.registerSftpLocalDirectory.mockImplementation((path: string) => Promise.resolve({
      directoryRef: path === "/Users/test" ? "local-home" : "local-chatgpt",
      revision: "1",
      displayName: path,
    }));
    client.listSftpLocalDirectory.mockImplementation((input) => Promise.resolve({
      directoryRef: input.directoryRef,
      revision: input.expectedRevision,
      entries: [],
      nextCursor: null,
    }));

    const { wrapper } = await mountView();
    const path = wrapper.find<HTMLInputElement>('[aria-label="Local path"]');
    await path.setValue("~/Documents/ChatGPT");
    await path.trigger("keydown", { key: "Enter" });
    await flushPromises();

    expect(client.registerSftpLocalDirectory).toHaveBeenLastCalledWith("/Users/test/Documents/ChatGPT");
    expect(wrapper.text()).not.toContain("Could not open the entered directory");
    wrapper.unmount();
  });

  it("keeps the local Pane usable and shows a visible error when opening a directory times out", async () => {
    pathApi.homeDir.mockResolvedValue("/Users/test");
    client.registerSftpLocalDirectory
      .mockResolvedValueOnce({ directoryRef: "local-home", revision: "1", displayName: "~/" })
      .mockRejectedValueOnce(JSON.stringify({
        code: "sftp.local_directory_timeout",
        messageKey: "errors.sftp.operationFailed",
      }));
    client.listSftpLocalDirectory.mockResolvedValue({
      directoryRef: "local-home", revision: "1", entries: [], nextCursor: null,
    });

    const { wrapper, tips } = await mountView();
    const path = wrapper.get<HTMLInputElement>('[aria-label="Local path"]');
    await path.setValue("~/Documents");
    await path.trigger("keydown", { key: "Enter" });
    await flushPromises();

    expect(path.element.value).toBe("~/");
    expect(client.releaseSftpLocalDirectory).not.toHaveBeenCalledWith({
      directoryRef: "local-home", expectedRevision: "1",
    });
    expect(tips.items[0]?.title).toContain("Opening the directory timed out");
    expect(wrapper.get<HTMLButtonElement>('button[aria-label="Refresh"]').element.disabled).toBe(false);
    wrapper.unmount();
  });

  it("opens a Windows local directory from its listing and shows a failure without losing the parent", async () => {
    pathApi.sep.mockReturnValue("\\");
    client.registerSftpLocalDirectory.mockResolvedValue({
      directoryRef: "local-root", revision: "1", displayName: "C:\\", rememberablePath: "C:\\",
    });
    client.listSftpLocalDirectory.mockImplementation((input) => Promise.resolve({
      directoryRef: input.directoryRef,
      revision: input.expectedRevision,
      entries: input.directoryRef === "local-root" ? [{
        entryRef: "program-files", displayName: "Program Files", kind: "directory",
        size: null, modifiedAtUnixMs: null,
      }] : [],
      nextCursor: null,
    }));
    client.openSftpLocalDirectoryChild.mockRejectedValueOnce(JSON.stringify({
      code: "sftp.local_directory_timeout", messageKey: "errors.sftp.operationFailed",
    }))
      .mockResolvedValueOnce({
        directoryRef: "program-files-dir", revision: "1", displayName: "Program Files",
        rememberablePath: "C:\\Program Files",
      });

    const { wrapper, tips } = await mountView();
    const entry = wrapper.get('[data-pane-id="sftp-local-pane"] .sftp-view__entries > button');
    await entry.trigger("dblclick");
    await flushPromises();
    expect(tips.items[0]?.title).toContain("Opening the directory timed out");
    expect(wrapper.text()).toContain("Opening the directory timed out");
    expect(wrapper.text()).toContain("Program Files");
    expect(client.releaseSftpLocalDirectory).not.toHaveBeenCalledWith({
      directoryRef: "local-root", expectedRevision: "1",
    });

    await entry.trigger("dblclick");
    await flushPromises();
    expect(client.openSftpLocalDirectoryChild).toHaveBeenCalledWith(expect.objectContaining({
      parentDirectoryRef: "local-root", entryRef: "program-files",
    }));
    expect(wrapper.get<HTMLInputElement>('[aria-label="Local path"]').element.value)
      .toBe("C:\\Program Files");
    expect(wrapper.text()).not.toContain("Opening the directory timed out");
    wrapper.unmount();
  });

  it("keeps the previous local listing when a typed directory cannot be read", async () => {
    pathApi.sep.mockReturnValue("\\");
    client.registerSftpLocalDirectory.mockImplementation((path: string) => Promise.resolve({
      directoryRef: path === "C:\\" ? "local-root" : "typed-dir",
      revision: "1", displayName: path,
    }));
    client.listSftpLocalDirectory.mockImplementation((input) => input.directoryRef === "typed-dir"
      ? Promise.reject(new Error("access denied"))
      : Promise.resolve({
        directoryRef: input.directoryRef, revision: input.expectedRevision,
        entries: [{ entryRef: "existing", displayName: "existing.txt", kind: "file", size: 1, modifiedAtUnixMs: null }],
        nextCursor: null,
      }));

    const { wrapper } = await mountView();
    const path = wrapper.get<HTMLInputElement>('[aria-label="Local path"]');
    await path.setValue("C:\\Program Files");
    await path.trigger("keydown", { key: "Enter" });
    await flushPromises();
    expect(wrapper.text()).toContain("existing.txt");
    expect(path.element.value).toBe("C:\\");
    expect(wrapper.text()).toContain("The entered directory could not be opened");
    expect(client.releaseSftpLocalDirectory).toHaveBeenCalledWith({
      directoryRef: "typed-dir", expectedRevision: "1",
    });
    expect(client.releaseSftpLocalDirectory).not.toHaveBeenCalledWith({
      directoryRef: "local-root", expectedRevision: "1",
    });
    wrapper.unmount();
  });

  it("turns picker paths into one-shot boundaries and defaults to no overwrite", async () => {
    dialog.open.mockResolvedValue(["/tmp/a.bin", "/tmp/b.bin"]);
    client.registerSftpLocalBoundary
      .mockResolvedValueOnce({ token: "token-a", kind: "uploadSource", displayName: "a.bin", size: 3 })
      .mockResolvedValueOnce({ token: "token-b", kind: "uploadSource", displayName: "b.bin", size: 5 });
    client.enqueueSftpTransfer.mockResolvedValue({});
    const { wrapper } = await mountView(readySnapshot());

    const remotePane = wrapper.find('[data-pane-id="sftp-remote-pane"]');
    await openPaneActions(remotePane);
    await paneAction(remotePane, "Choose files to upload")?.trigger("click");
    await flushPromises();

    expect(client.registerSftpLocalBoundary).toHaveBeenNthCalledWith(1, "uploadSource", "/tmp/a.bin");
    expect(client.enqueueSftpTransfer).toHaveBeenNthCalledWith(1, expect.objectContaining({
      direction: "upload",
      expectedBytes: 3,
      conflictPolicy: "failIfExists",
      target: { kind: "remote", path: { bytes: Array.from(new TextEncoder().encode("/a.bin")) } },
    }));
    expect(client.enqueueSftpTransfer).toHaveBeenCalledTimes(2);
    expect(client.enqueueSftpTransfer).toHaveBeenNthCalledWith(2, expect.objectContaining({
      direction: "upload",
      expectedBytes: 5,
      target: { kind: "remote", path: { bytes: Array.from(new TextEncoder().encode("/b.bin")) } },
    }));
    wrapper.unmount();
  });

  it("asks once before replacing a visible same-name target", async () => {
    dialog.open.mockResolvedValue("/tmp/release.bin");
    client.registerSftpLocalBoundary.mockResolvedValue({
      token: "replace-source",
      kind: "uploadSource",
      displayName: "release.bin",
      size: 8,
    });
    client.enqueueSftpTransfer.mockResolvedValue({});
    const { wrapper } = await mountView(readySnapshot());
    const remotePane = wrapper.find('[data-pane-id="sftp-remote-pane"]');
    await openPaneActions(remotePane);
    await paneAction(remotePane, "Choose files to upload")?.trigger("click");
    await flushPromises();

    expect(client.enqueueSftpTransfer).not.toHaveBeenCalled();
    const confirm = Array.from(document.body.querySelectorAll<HTMLButtonElement>("button"))
      .find((button) => button.textContent?.trim() === "Replace once");
    expect(confirm).toBeDefined();
    confirm?.click();
    await flushPromises();

    expect(client.enqueueSftpTransfer).toHaveBeenCalledWith(expect.objectContaining({
      conflictPolicy: "replaceSafely",
      target: { kind: "remote", path: { bytes: Array.from(new TextEncoder().encode("/release.bin")) } },
    }));
    wrapper.unmount();
  });

  it("changes remote file permissions through the selected session and exact file revision", async () => {
    client.mutateSftpFile.mockResolvedValue({
      sessionId: readySnapshot().sessions[0]!.sessionId,
      generation: "1",
    });
    const entry = {
      entryRef: "permission-entry",
      path: { bytes: Array.from(new TextEncoder().encode("/release.bin")) },
      displayName: "release.bin",
      kind: "file" as const,
      size: 8,
      modifiedAtUnixMs: 1_788_000_000_000,
      permissionBits: 0o100644,
    };
    const { wrapper } = await mountView(readySnapshot(), undefined, [entry]);
    await wrapper.find('[data-pane-id="sftp-remote-pane"] .sftp-view__entries > button')
      .trigger("contextmenu", { clientX: 40, clientY: 40 });
    await wrapper.findAll<HTMLButtonElement>('[role="menuitem"]')
      .find((button) => button.text().trim() === "Set permissions")?.trigger("click");
    await flushPromises();
    expect(document.body.textContent).toContain("Mode: 644");

    const checks = document.body.querySelectorAll<HTMLInputElement>('.sftp-view__permissions-grid input[type="checkbox"]');
    expect(checks).toHaveLength(9);
    checks[4]!.checked = true;
    checks[4]!.dispatchEvent(new Event("change", { bubbles: true }));
    checks[8]!.checked = true;
    checks[8]!.dispatchEvent(new Event("change", { bubbles: true }));
    await flushPromises();
    expect(document.body.textContent).toContain("Mode: 665");
    Array.from(document.body.querySelectorAll<HTMLButtonElement>("button"))
      .find((button) => button.textContent?.trim() === "Apply permissions")?.click();
    await flushPromises();

    expect(client.mutateSftpFile).toHaveBeenCalledWith({
      sessionId: readySnapshot().sessions[0]!.sessionId,
      expectedGeneration: "1",
      mutation: {
        kind: "setPermissions",
        path: entry.path,
        precondition: expect.objectContaining({ kind: "file", size: 8 }),
        expectedPermissionBits: 0o100644,
        mode: 0o665,
      },
    });
    expect(client.listSftpDirectory).toHaveBeenCalledTimes(2);
    wrapper.unmount();
  });

  it("applies replace all only to the current picker selection", async () => {
    dialog.open.mockResolvedValueOnce(["/tmp/a.bin", "/tmp/b.bin"]).mockResolvedValueOnce("/tmp/a.bin");
    client.registerSftpLocalBoundary
      .mockResolvedValueOnce({ token: "token-a", kind: "uploadSource", displayName: "a.bin", size: 3 })
      .mockResolvedValueOnce({ token: "token-b", kind: "uploadSource", displayName: "b.bin", size: 5 })
      .mockResolvedValueOnce({ token: "token-a-again", kind: "uploadSource", displayName: "a.bin", size: 3 });
    client.enqueueSftpTransfer.mockResolvedValue({ transferId: "transfer-a" });
    const entries = ["a.bin", "b.bin"].map((name) => ({
      entryRef: `entry-${name}`,
      path: { bytes: Array.from(new TextEncoder().encode(`/${name}`)) },
      displayName: name,
      kind: "file" as const,
      size: 8,
      modifiedAtUnixMs: 1_788_000_000_000,
      permissionBits: null,
    }));
    const { wrapper } = await mountView(readySnapshot(), undefined, entries);
    const remotePane = wrapper.find('[data-pane-id="sftp-remote-pane"]');
    await openPaneActions(remotePane);
    await paneAction(remotePane, "Choose files to upload")?.trigger("click");
    await flushPromises();

    const replaceAll = Array.from(document.body.querySelectorAll<HTMLButtonElement>("button"))
      .find((button) => button.textContent?.trim() === "Replace all");
    expect(replaceAll).toBeDefined();
    replaceAll?.click();
    await flushPromises();
    expect(client.enqueueSftpTransfer).toHaveBeenCalledTimes(2);
    expect(client.enqueueSftpTransfer).toHaveBeenNthCalledWith(1, expect.objectContaining({ conflictPolicy: "replaceSafely" }));
    expect(client.enqueueSftpTransfer).toHaveBeenNthCalledWith(2, expect.objectContaining({ conflictPolicy: "replaceSafely" }));

    await vi.waitFor(() => expect(remotePane.find('.sftp-view__entries > button').attributes('disabled')).toBeUndefined());
    await openPaneActions(remotePane);
    await paneAction(remotePane, "Choose files to upload")?.trigger("click");
    await flushPromises();
    expect(client.enqueueSftpTransfer).toHaveBeenCalledTimes(2);
    expect(Array.from(document.body.querySelectorAll<HTMLButtonElement>("button"))
      .some((button) => button.textContent?.trim() === "Replace once")).toBe(true);
    wrapper.unmount();
  });

  it("does not auto-split the workspace for additional live SFTP sessions", async () => {
    const first = readySnapshot().sessions[0]!;
    const second = {
      ...first,
      sessionId: "019d0000-0000-7000-8000-000000000399",
      generation: "9",
    };
    const { wrapper } = await mountView({
      snapshotRevision: "9",
      sessions: [first, second],
      transfers: [],
    });

    const remotePanes = wrapper.findAll(".sftp-view__pane")
      .filter((pane) => pane.element.closest('[data-pane-id]')?.getAttribute("data-pane-id") !== "sftp-local-pane");
    expect(remotePanes).toHaveLength(1);
    expect(remotePanes[0]!.find(".sftp-view__pane-status").text()).toMatch(/^\d+ms$/);
    wrapper.unmount();
  });

  it("keeps the selected Host and SFTP session when switching routes", async () => {
    const secondHost = { ...host, hostId: "019d0000-0000-7000-8000-000000000399", label: "Second Files" };
    const secondSession = { ...readySnapshot().sessions[0]!, hostId: secondHost.hostId };
    const { wrapper, router, setSnapshot } = await mountView({
      snapshotRevision: "4",
      sessions: [secondSession],
      transfers: [],
    }, undefined, "release.bin", { keepAlive: true, hosts: [host, secondHost] });
    const select = wrapper.get('[aria-label="SFTP Host for this Pane"]');
    await select.trigger("click");
    const option = Array.from(document.body.querySelectorAll<HTMLElement>('[role="option"]'))
      .find((item) => item.textContent?.includes(secondHost.label));
    expect(option).toBeDefined();
    option?.click();
    await flushPromises();
    const connect = wrapper.findAll("button").find((button) => button.text().includes("Connect SFTP"));
    await connect?.trigger("click");
    await flushPromises();
    expect(wrapper.get('[data-pane-id="sftp-remote-pane"] .sftp-view__pane-identity strong').text()).toContain(secondHost.label);
    expect(wrapper.text()).toContain("release.bin");

    await router.push("/terminal");
    await flushPromises();
    expect(client.disconnectSftpSession).not.toHaveBeenCalled();
    setSnapshot({
      snapshotRevision: "5",
      sessions: [{ ...secondSession, state: "closed" }],
      transfers: [],
    });
    await router.push("/sftp");
    await flushPromises();
    expect(wrapper.get('[data-pane-id="sftp-remote-pane"] .sftp-view__pane-identity strong').text()).toContain(secondHost.label);
    client.openSftpSession.mockClear();
    await wrapper.findAll("button").find((button) => button.text().includes("Connect SFTP"))?.trigger("click");
    await flushPromises();
    expect(client.openSftpSession).toHaveBeenCalledWith(expect.objectContaining({
      hostId: secondHost.hostId,
    }));
    expect(wrapper.get('[data-pane-id="sftp-remote-pane"] .sftp-view__pane-identity strong').text()).toContain(secondHost.label);
    wrapper.unmount();
  });

  it("refreshes cached Host labels on activation without reopening SFTP", async () => {
    const { wrapper, router } = await mountView(readySnapshot(), undefined, "release.bin", { keepAlive: true });
    expect(wrapper.get('[data-pane-id="sftp-remote-pane"] .sftp-view__pane-identity strong').text()).toContain("Files");
    await router.push("/terminal");
    await flushPromises();
    client.listHosts.mockResolvedValue([{ ...host, label: "Synced files" }]);
    await router.push("/sftp");
    await flushPromises();
    expect(wrapper.get('[data-pane-id="sftp-remote-pane"] .sftp-view__pane-identity strong').text()).toContain("Synced files");
    expect(client.openSftpSession).not.toHaveBeenCalled();
    expect(client.disconnectSftpSession).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("reconnects the same SFTP resource and fences an explicit recovery safety check to the new generation", async () => {
    const sessionId = readySnapshot().sessions[0]!.sessionId;
    const paused = {
      transferId: "019d0000-0000-7000-8000-000000000305",
      sessionId,
      generation: "1",
      direction: "upload" as const,
      source: { kind: "localBoundaryToken" as const, token: "local", displayName: "release.bin" },
      target: { kind: "remote" as const, path: { bytes: [47, 114] } },
      expectedBytes: 10,
      transferredBytes: 4,
      bytesPerSecond: null,
      remainingSeconds: null,
      stateRevision: "7",
      state: "pausedByDisconnect" as const,
      commitOutcome: "notCommitted" as const,
      failureCode: "transportLost" as const,
      cleanupResidual: null,
    };
    const snapshot: SftpSessionSnapshot = {
      snapshotRevision: "4",
      sessions: [{
        ...readySnapshot().sessions[0]!,
        sessionId,
        state: "failed",
        generation: "1",
        failure: { code: "transportLost", stage: "sftp-session", messageKey: "errors.sftp.sessionFailed" },
      }],
      transfers: [paused],
    };
    client.resumeSftpTransfer.mockResolvedValue({ ...paused, generation: "2", state: "transferring" });
    const { wrapper } = await mountView(snapshot);
    await openTransferActivity(wrapper);

    const connect = wrapper.findAll("button").find((button) => button.text().includes("Connect SFTP"));
    await connect?.trigger("click");
    await flushPromises();
    expect(client.openSftpSession).toHaveBeenCalledWith({
      hostId: host.hostId,
      expectedHostStateVersion: "7",
      sessionId,
    });
    expect(wrapper.text()).toContain("Paused after disconnect");

    expect(wrapper.text()).toContain("same remote object identity");
    const recoveryCheck = wrapper.findAll("button").find((button) => button.text().includes("Check recovery safety"));
    await recoveryCheck?.trigger("click");
    await flushPromises();
    expect(client.resumeSftpTransfer).toHaveBeenCalledWith({
      transferId: paused.transferId,
      expectedGeneration: "2",
      expectedStateRevision: "7",
    });
    wrapper.unmount();
  });

  it("keeps paused and residual transfers visible when another Ready session exists for the Host", async () => {
    const oldSessionId = "019d0000-0000-7000-8000-000000000306";
    const ready = readySnapshot().sessions[0]!;
    const residual = {
      transferId: "019d0000-0000-7000-8000-000000000307",
      sessionId: oldSessionId,
      generation: "1",
      direction: "download" as const,
      source: { kind: "remote" as const, path: { bytes: [47, 114] } },
      target: { kind: "localBoundaryToken" as const, token: "local", displayName: "release.bin" },
      expectedBytes: 10,
      transferredBytes: 4,
      bytesPerSecond: null,
      remainingSeconds: null,
      stateRevision: "8",
      state: "failed" as const,
      commitOutcome: "notCommitted" as const,
      failureCode: "cleanupIncomplete" as const,
      cleanupResidual: {
        kind: "localTemporaryTarget" as const,
        boundaryToken: "local",
        displayName: ".release.bin.part",
      },
    };
    const snapshot: SftpSessionSnapshot = {
      snapshotRevision: "6",
      sessions: [
        { ...ready, sessionId: oldSessionId, state: "failed", failure: { code: "transportLost", stage: "sftp-session", messageKey: "errors.sftp.sessionFailed" } },
        ready,
      ],
      transfers: [residual],
    };
    const { wrapper } = await mountView(snapshot);
    await openTransferActivity(wrapper);

    expect(wrapper.text()).toContain(".release.bin.part");
    expect(wrapper.text()).not.toContain("This session has no transfers yet.");
    wrapper.unmount();
  });

  it("cancels an active transfer with generation and state-revision fences", async () => {
    const transfer = {
      transferId: "019d0000-0000-7000-8000-000000000303",
      sessionId: readySnapshot().sessions[0]!.sessionId,
      generation: "2",
      direction: "download" as const,
      source: { kind: "remote" as const, path: { bytes: [47, 97] } },
      target: { kind: "localBoundaryToken" as const, token: "local", displayName: "a" },
      expectedBytes: 10,
      transferredBytes: 4,
      bytesPerSecond: 2,
      remainingSeconds: 3,
      stateRevision: "6",
      state: "transferring" as const,
      commitOutcome: "notCommitted" as const,
      failureCode: null,
      cleanupResidual: null,
    };
    client.cancelSftpTransfer.mockResolvedValue({ ...transfer, state: "cancelling" });
    const { wrapper } = await mountView(readySnapshot([transfer]));
    await openTransferActivity(wrapper);
    const cancel = wrapper.findAll("button").find((button) => button.text().includes("Cancel and clean up"));
    await cancel?.trigger("click");
    await flushPromises();

    expect(client.cancelSftpTransfer).toHaveBeenCalledWith({
      transferId: transfer.transferId,
      expectedGeneration: "2",
      expectedStateRevision: "6",
    });
    wrapper.unmount();
  });

  it("shows aggregate progress, readable sizes, and transfer speed", async () => {
    const gib = 1024 * 1024 * 1024;
    const transfer = {
      transferId: "019d0000-0000-7000-8000-000000000398",
      sessionId: readySnapshot().sessions[0]!.sessionId,
      generation: "1",
      direction: "upload" as const,
      source: { kind: "localBoundaryToken" as const, token: "local", displayName: "large.bin" },
      target: { kind: "remote" as const, path: { bytes: [47, 108] } },
      expectedBytes: 2 * gib,
      transferredBytes: gib,
      bytesPerSecond: 1024 * 1024,
      remainingSeconds: 60,
      stateRevision: "6",
      state: "transferring" as const,
      commitOutcome: "notCommitted" as const,
      failureCode: null,
      cleanupResidual: null,
    };
    const { wrapper } = await mountView(readySnapshot([transfer]));
    expect(wrapper.get("[data-sftp-transfer-toggle]").text()).toContain("50%");
    await openTransferActivity(wrapper);
    expect(wrapper.get(".sftp-view__transfer").text()).toContain("1 GB / 2 GB");
    expect(wrapper.get(".sftp-view__transfer").text()).toContain("1 MB/s");
    wrapper.unmount();
  });

  it("renders and operates a local-to-remote intent only once when it also has a legacy actor", async () => {
    const transferId = "019d0000-0000-7000-8000-000000000388";
    const legacy = {
      transferId,
      sessionId: readySnapshot().sessions[0]!.sessionId,
      generation: "1",
      direction: "upload" as const,
      source: { kind: "localBoundaryToken" as const, token: "local", displayName: "release.bin" },
      target: { kind: "remote" as const, path: { bytes: [47, 114] } },
      expectedBytes: 10,
      transferredBytes: 4,
      bytesPerSecond: 2,
      remainingSeconds: 3,
      stateRevision: "6",
      state: "transferring" as const,
      commitOutcome: "notCommitted" as const,
      failureCode: null,
      cleanupResidual: null,
    };
    const intent = {
      transferId,
      direction: "upload" as const,
      sourceDisplayName: "release.bin",
      targetDisplayName: "/release.bin",
      sourcePaneId: "sftp-local-pane",
      targetPaneId: "sftp-remote-pane",
      sourceEndpointRevision: "2",
      targetEndpointRevision: "3",
      sourceFence: { kind: "localCapability" as const, directoryRef: "local", revision: "1" },
      targetFence: { kind: "remoteSession" as const, sessionId: legacy.sessionId, generation: "1" },
      expectedBytes: 10,
      transferredBytes: 4,
      bytesPerSecond: 2,
      remainingSeconds: 3,
      stateRevision: "6",
      state: "transferring" as const,
      commitOutcome: "notCommitted" as const,
      failureCode: null,
      cleanupResidual: null,
    };
    client.cancelSftpTransferIntent.mockResolvedValue({ ...intent, state: "cancelling" });
    const { wrapper } = await mountView(readySnapshot([legacy]), {
      snapshotRevision: "6",
      transfers: [intent],
    });

    expect(wrapper.findAll(".sftp-view__transfer")).toHaveLength(0);
    await openTransferActivity(wrapper);
    expect(wrapper.findAll(".sftp-view__transfer")).toHaveLength(1);
    expect(wrapper.text()).toContain("Transfers 1");
    const cancel = wrapper.findAll("button").find((button) => button.text().includes("Cancel and clean up"));
    await cancel?.trigger("click");
    await flushPromises();
    expect(client.cancelSftpTransferIntent).toHaveBeenCalledOnce();
    expect(client.cancelSftpTransfer).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("requires explicit disconnect before closing a Ready remote Pane", async () => {
    const { wrapper } = await mountView(readySnapshot());
    const remotePane = wrapper.find('[data-pane-id="sftp-remote-pane"]');
    await openPaneActions(remotePane);
    await paneAction(remotePane, "Close current file Pane")?.trigger("click");
    await flushPromises();

    const confirm = Array.from(document.body.querySelectorAll<HTMLButtonElement>("button"))
      .find((button) => button.textContent?.includes("Disconnect and close"));
    expect(confirm).toBeDefined();
    confirm?.click();
    await flushPromises();

    expect(client.disconnectSftpSession).toHaveBeenCalledWith({
      sessionId: readySnapshot().sessions[0]!.sessionId,
      expectedGeneration: "1",
    });
    expect(wrapper.find('[data-pane-id="sftp-remote-pane"]').exists()).toBe(false);
    wrapper.unmount();
  });

  it("keeps a File Tab and its session when Core cannot finish disconnecting", async () => {
    const session = readySnapshot().sessions[0]!;
    const { wrapper, workspaceTabs, tips } = await mountView(readySnapshot(), undefined, "release.bin", {
      workspaceTab: "remote", initialHostId: host.hostId, initialSessionId: session.sessionId, initialGeneration: "1",
    });
    client.disconnectSftpSession.mockResolvedValue({ ...session, state: "failed" });
    const id = workspaceTabs.fileTabs[0]!.groupId;
    const closing = workspaceTabs.fileController(id)!.requestClose();
    await flushPromises();
    const confirm = Array.from(document.body.querySelectorAll<HTMLButtonElement>("button"))
      .find((button) => button.textContent?.includes("Disconnect and close"));
    expect(confirm).toBeDefined();
    confirm?.click();
    await flushPromises();

    expect(workspaceTabs.fileTabs.some((tab) => tab.groupId === id)).toBe(true);
    expect(workspaceTabs.fileSessionOwner(session.sessionId)).toBe(id);
    expect(tips.items.some((item) => item.message === "workspace_tab.file_cleanup_incomplete")).toBe(true);
    const cancel = Array.from(document.body.querySelectorAll<HTMLButtonElement>("button"))
      .find((button) => button.textContent?.trim() === "Cancel");
    cancel?.click();
    expect(await closing).toBe(false);
    wrapper.unmount();
  });

  it("rejects a second File Tab close with a busy code while the confirmation is open", async () => {
    const session = readySnapshot().sessions[0]!;
    const { wrapper, workspaceTabs } = await mountView(readySnapshot(), undefined, "release.bin", {
      workspaceTab: "remote", initialHostId: host.hostId, initialSessionId: session.sessionId, initialGeneration: "1",
    });
    const id = workspaceTabs.fileTabs[0]!.groupId;
    const closing = workspaceTabs.fileController(id)!.requestClose();
    await flushPromises();

    await expect(workspaceTabs.fileController(id)!.requestClose(true)).rejects.toThrow("workspace_tab.busy");
    await expect(workspaceTabs.fileController(id)!.requestClose()).rejects.toThrow("workspace_tab.busy");
    Array.from(document.body.querySelectorAll<HTMLButtonElement>("button"))
      .find((button) => button.textContent?.trim() === "Cancel")?.click();
    expect(await closing).toBe(false);
    expect(client.disconnectSftpSession).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("closes a remote File Tab after a native batch confirmation", async () => {
    const session = readySnapshot().sessions[0]!;
    const { wrapper, workspaceTabs } = await mountView(readySnapshot(), undefined, "release.bin", {
      workspaceTab: "remote", initialHostId: host.hostId, initialSessionId: session.sessionId, initialGeneration: "1",
    });
    const id = workspaceTabs.fileTabs[0]!.groupId;

    expect(await workspaceTabs.fileController(id)!.requestClose(true)).toBe(true);
    await flushPromises();
    expect(client.disconnectSftpSession).toHaveBeenCalledOnce();
    expect(workspaceTabs.fileTabs.some((tab) => tab.groupId === id)).toBe(false);
    expect(document.querySelector('[role="dialog"]')).toBeNull();
    wrapper.unmount();
  });

  it("retains a remote File Tab when a confirmed native batch cannot disconnect it", async () => {
    const session = readySnapshot().sessions[0]!;
    const { wrapper, workspaceTabs } = await mountView(readySnapshot(), undefined, "release.bin", {
      workspaceTab: "remote", initialHostId: host.hostId, initialSessionId: session.sessionId, initialGeneration: "1",
    });
    client.disconnectSftpSession.mockResolvedValue({ ...session, state: "failed" });
    const id = workspaceTabs.fileTabs[0]!.groupId;

    await expect(workspaceTabs.fileController(id)!.requestClose(true)).rejects.toThrow("workspace_tab.file_cleanup_incomplete");
    expect(workspaceTabs.fileTabs.some((tab) => tab.groupId === id)).toBe(true);
    expect(workspaceTabs.fileSessionOwner(session.sessionId)).toBe(id);
    wrapper.unmount();
  });

  it("keeps a remote Pane and session ownership after an incomplete close", async () => {
    const session = readySnapshot().sessions[0]!;
    const { wrapper, workspaceTabs } = await mountView(readySnapshot(), undefined, "release.bin", {
      workspaceTab: "remote", initialHostId: host.hostId, initialSessionId: session.sessionId, initialGeneration: "1",
    });
    client.disconnectSftpSession.mockResolvedValue({ ...session, state: "failed" });
    const remotePane = wrapper.findAll('[data-pane-id]').find((pane) => pane.find('.sftp-view__pane-endpoint').exists())!;
    expect(remotePane).toBeDefined();
    await openPaneActions(remotePane);
    await paneAction(remotePane, "Close current file Pane")?.trigger("click");
    await flushPromises();
    const confirm = Array.from(document.body.querySelectorAll<HTMLButtonElement>("button"))
      .find((button) => button.textContent?.includes("Disconnect and close"));
    confirm?.click();
    await flushPromises();

    expect(wrapper.findAll('[data-pane-id]').some((pane) => pane.find('.sftp-view__pane-endpoint').exists())).toBe(true);
    expect(workspaceTabs.fileSessionOwner(session.sessionId)).toBe(workspaceTabs.fileTabs[0]!.groupId);
    wrapper.unmount();
  });

  it("keeps a remote Pane connected when manual disconnect is incomplete", async () => {
    const session = readySnapshot().sessions[0]!;
    const { wrapper, workspaceTabs } = await mountView(readySnapshot(), undefined, "release.bin", {
      workspaceTab: "remote", initialHostId: host.hostId, initialSessionId: session.sessionId, initialGeneration: "1",
    });
    client.disconnectSftpSession.mockResolvedValue({ ...session, state: "failed" });
    const remotePane = wrapper.findAll('[data-pane-id]').find((pane) => pane.find('.sftp-view__pane-endpoint').exists())!;
    expect(remotePane).toBeDefined();
    await remotePane.get<HTMLButtonElement>('button[aria-label="Disconnect"]').trigger("click");
    await flushPromises();

    expect(workspaceTabs.fileSessionOwner(session.sessionId)).toBe(workspaceTabs.fileTabs[0]!.groupId);
    expect(wrapper.findAll('[data-pane-id]').some((pane) => pane.find('button[aria-label="Disconnect"]').exists())).toBe(true);
    wrapper.unmount();
  });

  it("requires a typed irreversible confirmation for remote deletion", async () => {
    client.mutateSftpFile.mockResolvedValue({
      sessionId: readySnapshot().sessions[0]!.sessionId,
      generation: "1",
    });
    const { wrapper } = await mountView(readySnapshot());
    await flushPromises();
    const entry = wrapper.find(".sftp-view__entries > button");
    await entry.trigger("click");
    const remotePane = wrapper.find('[data-pane-id="sftp-remote-pane"]');
    await openPaneActions(remotePane);
    await paneAction(remotePane, "Delete")?.trigger("click");
    await flushPromises();
    const confirm = Array.from(document.body.querySelectorAll<HTMLButtonElement>("button"))
      .find((button) => button.textContent?.trim() === "Confirm");
    confirm?.click();
    await flushPromises();

    expect(client.mutateSftpFile).toHaveBeenCalledWith({
      sessionId: readySnapshot().sessions[0]!.sessionId,
      expectedGeneration: "1",
      mutation: expect.objectContaining({
        kind: "delete",
        precondition: {
          kind: "file",
          size: 8,
          modifiedAtUnixMs: 1_788_000_000_000,
        },
        irreversibleConfirmed: true,
      }),
    });
    wrapper.unmount();
  });

  it.each(["metaKey", "ctrlKey", "shiftKey"] as const)("preserves %s selection through the pointer event sequence", async (modifier) => {
    const entries = ["a.bin", "b.bin", "c.bin"].map((displayName, index) => ({
      entryRef: `selection-${index}`,
      path: { bytes: Array.from(new TextEncoder().encode(`/${displayName}`)) },
      displayName, kind: "file" as const, size: 8, modifiedAtUnixMs: 1_788_000_000_000, permissionBits: null,
    }));
    const { wrapper } = await mountView(readySnapshot(), { snapshotRevision: "0", transfers: [] }, entries);
    const list = wrapper.find('[data-pane-id="sftp-remote-pane"] .sftp-view__entries');
    const rows = list.findAll('button[role="option"]');
    expect(list.attributes("aria-multiselectable")).toBe("true");
    await rows[0]!.trigger("click");
    const gesture = { button: 0, pointerId: 1, [modifier]: true };
    await rows[2]!.trigger("pointerdown", gesture);
    await rows[2]!.trigger("pointermove", { ...gesture, clientX: 50, clientY: 50 });
    await rows[2]!.trigger("pointerup", gesture);
    await rows[2]!.trigger("click", { [modifier]: true });
    expect(rows.map((row) => row.attributes("aria-selected")))
      .toEqual(modifier === "shiftKey" ? ["true", "true", "true"] : ["true", "false", "true"]);
    expect(client.prepareSftpTransferIntent).not.toHaveBeenCalled();
    if (modifier !== "shiftKey") {
      await rows[2]!.trigger("pointerdown", gesture);
      await rows[2]!.trigger("pointerup", gesture);
      await rows[2]!.trigger("click", { [modifier]: true });
      expect(rows.map((row) => row.attributes("aria-selected"))).toEqual(["true", "false", "false"]);
    }
    await rows[1]!.trigger("click");
    expect(rows.map((row) => row.attributes("aria-selected"))).toEqual(["false", "true", "false"]);
    wrapper.unmount();
  });

  it("selects multiple remote entries and confirms one bounded batch deletion", async () => {
    client.mutateSftpFile.mockResolvedValue({
      sessionId: readySnapshot().sessions[0]!.sessionId,
      generation: "1",
    });
    const entries = ["first.bin", "second.bin"].map((displayName, index) => ({
      entryRef: `remote-entry-${index}`,
      path: { bytes: Array.from(new TextEncoder().encode(`/${displayName}`)) },
      displayName,
      kind: "file" as const,
      size: 8,
      modifiedAtUnixMs: 1_788_000_000_000 + index,
      permissionBits: null,
    }));
    const { wrapper } = await mountView(
      readySnapshot(),
      { snapshotRevision: "0", transfers: [] },
      entries,
    );
    const remoteEntries = wrapper.findAll(
      '[data-pane-id="sftp-remote-pane"] .sftp-view__entries > button',
    );
    await remoteEntries[0]!.trigger("click");
    await remoteEntries[1]!.trigger("pointerdown", { button: 0, pointerId: 1, metaKey: true });
    await remoteEntries[1]!.trigger("pointerup", { button: 0, pointerId: 1, metaKey: true });
    await remoteEntries[1]!.trigger("click", { metaKey: true });
    expect(remoteEntries[0]!.attributes("aria-selected")).toBe("true");
    expect(remoteEntries[1]!.attributes("aria-selected")).toBe("true");

    await remoteEntries[1]!.trigger("contextmenu", { clientX: 40, clientY: 40 });
    await wrapper.findAll<HTMLButtonElement>('[role="menuitem"]')
      .find((button) => button.text().trim() === "Delete")?.trigger("click");
    await flushPromises();
    expect(document.body.textContent).toContain("Delete 2 items");
    const confirm = Array.from(document.body.querySelectorAll<HTMLButtonElement>("button"))
      .find((button) => button.textContent?.trim() === "Confirm");
    confirm?.click();
    await flushPromises();

    expect(client.mutateSftpFile).toHaveBeenCalledTimes(2);
    expect(client.mutateSftpFile.mock.calls.map((call) => call[0].mutation.kind))
      .toEqual(["delete", "delete"]);
    wrapper.unmount();
  });

  it("compresses the exact multi-selection into a no-overwrite ZIP target", async () => {
    client.mutateSftpFile.mockResolvedValue({
      sessionId: readySnapshot().sessions[0]!.sessionId,
      generation: "1",
    });
    const entries = ["first.bin", "folder"].map((displayName, index) => ({
      entryRef: `archive-entry-${index}`,
      path: { bytes: Array.from(new TextEncoder().encode(`/${displayName}`)) },
      displayName,
      kind: (index === 0 ? "file" : "directory") as "file" | "directory",
      size: index === 0 ? 8 : null,
      modifiedAtUnixMs: 1_788_000_000_000 + index,
      permissionBits: null,
    }));
    const { wrapper } = await mountView(
      readySnapshot(),
      { snapshotRevision: "0", transfers: [] },
      entries,
    );
    const remoteEntries = wrapper.findAll(
      '[data-pane-id="sftp-remote-pane"] .sftp-view__entries > button',
    );
    await remoteEntries[0]!.trigger("click");
    await remoteEntries[1]!.trigger("pointerdown", { button: 0, pointerId: 1, metaKey: true });
    await remoteEntries[1]!.trigger("pointerup", { button: 0, pointerId: 1, metaKey: true });
    await remoteEntries[1]!.trigger("click", { metaKey: true });
    await remoteEntries[1]!.trigger("contextmenu", { clientX: 40, clientY: 40 });
    await wrapper.findAll<HTMLButtonElement>('[role="menuitem"]')
      .find((button) => button.text().includes("Compress as ZIP"))?.trigger("click");
    await flushPromises();
    const targetName = document.body.querySelector<HTMLInputElement>("#sftp-file-utility-name");
    expect(targetName?.value).toBe("archive.zip");
    const confirm = Array.from(document.body.querySelectorAll<HTMLButtonElement>("button"))
      .find((button) => button.textContent?.trim() === "Confirm");
    confirm?.click();
    await flushPromises();

    expect(client.mutateSftpFile).toHaveBeenCalledWith({
      sessionId: readySnapshot().sessions[0]!.sessionId,
      expectedGeneration: "1",
      mutation: {
        kind: "createZip",
        sources: [
          expect.objectContaining({ archiveName: "first.bin" }),
          expect.objectContaining({ archiveName: "folder" }),
        ],
        target: { bytes: Array.from(new TextEncoder().encode("/archive.zip")) },
      },
    });
    wrapper.unmount();
  });

  it("shows the specific safe-commit failure when the server cannot create a ZIP", async () => {
    client.mutateSftpFile.mockRejectedValueOnce(JSON.stringify({
      code: "sftp.unsafe_no_replace_unsupported",
      messageKey: "errors.sftp.operationFailed",
    }));
    const { wrapper, tips } = await mountView(readySnapshot());
    const remoteEntry = wrapper.find('[data-pane-id="sftp-remote-pane"] .sftp-view__entries > button');
    await remoteEntry.trigger("click");
    await remoteEntry.trigger("contextmenu", { clientX: 40, clientY: 40 });
    await wrapper.findAll<HTMLButtonElement>('[role="menuitem"]')
      .find((button) => button.text().includes("Compress as ZIP"))?.trigger("click");
    await flushPromises();
    const confirm = Array.from(document.body.querySelectorAll<HTMLButtonElement>("button"))
      .find((button) => button.textContent?.trim() === "Confirm");
    confirm?.click();
    await flushPromises();

    expect(tips.items[0]?.title).toBe("Compress as ZIP");
    expect(tips.items[0]?.message).toContain("safe no-replace commit");
    expect(document.body.querySelector("#sftp-file-utility-name")).not.toBeNull();
    wrapper.unmount();
  });

  it("downloads an explicit HTTPS URL from the blank-space context menu", async () => {
    client.mutateSftpFile.mockResolvedValue({
      sessionId: readySnapshot().sessions[0]!.sessionId,
      generation: "1",
    });
    const { wrapper } = await mountView(readySnapshot());
    const entries = wrapper.find('[data-pane-id="sftp-remote-pane"] .sftp-view__entries');
    await entries.trigger("contextmenu", { clientX: 40, clientY: 40 });
    await wrapper.findAll<HTMLButtonElement>('[role="menuitem"]')
      .find((button) => button.text().includes("Download from the web"))?.trigger("click");
    await flushPromises();
    const urlInput = document.body.querySelector<HTMLInputElement>("#sftp-file-utility-url");
    const targetName = document.body.querySelector<HTMLInputElement>("#sftp-file-utility-name");
    urlInput?.dispatchEvent(new Event("focus"));
    if (urlInput) {
      urlInput.value = "https://downloads.example/release.zip";
      urlInput.dispatchEvent(new Event("input", { bubbles: true }));
      urlInput.dispatchEvent(new Event("blur", { bubbles: true }));
    }
    await flushPromises();
    expect(targetName?.value).toBe("release.zip");
    const confirm = Array.from(document.body.querySelectorAll<HTMLButtonElement>("button"))
      .find((button) => button.textContent?.trim() === "Confirm");
    confirm?.click();
    await flushPromises();
    expect(client.mutateSftpFile).toHaveBeenCalledWith({
      sessionId: readySnapshot().sessions[0]!.sessionId,
      expectedGeneration: "1",
      mutation: {
        kind: "downloadUrl",
        url: "https://downloads.example/release.zip",
        target: { bytes: Array.from(new TextEncoder().encode("/release.zip")) },
      },
    });
    wrapper.unmount();
  });

  it("shows the exact safe remote residual and exposes explicit retry or retain-for-exit actions", async () => {
    const transfer = {
      transferId: "019d0000-0000-7000-8000-000000000304",
      sessionId: readySnapshot().sessions[0]!.sessionId,
      generation: "3",
      direction: "upload" as const,
      source: { kind: "localBoundaryToken" as const, token: "local", displayName: "release.bin" },
      target: { kind: "remote" as const, path: { bytes: [47, 114, 101, 108, 101, 97, 115, 101] } },
      expectedBytes: 10,
      transferredBytes: 4,
      bytesPerSecond: null,
      remainingSeconds: null,
      stateRevision: "9",
      state: "failed" as const,
      commitOutcome: "notCommitted" as const,
      failureCode: "cleanupIncomplete" as const,
      cleanupResidual: {
        kind: "remoteTemporaryTarget" as const,
        path: { bytes: [47, 46, 114, 101, 108, 101, 97, 115, 101, 46, 112, 97, 114, 116] },
        displayPath: "/.release.part",
      },
    };
    client.retrySftpRemoteCleanup.mockResolvedValue({ ...transfer, state: "cancelled", cleanupResidual: null });
    client.retainSftpRemoteCleanupForExit.mockResolvedValue(transfer);
    const { wrapper } = await mountView(readySnapshot([transfer]));
    await openTransferActivity(wrapper);
    expect(wrapper.text()).toContain("/.release.part");

    const retry = wrapper.findAll("button").find((button) => button.text().includes("Retry cleanup with new connection"));
    await retry?.trigger("click");
    await flushPromises();
    expect(client.retrySftpRemoteCleanup).toHaveBeenCalledWith({
      transferId: transfer.transferId,
      expectedGeneration: "3",
      expectedStateRevision: "9",
      expectedHostStateVersion: "7",
    });

    const retain = wrapper.findAll("button").find((button) => button.text().includes("Retain and allow this exit"));
    await retain?.trigger("click");
    await flushPromises();
    const confirm = Array.from(document.body.querySelectorAll<HTMLButtonElement>("button"))
      .find((button) => button.textContent?.includes("Retain and permit once"));
    confirm?.click();
    await flushPromises();
    expect(client.retainSftpRemoteCleanupForExit).toHaveBeenCalledWith({
      transferId: transfer.transferId,
      expectedGeneration: "3",
      expectedStateRevision: "9",
      retainRemoteTemporaryFileConfirmed: true,
    });
    expect(wrapper.text()).toContain("not marked clean");
    wrapper.unmount();

    const uncertain = { ...transfer, commitOutcome: "uncertain" as const };
    const { wrapper: uncertainWrapper } = await mountView(readySnapshot([uncertain]));
    await openTransferActivity(uncertainWrapper);
    expect(uncertainWrapper.text()).toContain("Commit state uncertain");
    expect(uncertainWrapper.findAll("button").some((button) => button.text().includes("Retry cleanup with new connection"))).toBe(false);
    expect(uncertainWrapper.findAll("button").some((button) => button.text().includes("Retain and allow this exit"))).toBe(true);
    uncertainWrapper.unmount();
  });

  it("recovers or explicitly retains a server-to-server intent residual with both endpoint fences", async () => {
    const session = readySnapshot().sessions[0]!;
    const sourceFence = { kind: "remoteSession" as const, sessionId: session.sessionId, generation: "1" };
    const targetFence = { kind: "remoteSession" as const, sessionId: session.sessionId, generation: "1" };
    const intent = {
      transferId: "019d0000-0000-7000-8000-000000000314",
      direction: "serverToServer" as const,
      sourceDisplayName: "source.bin",
      targetDisplayName: "source.bin",
      sourcePaneId: "source-pane",
      targetPaneId: "target-pane",
      sourceEndpointRevision: "2",
      targetEndpointRevision: "3",
      sourceFence,
      targetFence,
      expectedBytes: 10,
      transferredBytes: 10,
      bytesPerSecond: null,
      remainingSeconds: null,
      stateRevision: "12",
      state: "failed" as const,
      commitOutcome: "committed" as const,
      failureCode: "cleanupIncomplete" as const,
      cleanupResidual: {
        kind: "remoteTemporaryTarget" as const,
        sessionId: session.sessionId,
        generation: "1",
        path: { bytes: [47, 46, 115, 111, 117, 114, 99, 101, 46, 112, 97, 114, 116] },
        displayPath: "/.source.part",
      },
    };
    client.retrySftpTransferIntentCleanup.mockResolvedValue({
      ...intent, state: "completed", failureCode: null, cleanupResidual: null,
    });
    client.retainSftpTransferIntentCleanupForExit.mockResolvedValue(intent);
    const { wrapper } = await mountView(readySnapshot(), { snapshotRevision: "5", transfers: [intent] });
    await openTransferActivity(wrapper);
    expect(wrapper.text()).toContain("/.source.part");

    const retry = wrapper.findAll("button").find((button) => button.text().includes("Retry cleanup with new connection"));
    await retry?.trigger("click");
    await flushPromises();
    expect(client.retrySftpTransferIntentCleanup).toHaveBeenCalledWith({
      transferId: intent.transferId,
      expectedSourceFence: sourceFence,
      expectedTargetFence: targetFence,
      expectedStateRevision: "12",
      expectedTargetHostStateVersion: "7",
    });

    const retain = wrapper.findAll("button").find((button) => button.text().includes("Retain and allow this exit"));
    await retain?.trigger("click");
    await flushPromises();
    const confirm = Array.from(document.body.querySelectorAll<HTMLButtonElement>("button"))
      .find((button) => button.textContent?.includes("Retain and permit once"));
    confirm?.click();
    await flushPromises();
    expect(client.retainSftpTransferIntentCleanupForExit).toHaveBeenCalledWith({
      transferId: intent.transferId,
      expectedSourceFence: sourceFence,
      expectedTargetFence: targetFence,
      expectedStateRevision: "12",
      retainRemoteTemporaryFileConfirmed: true,
    });
    wrapper.unmount();
  });

  it("browses a Core-authorized local directory and enqueues an exact local-to-remote pane intent", async () => {
    dialog.open.mockResolvedValue("/Users/test/Downloads");
    client.registerSftpLocalDirectory.mockResolvedValue({
      directoryRef: "local-directory-ref",
      revision: "3",
      displayName: "Downloads",
    });
    client.listSftpLocalDirectory.mockResolvedValue({
      directoryRef: "local-directory-ref",
      revision: "3",
      entries: [{
        entryRef: "local-entry-ref",
        displayName: "payload.bin",
        kind: "file",
        size: 12,
        modifiedAtUnixMs: 1_788_000_000_000,
      }],
      nextCursor: null,
    });
    let resolvePrepared!: (value: {
      intentToken: string;
      expiresAtUnixMs: number;
      sourceFence: { kind: "localCapability"; directoryRef: string; revision: string };
      targetFence: { kind: "remoteSession"; sessionId: string; generation: string };
    }) => void;
    client.prepareSftpTransferIntent.mockReturnValue(new Promise((resolve) => {
      resolvePrepared = resolve;
    }));
    client.enqueueSftpTransferIntent.mockResolvedValue({});

    const { wrapper } = await mountView(readySnapshot());
    const localPlacement = wrapper.find('[data-pane-id="sftp-local-pane"]');
    await localPlacement.trigger("pointerdown");
    expect(wrapper.find(".sftp-view__topbar").find(".sftp-view__local-folder-control").exists()).toBe(false);
    expect(localPlacement.find('button[aria-label="More file actions"]').exists()).toBe(true);
    expect(wrapper.find('[data-pane-id="sftp-remote-pane"] .sftp-view__conflict-control').exists()).toBe(false);
    await openPaneActions(localPlacement);
    await paneAction(localPlacement, "Change local folder")?.trigger("click");
    await flushPromises();

    expect(client.registerSftpLocalDirectory).toHaveBeenCalledWith("/Users/test/Downloads");
    expect(client.listSftpLocalDirectory).toHaveBeenCalledWith(expect.objectContaining({
      directoryRef: "local-directory-ref",
      expectedRevision: "3",
    }));
    expect(wrapper.text()).toContain("payload.bin");

    const localEntry = localPlacement.find(".sftp-view__entries > button");
    const remotePane = wrapper.find('[data-pane-id="sftp-remote-pane"] .sftp-view__pane');
    await dragEntryToPane(localEntry, remotePane);
    await flushPromises();

    expect(remotePane.find<HTMLButtonElement>('button[aria-label="Refresh"]').element.disabled).toBe(true);
    await openPaneActions(remotePane);
    expect(paneAction(remotePane, "Close current file Pane")?.element.disabled).toBe(true);
    expect(localPlacement.find<HTMLButtonElement>('.sftp-view__entries > button').element.disabled).toBe(true);

    resolvePrepared({
      intentToken: "single-use-intent",
      expiresAtUnixMs: Date.now() + 1_000,
      sourceFence: { kind: "localCapability", directoryRef: "local-directory-ref", revision: "3" },
      targetFence: { kind: "remoteSession", sessionId: readySnapshot().sessions[0]!.sessionId, generation: "1" },
    });
    await flushPromises();

    expect(client.prepareSftpTransferIntent).toHaveBeenCalledWith(expect.objectContaining({
      sourcePaneId: "sftp-local-pane",
      targetPaneId: "sftp-remote-pane",
      source: { kind: "localDirectoryEntry", directoryRef: "local-directory-ref", entryRef: "local-entry-ref" },
      target: {
        kind: "remoteDirectory",
        sessionId: readySnapshot().sessions[0]!.sessionId,
        expectedGeneration: "1",
        directoryRef: "remote-directory-root",
      },
      expectedBytes: 12,
      conflictPolicy: "failIfExists",
    }));
    expect(client.enqueueSftpTransferIntent).toHaveBeenCalledWith({ intentToken: "single-use-intent" });
    wrapper.unmount();
  });

  it("recursively copies a dragged local directory into a remote Pane", async () => {
    dialog.open.mockResolvedValue("/tmp/root");
    client.registerSftpLocalDirectory.mockResolvedValue({
      directoryRef: "local-root",
      revision: "1",
      displayName: "~/",
    });
    client.openSftpLocalDirectoryChild.mockResolvedValue({
      directoryRef: "local-tree",
      revision: "1",
      displayName: "tree",
    });
    client.listSftpLocalDirectory.mockImplementation((input) => Promise.resolve({
      directoryRef: input.directoryRef,
      revision: input.expectedRevision,
      entries: input.directoryRef === "local-root" ? [{
        entryRef: "local-tree-entry",
        displayName: "tree",
        kind: "directory",
        size: null,
        modifiedAtUnixMs: null,
      }] : [{
        entryRef: "local-file-entry",
        displayName: "payload.bin",
        kind: "file",
        size: 12,
        modifiedAtUnixMs: null,
      }],
      nextCursor: null,
    }));
    client.mutateSftpFile.mockResolvedValue({});
    client.prepareSftpTransferIntent.mockResolvedValue({
      intentToken: "recursive-intent",
      expiresAtUnixMs: Date.now() + 1_000,
      sourceFence: { kind: "localCapability", directoryRef: "local-tree", revision: "1" },
      targetFence: { kind: "remoteSession", sessionId: readySnapshot().sessions[0]!.sessionId, generation: "1" },
    });
    client.enqueueSftpTransferIntent.mockResolvedValue({});
    client.listSftpDirectory.mockImplementation((input) => Promise.resolve({
      sessionId: input.sessionId,
      generation: input.expectedGeneration,
      directoryRef: input.path.bytes.length > 1 ? "remote-tree" : "remote-root",
      path: input.path,
      entries: [],
      nextCursor: null,
    }));

    const { wrapper } = await mountView(readySnapshot());
    const localPane = wrapper.find('[data-pane-id="sftp-local-pane"]');
    await openPaneActions(localPane);
    await paneAction(localPane, "Change local folder")?.trigger("click");
    await flushPromises();
    const localEntry = wrapper.find('[data-pane-id="sftp-local-pane"] .sftp-view__entries > button');
    const remotePane = wrapper.find('[data-pane-id="sftp-remote-pane"] .sftp-view__pane');
    await dragEntryToPane(localEntry, remotePane);
    await flushPromises();

    expect(client.mutateSftpFile).toHaveBeenCalledWith(expect.objectContaining({
      mutation: {
        kind: "createDirectory",
        path: { bytes: Array.from(new TextEncoder().encode("/tree")) },
      },
    }));
    expect(client.prepareSftpTransferIntent).toHaveBeenCalledWith(expect.objectContaining({
      source: { kind: "localDirectoryEntry", directoryRef: "local-tree", entryRef: "local-file-entry" },
      target: expect.objectContaining({ kind: "remoteDirectory", directoryRef: "remote-directory-root" }),
      expectedBytes: 12,
    }));
    expect(client.enqueueSftpTransferIntent).toHaveBeenCalledWith({ intentToken: "recursive-intent" });
    wrapper.unmount();
  });

  it("creates an empty file from the blank-space context menu without requiring a selection", async () => {
    client.mutateSftpFile.mockResolvedValue({
      sessionId: readySnapshot().sessions[0]!.sessionId,
      generation: "1",
    });
    const { wrapper } = await mountView(readySnapshot());
    const entries = wrapper.find('[data-pane-id="sftp-remote-pane"] .sftp-view__entries');
    await entries.trigger("contextmenu", { clientX: 40, clientY: 40 });
    await wrapper.findAll<HTMLButtonElement>('[role="menuitem"]')
      .find((button) => button.text().trim() === "New empty file")?.trigger("click");
    await flushPromises();

    const nameInput = document.body.querySelector<HTMLInputElement>("#sftp-mutation-name");
    expect(nameInput).not.toBeNull();
    if (nameInput) {
      nameInput.value = "empty.txt";
      nameInput.dispatchEvent(new Event("input", { bubbles: true }));
    }
    await flushPromises();
    const confirm = Array.from(document.body.querySelectorAll<HTMLButtonElement>("button"))
      .find((button) => button.textContent?.trim() === "Confirm");
    confirm?.click();
    await flushPromises();

    expect(client.mutateSftpFile).toHaveBeenCalledWith({
      sessionId: readySnapshot().sessions[0]!.sessionId,
      expectedGeneration: "1",
      mutation: {
        kind: "createEmptyFile",
        path: { bytes: Array.from(new TextEncoder().encode("/empty.txt")) },
      },
    });
    wrapper.unmount();
  });
});
