import { mount } from "@vue/test-utils";
import { describe, expect, it } from "vitest";

import NvxProgress from "./NvxProgress.vue";

describe("NvxProgress", () => {
  it("keeps a real zero distinct from a non-value status", () => {
    const zero = mount(NvxProgress, {
      props: { label: "CPU", value: 0, status: "available" },
    });
    const progressbar = zero.get('[role="progressbar"]');
    expect(progressbar.attributes("aria-valuenow")).toBe("0");
    expect(zero.text()).toContain("0%");

    const disabled = mount(NvxProgress, {
      props: {
        label: "CPU",
        status: "disabled",
        statusLabel: "Monitoring disabled",
      },
    });
    expect(disabled.find('[role="progressbar"]').exists()).toBe(false);
    expect(disabled.get('[role="status"]').attributes("aria-label"))
      .toBe("CPU: Monitoring disabled");
  });

  it("retains the last value while exposing a stale status label", () => {
    const wrapper = mount(NvxProgress, {
      props: { label: "Memory", value: 42.4, status: "stale", statusLabel: "Stale" },
    });
    expect(wrapper.get('[role="progressbar"]').attributes("aria-valuenow")).toBe("42.4");
    expect(wrapper.get('[role="progressbar"]').attributes("aria-label")).toBe("Memory: Stale");
    expect(wrapper.text()).toContain("42%");
  });

  it("renders a compact ring without turning a missing value into zero", () => {
    const available = mount(NvxProgress, {
      props: { label: "CPU", value: 37.5, variant: "ring", size: "sm" },
    });
    expect(available.get('[role="progressbar"]').attributes("aria-valuenow")).toBe("37.5");
    expect(available.text()).toContain("38%");
    expect(available.get(".nvx-progress__ring-value").attributes("stroke-dasharray"))
      .not.toMatch(/^0 /);

    const disabled = mount(NvxProgress, {
      props: {
        label: "CPU",
        variant: "ring",
        status: "disabled",
        statusLabel: "Monitoring disabled",
      },
    });
    expect(disabled.find('[role="progressbar"]').exists()).toBe(false);
    expect(disabled.get('[role="status"]').attributes("aria-label"))
      .toBe("CPU: Monitoring disabled");
    expect(disabled.text()).toContain("—");
  });
});
