import type {
  PluginApiCall,
  PluginApiReply,
  PluginIsolatedBridgeAction,
  PluginIsolatedBridgeResponse,
  PluginIsolatedPendingAction,
} from "./core-api/generated/core-api";

const MAX_CALL_BYTES = 64 * 1024;
const MAX_QUEUED_CALLS = 8;

type BridgeOptions = {
  invoke: (sequence: number, action: PluginIsolatedBridgeAction) => Promise<PluginIsolatedBridgeResponse>;
  pending: (value: PluginIsolatedPendingAction | null) => void;
  unavailable: () => void;
};

// Only the trusted wrapper owns this object and its nonce-bearing invoke closure.
export class PluginIsolatedBridge {
  private sequence: number;
  private port: MessagePort | null = null;
  private active = true;
  private running = false;
  private queue: PluginApiCall[] = [];
  private waiting: PluginIsolatedPendingAction | null = null;

  constructor(sequence: number, private readonly options: BridgeOptions) {
    this.sequence = sequence;
  }

  attach(port: MessagePort) {
    if (!this.active || this.port) {
      port.close();
      return;
    }
    this.port = port;
    port.onmessage = (event: MessageEvent<unknown>) => {
      const call = parseCall(event.data);
      if (!call || !this.active) return;
      if (this.waiting || this.queue.length >= MAX_QUEUED_CALLS) {
        this.send({ callId: call.callId, outcome: { kind: "failed", code: "busy" } });
        return;
      }
      this.queue.push(call);
      void this.drain();
    };
    port.onmessageerror = () => this.dispose();
    port.start();
  }

  environment(locale: string, theme: string) {
    if (this.active) this.port?.postMessage({ type: "norishell.environment", locale, theme });
  }

  async approve(pendingId: string) {
    if (!this.active || this.running || this.waiting?.pendingId !== pendingId) return;
    this.running = true;
    this.waiting = null;
    this.options.pending(null);
    try {
      const result = await this.request({ kind: "approve", pendingId });
      if (this.active && result.reply) this.send(result.reply);
    } catch {
      this.fail();
    } finally {
      this.running = false;
      void this.drain();
    }
  }

  async cancel(pendingId: string) {
    if (!this.active || this.running || this.waiting?.pendingId !== pendingId) return;
    const callId = this.waiting.callId;
    this.running = true;
    this.waiting = null;
    this.options.pending(null);
    try {
      await this.request({ kind: "cancel", pendingId });
      if (this.active) this.send({ callId, outcome: { kind: "failed", code: "cancelled" } });
    } catch {
      this.fail();
    } finally {
      this.running = false;
      void this.drain();
    }
  }

  dispose() {
    if (!this.active) return;
    this.active = false;
    this.port?.close();
    this.port = null;
    this.queue = [];
    this.waiting = null;
    this.options.pending(null);
    // Sequence is reserved before every await, including an outstanding call.
    void this.request({ kind: "deactivate" }).catch(() => undefined);
  }

  private async request(action: PluginIsolatedBridgeAction) {
    const sequence = this.sequence++;
    const result = await this.options.invoke(sequence, action);
    if (result.nextSequence !== sequence + 1) throw new Error("bridge sequence mismatch");
    return result;
  }

  private async drain() {
    if (!this.active || this.running || this.waiting) return;
    const call = this.queue.shift();
    if (!call) return;
    this.running = true;
    try {
      const result = await this.request({ kind: "call", call });
      if (!this.active) return;
      if (result.pending) {
        this.waiting = result.pending;
        this.options.pending(result.pending);
        this.port?.postMessage({ type: "norishell.api.pending", callId: call.callId });
        for (const queued of this.queue.splice(0)) {
          this.send({ callId: queued.callId, outcome: { kind: "failed", code: "busy" } });
        }
      } else if (result.reply) {
        this.send(result.reply);
      }
    } catch {
      this.fail();
    } finally {
      this.running = false;
      void this.drain();
    }
  }

  private send(reply: PluginApiReply) {
    this.port?.postMessage({ type: "norishell.api.reply", reply });
  }

  private fail() {
    this.dispose();
    this.options.unavailable();
  }
}

function parseCall(value: unknown): PluginApiCall | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  const message = value as Record<string, unknown>;
  if (Object.keys(message).some((key) => key !== "type" && key !== "call")
    || message.type !== "norishell.api.request") return null;
  const call = message.call;
  if (!call || typeof call !== "object" || Array.isArray(call)) return null;
  const candidate = call as Record<string, unknown>;
  if (Object.keys(candidate).some((key) => key !== "callId" && key !== "operation")
    || typeof candidate.callId !== "string" || !/^[a-zA-Z0-9._-]{1,80}$/.test(candidate.callId)
    || !candidate.operation || typeof candidate.operation !== "object") return null;
  try {
    if (new TextEncoder().encode(JSON.stringify(call)).byteLength > MAX_CALL_BYTES) return null;
  } catch { return null; }
  // Core deserializes the operation's strict tagged union; this is only queue admission.
  return call as PluginApiCall;
}
