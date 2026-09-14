import { beforeEach, describe, expect, it } from "vitest";

import {
  applyPluginHostDomOperations,
  capturePluginHostDomSnapshot,
  pluginHostDomEpoch,
  preparePluginProtectedMount,
  revertPluginHostDom,
} from "./hostDomBroker";

const CONTEXT = "019d0000-0000-4000-8000-000000000001";

describe("host DOM broker", () => {
  beforeEach(() => {
    revertPluginHostDom("com.norishell.fixture");
    document.body.innerHTML = `
      <div id="app">
        <main data-plugin-dom-surface><span id="visible">Visible text</span></main>
        <section><span id="unregistered">Unregistered text</span></section>
        <section data-plugin-dom-surface><button><span id="button-label">Danger action</span></button></section>
        <section data-plugin-protected><span id="secret">Vault password</span></section>
        <div class="xterm"><span id="terminal">terminal output</span></div>
      </div>
    `;
  });

  it("omits protected and terminal descendants from plugin snapshots", () => {
    const snapshot = capturePluginHostDomSnapshot(CONTEXT);
    expect(snapshot).not.toBeNull();
    const text = snapshot?.nodes.map((node) => node.directText).filter(Boolean);
    expect(text).toContain("Visible text");
    expect(text).not.toContain("Vault password");
    expect(text).not.toContain("terminal output");
    expect(text).not.toContain("Unregistered text");
    expect(text).not.toContain("Danger action");
  });

  it("does not issue an ancestor handle or aggregate labels around protected descendants", () => {
    document.querySelector("main")!.innerHTML = `
      <div id="protected-ancestor" aria-label="Cloud host list">
        <span id="safe-sibling">Safe sibling</span>
        <section data-plugin-protected><span>cloud.example</span></section>
      </div>
    `;

    const snapshot = capturePluginHostDomSnapshot(CONTEXT)!;
    expect(snapshot.nodes.map((node) => node.directText)).toContain("Safe sibling");
    expect(snapshot.nodes.flatMap((node) => node.attributes).map((attribute) => attribute.value))
      .not.toContain("Cloud host list");
    expect(snapshot.nodes.flatMap((node) => node.attributes).map((attribute) => attribute.value))
      .not.toContain("protected-ancestor");
    expect(snapshot.nodes.map((node) => node.directText)).not.toContain("cloud.example");
  });

  it("applies fenced reversible operations to retained elements", () => {
    const snapshot = capturePluginHostDomSnapshot(CONTEXT);
    const visible = snapshot?.nodes.find((node) => node.directText === "Visible text");
    expect(snapshot && visible).toBeTruthy();
    expect(applyPluginHostDomOperations("com.norishell.fixture", {
      contextHandle: CONTEXT,
      snapshotRevision: snapshot!.snapshotRevision,
      operations: [{ kind: "setText", nodeHandle: visible!.nodeHandle, text: "Plugin text" }],
    }, pluginHostDomEpoch("com.norishell.fixture"))).toBe(true);
    expect(document.getElementById("visible")?.textContent).toBe("Plugin text");

    revertPluginHostDom("com.norishell.fixture");
    expect(document.getElementById("visible")?.textContent).toBe("Visible text");
  });

  it("rejects stale or cross-context operation batches", () => {
    const snapshot = capturePluginHostDomSnapshot(CONTEXT)!;
    const visible = snapshot.nodes.find((node) => node.directText === "Visible text")!;
    expect(applyPluginHostDomOperations("com.norishell.fixture", {
      contextHandle: "019d0000-0000-4000-8000-000000000099",
      snapshotRevision: snapshot.snapshotRevision,
      operations: [{ kind: "setText", nodeHandle: visible.nodeHandle, text: "Changed" }],
    }, pluginHostDomEpoch("com.norishell.fixture"))).toBe(false);
    expect(document.getElementById("visible")?.textContent).toBe("Visible text");
  });

  it("rejects a late response after permission invalidation", () => {
    const pluginId = "com.norishell.fixture";
    const epoch = pluginHostDomEpoch(pluginId);
    const snapshot = capturePluginHostDomSnapshot(CONTEXT)!;
    const visible = snapshot.nodes.find((node) => node.directText === "Visible text")!;
    revertPluginHostDom(pluginId);
    expect(applyPluginHostDomOperations(pluginId, {
      contextHandle: CONTEXT,
      snapshotRevision: snapshot.snapshotRevision,
      operations: [{ kind: "setText", nodeHandle: visible.nodeHandle, text: "Changed" }],
    }, epoch)).toBe(false);
    expect(document.getElementById("visible")?.textContent).toBe("Visible text");
  });

  it("rejects ancestor hide and style writes once a protected child makes its snapshot stale", () => {
    document.querySelector("main")!.innerHTML = '<div id="plugin-ancestor"><span>Visible text</span></div>';
    const snapshot = capturePluginHostDomSnapshot(CONTEXT)!;
    const ancestor = snapshot.nodes.find((node) => (
      node.attributes.some((attribute) => attribute.name === "id" && attribute.value === "plugin-ancestor")
    ))!;
    const shell = document.createElement("aside");
    shell.id = "cloud-shell";
    shell.setAttribute("data-plugin-protected", "");
    document.getElementById("plugin-ancestor")!.append(shell);

    expect(applyPluginHostDomOperations("com.norishell.fixture", {
      contextHandle: CONTEXT,
      snapshotRevision: snapshot.snapshotRevision,
      operations: [{ kind: "setHidden", nodeHandle: ancestor.nodeHandle, hidden: true }],
    }, pluginHostDomEpoch("com.norishell.fixture"))).toBe(false);
    expect(applyPluginHostDomOperations("com.norishell.fixture", {
      contextHandle: CONTEXT,
      snapshotRevision: snapshot.snapshotRevision,
      operations: [{ kind: "setStyle", nodeHandle: ancestor.nodeHandle, property: "color", value: "red" }],
    }, pluginHostDomEpoch("com.norishell.fixture"))).toBe(false);
    const ancestorElement = document.getElementById("plugin-ancestor")!;
    expect(ancestorElement.hasAttribute("hidden")).toBe(false);
    expect((ancestorElement as HTMLElement).style.color).toBe("");
  });

  it("rejects handles for a protected target or ancestor, and after replace, move, or removal", () => {
    const operation = (snapshotRevision: string, nodeHandle: string) => applyPluginHostDomOperations(
      "com.norishell.fixture",
      {
        contextHandle: CONTEXT,
        snapshotRevision,
        operations: [{ kind: "setStyle", nodeHandle, property: "color", value: "red" }],
      },
      pluginHostDomEpoch("com.norishell.fixture"),
    );

    const protectedTarget = capturePluginHostDomSnapshot(CONTEXT)!;
    const protectedTargetNode = protectedTarget.nodes.find((node) => node.directText === "Visible text")!;
    document.getElementById("visible")!.setAttribute("data-plugin-protected", "");
    expect(operation(protectedTarget.snapshotRevision, protectedTargetNode.nodeHandle)).toBe(false);

    document.querySelector("main")!.innerHTML = '<span id="visible">Visible text</span>';
    const replaced = capturePluginHostDomSnapshot(CONTEXT)!;
    const replacedNode = replaced.nodes.find((node) => node.directText === "Visible text")!;
    document.getElementById("visible")!.replaceWith(document.createElement("span"));
    expect(operation(replaced.snapshotRevision, replacedNode.nodeHandle)).toBe(false);

    document.querySelector("main")!.innerHTML = '<span id="visible">Visible text</span>';
    const moved = capturePluginHostDomSnapshot(CONTEXT)!;
    const movedNode = moved.nodes.find((node) => node.directText === "Visible text")!;
    const protectedRoot = document.createElement("aside");
    protectedRoot.setAttribute("data-plugin-protected", "");
    document.getElementById("app")!.append(protectedRoot);
    protectedRoot.append(document.getElementById("visible")!);
    expect(operation(moved.snapshotRevision, movedNode.nodeHandle)).toBe(false);

    document.querySelector("main")!.innerHTML = '<span id="visible">Visible text</span>';
    const removed = capturePluginHostDomSnapshot(CONTEXT)!;
    const removedNode = removed.nodes.find((node) => node.directText === "Visible text")!;
    document.querySelector("main #visible")!.remove();
    expect(operation(removed.snapshotRevision, removedNode.nodeHandle)).toBe(false);
  });

  it("prepares a protected mount by restoring only intersecting plugin state and invalidating its snapshots", () => {
    const pluginId = "com.norishell.fixture";
    const snapshot = capturePluginHostDomSnapshot(CONTEXT)!;
    const visible = snapshot.nodes.find((node) => node.directText === "Visible text")!;
    expect(applyPluginHostDomOperations(pluginId, {
      contextHandle: CONTEXT,
      snapshotRevision: snapshot.snapshotRevision,
      operations: [
        { kind: "setAttribute", nodeHandle: visible.nodeHandle, name: "title", value: "Plugin title" },
        { kind: "setStyle", nodeHandle: visible.nodeHandle, property: "color", value: "red" },
        { kind: "setHidden", nodeHandle: visible.nodeHandle, hidden: true },
      ],
    }, pluginHostDomEpoch(pluginId))).toBe(true);

    const shell = document.createElement("aside");
    shell.id = "cloud-shell";
    shell.setAttribute("data-plugin-protected", "");
    document.getElementById("visible")!.append(shell);
    preparePluginProtectedMount(shell);

    const visibleElement = document.getElementById("visible")!;
    expect(visibleElement.getAttribute("title")).toBeNull();
    expect((visibleElement as HTMLElement).style.color).toBe("");
    expect(visibleElement.hasAttribute("hidden")).toBe(false);
    expect(shell.isConnected).toBe(true);
    expect(applyPluginHostDomOperations(pluginId, {
      contextHandle: CONTEXT,
      snapshotRevision: snapshot.snapshotRevision,
      operations: [{ kind: "setText", nodeHandle: visible.nodeHandle, text: "Stale" }],
    }, pluginHostDomEpoch(pluginId))).toBe(false);
  });

  it("preserves a later protected child while reverting a prior text modification", () => {
    const pluginId = "com.norishell.fixture";
    const snapshot = capturePluginHostDomSnapshot(CONTEXT)!;
    const visible = snapshot.nodes.find((node) => node.directText === "Visible text")!;
    expect(applyPluginHostDomOperations(pluginId, {
      contextHandle: CONTEXT,
      snapshotRevision: snapshot.snapshotRevision,
      operations: [{ kind: "setText", nodeHandle: visible.nodeHandle, text: "Plugin text" }],
    }, pluginHostDomEpoch(pluginId))).toBe(true);

    const shell = document.createElement("aside");
    shell.setAttribute("data-plugin-protected", "");
    shell.textContent = "cloud.example";
    document.getElementById("visible")!.append(shell);
    revertPluginHostDom(pluginId);

    expect(shell.isConnected).toBe(true);
    expect(shell.textContent).toBe("cloud.example");
    expect(document.getElementById("visible")?.firstChild?.textContent).toBe("Visible text");
  });
});
