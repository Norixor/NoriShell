import { mount } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { ServerOverviewCard } from "../../core-api/generated/core-api";
import { i18n } from "../../locales";
import NvxSelect from "../ui/NvxSelect.vue";
import NvxOverviewPanel from "./NvxOverviewPanel.vue";

function card(
  label: string,
  state: ServerOverviewCard["connectionState"],
  index = label === "Alpha" ? 1 : 2,
  group: { groupId: string; label: string } | null = null,
): ServerOverviewCard {
  const hostId = `019d0000-0000-7000-8000-${String(index).padStart(12, "0")}`;
  return {
    catalogEntry: {
      host: {
        hostId,
        label,
        address: `${label.toLowerCase()}.example`,
        normalizedAddress: `${label.toLowerCase()}.example`,
        port: 22,
        username: "deploy",
        identityId: null,
        favorite: false,
        hasReadyCredential: false,
        stateVersion: "1",
      },
      group: group ? { ...group, stateVersion: "1" } : null,
      tags: [],
      recentConnection: null,
    },
    monitoringPolicy: {
      hostId,
      revision: "1",
      policy: {
        enabled: false,
        sampleIntervalSeconds: 15,
        sampleTimeoutSeconds: 5,
        diskMountIds: ["root"],
        networkInterfaceIds: ["aggregateNonLoopback"],
      },
    },
    connectionState: state,
    terminalCounts: { connecting: 0, running: 0, lost: 0, failed: 0 },
    sftpCounts: { connecting: 0, running: 0, lost: 0, failed: 0 },
    forwardCounts: { connecting: 0, running: 0, lost: 0, failed: 0 },
    metricsCounts: { connecting: 0, running: 0, lost: 0, failed: 0 },
    terminalSessionIds: [],
    metricsSession: null,
  };
}

