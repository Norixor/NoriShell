import { describe, expect, it } from "vitest";

import {
  parseTerminalWorkspaceLayout,
  projectTerminalWorkspaceLayout,
} from "./terminalWorkspaceLayout";

interface TestWorkspaceLayout {
  schemaVersion: number;
  activeTabId: string | null;
  tabs: Array<{
    tabId: string;
    activePaneId: string;
    layout: Record<string, unknown>;
    panes: Array<Record<string, unknown>>;
  }>;
}

function validLayout(): TestWorkspaceLayout {
  return {
    schemaVersion: 1,
    activeTabId: "tab-1",
    tabs: [{
      tabId: "tab-1",
      activePaneId: "pane-2",
      layout: {
        kind: "split",
        splitId: "split-1",
        direction: "horizontal",
        ratio: 0.4,
        first: { kind: "pane", paneId: "pane-1", terminalId: "pane-1" },
        second: { kind: "pane", paneId: "pane-2", terminalId: "pane-2" },
      },
      panes: [
        { kind: "sshHost", paneId: "pane-1", label: "Production", hostId: "host-1" },
        { kind: "local", paneId: "pane-2", label: "zsh" },
      ],
    }],
  };
}

describe("terminalWorkspaceLayout", () => {
  it("restores only the bounded non-secret workspace projection", () => {
    expect(parseTerminalWorkspaceLayout(validLayout())).toEqual(validLayout());
  });

  it("projects session panes without session, credential, or connection state", () => {
    const livePane = {
      kind: "session" as const,
      paneId: "pane-1",
      label: "Production",
      target: { kind: "host" as const, hostId: "host-1", expectedHostStateVersion: "42" },
      sessionId: "must-not-persist",
      credentialRefId: "must-not-persist",
      state: "running",
    };
    const projected = projectTerminalWorkspaceLayout([{
      tabId: "tab-1",
      activePaneId: "pane-1",
      layout: { kind: "pane", paneId: "pane-1", terminalId: "must-not-persist-session" },
      panes: [livePane],
    }], "tab-1");

    expect(projected).toEqual({
      schemaVersion: 1,
      activeTabId: "tab-1",
      tabs: [{
        tabId: "tab-1",
        activePaneId: "pane-1",
        layout: { kind: "pane", paneId: "pane-1", terminalId: "pane-1" },
        panes: [{ kind: "sshHost", paneId: "pane-1", label: "Production", hostId: "host-1" }],
      }],
    });
    expect(JSON.stringify(projected)).not.toMatch(
      /sessionId|credentialRefId|running|42|must-not-persist-session/u,
    );
  });

  it("persists only the Telnet endpoint and never a socket or risk acknowledgement", () => {
    const projected = projectTerminalWorkspaceLayout([{
      tabId: "tab-telnet",
      activePaneId: "pane-telnet",
      layout: { kind: "pane", paneId: "pane-telnet", terminalId: "socket-must-not-persist" },
      panes: [{
        kind: "telnet" as const,
        paneId: "pane-telnet",
        label: "legacy.example.test:23",
        endpoint: { address: "legacy.example.test", port: 23 },
      }],
    }], "tab-telnet");

    expect(projected.tabs[0]?.panes[0]).toEqual({
      kind: "telnet",
      paneId: "pane-telnet",
      label: "legacy.example.test:23",
      address: "legacy.example.test",
      port: 23,
    });
    expect(JSON.stringify(projected)).not.toMatch(/socket|acceptsCleartext|sessionId/u);
    expect(parseTerminalWorkspaceLayout(projected)).toEqual(projected);
  });

  it("accepts an empty workspace only with a null active tab", () => {
    expect(parseTerminalWorkspaceLayout({ schemaVersion: 1, activeTabId: null, tabs: [] }))
      .toEqual({ schemaVersion: 1, activeTabId: null, tabs: [] });
    expect(parseTerminalWorkspaceLayout({ schemaVersion: 1, activeTabId: "tab-1", tabs: [] }))
      .toBeNull();
  });

  it("rejects missing, duplicate, or mismatched pane identities", () => {
    const missing = validLayout();
    missing.tabs[0]!.panes.pop();
    expect(parseTerminalWorkspaceLayout(missing)).toBeNull();

    const duplicate = validLayout();
    duplicate.tabs.push({ ...duplicate.tabs[0]!, tabId: "tab-2" });
    expect(parseTerminalWorkspaceLayout(duplicate)).toBeNull();

    const inactive = validLayout();
    inactive.tabs[0]!.activePaneId = "pane-missing";
    expect(parseTerminalWorkspaceLayout(inactive)).toBeNull();
  });

  it("rejects secret-bearing or unsafe pane shapes by accepting only explicit kinds", () => {
    const credential = validLayout();
    credential.tabs[0]!.panes[0] = {
      kind: "session",
      paneId: "pane-1",
      label: "Production",
      credentialRefId: "credential-1",
    } as never;
    expect(parseTerminalWorkspaceLayout(credential)).toBeNull();

    const endpoint = validLayout();
    endpoint.tabs[0]!.panes[0] = {
      kind: "sshQuickConnect",
      paneId: "pane-1",
      label: "unsafe",
      endpoint: { address: "host\nname", port: 22, username: null },
    } as never;
    expect(parseTerminalWorkspaceLayout(endpoint)).toBeNull();
  });

  it("rejects corrupt layout trees, unsupported versions, and oversized workspaces", () => {
    const corrupt = validLayout();
    corrupt.tabs[0]!.layout.ratio = 2;
    expect(parseTerminalWorkspaceLayout(corrupt)).toBeNull();
    expect(parseTerminalWorkspaceLayout({ ...validLayout(), schemaVersion: 2 })).toBeNull();

    const oversized = validLayout();
    oversized.tabs = Array.from({ length: 33 }, (_, index) => ({
      ...validLayout().tabs[0]!,
      tabId: `tab-${index}`,
      activePaneId: `pane-${index}`,
      layout: { kind: "pane", paneId: `pane-${index}`, terminalId: "" },
      panes: [{ kind: "launcher", paneId: `pane-${index}`, label: "New connection" }],
    }));
    expect(parseTerminalWorkspaceLayout(oversized)).toBeNull();
  });
});

