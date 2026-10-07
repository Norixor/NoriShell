import { computed, watch, type Component } from "vue";
import type { Router } from "vue-router";

import type { NativeTrayAction, PluginApprovedTerminalChannelLaunch } from "./core-api/generated/core-api";
import { createUuidV7 } from "./core-api/ids";
import { listPluginProtocolLaunches } from "./core-api/plugin-terminal";
import { fetchSftpSessionSnapshot, listHosts } from "./core-api/client";
import { desktopClient } from "./core-api/desktop-client";
import { i18n } from "./locales";
import { filterSingleDocumentDesktopSessions } from "./workspace-desktop-handoff";
import { snapshotWorkspaceTabs, updateOwnWorkspaceTabProjection } from "./workspace-tab-windows";
import { createFileRecovery } from "./workspace-file-handoff";
import { showWorkspaceTabFailure } from "./workspace-tab-errors";
import type { FileTabHandoffSnapshot } from "./views/fileTabHandoffSnapshot";
import {
  useWorkspaceTabsStore, workspacePageTabForRoute, type TerminalHeaderController, type WorkspacePageTab,
} from "./stores/workspaceTabs";
import { createSftpTerminalLaunch } from "./views/sftpTerminalLaunch";
import type { WorkspaceTabShellAction } from "./workspace-tab-shell-action";
import { useUiStore } from "./stores/ui";
import type { WorkspaceTabStateTone, WorkspaceTerminalKind } from "./workspace-tab-state-tone";

/**
 * Workspace Tabs hosted inside one document (Android, browser preview). The
 * per-kind stores already hold every Tab the views own; this module only orders
 * them into one strip and routes activation, close and create to the owning view
 * through the controllers the views register. Nothing here creates, reconnects or
 * disconnects a Session: Terminal and Desktop controllers keep their confirmation
 * semantics, and a Page Tab close only drops a route projection.
 */
export type SingleDocumentTabKind = "terminal" | "file" | "desktop" | "page";

export interface SingleDocumentTab {
  id: string;
  kind: SingleDocumentTabKind;
  route: string;
  label: string;
  /** Built-in Page Tabs carry a locale key; the strip resolves it in the current language. */
  labelKey: string | null;
  stateLabel: string;
  stateTone: WorkspaceTabStateTone;
  /** Resource type behind a Terminal Tab (SSH, local Shell, Telnet, plugin, or an unconnected launcher). */
  terminalKind: WorkspaceTerminalKind | null;
  paneCount: number;
  hostId: string | null;
  bellAttention: boolean;
  compact: boolean;
}

const KIND_ROUTES: Record<SingleDocumentTabKind, string> = {
  terminal: "/terminal", file: "/sftp", desktop: "/desktop", page: "",
};

export function useSingleDocumentTabs() {
  const store = useWorkspaceTabsStore();
  const tabs = computed<SingleDocumentTab[]>(() => [
    ...store.terminalTabs.map((tab) => ({
      id: tab.groupId, kind: "terminal" as const, route: KIND_ROUTES.terminal, label: tab.label,
      labelKey: null, stateLabel: tab.stateLabel, stateTone: tab.stateTone ?? "neutral" as const,
      terminalKind: tab.terminalKind ?? null, paneCount: tab.paneCount ?? 1,
      hostId: tab.hostId ?? null, bellAttention: Boolean(tab.bellAttention), compact: false,
    })),
    ...store.fileTabs.map((tab) => ({
      id: tab.groupId, kind: "file" as const, route: KIND_ROUTES.file, label: tab.label,
      labelKey: null, stateLabel: "", stateTone: "neutral" as const, terminalKind: null, paneCount: tab.paneCount,
      hostId: tab.hostId, bellAttention: false, compact: false,
    })),
    ...store.desktopTabs.map((tab) => ({
      id: tab.groupId, kind: "desktop" as const, route: KIND_ROUTES.desktop, label: tab.label,
      labelKey: null, stateLabel: tab.stateLabel, stateTone: tab.stateTone ?? "neutral" as const, terminalKind: null, paneCount: 1,
      hostId: null, bellAttention: false, compact: false,
    })),
    ...store.pageTabs.map((tab) => ({
      id: tab.groupId, kind: "page" as const, route: tab.route, label: tab.label, labelKey: tab.labelKey,
      stateLabel: "", stateTone: "neutral" as const, terminalKind: null, paneCount: 0,
      hostId: null, bellAttention: false, compact: true,
    })),
  ]);
  return { store, tabs };
}

