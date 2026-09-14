import { mount } from "@vue/test-utils";
import { nextTick } from "vue";
import { afterEach, describe, expect, it } from "vitest";

import type { PluginUiContribution, PluginUiNode } from "../../core-api/generated/core-api";
import { listPluginDialogs } from "../../plugins/pluginDialogRegistry";
import { NvxDialog } from "../ui";
import NvxPluginUiNode from "./NvxPluginUiNode.vue";

function contribution(instanceGeneration = "1"): PluginUiContribution {
  return {
    pluginId: "com.example.dialog",
    pluginName: "Dialog fixture",
    artifactFingerprintSha256: "1".repeat(64),
    packageSha256: "a".repeat(64),
    instanceGeneration,
    stateVersion: "1",
    contributionRevision: "1",
    target: {
      targetId: "app.page",
      surfaceKind: "page",
      contextHandle: "019d0000-0000-4000-8000-000000000001",
      targetRevision: "1",
      displayLabel: null,
    },
    document: {
      schemaVersion: 1,
      rootNodeId: "dialog",
      nodes: [],
    },
  };
}

const dialogNode: PluginUiNode = {
  kind: "dialog",
  nodeId: "dialog",
  title: "Sign in",
  description: null,
  triggerLabel: "Sign in",
  closeLabel: "Close sign in",
  children: [],
};

afterEach(() => {
  document.body.replaceChildren();
  listPluginDialogs();
});

describe("NvxPluginUiNode declarative dialog", () => {
  it("marks the host dialog protected and registers only its renderer-owned content", async () => {
    const current = contribution();
    const wrapper = mount(NvxPluginUiNode, {
      attachTo: document.body,
      props: {
        nodeId: "dialog",
        nodeById: { dialog: dialogNode },
        values: {},
        busy: false,
        contribution: current,
      },
    });

    await wrapper.get("button").trigger("click");
    await nextTick();
    expect(wrapper.findComponent(NvxDialog).props("pluginProtected")).toBe(true);

    const state = listPluginDialogs()[0];
    expect(state?.registration?.owner).toEqual({
      pluginId: current.pluginId,
      packageSha256: current.packageSha256,
      instanceGeneration: current.instanceGeneration,
    });
    expect(state?.registration?.content.closest('[role="dialog"], dialog[open]')).toBe(state?.dialog);

    await wrapper.setProps({ contribution: contribution("2") });
    await nextTick();
    expect(listPluginDialogs()[0]?.registration?.owner.instanceGeneration).toBe("2");

    listPluginDialogs()[0]?.registration?.close();
    await nextTick();
    expect(listPluginDialogs()).toEqual([]);

    wrapper.unmount();
    expect(listPluginDialogs()).toEqual([]);
  });
});
