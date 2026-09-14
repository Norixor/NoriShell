import { flushPromises, mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({
  deleteNativeTerminalHistory: vi.fn(),
  getNativeTerminalSettings: vi.fn(),
  getNativeTerminalSnapshot: vi.fn(),
  listNativeTerminalHistory: vi.fn(),
}));
const client = vi.hoisted(() => ({
  changeTerminalInputFocus: vi.fn(),
  fetchTerminalInputFocusSnapshot: vi.fn(),
}));

vi.mock("../../core-api/native-terminal", () => api);
vi.mock("../../core-api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../core-api/client")>();
  return { ...actual, ...client };
});

import { i18n } from "../../locales";
import {
  focusTerminalInputTarget,
  registerTerminalInputTarget,
  resetTerminalInputFocusForTests,
} from "../../terminal-input-target";
import { useNativeTerminalStore } from "../../stores/nativeTerminal";
import NvxNativeTerminalTools from "./NvxNativeTerminalTools.vue";

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

const session = {
  kind: "ssh" as const,
  sessionId: "019d0000-0000-7000-8000-000000000201",
  generation: "3",
  channelId: "019d0000-0000-7000-8000-000000000203",
  paneId: "pane-a",
};

function historyEntry(command: string, id = "019d0000-0000-7000-8000-000000000301") {
  return {
    entryId: id,
    scope: { kind: "host" as const, hostId: "host-a" },
    command,
    completedAtUnixMs: 1,
    elapsedMillis: 1,
    exitCode: 0,
  };
}

function focusTarget() {
  return {
    kind: "ssh" as const,
    target: {
      sessionId: "019d0000-0000-7000-8000-000000000401",
      expectedGeneration: "1",
      expectedStateRevision: "1",
      channelId: "019d0000-0000-7000-8000-000000000402",
      attachmentId: "019d0000-0000-7000-8000-000000000403",
      viewId: "019d0000-0000-7000-8000-000000000404",
    },
  };
}

function configureNative(pinia: ReturnType<typeof createPinia>) {
  const native = useNativeTerminalStore(pinia);
  native.available = true;
  native.applySettings({
    settings: {
      historyEnabled: true,
      persistEncrypted: false,
      historyMaxEntries: 500,
      historyRetentionDays: 30,
      historyPaused: false,
      notificationsEnabled: false,
      notificationThresholdSeconds: 60,
    },
    settingsRevision: "1",
    historyAvailable: true,
    historyPersistenceFailed: false,
  });
  native.sessions = [{
    session,
    shellKind: "zsh",
    captureState: "ready",
    failureCode: null,
    activity: "prompt",
    capturesCommand: true,
    historyPaused: false,
    promptObserved: true,
    promptSequence: "1",
    promptInputSequence: "1",
    promptInputEpoch: "1",
    completionCursor: "1",
  }];
  return native;
}

function mountTools(draft = "") {
  const pinia = createPinia();
  setActivePinia(pinia);
  const native = configureNative(pinia);
  const wrapper = mount(NvxNativeTerminalTools, {
    attachTo: document.body,
    props: {
      paneId: "pane-a",
      label: "Host A",
      hostId: "host-a",
      session,
      active: true,
      writable: true,
      draft,
      currentDraft: () => draft,
      enable: vi.fn(),
    },
    global: { plugins: [pinia, i18n] },
  });
  return { wrapper, native };
}

