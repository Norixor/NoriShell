import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";

import {
  closePluginTargetContext,
  invokePluginUiAction,
  listPluginExtensionTargets,
  listPluginUiContributions,
  openPluginTargetContext,
} from "../core-api/client";
import { usePluginExtensionsStore } from "./pluginExtensions";

vi.mock("../core-api/client", () => ({
  closePluginTargetContext: vi.fn(async () => undefined),
  invokePluginUiAction: vi.fn(),
  listPluginExtensionTargets: vi.fn(),
  listPluginNavigation: vi.fn(async () => []),
  listPluginUiContributions: vi.fn(async () => []),
  openPluginTargetContext: vi.fn(),
}));

describe("plugin extension target leases", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.clearAllMocks();
    vi.mocked(listPluginUiContributions).mockResolvedValue([]);
    vi.mocked(listPluginExtensionTargets).mockResolvedValue([{
      targetId: "terminal.toolbar",
      surfaceKind: "toolbar",
      requiredCapability: "uiPanel",
      contextual: true,
      acceptsForms: true,
    }]);
    vi.mocked(openPluginTargetContext).mockResolvedValue({
      targetId: "terminal.toolbar",
      surfaceKind: "toolbar",
      contextHandle: "019d0000-0000-4000-8000-000000000001",
      targetRevision: "1",
      displayLabel: null,
    });
  });

  it("reference-counts one Core context for duplicate renderer owners", async () => {
    const store = usePluginExtensionsStore();
    const first = await store.acquireTarget("terminal.toolbar", "pane-1");
    const second = await store.acquireTarget("terminal.toolbar", "pane-1");
    expect(openPluginTargetContext).toHaveBeenCalledTimes(1);

    await store.releaseTarget(first);
    expect(closePluginTargetContext).not.toHaveBeenCalled();
    await store.releaseTarget(second);
    expect(closePluginTargetContext).toHaveBeenCalledWith({
      contextHandle: first.context.contextHandle,
      expectedTargetRevision: "1",
    });
  });
  it("does not revive a released context when a listing arrives late", async () => {
    const store = usePluginExtensionsStore();
    const lease = await store.acquireTarget("terminal.toolbar", "pane-1");
    let resolve!: (value: never[]) => void;
    vi.mocked(listPluginUiContributions).mockImplementationOnce(() => new Promise((done) => { resolve = done; }));
    const pending = store.loadTargetContributions(lease.context);
    await store.releaseTarget(lease);
    resolve([]);
    await pending;
    expect(store.forContext(lease.context)).toEqual([]);
    expect(store.loadingHandles.size).toBe(0);
    await store.loadTargetContributions(lease.context);
    expect(listPluginUiContributions).toHaveBeenCalledTimes(1);
  });

  it("keeps a newer empty result when an older listing returns contributions", async () => {
    const store = usePluginExtensionsStore();
    const lease = await store.acquireTarget("terminal.toolbar", "pane-1");
    let resolve!: (value: Awaited<ReturnType<typeof listPluginUiContributions>>) => void;
    vi.mocked(listPluginUiContributions).mockImplementationOnce(() => new Promise((done) => { resolve = done; }));
    const old = store.loadTargetContributions(lease.context);
    await store.loadTargetContributions(lease.context);
    resolve([{ pluginId: "test.plugin", target: lease.context } as never]);
    await old;
    expect(store.forContext(lease.context)).toEqual([]);
    expect(store.loadingHandles.size).toBe(0);
  });

  it("refreshes empty live targets after settings change and skips released targets", async () => {
    const store = usePluginExtensionsStore();
    const first = await store.acquireTarget("terminal.toolbar", "pane-1");
    await store.refreshPluginContributions("test.plugin");
    expect(listPluginUiContributions).toHaveBeenCalledExactlyOnceWith({ target: first.context });
    await store.releaseTarget(first);
    await store.refreshPluginContributions("test.plugin");
    expect(listPluginUiContributions).toHaveBeenCalledTimes(1);
  });

  it("does not overwrite an action reply with a pre-action list", async () => {
    const store = usePluginExtensionsStore();
    const lease = await store.acquireTarget("terminal.toolbar", "pane-1");
    const item = {
      pluginId: "test.status", artifactFingerprintSha256: "a", packageSha256: "b",
      instanceGeneration: "1", stateVersion: "1", contributionRevision: "1", target: lease.context,
    } as never;
    vi.mocked(listPluginUiContributions).mockResolvedValueOnce([item]);
    await store.loadTargetContributions(lease.context);
    let resolve!: (value: Awaited<ReturnType<typeof listPluginUiContributions>>) => void;
    vi.mocked(listPluginUiContributions).mockImplementationOnce(() => new Promise((done) => { resolve = done; }));
    const pending = store.loadTargetContributions(lease.context);
    const updated = { ...store.forContext(lease.context)[0]!, contributionRevision: "2" };
    vi.mocked(invokePluginUiAction).mockResolvedValueOnce({ contribution: updated } as never);
    await store.invokeAction(store.forContext(lease.context)[0]!, "refresh", [], null);
    resolve([item]);
    await pending;
    expect(store.forContext(lease.context)[0]?.contributionRevision).toBe("2");
    expect(store.loadingHandles.size).toBe(0);
  });

  it.each([false, true])("preserves the action failure after a refresh (refresh fails: %s)", async (refreshFails) => {
    const store = usePluginExtensionsStore();
    const lease = await store.acquireTarget("terminal.toolbar", "pane-1");
    const item = {
      pluginId: "test.protocol", artifactFingerprintSha256: "a", packageSha256: "b",
      instanceGeneration: "1", stateVersion: "1", contributionRevision: "1", target: lease.context,
    } as never;
    const actionError = { code: "plugin.runtime_rejected" };
    vi.mocked(invokePluginUiAction).mockRejectedValueOnce(actionError);
    if (refreshFails) {
      vi.mocked(listPluginUiContributions).mockRejectedValueOnce({ code: "plugin.capability_denied" });
    }
    expect(await store.invokeAction(item, "open", [], null)).toBeNull();
    expect(listPluginUiContributions).toHaveBeenCalledWith({ target: lease.context });
    expect(store.error).toEqual(actionError);
    expect(store.busyActionKey).toBeNull();
  });

  it("marks automatic actions as background and interactive actions as foreground", async () => {
    const store = usePluginExtensionsStore();
    const lease = await store.acquireTarget("terminal.toolbar", "pane-1");
    const item = {
      pluginId: "test.status", artifactFingerprintSha256: "a", packageSha256: "b",
      instanceGeneration: "1", stateVersion: "1", contributionRevision: "1", target: lease.context,
    } as never;
    vi.mocked(listPluginUiContributions).mockResolvedValueOnce([item]);
    await store.loadTargetContributions(lease.context);
    const updated = { ...store.forContext(lease.context)[0]!, contributionRevision: "2" };
    vi.mocked(invokePluginUiAction).mockResolvedValue({ contribution: updated } as never);

    await store.invokeAction(item, "refresh", [], null, true);
    expect(invokePluginUiAction).toHaveBeenLastCalledWith(expect.objectContaining({ background: true }));
  });

  it("reuses only completely unchanged fenced contributions and retains removals", async () => {
    const store = usePluginExtensionsStore();
    const lease = await store.acquireTarget("terminal.toolbar", "pane-1");
    const original = {
      pluginId: "test.tunnels", pluginName: "Tunnels", artifactFingerprintSha256: "a", packageSha256: "b",
      instanceGeneration: "1", stateVersion: "1", contributionRevision: "1", target: lease.context,
      document: { schemaVersion: 1, rootNodeId: "status", nodes: [{ kind: "text", nodeId: "status", text: "Ready", style: "body", tone: "neutral" }] },
    };
    vi.mocked(listPluginUiContributions).mockResolvedValueOnce(structuredClone([original]) as never);
    await store.loadTargetContributions(lease.context);
    const initial = store.forContext(lease.context)[0];
    vi.mocked(listPluginUiContributions).mockResolvedValueOnce(structuredClone([original]) as never);
    await store.loadTargetContributions(lease.context);
    expect(store.forContext(lease.context)[0]).toBe(initial);

    const changedDocument = structuredClone(original);
    changedDocument.document.nodes[0]!.text = "Unavailable";
    vi.mocked(listPluginUiContributions).mockResolvedValueOnce([changedDocument] as never);
    await store.loadTargetContributions(lease.context);
    const changed = store.forContext(lease.context)[0];
    expect(changed).not.toBe(initial);
    expect(changed?.document.nodes[0]).toHaveProperty("text", "Unavailable");

    vi.mocked(listPluginUiContributions).mockResolvedValueOnce([{ ...changedDocument, stateVersion: "2" }] as never);
    await store.loadTargetContributions(lease.context);
    expect(store.forContext(lease.context)[0]).not.toBe(changed);
    expect(store.forContext(lease.context)[0]?.stateVersion).toBe("2");
    vi.mocked(listPluginUiContributions).mockResolvedValueOnce([]);
    await store.loadTargetContributions(lease.context);
    expect(store.forContext(lease.context)).toEqual([]);
  });

});
