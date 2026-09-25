import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { PluginSshSyncBrowserSnapshot, PluginUiContribution } from "../../core-api/generated/core-api";
import { i18n } from "../../locales";
import NvxPluginSshSyncBrowser from "./NvxPluginSshSyncBrowser.vue";
import NvxPluginUiDocument from "./NvxPluginUiDocument.vue";

const mocks = vi.hoisted(() => ({ read: vi.fn(), listen: vi.fn(), prepare: vi.fn(), stop: vi.fn() }));
vi.mock("../../core-api/client", () => ({ readPluginSshSyncBrowser: mocks.read }));
vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listen }));
vi.mock("../../plugins/hostDomBroker", () => ({ preparePluginProtectedMount: mocks.prepare }));

function contribution(): PluginUiContribution {
  return {
    pluginId: "org.fixture", pluginName: "Fixture", artifactFingerprintSha256: "a".repeat(64),
    packageSha256: "b".repeat(64), instanceGeneration: "1", stateVersion: "2", contributionRevision: "3",
    target: { targetId: "app.page", surfaceKind: "page", contextHandle: "context", targetRevision: "4", displayLabel: null },
    document: {
      schemaVersion: 1, rootNodeId: "browser", nodes: [
        { kind: "sshSyncBrowser", nodeId: "browser", profileId: "primary", children: ["summary", "name", "run"] },
        { kind: "text", nodeId: "summary", text: "Plugin summary", style: "heading", tone: "neutral" },
        { kind: "textField", nodeId: "name", fieldId: "name", label: "Name", value: "value", placeholder: null, fieldKind: "text", required: false, disabled: false },
        { kind: "button", nodeId: "run", actionId: "run", label: "Run", icon: null, variant: "primary", disabled: false },
      ],
    },
  };
}