describe("NvxNativeTerminalTools", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.clearAllMocks();
    localStorage.clear();
    resetTerminalInputFocusForTests();
    i18n.global.locale.value = "en";
    client.fetchTerminalInputFocusSnapshot.mockResolvedValue({ focusEpoch: "0", target: null, lease: null });
    client.changeTerminalInputFocus.mockImplementation(async ({ expectedFocusEpoch, target }) => ({
      focusEpoch: (BigInt(expectedFocusEpoch) + 1n).toString(), target, lease: null,
    }));
    api.listNativeTerminalHistory.mockResolvedValue([]);
  });

  afterEach(() => {
    vi.useRealTimers();
    document.querySelectorAll(".nvx-dialog__backdrop").forEach((element) => element.remove());
  });

  it("drops an old history response after query, scope/session, close, or Vault-history epoch changes", async () => {
    const pending = deferred<ReturnType<typeof historyEntry>[]>();
    api.listNativeTerminalHistory.mockReturnValueOnce(pending.promise).mockResolvedValue([]);
    const inputTarget = focusTarget();
    registerTerminalInputTarget({ id: "pane-a", label: () => "Pane A", focusTarget: () => inputTarget, applyFocusLease: vi.fn(), canAcceptInput: () => true, canAcceptRawInput: () => true, send: vi.fn() });
    await focusTerminalInputTarget("pane-a");
    const { wrapper, native } = mountTools();
    const opening = (wrapper.vm as unknown as { openHistory(): Promise<void> }).openHistory();
    await vi.advanceTimersByTimeAsync(0);

    const queryInput = document.querySelector<HTMLInputElement>(".nvx-dialog input");
    queryInput!.value = "new query";
    queryInput!.dispatchEvent(new Event("input", { bubbles: true }));
    await wrapper.setProps({ hostId: "host-b", session: { ...session, generation: "4" } });
    native.applySettings({ ...native.settingsSnapshot!, settingsRevision: "2", historyAvailable: false });
    document.querySelector<HTMLButtonElement>(".nvx-dialog__header button")?.click();
    pending.resolve([historyEntry("old command")]);
    await opening;
    await flushPromises();

    expect(document.body.textContent).not.toContain("old command");
    wrapper.unmount();
  });

  it("sends only a matching suffix without Enter and invalidates its draft after concurrent typing before ack", async () => {
    const target = focusTarget();
    const ack = deferred<void>();
    const send = vi.fn().mockReturnValue(ack.promise);
    registerTerminalInputTarget({ id: "pane-a", label: () => "Pane A", focusTarget: () => target, applyFocusLease: vi.fn(), canAcceptInput: () => true, canAcceptRawInput: () => true, send });
    await focusTerminalInputTarget("pane-a");
    api.listNativeTerminalHistory.mockResolvedValue([historyEntry("git status")]);
    const { wrapper } = mountTools();
    await wrapper.setProps({ draft: "git", currentDraft: () => "git" });
    await vi.advanceTimersByTimeAsync(220);
    await flushPromises();

    const suggestion = Array.from(document.querySelectorAll<HTMLButtonElement>(".native-terminal-tools__suggestions > button"))
      .find((button) => button.textContent?.includes("git status"));
    expect(suggestion).toBeTruthy();
    suggestion?.click();
    await flushPromises();
    expect(send).toHaveBeenCalledWith(" status");
    await wrapper.setProps({ draft: "gitx", currentDraft: () => "gitx" });
    ack.resolve();
    await flushPromises();
    expect(send.mock.calls[0]?.[0]).not.toContain("\r");
    expect(wrapper.emitted("invalidateDraft")).toHaveLength(1);
    wrapper.unmount();
  });

  it("never uses global Host history as automatic suggestions for an unsaved Quick Connect", async () => {
    const { wrapper } = mountTools();
    await wrapper.setProps({ hostId: null, draft: "git", currentDraft: () => "git" });
    await vi.advanceTimersByTimeAsync(220);
    expect(api.listNativeTerminalHistory).not.toHaveBeenCalled();
    expect(document.querySelector(".native-terminal-tools__suggestions")).toBeNull();
    wrapper.unmount();
  });

  it("does not surface an old suggestion after a session scope change", async () => {
    const pending = deferred<ReturnType<typeof historyEntry>[]>();
    api.listNativeTerminalHistory.mockReturnValueOnce(pending.promise);
    const { wrapper } = mountTools();
    await wrapper.setProps({ draft: "git", currentDraft: () => "git" });
    await vi.advanceTimersByTimeAsync(220);
    await wrapper.setProps({ hostId: "host-b", session: { ...session, generation: "4" } });
    pending.resolve([historyEntry("git status")]);
    await flushPromises();

    expect(document.querySelector(".native-terminal-tools__suggestions")).toBeNull();
    wrapper.unmount();
  });
});
