export type TerminalBellAudioAvailability = "ready" | "gesture-required" | "unavailable";

type AudioContextConstructor = new () => AudioContext;

function browserAudioContextConstructor(): AudioContextConstructor | null {
  if (typeof window === "undefined") return null;
  const candidate = window as typeof window & { webkitAudioContext?: AudioContextConstructor };
  return window.AudioContext ?? candidate.webkitAudioContext ?? null;
}

/**
  * Create or resume the audio context only in a user gesture that entered the terminal surface. BEL never triggers permission
  * requests or autoplay attempts; failures remain local to this terminal view.
 */
export class TerminalBellAudio {
  private context: AudioContext | null = null;
  private permanentlyUnavailable = false;

  async enableFromUserGesture(): Promise<TerminalBellAudioAvailability> {
    if (this.permanentlyUnavailable) return "unavailable";
    const AudioContextClass = browserAudioContextConstructor();
    if (!AudioContextClass) {
      this.permanentlyUnavailable = true;
      return "unavailable";
    }
    try {
      this.context ??= new AudioContextClass();
      if (this.context.state !== "running") await this.context.resume();
      return this.context.state === "running" ? "ready" : "gesture-required";
    } catch {
      return "gesture-required";
    }
  }

  play() {
    const context = this.context;
    if (!context || context.state !== "running") return false;
    try {
      const gain = context.createGain();
      const oscillator = context.createOscillator();
      const now = context.currentTime;
      gain.gain.setValueAtTime(0.035, now);
      gain.gain.exponentialRampToValueAtTime(0.0001, now + 0.07);
      oscillator.frequency.setValueAtTime(880, now);
      oscillator.connect(gain);
      gain.connect(context.destination);
      oscillator.start(now);
      oscillator.stop(now + 0.07);
      return true;
    } catch {
      return false;
    }
  }

  dispose() {
    const context = this.context;
    this.context = null;
    if (context && context.state !== "closed") void context.close().catch(() => undefined);
  }
}
