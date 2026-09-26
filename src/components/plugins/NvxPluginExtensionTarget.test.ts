import { enableAutoUnmount, flushPromises, mount } from "@vue/test-utils";
import { createPinia, getActivePinia, setActivePinia } from "pinia";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ref } from "vue";

import { i18n } from "../../locales";
import * as coreClient from "../../core-api/client";
import type { PluginExtensionTargetContext, PluginUiContribution } from "../../core-api/generated/core-api";
import { usePluginExtensionsStore } from "../../stores/pluginExtensions";
import { useTipsStore } from "../../stores/tips";
import NvxPluginUiDocument from "./NvxPluginUiDocument.vue";
import NvxPluginExtensionTarget from "./NvxPluginExtensionTarget.vue";

vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true }));

enableAutoUnmount(afterEach);

describe("NvxPluginExtensionTarget", () => {
  beforeEach(() => {
    i18n.global.locale.value = "zh-CN";
    setActivePinia(createPinia());
  });

  afterEach(() => vi.restoreAllMocks());

  it("shows a recoverable page acquisition error and keeps an empty successful target quiet", async () => {
    const extensions = usePluginExtensionsStore();
    const context = { targetId: "app.page", surfaceKind: "page" as const, contextHandle: "retry", targetRevision: "1", displayLabel: null };
    const acquire = vi.spyOn(extensions, "acquireTarget")
      .mockRejectedValueOnce(new Error("private diagnostic"))
      .mockResolvedValue({ key: "retry", context });
    vi.spyOn(extensions, "releaseTarget").mockResolvedValue();
    vi.spyOn(extensions, "loadTargetContributions").mockResolvedValue([]);
    const wrapper = mount(NvxPluginExtensionTarget, { props: { targetId: "app.page" }, global: { plugins: [i18n, getActivePinia()!] } });
    await flushPromises();
    expect(wrapper.find(".nvx-inline-notice").exists()).toBe(true);
    expect(wrapper.text()).toContain("插件内容加载失败");
    expect(wrapper.text()).not.toContain("private diagnostic");
    await wrapper.get("button").trigger("click");
    await flushPromises();
    expect(acquire).toHaveBeenCalledTimes(2);
    expect(wrapper.find(".plugin-extension-target").exists()).toBe(false);
  });

  it("hides stale content after a refresh failure and uses compact recovery outside pages", async () => {
    const extensions = usePluginExtensionsStore();
    const context = { targetId: "app.header.actions", surfaceKind: "toolbar" as const, contextHandle: "header", targetRevision: "1", displayLabel: null };
    vi.spyOn(extensions, "acquireTarget").mockResolvedValue({ key: "header", context });
    vi.spyOn(extensions, "releaseTarget").mockResolvedValue();
    const load = vi.spyOn(extensions, "loadTargetContributions").mockResolvedValue([]);
    const wrapper = mount(NvxPluginExtensionTarget, { props: { targetId: context.targetId }, global: { plugins: [i18n, getActivePinia()!] } });
    await flushPromises();
    vi.spyOn(extensions, "forContext").mockReturnValue([{
      pluginId: "stale", pluginName: "stale", target: context,
      instanceGeneration: "1", packageSha256: "a".repeat(64),
      document: { schemaVersion: 1, rootNodeId: "root", nodes: [{ kind: "text", nodeId: "root", text: "stale content", style: "body", tone: "neutral" }] },
    }] as never);
    load.mockRejectedValueOnce(new Error("revoked"));
    window.dispatchEvent(new CustomEvent("norishell:plugin-runtime-invalidated"));
    await flushPromises();
    expect(wrapper.find(".nvx-inline-notice").exists()).toBe(false);
    expect(wrapper.get("button").attributes("aria-label")).toContain("插件内容加载失败");
    expect(wrapper.findComponent(NvxPluginUiDocument).exists()).toBe(false);
    vi.mocked(extensions.forContext).mockReturnValue([]);
    await wrapper.get("button").trigger("click");
    await flushPromises();
    expect(wrapper.find("button").exists()).toBe(false);
  });

  it("ignores an acquisition error from an obsolete target generation", async () => {
    const extensions = usePluginExtensionsStore();
    let rejectOld!: (error: Error) => void;
    vi.spyOn(extensions, "acquireTarget")
      .mockImplementationOnce(() => new Promise((_resolve, reject) => { rejectOld = reject; }))
      .mockResolvedValue({ key: "new", context: { targetId: "app.page", surfaceKind: "page", contextHandle: "new", targetRevision: "1", displayLabel: null } });
    vi.spyOn(extensions, "releaseTarget").mockResolvedValue();
    vi.spyOn(extensions, "loadTargetContributions").mockResolvedValue([]);
    const wrapper = mount(NvxPluginExtensionTarget, { props: { targetId: "app.page", instanceKey: "old" }, global: { plugins: [i18n, getActivePinia()!] } });
    await flushPromises();
    await wrapper.setProps({ instanceKey: "new" });
    await flushPromises();
    rejectOld(new Error("old failure"));
    await flushPromises();
    expect(wrapper.find(".nvx-inline-notice").exists()).toBe(false);
    expect(wrapper.find(".plugin-extension-target").exists()).toBe(false);
  });

  it("filters exact declared routes and renders only the selected plugin", async () => {
    const extensions = usePluginExtensionsStore();
    const context = { targetId: "app.content.floating", surfaceKind: "overlay" as const, contextHandle: "route-context", targetRevision: "1", displayLabel: "Hosts" };
    const contribution = (id: string, routes?: string[]) => ({ pluginId: id, pluginName: id, icon: "play", routePaths: routes, artifactFingerprintSha256: "a".repeat(64), packageSha256: "b".repeat(64), instanceGeneration: "1", stateVersion: "1", contributionRevision: "1", target: context, document: { schemaVersion: 1, rootNodeId: "root", nodes: [{ kind: "text", nodeId: "root", text: id, style: "body", tone: "neutral" }] } });
    const items = [contribution("test.hosts", ["/hosts"]), contribution("test.terminal", ["/terminal"]), contribution("test.all")];
    vi.spyOn(extensions, "acquireTarget").mockResolvedValue({ key: "route-context", context });
    vi.spyOn(extensions, "releaseTarget").mockResolvedValue();
    vi.spyOn(extensions, "loadTargetContributions").mockResolvedValue(items as never);
    vi.spyOn(extensions, "forContext").mockReturnValue(items as never);
    const wrapper = mount(NvxPluginExtensionTarget, { props: { targetId: "app.content.floating", routePath: "/hosts", selectedPluginId: "test.hosts" }, global: { plugins: [i18n, getActivePinia()!] } });
    await flushPromises();
    expect(wrapper.findAllComponents(NvxPluginUiDocument)).toHaveLength(1);
    expect(wrapper.getComponent(NvxPluginUiDocument).props("contribution").pluginId).toBe("test.hosts");
    expect(wrapper.emitted("availability")?.at(-1)).toEqual([2]);
    await wrapper.setProps({ routePath: "/terminal" });
    expect(wrapper.findAllComponents(NvxPluginUiDocument)).toHaveLength(0);
    wrapper.unmount();
  });

  it("opens the current page plugin settings from a declared button without invoking the guest", async () => {
    const extensions = usePluginExtensionsStore();
    const context = { targetId: "app.page", surfaceKind: "page" as const, contextHandle: "sync-page", targetRevision: "1", displayLabel: "Sync" };
    const contribution = {
      pluginId: "com.norishell.self-host-sync", pluginName: "Sync", target: context,
      instanceGeneration: "1", packageSha256: "a".repeat(64), onOpenActionId: "sync.pageOpened",
      document: { schemaVersion: 1, rootNodeId: "root", nodes: [{
        kind: "button", nodeId: "root", actionId: "norishell.openSettings:serverUrl",
        label: "Enter server address", icon: "settings", variant: "primary", disabled: false,
      }] },
    };
    vi.spyOn(extensions, "acquireTarget").mockResolvedValue({ key: "sync-page", context });
    vi.spyOn(extensions, "loadTargetContributions").mockResolvedValue([contribution] as never);
    vi.spyOn(extensions, "forContext").mockReturnValue([contribution] as never);
    vi.spyOn(extensions, "releaseTarget").mockResolvedValue();
    const invokeAction = vi.spyOn(extensions, "invokeAction").mockResolvedValue(null);
    const wrapper = mount(NvxPluginExtensionTarget, {
      props: { targetId: "app.page", selectedPluginId: contribution.pluginId },
      global: { plugins: [i18n, getActivePinia()!] },
    });
    await flushPromises();
    invokeAction.mockClear();
    await wrapper.get("button").trigger("click");
    await flushPromises();
    expect(wrapper.emitted("openSettings")).toEqual([[
      contribution.pluginId, contribution.pluginName, "serverUrl",
    ]]);
    expect(invokeAction).not.toHaveBeenCalled();
    await wrapper.vm.refreshAfterSettingsChange(contribution.pluginId);
    expect(invokeAction).toHaveBeenCalledOnce();
    expect(invokeAction.mock.calls[0]?.[1]).toBe("sync.pageOpened");
  });

  it("releases a route target acquired after its renderer was unmounted", async () => {
    const extensions = usePluginExtensionsStore();
    let resolveLease!: (lease: Awaited<ReturnType<typeof extensions.acquireTarget>>) => void;
    vi.spyOn(extensions, "acquireTarget").mockImplementation(() => new Promise((resolve) => { resolveLease = resolve; }));
    const load = vi.spyOn(extensions, "loadTargetContributions").mockResolvedValue([]);
    const release = vi.spyOn(extensions, "releaseTarget").mockResolvedValue();
    const wrapper = mount(NvxPluginExtensionTarget, {
      props: { targetId: "app.content.before", instanceKey: "route:old" },
      global: { plugins: [i18n, getActivePinia()!] },
    });
    await flushPromises();
    wrapper.unmount();
    const lease = {
      key: "app.content.before\u0000route:old",
      context: { targetId: "app.content.before", surfaceKind: "inline" as const, contextHandle: "old", targetRevision: "1", displayLabel: null },
    };
    resolveLease(lease);
    await flushPromises();
    expect(release).toHaveBeenCalledExactlyOnceWith(lease);
    expect(load).not.toHaveBeenCalled();
  });

  it("keeps the current route lease when an older acquisition arrives late", async () => {
    const extensions = usePluginExtensionsStore();
    type Lease = Awaited<ReturnType<typeof extensions.acquireTarget>>;
    const pending = new Map<string, (lease: Lease) => void>();
    vi.spyOn(extensions, "acquireTarget").mockImplementation((_id, key) => new Promise((resolve) => { pending.set(key, resolve); }));
    const load = vi.spyOn(extensions, "loadTargetContributions").mockResolvedValue([]);
    const release = vi.spyOn(extensions, "releaseTarget").mockResolvedValue();
    const wrapper = mount(NvxPluginExtensionTarget, {
      props: { targetId: "app.content.before", instanceKey: "old" }, global: { plugins: [i18n, getActivePinia()!] },
    });
    await flushPromises();
    await wrapper.setProps({ instanceKey: "current" });
    await flushPromises();
    const makeLease = (key: string): Lease => ({
      key: `app.content.before\u0000${key}`,
      context: { targetId: "app.content.before", surfaceKind: "inline", contextHandle: key, targetRevision: "1", displayLabel: null },
    });
    const current = makeLease("current");
    pending.get("current")?.(current);
    await flushPromises();
    const old = makeLease("old");
    pending.get("old")?.(old);
    await flushPromises();
    expect(load).toHaveBeenCalledExactlyOnceWith(current.context);
    expect(release).toHaveBeenCalledExactlyOnceWith(old);
    wrapper.unmount();
    await flushPromises();
    expect(release).toHaveBeenLastCalledWith(current);
  });

  it("refreshes an open target when a plugin runtime becomes ready or is invalidated", async () => {
    const extensions = usePluginExtensionsStore();
    const context = {
      targetId: "app.header.actions",
      surfaceKind: "toolbar" as const,
      contextHandle: "019d0000-0000-4000-8000-000000000901",
      targetRevision: "1",
      displayLabel: null,
    };
    vi.spyOn(extensions, "acquireTarget").mockResolvedValue({
      key: "app.header.actions\u0000global",
      context,
    });
    const loadTargetContributions = vi
      .spyOn(extensions, "loadTargetContributions")
      .mockResolvedValue([]);
    const releaseTarget = vi.spyOn(extensions, "releaseTarget").mockResolvedValue();

    const wrapper = mount(NvxPluginExtensionTarget, {
      props: { targetId: "app.header.actions", showIdentity: false },
      global: { plugins: [i18n, getActivePinia()!] },
    });
    await flushPromises();
    expect(loadTargetContributions).toHaveBeenCalledTimes(1);

    window.dispatchEvent(new CustomEvent("norishell:plugin-runtime-ready"));
    await flushPromises();
    expect(loadTargetContributions).toHaveBeenCalledTimes(2);

    window.dispatchEvent(new CustomEvent("norishell:plugin-runtime-invalidated"));
    await flushPromises();
    expect(loadTargetContributions).toHaveBeenCalledTimes(3);

    wrapper.unmount();
    await flushPromises();
    expect(releaseTarget).toHaveBeenCalledOnce();
    window.dispatchEvent(new CustomEvent("norishell:plugin-runtime-ready"));
    await flushPromises();
    expect(loadTargetContributions).toHaveBeenCalledTimes(3);
  });

  it("explains denied plugin access without naming an unrelated capability", async () => {
    const extensions = usePluginExtensionsStore();
    const tips = useTipsStore();
    const context = {
      targetId: "app.page",
      surfaceKind: "page" as const,
      contextHandle: "019d0000-0000-4000-8000-000000000902",
      targetRevision: "1",
      displayLabel: "Norixor",
    };
    const contribution = {
      pluginId: "org.norixor",
      pluginName: "Norixor",
      artifactFingerprintSha256: "a".repeat(64),
      packageSha256: "b".repeat(64),
      instanceGeneration: "1",
      stateVersion: "2",
      contributionRevision: "1",
      target: context,
      document: {
        schemaVersion: 1,
        rootNodeId: "root",
        nodes: [{
          kind: "button",
          nodeId: "root",
          actionId: "connect",
          label: "Connect",
          icon: null,
          variant: "primary",
          disabled: false,
        }],
      },
    };
    vi.spyOn(extensions, "acquireTarget").mockResolvedValue({
      key: "app.page\u0000org.norixor|account",
      context,
    });
    vi.spyOn(extensions, "loadTargetContributions").mockResolvedValue([contribution] as never);
    vi.spyOn(extensions, "forContext").mockReturnValue([contribution] as never);
    vi.spyOn(extensions, "releaseTarget").mockResolvedValue();
    vi.spyOn(extensions, "invokeAction").mockImplementation(async () => {
      extensions.error = {
        requestId: "019d0000-0000-4000-8000-000000000903",
        code: "plugin.capability_denied",
        category: "permission",
        retryStrategy: { kind: "never" },
        messageKey: "errors.plugin.capabilityDenied",
        details: null,
      };
      return null;
    });

    const wrapper = mount(NvxPluginExtensionTarget, {
      props: {
        targetId: "app.page",
        instanceKey: "org.norixor|account",
        showIdentity: false,
      },
      global: { plugins: [i18n, getActivePinia()!] },
    });
    await flushPromises();
    await wrapper.getComponent(NvxPluginUiDocument).get("button").trigger("click");
    await flushPromises();

    expect(tips.items[0]?.title).toContain("权限检查");
    expect(tips.items[0]?.title).not.toContain("SSH 同步");
    expect(tips.items[0]?.title).not.toContain("刷新");
    expect(wrapper.find(".plugin-extension-target__feedback").exists()).toBe(false);
    expect(wrapper.find(".plugin-extension-target__identity").exists()).toBe(false);
  });

  it("runs a page contribution's protocol-seven on-open action once", async () => {
    const extensions = usePluginExtensionsStore();
    const tips = useTipsStore();
    tips.show({
      scope: "plugin-action:app.page:org.norixor|account",
      tone: "error",
      title: "stale automatic refresh error",
    });
    const context = {
      targetId: "app.page",
      surfaceKind: "page" as const,
      contextHandle: "019d0000-0000-4000-8000-000000000904",
      targetRevision: "1",
      displayLabel: "Norixor",
    };
    const contribution = {
      pluginId: "org.norixor",
      pluginName: "Norixor",
      signerFingerprintSha256: "a".repeat(64),
      packageSha256: "b".repeat(64),
      instanceGeneration: "1",
      stateVersion: "2",
      contributionRevision: "1",
      target: context,
      onOpenActionId: "refresh",
      document: {
        schemaVersion: 1,
        rootNodeId: "root",
        nodes: [{
          kind: "button",
          nodeId: "root",
          actionId: "refresh",
          label: "Refresh",
          icon: null,
          variant: "secondary",
          disabled: false,
        }],
      },
    };
    vi.spyOn(extensions, "acquireTarget").mockResolvedValue({
      key: "app.page\u0000org.norixor|account",
      context,
    });
    vi.spyOn(extensions, "loadTargetContributions").mockResolvedValue([contribution] as never);
    vi.spyOn(extensions, "forContext").mockReturnValue([contribution] as never);
    vi.spyOn(extensions, "releaseTarget").mockResolvedValue();
    const invokeAction = vi.spyOn(extensions, "invokeAction").mockResolvedValue(null);

    mount(NvxPluginExtensionTarget, {
      props: {
        targetId: "app.page",
        instanceKey: "org.norixor|account",
        showIdentity: false,
      },
      global: { plugins: [i18n, getActivePinia()!] },
    });
    await flushPromises();

    expect(invokeAction).toHaveBeenCalledOnce();
    expect(invokeAction.mock.calls[0]?.[1]).toBe("refresh");
    expect(invokeAction.mock.calls[0]?.[2]).toEqual([]);
    expect(invokeAction.mock.calls[0]?.[4]).toBe(true);
    expect(tips.items).toEqual([]);
  });
  it("initializes tools when shown or selected and waits for another action to finish", async () => {
    const extensions = usePluginExtensionsStore();
    const context = { targetId: "terminal.tools", surfaceKind: "panel" as const, contextHandle: "connected-session", targetRevision: "1", displayLabel: "QA" };
    const items = ["disk", "docker"].map((id) => ({
      pluginId: id, pluginName: id, artifactFingerprintSha256: "a".repeat(64),
      packageSha256: "b".repeat(64), instanceGeneration: "1", stateVersion: "1",
      contributionRevision: "1", target: context, onOpenActionId: `${id}:hosts`,
      document: { schemaVersion: 1, rootNodeId: "root", nodes: [{ kind: "text", nodeId: "root", text: id, style: "body", tone: "neutral" }] },
    }));
    const liveItems = ref(items);
    vi.spyOn(extensions, "acquireTarget").mockResolvedValue({ key: "session", context } as never);
    vi.spyOn(extensions, "releaseTarget").mockResolvedValue();
    vi.spyOn(extensions, "loadTargetContributions").mockImplementation(async () => liveItems.value as never);
    vi.spyOn(extensions, "forContext").mockImplementation(() => liveItems.value as never);
    const invoke = vi.spyOn(extensions, "invokeAction").mockImplementation(async (contribution) => {
      liveItems.value = liveItems.value.map((item) => item.pluginId === contribution.pluginId
        ? { ...item, contributionRevision: String(Number(item.contributionRevision) + 1), onOpenActionId: `${item.pluginId}:open-next` }
        : item);
      return null;
    });
    const wrapper = mount(NvxPluginExtensionTarget, { props: {
      targetId: "terminal.tools", selectedPluginId: null, disabled: true,
    }, global: { plugins: [i18n, getActivePinia()!] } });
    await flushPromises();
    expect(invoke).not.toHaveBeenCalled();
    extensions.busyActionKey = "other-action";
    await wrapper.setProps({ selectedPluginId: "disk", disabled: false });
    await flushPromises();
    expect(invoke).not.toHaveBeenCalled();
    extensions.busyActionKey = null;
    await flushPromises();
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(invoke.mock.calls[0]?.[1]).toBe("disk:hosts");
    window.dispatchEvent(new CustomEvent("norishell:plugin-runtime-ready"));
    await flushPromises();
    expect(invoke).toHaveBeenCalledTimes(1);
    await wrapper.setProps({ selectedPluginId: "docker" });
    await flushPromises();
    expect(invoke.mock.calls[1]?.[1]).toBe("docker:hosts");
    await wrapper.setProps({ disabled: true, selectedPluginId: null });
    await wrapper.setProps({ disabled: false, selectedPluginId: "disk" });
    await flushPromises();
    expect(invoke.mock.calls[2]?.[1]).toBe("disk:open-next");
    expect(invoke).toHaveBeenCalledTimes(3);
  });

  it("refreshes visible footer hooks without revision loops and stops on disable or failure", async () => {
    vi.useFakeTimers();
    vi.stubGlobal("IntersectionObserver", undefined);
    const extensions = usePluginExtensionsStore();
    const context = { targetId: "terminal.footer", surfaceKind: "inline" as const, contextHandle: "footer", targetRevision: "1", displayLabel: "SSH" };
    const liveItems = ref([{
      pluginId: "test.status", pluginName: "Status", artifactFingerprintSha256: "a".repeat(64),
      packageSha256: "b".repeat(64), instanceGeneration: "1", stateVersion: "1",
      contributionRevision: "1", target: context, onOpenActionId: "refresh",
      autoRefresh: { actionId: "refresh", intervalMs: 5000 },
      document: { schemaVersion: 1, rootNodeId: "root", nodes: [{ kind: "text", nodeId: "root", text: "CPU 12%", style: "body", tone: "neutral" }] },
    }]);
    vi.spyOn(extensions, "acquireTarget").mockResolvedValue({ key: "footer", context });
    vi.spyOn(extensions, "releaseTarget").mockResolvedValue();
    vi.spyOn(extensions, "loadTargetContributions").mockImplementation(async () => liveItems.value as never);
    vi.spyOn(extensions, "forContext").mockImplementation(() => liveItems.value as never);
    const invoke = vi.spyOn(extensions, "invokeAction").mockImplementation(async () => {
      liveItems.value = liveItems.value.map((item) => ({ ...item, contributionRevision: String(Number(item.contributionRevision) + 1) }));
      return { contribution: liveItems.value[0], hostDomOperations: null, clipboardText: null, hostApprovalId: null, sshSyncStatus: null, terminalInputSuggestion: null } as never;
    });
    const wrapper = mount(NvxPluginExtensionTarget, { attachTo: document.body,
      props: { targetId: "terminal.footer", showIdentity: false }, global: { plugins: [i18n, getActivePinia()!] } });
    try {
      await flushPromises();
      expect(invoke).toHaveBeenCalledTimes(1);
      expect(invoke.mock.calls[0]?.[4]).toBe(true);
      await vi.advanceTimersByTimeAsync(4999);
      expect(invoke).toHaveBeenCalledTimes(1);
      await vi.advanceTimersByTimeAsync(1);
      expect(invoke).toHaveBeenCalledTimes(2);
      expect(invoke.mock.calls[1]?.[4]).toBe(true);
      await wrapper.setProps({ disabled: true });
      await vi.advanceTimersByTimeAsync(10000);
      expect(invoke).toHaveBeenCalledTimes(2);
      await wrapper.setProps({ disabled: false });
      await flushPromises();
      expect(invoke).toHaveBeenCalledTimes(3);
      invoke.mockResolvedValue(null);
      await vi.advanceTimersByTimeAsync(5000);
      expect(invoke).toHaveBeenCalledTimes(4);
      expect(wrapper.text()).toContain("CPU 12%");
      expect(wrapper.get('[role="status"]').text()).toBe(i18n.global.t("plugins.tools.autoRefreshPaused"));
      const previousLocale = i18n.global.locale.value;
      i18n.global.locale.value = "en";
      await wrapper.vm.$nextTick();
      expect(wrapper.get('[role="status"]').text()).toBe("Auto-refresh paused");
      i18n.global.locale.value = "zh-CN";
      await wrapper.vm.$nextTick();
      expect(wrapper.get('[role="status"]').text()).toBe("自动刷新已暂停");
      i18n.global.locale.value = previousLocale;
      await vi.advanceTimersByTimeAsync(15000);
      expect(invoke).toHaveBeenCalledTimes(4);
      expect(useTipsStore().items).toEqual([]);
      const nextContext = { ...context, contextHandle: "footer-next", targetRevision: "2" };
      vi.mocked(extensions.acquireTarget).mockResolvedValue({ key: "footer-next", context: nextContext });
      liveItems.value = liveItems.value.map((item) => ({ ...item, target: nextContext }));
      await wrapper.setProps({ instanceKey: "ssh-next" });
      await flushPromises();
      expect(wrapper.find('[role="status"]').exists()).toBe(false);
      expect(wrapper.text()).toContain("CPU 12%");
      wrapper.unmount();
      expect(vi.getTimerCount()).toBe(0);
    } finally { vi.useRealTimers(); vi.unstubAllGlobals(); }
  });

  it("keeps another plugin's open select and draft stable during a CPU tick and sibling reload", async () => {
    vi.useFakeTimers();
    vi.stubGlobal("IntersectionObserver", undefined);
    const extensions = usePluginExtensionsStore();
    const context = (targetId: string): PluginExtensionTargetContext => ({
      targetId, surfaceKind: "inline", contextHandle: targetId, targetRevision: "1", displayLabel: "SSH",
    });
    const footerContext = context("terminal.footer");
    const toolsContext = context("terminal.tools");
    const cpu: PluginUiContribution = {
      pluginId: "test.processes", pluginName: "Processes", artifactFingerprintSha256: "a".repeat(64),
      packageSha256: "b".repeat(64), instanceGeneration: "1", stateVersion: "1", contributionRevision: "1",
      target: footerContext, autoRefresh: { actionId: "refresh", intervalMs: 5000 },
      document: { schemaVersion: 1, rootNodeId: "cpu", nodes: [{ kind: "text", nodeId: "cpu", text: "CPU 12%", style: "body", tone: "neutral" }] },
    };
    const tunnels: PluginUiContribution = {
      ...cpu, pluginId: "test.tunnels", pluginName: "Tunnels", target: toolsContext, autoRefresh: null,
      document: { schemaVersion: 1, rootNodeId: "root", nodes: [
        { kind: "stack", nodeId: "root", direction: "vertical", align: "stretch", gap: 8, children: ["draft", "forward", "stop"] },
        { kind: "textField", nodeId: "draft", fieldId: "draft", label: "Draft", value: "", placeholder: null, fieldKind: "text", required: false, disabled: false },
        { kind: "select", nodeId: "forward", fieldId: "forward", label: "Forward to stop", value: "first", disabled: false,
          options: [{ value: "first", label: "First", disabled: false }, { value: "second", label: "Second", disabled: false }] },
        { kind: "button", nodeId: "stop", actionId: "stop", label: "Stop", variant: "secondary", icon: null, disabled: false },
      ] },
    };
    let cpuRevision = "1";
    vi.spyOn(coreClient, "listPluginExtensionTargets").mockResolvedValue([footerContext, toolsContext].map((item) => ({
      targetId: item.targetId, surfaceKind: item.surfaceKind, requiredCapability: "uiPanel", contextual: true, acceptsForms: true,
    })));
    vi.spyOn(coreClient, "openPluginTargetContext").mockImplementation(async (input) => context(input.targetId));
    vi.spyOn(coreClient, "closePluginTargetContext").mockResolvedValue();
    vi.spyOn(coreClient, "listPluginUiContributions").mockImplementation(async ({ target }) => JSON.parse(JSON.stringify(
      target.targetId === "terminal.footer" ? [{ ...cpu, contributionRevision: cpuRevision }]
        : [{ ...cpu, contributionRevision: cpuRevision, target: toolsContext, autoRefresh: null }, tunnels],
    )));
    let completeCpu!: (response: Awaited<ReturnType<typeof coreClient.invokePluginUiAction>>) => void;
    const rpc = vi.spyOn(coreClient, "invokePluginUiAction").mockImplementation(() => new Promise((resolve) => { completeCpu = resolve; }));
    const footer = mount(NvxPluginExtensionTarget, { attachTo: document.body,
      props: { targetId: "terminal.footer", instanceKey: "ssh", showIdentity: false }, global: { plugins: [i18n, getActivePinia()!] } });
    const tools = mount(NvxPluginExtensionTarget, { attachTo: document.body,
      props: { targetId: "terminal.tools", instanceKey: "ssh", selectedPluginId: "test.tunnels", showIdentity: false }, global: { plugins: [i18n, getActivePinia()!] } });
    try {
      await flushPromises();
      const before = extensions.forContext(toolsContext).find((item) => item.pluginId === tunnels.pluginId);
      const documentElement = tools.getComponent(NvxPluginUiDocument).element;
      const draft = tools.get("input");
      await draft.setValue("user draft");
      const select = tools.get('[role="combobox"]');
      await select.trigger("click");
      await select.trigger("keydown", { key: "ArrowDown" });
      const popup = document.querySelector('[role="listbox"]');
      const highlighted = select.attributes("aria-activedescendant");
      expect(popup).not.toBeNull();
      vi.advanceTimersByTime(5000);
      await flushPromises();
      expect(rpc).toHaveBeenCalledTimes(1);
      expect(extensions.busyPluginId).toBe(cpu.pluginId);
      expect(select.attributes("disabled")).toBeUndefined();
      expect(select.attributes("aria-expanded")).toBe("true");
      expect(draft.attributes("disabled")).toBeUndefined();
      expect(draft.element).toHaveProperty("value", "user draft");
      expect(tools.getComponent(NvxPluginUiDocument).props("busy")).toBe(false);
      const stop = tools.findAll("button").find((button) => button.text() === "Stop")!;
      expect(stop.attributes("disabled")).toBeDefined();
      await stop.trigger("click");
      expect(await extensions.invokeAction(tunnels, "stop", [], null)).toBeNull();
      expect(rpc).toHaveBeenCalledTimes(1);
      cpuRevision = "2";
      completeCpu({ contribution: { ...cpu, contributionRevision: cpuRevision }, hostDomOperations: null,
        clipboardText: null, hostApprovalId: null, sshSyncStatus: null, terminalInputSuggestion: null, isolatedSurfaceOpened: false });
      await flushPromises();
      expect(extensions.busyActionKey).toBeNull();
      expect(extensions.busyPluginId).toBeNull();
      expect(extensions.forContext(toolsContext).find((item) => item.pluginId === tunnels.pluginId)).toBe(before);
      expect(extensions.forContext(toolsContext).find((item) => item.pluginId === cpu.pluginId)?.contributionRevision).toBe("2");
      expect(tools.getComponent(NvxPluginUiDocument).element).toBe(documentElement);
      expect(document.querySelector('[role="listbox"]')).toBe(popup);
      expect(select.attributes("aria-expanded")).toBe("true");
      expect(select.attributes("aria-activedescendant")).toBe(highlighted);
      expect(draft.element).toHaveProperty("value", "user draft");
    } finally { footer.unmount(); tools.unmount(); vi.useRealTimers(); vi.unstubAllGlobals(); }
  });

  it("requests dialog dismissal only after the self-host sync login reaches connected", async () => {
    const extensions = usePluginExtensionsStore();
    const context: PluginExtensionTargetContext = {
      targetId: "app.page", surfaceKind: "page", contextHandle: "sync-login", targetRevision: "1", displayLabel: "Sync",
    };
    const contribution: PluginUiContribution = {
      pluginId: "com.norishell.self-host-sync", pluginName: "Sync", artifactFingerprintSha256: "a".repeat(64),
      packageSha256: "b".repeat(64), instanceGeneration: "1", stateVersion: "1", contributionRevision: "1",
      target: context,
      document: { schemaVersion: 1, rootNodeId: "login", nodes: [{
        kind: "button", nodeId: "login", actionId: "sync.login", label: "Log in", icon: null,
        variant: "primary", disabled: false,
      }] },
    };
    vi.spyOn(extensions, "acquireTarget").mockResolvedValue({ key: "sync-login", context });
    vi.spyOn(extensions, "loadTargetContributions").mockResolvedValue([contribution]);
    vi.spyOn(extensions, "forContext").mockReturnValue([contribution]);
    vi.spyOn(extensions, "releaseTarget").mockResolvedValue();
    const invoke = vi.spyOn(extensions, "invokeAction").mockResolvedValue({
      contribution: { ...contribution, contributionRevision: "2" },
      sshSyncStatus: { profileId: "primary", accountState: "disconnected", operationState: "failed", stableErrorCode: "authorizationDenied" },
      hostDomOperations: null, clipboardText: null, hostApprovalId: null,
      terminalInputSuggestion: null, isolatedSurfaceOpened: false,
    } as never);
    const wrapper = mount(NvxPluginExtensionTarget, {
      props: { targetId: "app.page", selectedPluginId: contribution.pluginId },
      global: { plugins: [i18n, getActivePinia()!] },
    });
    await flushPromises();
    const requestAction = wrapper.getComponent(NvxPluginUiDocument).props("requestAction") as (
      actionId: string, fields: [],
    ) => Promise<{ closeDialogOnSuccess: boolean } | null>;
    expect((await requestAction("sync.login", []))?.closeDialogOnSuccess).toBe(false);
    invoke.mockResolvedValue({
      contribution: { ...contribution, contributionRevision: "2" },
      sshSyncStatus: { profileId: "primary", accountState: "connected", operationState: "idle", stableErrorCode: null },
      hostDomOperations: null, clipboardText: null, hostApprovalId: null,
      terminalInputSuggestion: null, isolatedSurfaceOpened: false,
    } as never);
    expect((await requestAction("sync.login", []))?.closeDialogOnSuccess).toBe(true);
    expect((await requestAction("sync.status", []))?.closeDialogOnSuccess).toBe(false);
    const tips = useTipsStore();
    invoke.mockResolvedValue({
      contribution: { ...contribution, contributionRevision: "2" },
      sshSyncStatus: { profileId: "primary", accountState: "connected", operationState: "needsReview", stableErrorCode: "vaultLocked", diagnosticCode: null },
      hostDomOperations: null, clipboardText: null, hostApprovalId: null,
      terminalInputSuggestion: null, isolatedSurfaceOpened: false,
    } as never);
    await requestAction("sync.status", []);
    expect(tips.items.at(-1)?.title).toContain("Vault 已锁定");
    invoke.mockResolvedValue({
      contribution: { ...contribution, contributionRevision: "2" },
      sshSyncStatus: { profileId: "primary", accountState: "connected", operationState: "needsReview", stableErrorCode: null, diagnosticCode: null },
      hostDomOperations: null, clipboardText: null, hostApprovalId: null,
      terminalInputSuggestion: null, isolatedSurfaceOpened: false,
    } as never);
    await requestAction("sync.status", []);
    expect(tips.items.at(-1)?.title).toContain("请在同步页继续审阅");
    invoke.mockResolvedValue({
      contribution: { ...contribution, contributionRevision: "2" },
      sshSyncStatus: { profileId: "primary", accountState: "connected", operationState: "failed", stableErrorCode: "remoteRequestRejected", diagnosticCode: null, httpStatus: 404 },
      hostDomOperations: null, clipboardText: null, hostApprovalId: null,
      terminalInputSuggestion: null, isolatedSurfaceOpened: false,
    } as never);
    await requestAction("sync.status", []);
    expect(tips.items.at(-1)?.title).toContain("HTTP 404");
    invoke.mockResolvedValue({
      contribution: { ...contribution, contributionRevision: "2" },
      sshSyncStatus: { profileId: "primary", accountState: "disconnected", operationState: "failed", stableErrorCode: "accountNotConnected", diagnosticCode: null },
      hostDomOperations: null, clipboardText: null, hostApprovalId: null,
      terminalInputSuggestion: null, isolatedSurfaceOpened: false,
    } as never);
    await requestAction("sync.status", []);
    expect(tips.items.at(-1)?.title).toContain("请先在同步页登录");
    wrapper.unmount();
  });

});
