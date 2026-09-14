import { afterEach, describe, expect, it, vi } from "vitest";

import { TerminalBellAudio } from "./terminal-bell";

describe("terminal bell audio", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("does not create audio until a user-gesture entry point asks for it", async () => {
    class AudioContextMock {
      static lastInstance: AudioContextMock | null = null;
      state: AudioContextState = "running";
      currentTime = 0;
      resume = vi.fn().mockResolvedValue(undefined);
      createGain = vi.fn(() => ({ gain: { setValueAtTime() {}, exponentialRampToValueAtTime() {} }, connect() {} }));
      createOscillator = vi.fn(() => ({ frequency: { setValueAtTime() {} }, connect() {}, start() {}, stop() {} }));
      destination = {} as AudioDestinationNode;
      close = vi.fn().mockResolvedValue(undefined);

      constructor() { AudioContextMock.lastInstance = this; }
    }
    vi.stubGlobal("AudioContext", AudioContextMock);
    const bell = new TerminalBellAudio();
    expect(AudioContextMock.lastInstance).toBeNull();
    await expect(bell.enableFromUserGesture()).resolves.toBe("ready");
    expect(AudioContextMock.lastInstance).not.toBeNull();
    expect(bell.play()).toBe(true);
    bell.dispose();
    expect(AudioContextMock.lastInstance!.close).toHaveBeenCalledOnce();
  });

  it("reports unavailable when the platform has no audio context", async () => {
    vi.stubGlobal("AudioContext", undefined);
    await expect(new TerminalBellAudio().enableFromUserGesture()).resolves.toBe("unavailable");
  });
});