/** The Tab the strip highlights for the current route: the active Tab of that route's kind, or a Page Tab of that route. */
export function activeSingleDocumentTabId(
  store: ReturnType<typeof useWorkspaceTabsStore>, routePath: string,
): string {
  if (routePath === "/terminal") return store.activeTerminalTabId;
  if (routePath === "/sftp") return store.activeFileTabId;
  if (routePath === "/desktop") return store.activeDesktopTabId;
  return store.pageTabs.find((tab) => tab.route === routePath)?.groupId ?? "";
}

export async function activateSingleDocumentTab(
  store: ReturnType<typeof useWorkspaceTabsStore>, router: Router, tab: SingleDocumentTab,
): Promise<void> {
  // Leaving the Terminal page revokes its input focus before another view takes over.
  if (tab.kind !== "terminal" && router.currentRoute.value.path === "/terminal") {
    await store.terminalController?.deactivate();
  }
  if (tab.kind === "page") {
    if (router.currentRoute.value.path !== tab.route) await router.push(tab.route);
    return;
  }
  if (router.currentRoute.value.path !== tab.route) await router.push(tab.route);
  if (tab.kind === "terminal") store.terminalController?.activate(tab.id);
  else if (tab.kind === "file") store.activateFileTab(tab.id);
  // A recovered Desktop Tab may be shown before the Desktop view has mounted.
  else (await awaitController(() => store.desktopController)).activate(tab.id);
}

/**
 * After this document reloads (Activity recreation, WebView reload) its Tab projections
 * are gone while Core still runs the resources. File Tabs come back from the records this
 * document keeps in the Core Tab registry, in the same File Tab projection a desktop Tab
 * view records for itself: layout, Pane endpoints and directories, local directories
 * reopened at their recorded paths and remote sessions verified at their exact
 * generation, as desktop crash recovery does. A recorded Tab whose session ended is not
 * restored and nothing reconnects. Any live SftpSession still without a Tab is bound, by
 * its exact session and generation, to a remote File Tab of its own. Desktop Tabs keep
 * their Core records (`desktop:<sessionId>`), so the strip shows them again before the
 * Desktop page mounts. Terminal Tabs restore in the Terminal view.
 */
export async function restoreSingleDocumentResources(
  store: ReturnType<typeof useWorkspaceTabsStore> = useWorkspaceTabsStore(),
): Promise<void> {
  const [recorded] = await Promise.all([restoreSingleDocumentFileTabs(store), restoreSingleDocumentDesktopTabs(store)]);
  // Records are written only after recovery read them, so a reload never erases them first.
  stopFilePersistence ??= startSingleDocumentFilePersistence(store, recorded);
}

let stopFilePersistence: (() => void) | null = null;

/** Tests reset the once-per-document persistence. */
export function resetSingleDocumentFilePersistenceForTests(): void {
  stopFilePersistence?.();
  stopFilePersistence = null;
}

async function restoreRecordedFileTabs(
  store: ReturnType<typeof useWorkspaceTabsStore>,
): Promise<Map<string, FileTabHandoffSnapshot>> {
  const recorded = new Map<string, FileTabHandoffSnapshot>();
  const records = (await snapshotWorkspaceTabs()).owned.filter((record) => record.kind === "file");
  // Only import and discard are used; this document never navigates for a restore.
  const recovery = createFileRecovery(store, documentRouter as Router);
  for (const record of records) {
    if (store.fileTabs.some((tab) => tab.groupId === record.id)) continue;
    try {
      // `import` validates the recorded payload before anything is staged.
      await recovery.import(record.id, record.payload as FileTabHandoffSnapshot);
      if (!store.admitImportedFileTab(record.id)) {
        await recovery.discard(record.id);
        throw new Error("workspace_tab.file_admission_failed");
      }
      const restored = store.importedFileSnapshots.get(record.id);
      if (restored) recorded.set(record.id, restored);
    } catch (error) {
      showWorkspaceTabFailure(error, `workspace-tab-restore:${record.id}`, "workspace_tab.restoration_failed");
    }
  }
  return recorded;
}

