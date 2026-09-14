import { describe, expect, it, vi } from "vitest";
import type { SftpTransferIntentSummary } from "./core-api/generated/core-api";
import { registerNativeTransferNavigation, takeNativeTransferNavigation, matchesNativeTransferNavigation } from "./native-transfer-navigation";
const source = { kind: "remoteSession" as const, sessionId: "remote", generation: "9007199254740993" };
const target = { kind: "localCapability" as const, directoryRef: "directory", revision: "7" };
const navigation = { kind: "intent" as const, transferId: "transfer", minimumRevision: "9007199254740993", source, target };
function summary(revision: string, generation = source.generation) {
  return { transferId: navigation.transferId, stateRevision: revision, sourceFence: { ...source, generation }, targetFence: target } as SftpTransferIntentSummary;
}
describe("native transfer navigation fence", () => {
  it("accepts same transfer progress beyond the menu revision", () => {
    expect(matchesNativeTransferNavigation(navigation, [summary("9007199254740994")], [])).toBe(true);
  });
  it("rejects generation changes and older snapshots", () => {
    expect(matchesNativeTransferNavigation(navigation, [summary("9007199254740994", "9007199254740994")], [])).toBe(false);
    expect(matchesNativeTransferNavigation(navigation, [summary("9007199254740992")], [])).toBe(false);
  });
  it("consumes an immutable navigation once and expires unused entries", () => {
    registerNativeTransferNavigation("operation", navigation);
    expect(takeNativeTransferNavigation("operation")).toEqual(navigation);
    expect(takeNativeTransferNavigation("operation")).toBeNull();
    vi.useFakeTimers();
    try {
      registerNativeTransferNavigation("expired", navigation);
      vi.advanceTimersByTime(60_001);
      expect(takeNativeTransferNavigation("expired")).toBeNull();
    } finally { vi.useRealTimers(); }
  });
});
