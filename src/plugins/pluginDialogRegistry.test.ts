import { afterEach, describe, expect, it, vi } from "vitest";

import {
  listPluginDialogs,
  pluginDialogOwnerMatches,
  registerPluginDialog,
} from "./pluginDialogRegistry";

const owner = {
  pluginId: "com.example.dialog",
  packageSha256: "a".repeat(64),
  instanceGeneration: "1",
};

function appendDialog() {
  const dialog = document.createElement("section");
  dialog.setAttribute("role", "dialog");
  const content = document.createElement("div");
  dialog.append(content);
  document.body.append(dialog);
  return { dialog, content };
}

afterEach(() => {
  document.body.replaceChildren();
  listPluginDialogs();
});

describe("plugin dialog registry", () => {
  it("matches a registration only to its actual closest dialog ancestor", () => {
    const { dialog, content } = appendDialog();
    const nested = document.createElement("section");
    nested.setAttribute("role", "dialog");
    content.append(nested);
    const dispose = registerPluginDialog(content, owner, vi.fn());

    const states = listPluginDialogs();
    expect(states).toHaveLength(2);
    expect(states.find((state) => state.dialog === dialog)?.registration?.owner).toEqual(owner);
    expect(states.find((state) => state.dialog === nested)?.registration).toBeNull();
    dispose();
  });

  it("does not trust a plugin-protected DOM attribute", () => {
    const { dialog } = appendDialog();
    dialog.setAttribute("data-plugin-protected", "");

    expect(listPluginDialogs()).toEqual([{ dialog, registration: null }]);
  });

  it("keeps a replacement registration when an older disposer runs", () => {
    const { content, dialog } = appendDialog();
    const first = registerPluginDialog(content, owner, vi.fn());
    const replacementOwner = { ...owner, instanceGeneration: "2" };
    const second = registerPluginDialog(content, replacementOwner, vi.fn());

    first();
    expect(listPluginDialogs()).toEqual([{
      dialog,
      registration: expect.objectContaining({ owner: replacementOwner }),
    }]);
    expect(pluginDialogOwnerMatches(replacementOwner, owner)).toBe(false);

    second();
    expect(listPluginDialogs()).toEqual([{ dialog, registration: null }]);
  });

  it("removes disconnected content before a replacement can be inspected", () => {
    const { dialog, content } = appendDialog();
    registerPluginDialog(content, owner, vi.fn());
    content.remove();
    const replacement = document.createElement("div");
    dialog.append(replacement);
    const dispose = registerPluginDialog(replacement, { ...owner, instanceGeneration: "2" }, vi.fn());

    expect(listPluginDialogs()[0]?.registration?.content).toBe(replacement);
    dispose();
  });
});
