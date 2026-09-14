import { flushPromises, mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";

const client = vi.hoisted(() => ({
  changeTerminalInputFocus: vi.fn(),
  fetchTerminalInputFocusSnapshot: vi.fn(),
}));

vi.mock("../../core-api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../core-api/client")>();
  return { ...actual, ...client };
});

import { i18n } from "../../locales";
import { useQuickCommandsStore } from "../../stores/quickCommands";
import {
  focusTerminalInputTarget,
  registerTerminalInputTarget,
  resetTerminalInputFocusForTests,
} from "../../terminal-input-target";
import NvxQuickCommandsSidebar from "./NvxQuickCommandsSidebar.vue";

describe("NvxQuickCommandsSidebar", () => {
  beforeEach(() => {
    localStorage.clear();
    resetTerminalInputFocusForTests();
    client.fetchTerminalInputFocusSnapshot.mockResolvedValue({
      focusEpoch: "0",
      target: null,
      lease: null,
    });
    client.changeTerminalInputFocus.mockImplementation(async ({ expectedFocusEpoch, target }) => ({
      focusEpoch: (BigInt(expectedFocusEpoch) + 1n).toString(),
      target,
      lease: null,
    }));
    setActivePinia(createPinia());
    i18n.global.locale.value = "en";
  });

  it("runs in the focused terminal and copies without editing the item", async () => {
    const store = useQuickCommandsStore();
    expect(store.save({ label: "Disk", command: "df -h" })).toBe("saved");
    const send = vi.fn().mockResolvedValue(undefined);
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText },
    });
    const unregister = registerTerminalInputTarget({
      id: "focused-pane",
      label: () => "deploy@example.com",
      focusTarget: () => ({
        kind: "ssh",
        target: {
          sessionId: "019d0000-0000-7000-8000-000000000601",
          expectedGeneration: "1",
          expectedStateRevision: "2",
          channelId: "019d0000-0000-7000-8000-000000000602",
          attachmentId: "019d0000-0000-7000-8000-000000000603",
          viewId: "019d0000-0000-7000-8000-000000000604",
        },
      }),
      applyFocusLease: () => undefined,
      canAcceptInput: () => true,
      send,
    });
    await focusTerminalInputTarget("focused-pane");
    const wrapper = mount(NvxQuickCommandsSidebar, {
      global: { plugins: [i18n] },
    });

    await wrapper.get('button[aria-label="Run Disk"]').trigger("click");
    await flushPromises();
    expect(send).toHaveBeenCalledWith("df -h\r");

    await wrapper.get('button[aria-label="Copy the command for Disk"]').trigger("click");
    await flushPromises();
    expect(writeText).toHaveBeenCalledWith("df -h");

    wrapper.unmount();
    unregister();
  });
});