describe("plugin protocol workspace persistence", () => {
  it("persists only profile fields and no runtime session or grant", () => {
    const projection = projectTerminalWorkspaceLayout([{
      tabId: "tab", activePaneId: "pane", layout: { kind: "pane", paneId: "pane", terminalId: "runtime" },
      panes: [{ kind: "plugin", paneId: "pane", label: "Serial", profile: {
        pluginId: "provider.test", providerId: "serial", schemaHash: "schema", configuration: { baud: 115200 },
        ...{ sessionId: "session", authorizationToken: "private" },
      } }],
    }], "tab");
    expect(JSON.stringify(projection)).not.toMatch(/runtime|sessionId|authorizationToken|private/);
    expect(parseTerminalWorkspaceLayout(projection)?.tabs[0]?.panes[0]).toEqual({
      kind: "plugin", paneId: "pane", label: "Serial", pluginId: "provider.test", providerId: "serial",
      schemaHash: "schema", configuration: { baud: 115200 },
    });
  });
  it("rejects nested configuration and invalid schema metadata", () => {
    const layout = { schemaVersion: 1, activeTabId: "tab", tabs: [{
      tabId: "tab", activePaneId: "pane", layout: { kind: "pane", paneId: "pane", terminalId: "pane" },
      panes: [{ kind: "plugin", paneId: "pane", label: "Serial", pluginId: "provider.test", providerId: "serial",
        schemaHash: "schema", configuration: { nested: { unexpected: true } } }],
    }] };
    expect(parseTerminalWorkspaceLayout(layout)).toBeNull();
  });
});
