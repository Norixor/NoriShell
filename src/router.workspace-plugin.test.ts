import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";

const shell = vi.hoisted(() => ({
  tabView: false,
  label: "main",
  open: vi.fn(async () => undefined),
  navigation: [] as unknown[],
  load: vi.fn(async () => undefined),
}));

vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true }));
vi.mock("./workspace-window-context", () => ({ isWorkspaceTabView: () => shell.tabView }));
vi.mock("./workspace-tab-windows", () => ({ workspaceWindowLabel: () => shell.label }));
vi.mock("./workspace-tab-view-shell", () => ({ openManagedPluginPage: shell.open }));
vi.mock("./stores/pluginExtensions", () => ({
  usePluginExtensionsStore: () => ({ navigation: shell.navigation, loadNavigation: shell.load }),
}));

import { guardPluginPageRoute } from "./router";

const item = { pluginId: "org.example.sync", navigation: { pageId: "home", label: "Sync", icon: "cloud" } };
const path = "/plugin/org.example.sync/home";

describe("plugin Page Tab routes", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    shell.tabView = false;
    shell.label = "main";
    shell.navigation = [item];
    shell.open.mockClear();
    shell.load.mockClear();
  });

  it("lets the page's own Tab WebView render it", async () => {
    shell.tabView = true;
    expect(await guardPluginPageRoute(path)).toBeUndefined();
    expect(shell.open).not.toHaveBeenCalled();
  });

  it("opens the Page Tab instead of rendering the page in a shell", async () => {
    expect(await guardPluginPageRoute(path)).toBe(false);
    expect(shell.open).toHaveBeenCalledWith(item);
  });

  it("gives an initial navigation a shell page to land on", async () => {
    expect(await guardPluginPageRoute(path, true)).toBe("/terminal");
    shell.label = "workspace-a";
    expect(await guardPluginPageRoute(path, true)).toBe("/workspace-window");
  });

  it("loads navigation once and falls back to Plugins for an unknown page", async () => {
    shell.navigation = [];
    expect(await guardPluginPageRoute(path)).toBe("/plugins");
    expect(shell.load).toHaveBeenCalledOnce();
    expect(shell.open).not.toHaveBeenCalled();
  });
});
