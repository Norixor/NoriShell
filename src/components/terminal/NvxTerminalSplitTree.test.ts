import { mount } from "@vue/test-utils";
import { h } from "vue";
import { describe, expect, it, vi } from "vitest";

import NvxTerminalSplitTree from "./NvxTerminalSplitTree.vue";
import { createTerminalPane, splitTerminalPane } from "./terminalLayout";

describe("NvxTerminalSplitTree", () => {
  it("overlays the horizontal separator without reserving a visible layout gap", () => {
    const node = splitTerminalPane(
      createTerminalPane("pane-a"),
      "pane-a",
      "horizontal",
      "pane-b",
      "split-a",
    );
    const wrapper = mount(NvxTerminalSplitTree, {
      props: { node, activePaneId: "pane-b", separatorLabel: "调整 Pane 大小" },
    });

    const panes = wrapper.findAll(".nvx-terminal-split-tree__pane");
    expect(panes[0]?.attributes("style")).toContain("width: 50%");
    expect(panes[1]?.attributes("style")).toContain("left: 50%");
    expect(panes[1]?.attributes("style")).toContain("width: 50%");

    const separator = wrapper.get(".nvx-terminal-split-tree__separator--horizontal");
    expect(separator.attributes("style")).toContain("left: calc(50% - 2.5px)");
    expect(separator.attributes("style")).toContain("width: calc(0% + 5px)");
  });

  it("keeps keyboard resize behavior on the overlaid separator", async () => {
    const node = splitTerminalPane(
      createTerminalPane("pane-a"),
      "pane-a",
      "vertical",
      "pane-b",
      "split-a",
    );
    const wrapper = mount(NvxTerminalSplitTree, {
      props: { node, activePaneId: "pane-b", separatorLabel: "调整 Pane 大小" },
    });

    const separator = wrapper.get(".nvx-terminal-split-tree__separator--vertical");
    expect(separator.attributes("style")).toContain("top: calc(50% - 2.5px)");
    expect(separator.attributes("style")).toContain("height: calc(0% + 5px)");

    await separator.trigger("keydown", { key: "ArrowDown" });
    expect(wrapper.emitted("resize")).toEqual([["split-a", 0.55]]);
  });

  it("disables only the split direction that cannot keep two minimum-size Panes", async () => {
    const bounds = vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue({
      width: 700,
      height: 600,
      top: 0,
      right: 700,
      bottom: 600,
      left: 0,
      x: 0,
      y: 0,
      toJSON: () => ({}),
    });
    const wrapper = mount(NvxTerminalSplitTree, {
      props: {
        node: createTerminalPane("pane-a"),
        activePaneId: "pane-a",
        separatorLabel: "调整 Pane 大小",
      },
      slots: {
        pane: ({ canSplitHorizontal, canSplitVertical }) => h("div", {
          class: "split-availability",
          "data-horizontal": String(canSplitHorizontal),
          "data-vertical": String(canSplitVertical),
        }),
      },
    });
    await wrapper.vm.$nextTick();

    expect(wrapper.get(".split-availability").attributes("data-horizontal")).toBe("false");
    expect(wrapper.get(".split-availability").attributes("data-vertical")).toBe("true");
    bounds.mockRestore();
  });

  it("allows a third horizontal Pane when the full workspace can rebalance to three minimums", async () => {
    const bounds = vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue({
      width: 1200,
      height: 600,
      top: 0,
      right: 1200,
      bottom: 600,
      left: 0,
      x: 0,
      y: 0,
      toJSON: () => ({}),
    });
    const node = splitTerminalPane(
      createTerminalPane("pane-a"),
      "pane-a",
      "horizontal",
      "pane-b",
      "split-a",
    );
    const wrapper = mount(NvxTerminalSplitTree, {
      props: { node, activePaneId: "pane-b", separatorLabel: "调整 Pane 大小" },
      slots: {
        pane: ({ pane, canSplitHorizontal }) => h("div", {
          class: "split-availability",
          "data-pane": pane.paneId,
          "data-horizontal": String(canSplitHorizontal),
        }),
      },
    });
    await wrapper.vm.$nextTick();

    expect(wrapper.findAll(".split-availability").map(
      (pane) => pane.attributes("data-horizontal"),
    )).toEqual(["true", "true"]);
    bounds.mockRestore();
  });
});