async function restoreSingleDocumentFileTabs(
  store: ReturnType<typeof useWorkspaceTabsStore>,
): Promise<Map<string, FileTabHandoffSnapshot>> {
  const previouslyActive = store.activeFileTabId;
  const recorded = await restoreRecordedFileTabs(store);
  const live = (await fetchSftpSessionSnapshot()).sessions.filter((session) => session.state !== "closed"
    && !store.fileSessionOwner(session.sessionId)
    && !store.fileTabs.some((tab) => tab.initialSessionId === session.sessionId));
  const hosts = live.length ? new Map((await listHosts().catch(() => [])).map((host) => [host.hostId, host.label])) : new Map();
  let first = recorded.keys().next().value ?? "";
  for (const session of live) {
    const id = `file:${crypto.randomUUID()}`;
    const label = (session.hostId && hosts.get(session.hostId)) || i18n.global.t("fileWorkspace.remoteTab");
    if (!store.createInitialFileTab(id, "remote", session.hostId, label, session.sessionId, session.generation)) continue;
    // Claim at once so no other Tab can adopt the same session before this one mounts.
    if (!store.claimFileSession(session.sessionId, id)) continue;
    first ||= id;
  }
  // A Tab the user already had keeps the focus; otherwise the first recovered one is current.
  if (previouslyActive && store.fileTabs.some((tab) => tab.groupId === previouslyActive)) store.activateFileTab(previouslyActive);
  else if (first) store.activateFileTab(first);
  return recorded;
}

/**
 * Keeps this document's File Tab records in the Core registry current. Each mounted File
 * Tab publishes its projection through its controller; a Tab not mounted since the reload
 * keeps its recovered projection. Closed Tabs drop out of the next write. Writes are
 * debounced and always carry the complete set, so Core never holds a partial update.
 */
export function startSingleDocumentFilePersistence(
  store: ReturnType<typeof useWorkspaceTabsStore>,
  recovered: ReadonlyMap<string, FileTabHandoffSnapshot> = new Map(),
): () => void {
  const known = new Map(recovered);
  const observers = new Map<string, { controller: unknown; stop: () => void }>();
  let timer: ReturnType<typeof setTimeout> | undefined;
  let lastWritten: string | null = null;
  let writes = Promise.resolve();
  const flush = () => {
    timer = undefined;
    const tabs = store.fileTabs.flatMap((tab) => known.get(tab.groupId) ?? []);
    const body = JSON.stringify(tabs);
    if (body === lastWritten) return;
    writes = writes.then(() => updateOwnWorkspaceTabProjection({ schemaVersion: 1, kind: "mainDocumentFileTabs", tabs }))
      .then(() => { lastWritten = body; })
      // A refused write keeps the previous records; the next change retries the whole set.
      .catch(() => { lastWritten = null; });
  };
  const schedule = () => {
    if (timer !== undefined) clearTimeout(timer);
    timer = setTimeout(flush, FILE_PERSISTENCE_DEBOUNCE_MS);
  };
  const stopWatch = watch(
    () => store.fileTabs.map((tab) => [tab.groupId, store.fileController(tab.groupId)] as const),
    (entries) => {
      const ids = new Set(entries.map(([id]) => id));
      for (const id of [...known.keys()]) if (!ids.has(id)) known.delete(id);
      for (const [id, observer] of [...observers]) {
        const controller = entries.find(([entry]) => entry === id)?.[1];
        if (controller !== observer.controller) { observer.stop(); observers.delete(id); }
      }
      for (const [id, controller] of entries) {
        if (!controller || observers.has(id)) continue;
        const stop = controller.observeHandoffSnapshot((snapshot) => {
          if (snapshot.tab.groupId !== id) return;
          known.set(id, snapshot);
          schedule();
        });
        observers.set(id, { controller, stop });
      }
      schedule();
    },
    { immediate: true },
  );
  return () => {
    stopWatch();
    if (timer !== undefined) clearTimeout(timer);
    for (const observer of observers.values()) observer.stop();
    observers.clear();
  };
}

const FILE_PERSISTENCE_DEBOUNCE_MS = 300;