describe("NvxOverviewPanel", () => {
  let resizeCallback: ResizeObserverCallback | null;
  const observe = vi.fn();
  const disconnect = vi.fn();

  beforeEach(() => {
    i18n.global.locale.value = "zh-CN";
    resizeCallback = null;
    observe.mockReset();
    disconnect.mockReset();
    vi.stubGlobal("ResizeObserver", class {
      constructor(callback: ResizeObserverCallback) {
        resizeCallback = callback;
      }

      observe = observe;
      disconnect = disconnect;
    });
  });

  afterEach(() => vi.unstubAllGlobals());

  it("filters without omitting saved Hosts from the source snapshot", async () => {
    const snapshot = {
      snapshotRevision: "1",
      cards: [card("Alpha", "connected"), card("Beta", "disconnected")],
    };
    const wrapper = mount(NvxOverviewPanel, {
      props: { snapshot },
      global: { plugins: [i18n] },
    });
    expect(wrapper.findAll(".nvx-overview-panel__card")).toHaveLength(2);

    await wrapper.get("#overview-search").setValue("beta.example");
    expect(wrapper.findAll(".nvx-overview-panel__card")).toHaveLength(1);
    expect(wrapper.text()).toContain("Beta");
    expect(snapshot.cards).toHaveLength(2);
  });

  it("keeps Card order stable when monitoring-derived connection state changes", async () => {
    const alpha = card("Alpha", "connected", 1);
    const beta = card("Beta", "degraded", 2);
    const wrapper = mount(NvxOverviewPanel, {
      props: {
        snapshot: { snapshotRevision: "1", cards: [beta, alpha] },
      },
      global: { plugins: [i18n] },
    });
    const labels = () => wrapper.findAll(".nvx-overview-panel__card h2")
      .map((heading) => heading.text());

    expect(labels()).toEqual(["Alpha", "Beta"]);
    await wrapper.setProps({
      snapshot: {
        snapshotRevision: "2",
        cards: [
          { ...beta, connectionState: "connected" },
          { ...alpha, connectionState: "failed" },
        ],
      },
    });
    expect(labels()).toEqual(["Alpha", "Beta"]);
  });

  it("reads Host groups from the Overview snapshot and combines group and status filters", async () => {
    const productionGroup = {
      groupId: "019d0000-0000-7000-8000-000000000101",
      label: "Production",
    };
    const stagingGroup = {
      groupId: "019d0000-0000-7000-8000-000000000102",
      label: "Staging",
    };
    const snapshot = {
      snapshotRevision: "1",
      cards: [
        card("Alpha", "connected", 1, productionGroup),
        card("Beta", "failed", 2, stagingGroup),
        card("Gamma", "disconnected", 3),
      ],
    };
    const wrapper = mount(NvxOverviewPanel, {
      props: { snapshot },
      global: { plugins: [i18n] },
    });
    const groupSelect = wrapper.getComponent(NvxSelect);

    expect(groupSelect.props("options")).toEqual([
      { value: "all", label: "全部分组" },
      { value: productionGroup.groupId, label: "Production（1）" },
      { value: stagingGroup.groupId, label: "Staging（1）" },
      { value: "ungrouped", label: "未分组（1）" },
    ]);
    expect(wrapper.text()).toContain("3 台主机 · 1 台在线 · 1 台异常");
    expect(wrapper.findAll(".nvx-overview-panel__section")).toHaveLength(3);
    expect(wrapper.text()).toContain("Production · 1 台");
    expect(wrapper.text()).toContain("Staging · 1 台");
    expect(wrapper.text()).toContain("未分组 · 1 台");

    groupSelect.vm.$emit("update:modelValue", stagingGroup.groupId);
    await wrapper.vm.$nextTick();
    expect(wrapper.findAll(".nvx-overview-panel__card")).toHaveLength(1);
    expect(wrapper.findAll(".nvx-overview-panel__section")).toHaveLength(1);
    expect(wrapper.text()).toContain("Beta");

    wrapper.getComponent(NvxSelect).vm.$emit("update:modelValue", "all");
    await wrapper.vm.$nextTick();
    const issueButton = wrapper.findAll(".nvx-overview-panel__status-filters button")
      .find((button) => button.text() === "异常");
    await issueButton!.trigger("click");
    expect(wrapper.findAll(".nvx-overview-panel__card")).toHaveLength(1);
    expect(wrapper.text()).toContain("Beta");
    expect(snapshot.cards).toHaveLength(3);
  });

  it("moves keyboard focus between visible Cards", async () => {
    const wrapper = mount(NvxOverviewPanel, {
      attachTo: document.body,
      props: {
        snapshot: {
          snapshotRevision: "1",
          cards: [card("Alpha", "connected"), card("Beta", "disconnected")],
        },
      },
      global: { plugins: [i18n] },
    });
    const cards = wrapper.findAll<HTMLElement>(".nvx-overview-panel__card");
    cards[0]!.element.focus();
    await cards[0]!.trigger("keydown", { key: "ArrowRight" });
    expect(document.activeElement).toBe(cards[1]!.element);
    wrapper.unmount();
  });

  it("windows large Host catalogs instead of mounting every Card", async () => {
    const largeCatalog = Array.from({ length: 80 }, (_, index) => (
      card(`Host ${String(index).padStart(2, "0")}`, "disconnected", index + 1)
    ));
    const wrapper = mount(NvxOverviewPanel, {
      props: {
        snapshot: {
          snapshotRevision: "1",
          cards: largeCatalog,
        },
      },
      global: { plugins: [i18n] },
    });
    const panel = wrapper.get<HTMLElement>(".nvx-overview-panel").element;
    const viewport = wrapper.get<HTMLElement>(".nvx-overview-panel__grid-viewport").element;
    vi.spyOn(panel, "clientHeight", "get").mockReturnValue(720);
    vi.spyOn(panel, "getBoundingClientRect").mockReturnValue({ top: 100 } as DOMRect);
    vi.spyOn(viewport, "getBoundingClientRect").mockImplementation(() => ({
      top: 280 - panel.scrollTop,
    }) as DOMRect);
    resizeCallback?.([], {} as ResizeObserver);
    await wrapper.vm.$nextTick();

    const renderedBeforeScroll = wrapper.findAll(".nvx-overview-panel__card");
    expect(renderedBeforeScroll.length).toBeGreaterThan(0);
    expect(renderedBeforeScroll.length).toBeLessThan(largeCatalog.length);
    expect(wrapper.get(".nvx-overview-panel__grid-viewport").attributes("style"))
      .toContain("height:");

    panel.scrollTop = 4_000;
    await wrapper.get(".nvx-overview-panel").trigger("scroll");
    await wrapper.vm.$nextTick();
    expect(wrapper.findAll(".nvx-overview-panel__card").length).toBeLessThan(largeCatalog.length);
    expect(wrapper.text()).not.toContain("Host 00");
  });

  it("recomputes columns from the scroll viewport and focuses across an unmounted row", async () => {
    const largeCatalog = Array.from({ length: 80 }, (_, index) => (
      card(`Host ${String(index).padStart(2, "0")}`, "disconnected", index + 1)
    ));
    const wrapper = mount(NvxOverviewPanel, {
      attachTo: document.body,
      props: {
        snapshot: {
          snapshotRevision: "1",
          cards: largeCatalog,
        },
      },
      global: { plugins: [i18n] },
    });
    const panel = wrapper.get<HTMLElement>(".nvx-overview-panel").element;
    const viewport = wrapper.get<HTMLElement>(".nvx-overview-panel__grid-viewport").element;
    vi.spyOn(panel, "clientWidth", "get").mockReturnValue(1_024);
    vi.spyOn(panel, "clientHeight", "get").mockReturnValue(720);
    vi.spyOn(panel, "getBoundingClientRect").mockReturnValue({
      top: 100,
    } as DOMRect);
    vi.spyOn(viewport, "clientWidth", "get").mockReturnValue(1_024);
    vi.spyOn(viewport, "getBoundingClientRect").mockImplementation(() => ({
      top: 280 - panel.scrollTop,
    }) as DOMRect);
    resizeCallback?.([], {} as ResizeObserver);
    await wrapper.vm.$nextTick();

    const firstRendered = wrapper.get<HTMLElement>(".nvx-overview-panel__card");
    firstRendered.element.focus();
    for (let index = 0; index < 24; index += 1) {
      await wrapper.get<HTMLElement>(".nvx-overview-panel__card:focus").trigger("keydown", {
        key: "ArrowRight",
      });
      await wrapper.vm.$nextTick();
    }

    expect(panel.scrollTop).toBeGreaterThan(180);
    expect((document.activeElement as HTMLElement | null)?.textContent).toContain("Host 24");
    wrapper.unmount();
    expect(disconnect).toHaveBeenCalledOnce();
  });
});