function snapshot(overrides: Partial<PluginSshSyncBrowserSnapshot> = {}): PluginSshSyncBrowserSnapshot {
  return {
    state: "ready", profileId: "primary", cacheRevision: "1", hostCount: 2, credentialCount: 1, desktopProfileCount: 1,
    hostRowsOmitted: 0, credentialRowsOmitted: 0, desktopProfileRowsOmitted: 0, remoteUpdatedAtUnixMs: null,
    hosts: [
      { rowId: "h1", label: "Production", address: "192.0.2.10", port: 22, username: "deploy", tags: ["prod"], updatedAtUnixMs: 1727000000000 },
      { rowId: "h2", label: "Staging", address: "2001:db8::2", port: 2222, username: null, tags: [], updatedAtUnixMs: null },
    ],
    credentials: [{ rowId: "c1", label: "Deploy key", materialKind: "privateKey", hostRowIds: ["h1"], desktopProfileRowIds: ["d1"], updatedAtUnixMs: 1727000000000 }],
    desktopProfiles: [{ rowId: "d1", label: "Production desktop", protocol: "rdp", address: "192.0.2.20", port: 3389, username: "administrator", domain: "EXAMPLE", updatedAtUnixMs: 1727000000000 }],
    ...overrides,
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

let wrappers: VueWrapper[] = [];
function render() {
  const wrapper = mount(NvxPluginSshSyncBrowser, {
    props: { contribution: contribution(), nodeId: "browser", profileId: "primary" },
    slots: { default: "Plugin summary" }, global: { plugins: [i18n] },
  });
  wrappers.push(wrapper);
  return wrapper;
}
function invalidate(payload: { pluginId?: string | null; profileId?: string | null }) {
  const handler = mocks.listen.mock.calls[0]?.[1];
  handler({ payload });
}
async function tab(wrapper: VueWrapper, index: number) {
  await wrapper.findAll(".ssh-sync-browser__nav-item")[index]!.trigger("click");
}

beforeEach(() => {
  vi.resetAllMocks();
  i18n.global.locale.value = "en";
  mocks.listen.mockResolvedValue(mocks.stop);
  mocks.read.mockResolvedValue(snapshot());
});
afterEach(() => {
  for (const wrapper of wrappers) wrapper.unmount();
  wrappers = [];
});

describe("NvxPluginSshSyncBrowser", () => {
  it("protects the empty root before subscribing and reading the exact Core fence", async () => {
    const wait = deferred<PluginSshSyncBrowserSnapshot>();
    mocks.read.mockReturnValue(wait.promise);
    mocks.prepare.mockImplementation((root: Element) => {
      expect(root.hasAttribute("data-plugin-protected")).toBe(true);
      expect(root.textContent).not.toContain("Production");
    });
    const wrapper = render();
    expect(wrapper.find("[data-plugin-protected]").exists()).toBe(true);
    await flushPromises();
    expect(mocks.prepare.mock.invocationCallOrder[0]).toBeLessThan(mocks.listen.mock.invocationCallOrder[0]!);
    expect(mocks.listen.mock.invocationCallOrder[0]).toBeLessThan(mocks.read.mock.invocationCallOrder[0]!);
    expect(mocks.read).toHaveBeenCalledWith({
      pluginId: "org.fixture", artifactFingerprintSha256: "a".repeat(64), expectedPackageSha256: "b".repeat(64),
      instanceGeneration: "1", expectedStateVersion: "2", expectedContributionRevision: "3",
      targetId: "app.page", contextHandle: "context", expectedTargetRevision: "4", nodeId: "browser",
    });
    await tab(wrapper, 1);
    expect(wrapper.text()).toContain("Loading");
    expect(wrapper.findAll(".ssh-sync-browser__count").map((node) => node.text())).toEqual(["—", "—", "—"]);
    wait.resolve(snapshot());
    await flushPromises();
    expect(wrapper.text()).toContain("Production");
  });

  it("keeps search, selection, credential associations, and pagination local", async () => {
    const wrapper = render();
    await flushPromises();
    await tab(wrapper, 1);
    expect(wrapper.text()).toContain("[2001:db8::2]:2222");
    await wrapper.get("input").setValue("prod");
    expect(wrapper.findAll("tbody tr")).toHaveLength(1);
    await wrapper.get("tbody button").trigger("click");
    expect(wrapper.get("aside").text()).toContain("deploy");
    expect(wrapper.get("aside").text()).toContain("prod");
    await tab(wrapper, 2);
    await wrapper.get("tbody button").trigger("click");
    expect(wrapper.get("aside").text()).toContain("Private key");
    expect(wrapper.get("aside").text()).toContain("Production");
    expect(wrapper.get("aside").text()).toContain("Production desktop");
    await tab(wrapper, 3);
    expect(wrapper.text()).toContain("RDP");
    await wrapper.get("tbody button").trigger("click");
    expect(wrapper.get("aside").text()).toContain("administrator");
    expect(wrapper.get("aside").text()).toContain("EXAMPLE");
    expect(wrapper.emitted("action")).toBeUndefined();
    expect(wrapper.emitted("field")).toBeUndefined();
    expect(mocks.read).toHaveBeenCalledTimes(1);
    expect(wrapper.html()).not.toContain("SecretRef");
  });

  it("shows real totals and explicitly identifies bounded omitted rows and associations", async () => {
    mocks.read.mockResolvedValue(snapshot({ hostCount: 12, hostRowsOmitted: 10, desktopProfileCount: 12, desktopProfileRowsOmitted: 10,
      credentials: [
        { rowId: "c1", label: "Deploy key", materialKind: "privateKey", hostRowIds: ["h1"], desktopProfileRowIds: ["d1"], updatedAtUnixMs: 1727000000000 },
        { rowId: "c2", label: "Other key", materialKind: "privateKey", hostRowIds: [], desktopProfileRowIds: [], updatedAtUnixMs: null },
      ], credentialCount: 2,
    }));
    const wrapper = render();
    await flushPromises();
    expect(wrapper.findAll(".ssh-sync-browser__count").map((node) => node.text())).toEqual(["12", "2", "12"]);
    await tab(wrapper, 1);
    expect(wrapper.get(".ssh-sync-browser__omitted").text()).toContain("10");
    await tab(wrapper, 2);
    await wrapper.get("tbody button").trigger("click");
    expect(wrapper.get("aside").text()).toContain("Production");
    expect(wrapper.get("aside").text()).toContain("Only associations to loaded hosts");
    expect(wrapper.get("aside").text()).toContain("Only associations to loaded remote desktops");
    await wrapper.findAll("tbody button")[1]!.trigger("click");
    expect(wrapper.get("aside").text()).toContain("No associated hosts are listed in this view.");
    await tab(wrapper, 3);
    expect(wrapper.get(".ssh-sync-browser__omitted").text()).toContain("10");
  });

  it("sorts names and real item timestamps locally, retaining unknown timestamps last", async () => {
    const wrapper = render();
    await flushPromises();
    await tab(wrapper, 1);
    expect(wrapper.findAll("tbody tr")[0]!.text()).toContain("Production");
    await wrapper.findAll("thead button")[0]!.trigger("click");
    expect(wrapper.findAll("tbody tr")[0]!.text()).toContain("Staging");
    expect(wrapper.findAll("th")[0]!.attributes("aria-sort")).toBe("descending");
    await wrapper.findAll("thead button")[3]!.trigger("click");
    expect(wrapper.findAll("tbody tr")[0]!.text()).toContain("Production");
    expect(wrapper.findAll("tbody tr")[1]!.findAll("td")[4]!.text()).toBe("—");
    await wrapper.findAll("thead button")[3]!.trigger("click");
    expect(wrapper.findAll("tbody tr")[0]!.text()).toContain("Production");
    expect(wrapper.text()).toContain("Cloud copy");
    expect(wrapper.text()).not.toContain("Online");
    expect(mocks.read).toHaveBeenCalledTimes(1);
    expect(wrapper.emitted("action")).toBeUndefined();
  });

  it("paginates cached rows without requests or plugin actions", async () => {
    const hosts = Array.from({ length: 26 }, (_, index) => ({
      rowId: `h${index}`, label: `Host ${index}`, address: "192.0.2.1", port: 22, username: null, tags: [], updatedAtUnixMs: null,
    }));
    mocks.read.mockResolvedValue(snapshot({ hosts, hostCount: 26 }));
    const wrapper = render();
    await flushPromises();
    await tab(wrapper, 1);
    expect(wrapper.findAll("tbody tr")).toHaveLength(25);
    await wrapper.findAll(".ssh-sync-browser__pagination button")[1]!.trigger("click");
    expect(wrapper.findAll("tbody tr")).toHaveLength(1);
    expect(wrapper.get("tbody").text()).toContain("Host 25");
    expect(mocks.read).toHaveBeenCalledTimes(1);
  });

  it.each(["notLoaded", "needsCreation", "needsUnlock", "permissionDenied", "failed", "empty"] as const)("keeps %s protected without representing missing data as zero", async (state) => {
    mocks.read.mockResolvedValue(snapshot({
      state, hosts: [], credentials: [], desktopProfiles: [], hostCount: 0, credentialCount: 0,
      desktopProfileCount: 0,
    }));
    const wrapper = render();
    await flushPromises();
    await tab(wrapper, 1);
    expect(wrapper.find("table").exists()).toBe(false);
    expect(wrapper.get("[data-plugin-protected] [role=status]").text()).not.toBe("");
    expect(wrapper.findAll(".ssh-sync-browser__count")[0]!.text()).toBe(state === "empty" ? "0" : "—");
  });

  it("reloads on matching invalidation and ignores stale reads across the pre/post refresh events", async () => {
    const wrapper = render();
    await flushPromises();
    await tab(wrapper, 1);
    invalidate({ pluginId: "another-plugin" });
    await flushPromises();
    expect(wrapper.text()).toContain("Production");
    invalidate({ pluginId: "org.fixture", profileId: "another-profile" });
    await flushPromises();
    expect(wrapper.text()).toContain("Production");
    expect(mocks.read).toHaveBeenCalledTimes(1);

    const beforeRefresh = deferred<PluginSshSyncBrowserSnapshot>();
    const afterRefresh = deferred<PluginSshSyncBrowserSnapshot>();
    mocks.read.mockReturnValueOnce(beforeRefresh.promise).mockReturnValueOnce(afterRefresh.promise);
    invalidate({ pluginId: "org.fixture", profileId: "primary" });
    await flushPromises();
    expect(wrapper.text()).not.toContain("Production");
    expect(mocks.read).toHaveBeenCalledTimes(2);

    invalidate({ pluginId: null, profileId: null });
    await flushPromises();
    expect(mocks.read).toHaveBeenCalledTimes(3);
    beforeRefresh.resolve(snapshot());
    await flushPromises();
    expect(wrapper.text()).not.toContain("Production");
    afterRefresh.resolve(snapshot({
      cacheRevision: "2", hostCount: 1, hosts: [
        { rowId: "h3", label: "Refreshed", address: "192.0.2.30", port: 22, username: null, tags: [], updatedAtUnixMs: null },
      ],
    }));
    await flushPromises();
    expect(wrapper.text()).toContain("Refreshed");
    expect(wrapper.text()).not.toContain("Production");
  });

  it.each(["packageSha256", "instanceGeneration", "contributionRevision", "stateVersion"] as const)("clears %s changes and discards an obsolete response", async (field) => {
    const first = deferred<PluginSshSyncBrowserSnapshot>();
    const second = deferred<PluginSshSyncBrowserSnapshot>();
    mocks.read.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
    const wrapper = render();
    await flushPromises();
    await tab(wrapper, 1);
    await wrapper.setProps({ contribution: { ...contribution(), [field]: "next" } });
    first.resolve(snapshot());
    await flushPromises();
    expect(wrapper.text()).not.toContain("Production");
    second.resolve(snapshot({ hosts: [], credentials: [], hostCount: 0, credentialCount: 0 }));
    await flushPromises();
    expect(wrapper.text()).not.toContain("Production");
    expect(mocks.read).toHaveBeenCalledTimes(2);
  });

  it("clears an already rendered projection on a profile switch and rejects mismatched profiles", async () => {
    const wrapper = render();
    await flushPromises();
    await tab(wrapper, 1);
    expect(wrapper.text()).toContain("Production");
    const wait = deferred<PluginSshSyncBrowserSnapshot>();
    mocks.read.mockReturnValue(wait.promise);
    await wrapper.setProps({ profileId: "other" });
    expect(wrapper.text()).not.toContain("Production");
    wait.resolve(snapshot());
    await flushPromises();
    expect(wrapper.text()).not.toContain("Production");
    expect(wrapper.find("table").exists()).toBe(false);
  });

  it("fails closed when invalidation subscription fails and cleans up on unmount", async () => {
    mocks.listen.mockRejectedValueOnce(new Error("not available"));
    const failed = render();
    await flushPromises();
    await tab(failed, 1);
    expect(mocks.read).not.toHaveBeenCalled();
    expect(failed.find("table").exists()).toBe(false);
    const wait = deferred<PluginSshSyncBrowserSnapshot>();
    mocks.read.mockReturnValue(wait.promise);
    const wrapper = render();
    await flushPromises();
    wrapper.unmount();
    wait.resolve(snapshot());
    await flushPromises();
    expect(mocks.stop).toHaveBeenCalledTimes(1);
  });

  it("preserves declared overview fields but never forwards browser interactions to the document", async () => {
    const wrapper = mount(NvxPluginUiDocument, { props: { contribution: contribution(), busy: false }, global: { plugins: [i18n] } });
    wrappers.push(wrapper);
    await flushPromises();
    expect(wrapper.find("h1").exists()).toBe(false);
    await wrapper.get("input").setValue("summary choice");
    await tab(wrapper, 1);
    await wrapper.get("input").setValue("prod");
    await wrapper.get("tbody button").trigger("click");
    expect(wrapper.emitted("action")).toBeUndefined();
    await tab(wrapper, 0);
    await wrapper.get(".ssh-sync-browser__overview button").trigger("click");
    expect(wrapper.emitted("action")).toEqual([["run", [{ fieldId: "name", value: "summary choice" }]]]);
  });

  it("does not render the browser on an inline contribution", async () => {
    const page = contribution();
    page.target.surfaceKind = "inline";
    const wrapper = mount(NvxPluginUiDocument, { props: { contribution: page, busy: false }, global: { plugins: [i18n] } });
    wrappers.push(wrapper);
    await flushPromises();
    expect(wrapper.find("[data-plugin-protected]").exists()).toBe(false);
    expect(mocks.read).not.toHaveBeenCalled();
  });
});