async function restoreSingleDocumentDesktopTabs(store: ReturnType<typeof useWorkspaceTabsStore>): Promise<void> {
  // The Desktop view publishes its own Tabs once it is mounted.
  if (store.desktopController) return;
  const [sessions, ownership] = await Promise.all([desktopClient.snapshot(), snapshotWorkspaceTabs()]);
  const owned = filterSingleDocumentDesktopSessions(sessions, ownership);
  if (!owned.length || store.desktopController) return;
  store.syncDesktopState({
    tabs: owned.map((session) => ({ groupId: `desktop:${session.id}`, label: session.profile.label,
      stateLabel: i18n.global.t(`desktop.states.${session.state}`) })),
    activeTabId: `desktop:${owned[0]!.id}`,
    busy: false,
  });
}

export async function closeSingleDocumentTab(
  store: ReturnType<typeof useWorkspaceTabsStore>, router: Router, tab: SingleDocumentTab,
): Promise<boolean> {
  if (tab.kind === "terminal") {
    // The Terminal view confirms running Panes itself; a blocked request keeps the Tab.
    if (router.currentRoute.value.path !== tab.route) await router.push(tab.route);
    return store.terminalController?.close(tab.id) ?? false;
  }
  if (tab.kind === "file") return store.requestCloseFileTab(tab.id);
  if (tab.kind === "desktop") {
    if (router.currentRoute.value.path !== tab.route) await router.push(tab.route);
    await (await awaitController(() => store.desktopController)).close(tab.id);
    return true;
  }
  const removed = store.closePageTab(tab.id);
  if (removed && router.currentRoute.value.path === tab.route) {
    await router.push(store.terminalTabs.length || !store.pageTabs.length ? "/terminal" : store.pageTabs[0]!.route);
  }
  return removed !== null;
}

/**
 * The "+" action follows the "when creating a new page" preference. The joint New
 * page is a Page Tab in this document; welcome and local terminals ask the Terminal
 * view, and the SFTP welcome shows the file page without a Tab.
 */
export async function createSingleDocumentTab(
  store: ReturnType<typeof useWorkspaceTabsStore>, router: Router,
): Promise<void> {
  const behavior = useUiStore().newTerminalBehavior;
  if (behavior === "welcome") {
    const page = workspacePageTabForRoute("/new");
    if (page) await openSingleDocumentPage(store, router, page);
    return;
  }
  if (behavior === "sftpWelcome") {
    if (router.currentRoute.value.path === "/terminal") await store.terminalController?.deactivate();
    store.showFileWelcome();
    await router.push("/sftp");
    return;
  }
  if (router.currentRoute.value.path !== "/terminal") await router.push("/terminal");
  store.terminalController?.createTab?.(behavior === "localTerminal" ? "local" : "welcome");
}

export async function openSingleDocumentPage(
  store: ReturnType<typeof useWorkspaceTabsStore>, router: Router, page: WorkspacePageTab,
): Promise<void> {
  if (router.currentRoute.value.path === "/terminal") await store.terminalController?.deactivate();
  store.importPageTab(page);
  if (router.currentRoute.value.path !== page.route) await router.push(page.route);
}

let documentRouter: Router | null = null;

/** The app shell registers its router once; shell actions in this document navigate through it. */
export function setSingleDocumentRouter(router: Router | null): void {
  documentRouter = router;
}

const CONTROLLER_WAIT_MS = 2_000;

async function awaitController<T>(read: () => T | null | undefined): Promise<T> {
  const deadline = Date.now() + CONTROLLER_WAIT_MS;
  for (;;) {
    const value = read();
    if (value) return value;
    if (Date.now() > deadline) throw new Error("workspace_tab.manager_unavailable");
    await new Promise((resolve) => window.setTimeout(resolve, 16));
  }
}

function requireRouter(): Router {
  if (!documentRouter) throw new Error("workspace_tab.manager_unavailable");
  return documentRouter;
}

/** Shows the Terminal page and waits for its view to register the capability it needs. */
async function terminalControllerWith<K extends keyof TerminalHeaderController>(key: K) {
  const router = requireRouter();
  const store = useWorkspaceTabsStore();
  if (router.currentRoute.value.path !== "/terminal") await router.push("/terminal");
  return awaitController(() => (store.terminalController?.[key] ? store.terminalController : null));
}

/**
 * Plugin protocol launches carry Core-assigned Tab and Pane ids. The single document
 * opens each pending launch as a Terminal Tab of its own; the Pane claims the Core
 * record itself, so an expired or already claimed launch never opens.
 */
export async function openSingleDocumentPluginProtocolLaunches(): Promise<void> {
  const pending = (await listPluginProtocolLaunches())
    .filter((launch) => !launch.claimed && launch.expiresAtUnixMs > Date.now());
  if (!pending.length) return;
  const controller = await terminalControllerWith("reconcilePluginProtocolLaunches");
  await controller.reconcilePluginProtocolLaunches!();
}

/** A Core-approved plugin terminal channel opens once, in a new Terminal Tab of this document. */
export async function openSingleDocumentApprovedPluginChannel(payload: PluginApprovedTerminalChannelLaunch): Promise<void> {
  const controller = await terminalControllerWith("openApprovedPluginChannel");
  await controller.openApprovedPluginChannel!(payload);
}

/** Focuses the existing Pane of exactly this Session and generation; it never reconnects. */
export async function focusSingleDocumentTerminalSession(
  action: Extract<NativeTrayAction, { kind: "focusTerminal" | "focusSshSession" | "focusLocalSession" | "focusTelnet" }>,
): Promise<void> {
  const controller = await terminalControllerWith("activate");
  const focused = action.kind === "focusTerminal" ? controller.focusNativeSession(action.scope)
    : action.kind === "focusSshSession" ? controller.focusSshSession(action.sessionId, action.generation)
      : action.kind === "focusLocalSession" ? controller.focusLocalSession(action.sessionId, action.generation)
        : controller.focusTelnetSession(action.sessionId, action.generation, action.socketId);
  if (!focused) throw new Error("workspace_tab.session_unavailable");
}

/** Opens a saved RDP/VNC profile as a Desktop Tab owned by this document. */
export async function openSingleDocumentDesktop(profileId: string): Promise<void> {
  const router = requireRouter();
  const store = useWorkspaceTabsStore();
  if (router.currentRoute.value.path === "/terminal") await store.terminalController?.deactivate();
  if (router.currentRoute.value.path !== "/desktop") await router.push("/desktop");
  const controller = await awaitController(() => (store.desktopController?.openProfile ? store.desktopController : null));
  await controller.openProfile!(profileId);
}

/** In-document counterpart of the desktop shell actions: Tabs open in this document instead of a native Tab WebView. */
export async function performSingleDocumentShellAction(action: WorkspaceTabShellAction): Promise<void> {
  const router = documentRouter;
  if (!router) throw new Error("workspace_tab.manager_unavailable");
  const store = useWorkspaceTabsStore();
  if (action.type === "navigate") {
    if (action.path === "/terminal" && action.query?.hostId) {
      await router.push({ path: "/terminal", query: { ...action.query, connectOperationId: action.query.connectOperationId ?? createUuidV7() } });
      return;
    }
    await router.push({ path: action.path, query: action.query });
    return;
  }
  if (action.type === "new-terminal") {
    if (router.currentRoute.value.path !== "/terminal") await router.push("/terminal");
    const controller = await awaitController(() => store.terminalController);
    controller.createTab?.(action.behavior === "local" ? "local" : "welcome");
    return;
  }
  if (action.type === "new-file") {
    if (router.currentRoute.value.path === "/terminal") await store.terminalController?.deactivate();
    const remoteHost = action.kind === "remote" ? action.hostId ?? null : null;
    const created = store.createInitialFileTab(`file:${crypto.randomUUID()}`, action.kind, remoteHost, action.label ?? "",
      remoteHost ? createUuidV7() : undefined);
    if (!created) throw new Error("workspace_tab.invalid_action");
    if (router.currentRoute.value.path !== "/sftp") await router.push("/sftp");
    return;
  }
  if (action.type === "sftp-directory-terminal") {
    const operationId = createSftpTerminalLaunch(action.hostId, action.pathBytes);
    if (!operationId) throw new Error("workspace_tab.invalid_action");
    await router.push({ path: "/terminal", query: { hostId: action.hostId, source: "sftpDirectory", connectOperationId: operationId } });
    return;
  }
  if (action.type === "new-desktop") {
    await openSingleDocumentDesktop(action.profileId);
    return;
  }
  throw new Error("workspace_tab.invalid_action");
}

/** After a New page action succeeded, the entry page leaves the strip like its native counterpart. */
export function closeSingleDocumentNewPage(): void {
  const store = useWorkspaceTabsStore();
  for (const tab of [...store.pageTabs]) if (tab.pageType === "newPage") store.closePageTab(tab.groupId);
}

export type { Component };
