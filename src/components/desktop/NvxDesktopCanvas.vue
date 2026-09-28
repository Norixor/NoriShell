<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, shallowRef, watch } from "vue";
import { useI18n } from "vue-i18n";
import { parseCoreApiError } from "../../core-api/client";
import { desktopClient } from "../../core-api/desktop-client";
import type { DesktopInputEvent, DesktopSessionSummary } from "../../core-api/generated/core-api";
import { detectDesktopPlatform } from "../../platform";
import { preparePluginProtectedMount } from "../../plugins/hostDomBroker";
import { decodeDesktopFrame, desktopPointerButtons, desktopKey, reservedDesktopKey, type DesktopCursorShape, type DesktopFramePatch } from "./input";
const props = withDefaults(defineProps<{
  session: DesktopSessionSummary;
  active: boolean;
  fit: boolean;
  panning: boolean;
  /** macOS device preference: deliver ⌘ as the remote Control key. */
  commandAsControl?: boolean;
  /** Device preference: request device-pixel resolution and show actual size at one device pixel per remote pixel. */
  hiDpi?: boolean;
}>(), { commandAsControl: false, hiDpi: false });
const emit = defineEmits<{ error: []; resolutionError: [error: unknown] }>();
const { t } = useI18n();

const inputSink = ref<HTMLTextAreaElement | null>(null);
const root = ref<HTMLElement | null>(null);
const viewport = ref<HTMLElement | null>(null);
const canvas = ref<HTMLCanvasElement | null>(null);
const ready = ref(false);
const controlled = ref(false);
const panningNow = ref(false);
const frameSize = ref({ width: 0, height: 0 });
const viewportSize = ref({ width: 0, height: 0 });
const devicePixelRatioValue = ref(1);
const cursorShape = shallowRef<DesktopCursorShape>({ kind: "default" });
// CSS pixels per remote pixel for the current display mode.
const displayScale = computed(() => {
  if (!frameSize.value.width || !frameSize.value.height) return 1;
  if (props.fit && !props.panning) {
    if (!viewportSize.value.width || !viewportSize.value.height) return 1;
    return Math.min(viewportSize.value.width / frameSize.value.width, viewportSize.value.height / frameSize.value.height);
  }
  return props.hiDpi ? 1 / devicePixelRatioValue.value : 1;
});
const canvasSize = computed(() => {
  const scale = displayScale.value;
  if (scale === 1 || !frameSize.value.width || !frameSize.value.height) return undefined;
  return { width: `${frameSize.value.width * scale}px`, height: `${frameSize.value.height * scale}px` };
});
const MAX_CURSOR_CSS_PX = 128;
/**
 * Renders the remote cursor bitmap as a PNG at device resolution. `scale` is CSS pixels per remote pixel;
 * on HiDPI screens the image carries the device pixel ratio through image-set so the pointer stays sharp.
 */
function cursorImage(shape: Extract<DesktopCursorShape, { kind: "bitmap" }>, scale: number, ratio: number) {
  try {
    const cssScale = Math.min(scale, MAX_CURSOR_CSS_PX / Math.max(shape.width, shape.height));
    const cssWidth = Math.max(1, Math.round(shape.width * cssScale)), cssHeight = Math.max(1, Math.round(shape.height * cssScale));
    const width = Math.max(1, Math.round(cssWidth * ratio)), height = Math.max(1, Math.round(cssHeight * ratio));
    const source = document.createElement("canvas");
    source.width = shape.width; source.height = shape.height;
    const sourceContext = source.getContext("2d");
    if (!sourceContext) return null;
    sourceContext.putImageData(new ImageData(shape.rgba, shape.width, shape.height), 0, 0);
    let image = source;
    if (width !== shape.width || height !== shape.height) {
      const target = document.createElement("canvas");
      target.width = width; target.height = height;
      const targetContext = target.getContext("2d");
      if (!targetContext) return null;
      targetContext.drawImage(source, 0, 0, width, height);
      image = target;
    }
    const url = image.toDataURL("image/png");
    if (!url.startsWith("data:image/png")) return null;
    // Cursor hotspots are expressed in CSS pixels regardless of the image resolution.
    const hotspotX = Math.min(cssWidth - 1, Math.round(shape.hotspotX * (cssWidth / shape.width)));
    const hotspotY = Math.min(cssHeight - 1, Math.round(shape.hotspotY * (cssHeight / shape.height)));
    if (ratio !== 1) {
      const set = `-webkit-image-set(url("${url}") ${ratio}x) ${hotspotX} ${hotspotY}, default`;
      if (typeof CSS !== "undefined" && CSS.supports?.("cursor", set)) return set;
      // Without image-set support fall back to a CSS-pixel image of the same size.
      const plain = document.createElement("canvas");
      plain.width = cssWidth; plain.height = cssHeight;
      const plainContext = plain.getContext("2d");
      if (!plainContext) return null;
      plainContext.drawImage(source, 0, 0, cssWidth, cssHeight);
      return `url("${plain.toDataURL("image/png")}") ${hotspotX} ${hotspotY}, default`;
    }
    return `url("${url}") ${hotspotX} ${hotspotY}, default`;
  } catch {
    return null;
  }
}
// Resizing in fit mode changes the scale continuously; quantising it keeps PNG encoding off the resize path.
let cursorCache: { shape: DesktopCursorShape; key: string; css: string } | null = null;
const cursorCss = computed(() => {
  const shape = cursorShape.value;
  if (shape.kind === "hidden") return "none";
  if (shape.kind !== "bitmap") return "default";
  const scale = Math.round(displayScale.value * 20) / 20 || 0.05;
  const ratio = Math.round(devicePixelRatioValue.value * 4) / 4 || 1;
  const key = `${scale}:${ratio}`;
  if (cursorCache?.shape === shape && cursorCache.key === key) return cursorCache.css;
  const css = cursorImage(shape, scale, ratio) ?? "default";
  cursorCache = { shape, key, css };
  return css;
});
const canvasStyle = computed(() => {
  // Pan mode keeps the grab cursors from the stylesheet.
  const cursor = props.panning ? undefined : cursorCss.value;
  return { ...canvasSize.value, cursor };
});

let reportedFrameError = false;
let mounted = false;
let frameBusy = false;
let after = 0n;
let afterCursor = 0n;
let pullTimer: ReturnType<typeof setTimeout> | undefined;
let pullFrame: number | undefined;

let intent = 0;
let epoch: string | null = null;
let inputSequence = 0n;
let pending = 0;
let chain = Promise.resolve();
let focusChain = Promise.resolve();
let keyboardAcquire: Promise<void> | null = null;
let pan: {
  pointerId: number;
  x: number;
  y: number;
  left: number;
  top: number;
} | null = null;
const metaChorded = new Set<string>();
let pendingMove: DesktopInputEvent | null = null;
let pendingWheel: { x: number; y: number; deltaX: number; deltaY: number } | null = null;
let inputFrame: number | undefined;

const canRun = computed(() => props.active && props.session.state === "running");
const identity = computed(() => `${props.session.id}:${props.session.generation}`);

function requestFrame(callback: () => void): number {
  return typeof requestAnimationFrame === "function" ? requestAnimationFrame(callback) : Number(setTimeout(callback, 16));
}
function cancelFrame(handle: number) {
  if (typeof cancelAnimationFrame === "function") cancelAnimationFrame(handle);
  else clearTimeout(handle);
}

function available() {
  return mounted
    && canRun.value
    && !props.panning
    && !document.hidden
    && document.hasFocus()
    && !document.querySelector('[role="dialog"][aria-modal="true"]');
}

function stopPanning() {
  if (pan && canvas.value?.hasPointerCapture?.(pan.pointerId)) {
    canvas.value.releasePointerCapture?.(pan.pointerId);
  }
  pan = null;
  panningNow.value = false;
}

function discardPendingInput() {
  if (inputFrame !== undefined) { cancelFrame(inputFrame); inputFrame = undefined; }
  pendingMove = null;
  pendingWheel = null;
}

function invalidate() {
  stopPanning();
  metaChorded.clear();
  discardPendingInput();
  intent++;
  epoch = null;
  controlled.value = false;
  focusChain = focusChain
    .catch(() => undefined)
    .then(async () => {
      await desktopClient.focus(null);
    })
    .catch(() => undefined);
  return focusChain;
}

async function acquire() {
  const ticket = ++intent;
  const session = props.session;
  const key = identity.value;

  epoch = null;
  controlled.value = false;
  focusChain = focusChain
    .catch(() => undefined)
    .then(async () => {
      if (!available() || ticket !== intent || key !== identity.value) return;

      const next = await desktopClient.focus(session);
      if (available() && ticket === intent && key === identity.value) {
        epoch = next;
        inputSequence = 0n;
        controlled.value = true;
      } else {
        await desktopClient.focus(null);
      }
    })
    .catch(() => {
      if (ticket === intent) {
        epoch = null;
        controlled.value = false;
      }
    });
  await focusChain;
}

/**
 * Queues one input event behind the previous ones. Droppable events (pointer moves and wheel) are
 * discarded when Core's queue is full instead of revoking control; every other failure invalidates.
 */
function send(input: DesktopInputEvent, droppable = false) {
  if (!available() || !epoch) return Promise.resolve(false);
  if (pending >= 32) {
    if (droppable) return Promise.resolve(false);
    void invalidate();
    emit("error");
    return Promise.resolve(false);
  }

  const session = props.session;
  const key = identity.value;
  const fence = epoch;
  const ticket = intent;
  const sequence = String(++inputSequence);

  pending++;
  const result = chain
    .then(async () => {
      if (!available() || key !== identity.value || fence !== epoch || ticket !== intent) {
        return false;
      }

      await desktopClient.input({
        sessionId: session.id,
        generation: session.generation,
        focusEpoch: fence,
        sequence,
        input,
      });
      return true;
    })
    .catch((error: unknown) => {
      if (ticket !== intent) return false;
      if (droppable && parseCoreApiError(error)?.code === "desktop.resourceLimit") return false;
      void invalidate();
      emit("error");
      return false;
    })
    .finally(() => {
      pending--;
    });
  chain = result.then(() => undefined);
  return result;
}

function scheduleInputFlush() {
  if (inputFrame !== undefined) return;
  inputFrame = requestFrame(() => { inputFrame = undefined; flushPendingInput(); });
}

/** Sends the coalesced move and wheel before any button or key event so remote ordering is preserved. */
function flushPendingInput() {
  if (inputFrame !== undefined) { cancelFrame(inputFrame); inputFrame = undefined; }
  const move = pendingMove, wheel = pendingWheel;
  pendingMove = null;
  pendingWheel = null;
  if (move) void send(move, true);
  if (wheel) void send({ kind: "wheel", ...wheel }, true);
}

function point(event: MouseEvent) {
  const rect = canvas.value?.getBoundingClientRect();
  if (!rect || !rect.width || !rect.height) return { x: 0, y: 0 };

  return {
    x: Math.max(
      0,
      Math.min(
        (canvas.value?.width ?? 1) - 1,
        Math.floor((event.clientX - rect.left) / rect.width * (canvas.value?.width ?? 1)),
      ),
    ),
    y: Math.max(
      0,
      Math.min(
        (canvas.value?.height ?? 1) - 1,
        Math.floor((event.clientY - rect.top) / rect.height * (canvas.value?.height ?? 1)),
      ),
    ),
  };
}

async function pointer(event: PointerEvent) {
  if (props.panning) {
    if (event.type === "pointerdown") {
      if (event.button !== 0 || !viewport.value) return;
      event.preventDefault();
      stopPanning();
      canvas.value?.focus({ preventScroll: true });
      pan = {
        pointerId: event.pointerId,
        x: event.clientX,
        y: event.clientY,
        left: viewport.value.scrollLeft,
        top: viewport.value.scrollTop,
      };
      panningNow.value = true;
      canvas.value?.setPointerCapture?.(event.pointerId);
      return;
    }
    if (!pan || pan.pointerId !== event.pointerId) return;
    event.preventDefault();
    if (event.type === "pointermove" && viewport.value) {
      viewport.value.scrollLeft = pan.left - (event.clientX - pan.x);
      viewport.value.scrollTop = pan.top - (event.clientY - pan.y);
    }
    if (event.type === "pointerup" || event.type === "pointercancel") {
      stopPanning();
    }
    return;
  }
  event.preventDefault();
  if (event.type === "pointerdown") {
    inputSink.value?.focus();
    canvas.value?.setPointerCapture(event.pointerId);
    if (!epoch) {
      await acquire();
      return;
    }
  }
  const input: DesktopInputEvent = { kind: "pointer", ...point(event), buttons: desktopPointerButtons(event) };
  if (event.type === "pointermove") {
    if (!epoch) return;
    // Only the latest position of an animation frame is sent; buttons come from the same event.
    pendingMove = input;
    // A pending wheel is sent after the move, so it must carry the newest position too.
    if (pendingWheel) pendingWheel = { ...pendingWheel, x: input.x, y: input.y };
    scheduleInputFlush();
    return;
  }
  flushPendingInput();
  void send(input);
}

function pointerCancel(event: PointerEvent) {
  if (props.panning) {
    void pointer(event);
    return;
  }
  void invalidate();
}

function key(event: KeyboardEvent, down: boolean) {
  if (props.panning) return;
  if (reservedDesktopKey(event)) return;
  if (event.isComposing || event.key === "Process") return;
  event.preventDefault();
  const input = desktopKey(event, down, { commandAsControl: props.commandAsControl });
  if (!input || input.kind !== "key") return;
  // macOS WebKit does not deliver keyup for keys pressed while ⌘ is held, which would leave the remote key
  // stuck and auto-repeating. Such chords are sent as an immediate press and release; their late keyups are ignored.
  let inputs: DesktopInputEvent[] = [input];
  if (event.metaKey && detectDesktopPlatform() === "macos" && !/^(Meta|Control|Alt|Shift)/.test(event.code)) {
    if (!down) {
      if (metaChorded.delete(event.code)) return;
    } else {
      metaChorded.add(event.code);
      inputs = [input, { ...input, down: false }];
    }
  } else if (!down) {
    metaChorded.delete(event.code);
  }
  if (!down && /^Meta/.test(event.code)) metaChorded.clear();
  const deliver = () => { flushPendingInput(); for (const item of inputs) void send(item); };
  if (epoch) { deliver(); return; }
  if (!available() || document.activeElement !== inputSink.value) return;
  // A moved WebView may keep DOM focus after its old Core input lease is revoked.
  keyboardAcquire ??= acquire().finally(() => { keyboardAcquire = null; });
  void keyboardAcquire.then(deliver);
}

function composed(event: CompositionEvent) {
  if (props.panning) return;
  if (event.data) {
    const input: DesktopInputEvent = { kind: "text", text: event.data };
    if (epoch) { flushPendingInput(); void send(input); }
    else if (available() && document.activeElement === inputSink.value) {
      keyboardAcquire ??= acquire().finally(() => { keyboardAcquire = null; });
      void keyboardAcquire.then(() => { flushPendingInput(); return send(input); });
    }
  }
  if (inputSink.value) inputSink.value.value = "";
}

const clampDelta = (value: number) => Math.max(-32768, Math.min(32767, Math.round(value)));
function wheel(event: WheelEvent) {
  if (props.panning) return;
  event.preventDefault();
  if (!epoch) return;
  const scale = event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? 400 : 1;
  const position = point(event);
  const deltaX = event.deltaX * scale, deltaY = event.deltaY * scale;
  // Deltas accumulate within one animation frame and travel with the latest pointer position.
  pendingWheel = pendingWheel
    ? { ...position, deltaX: clampDelta(pendingWheel.deltaX + deltaX), deltaY: clampDelta(pendingWheel.deltaY + deltaY) }
    : { ...position, deltaX: clampDelta(deltaX), deltaY: clampDelta(deltaY) };
  scheduleInputFlush();
}

function canPull() {
  return mounted && canRun.value && !document.hidden;
}

function cancelScheduledPull() {
  clearTimeout(pullTimer);
  pullTimer = undefined;
  if (pullFrame !== undefined) { cancelFrame(pullFrame); pullFrame = undefined; }
}

function schedulePull(delay: "frame" | number) {
  cancelScheduledPull();
  if (!canPull()) return;
  if (delay === "frame") pullFrame = requestFrame(() => { pullFrame = undefined; void pull(); });
  else pullTimer = setTimeout(() => { pullTimer = undefined; void pull(); }, delay);
}

function syncFramePolling() {
  if (!canPull()) cancelScheduledPull();
  else if (!frameBusy && pullTimer === undefined && pullFrame === undefined) void pull();
}

/** Paints every rect and reports whether the image now continues from this frame. */
function paint(frame: DesktopFramePatch) {
  const target = canvas.value;
  const context = target?.getContext("2d");
  if (!target || !context) return false;
  if (frame.base !== 0n && (frame.base !== after || target.width !== frame.width || target.height !== frame.height)) {
    // The patch does not continue our image; ask for a full frame instead of painting a corrupt one.
    after = 0n;
    return false;
  }
  if (target.width !== frame.width || target.height !== frame.height) {
    target.width = frame.width;
    target.height = frame.height;
  }
  if (frameSize.value.width !== frame.width || frameSize.value.height !== frame.height) {
    frameSize.value = { width: frame.width, height: frame.height };
  }
  for (const rect of frame.rects) context.putImageData(new ImageData(rect.rgba, rect.width, rect.height), rect.x, rect.y);
  return true;
}

/** One long-poll request at a time; the next one starts after painting (next animation frame) or right after an empty reply. */
async function pull() {
  if (!canPull() || frameBusy) return;
  frameBusy = true;
  const started = performance.now();
  const key = identity.value;
  const session = props.session;
  let next: "frame" | number = "frame";
  try {
    const data = await desktopClient.frame(session, String(after), String(afterCursor));
    if (!mounted || !canRun.value || document.hidden || key !== identity.value) return;
    const decoded = decodeDesktopFrame(data, after);
    reportedFrameError = false;
    if (!decoded) {
      // Core already waited for new data; a reply that returned at once did not, so avoid a hot loop.
      next = performance.now() - started < 20 ? 50 : 0;
      return;
    }
    if (decoded.frame && paint(decoded.frame)) after = decoded.frameSequence;
    if (decoded.cursor) {
      cursorShape.value = decoded.cursor.kind === "bitmap" ? { ...decoded.cursor, rgba: decoded.cursor.rgba.slice() } : decoded.cursor;
      afterCursor = decoded.cursorSequence;
    }
  } catch (error) {
    if (key !== identity.value || !canRun.value) return;
    next = 250;
    // Tab ownership is still being projected; the next snapshot or retry resolves it without user action.
    if (parseCoreApiError(error)?.code.startsWith("workspace_tab.")) return;
    if (!reportedFrameError) {
      reportedFrameError = true;
      emit("error");
    }
  } finally {
    frameBusy = false;
    if (canPull()) {
      if (next === 0) void pull();
      else schedulePull(next);
    }
  }
}

function visibility() {
  syncFramePolling();
  scheduleResolution();
  if (
    canRun.value
    && !document.hidden
    && document.hasFocus()
    && !document.querySelector('[role="dialog"][aria-modal="true"]')
  ) return;
  stopPanning();
  void invalidate();
}

let resolutionTimer: ReturnType<typeof setTimeout> | undefined;
let resolutionBusy = false;
let attemptedResolution = "";
function desiredResolution() {
  const size = viewportSize.value;
  const profile = props.session.profile;
  const adaptive = profile.protocol === "rdp" ? profile.rdpResolutionMode === "adaptive" : profile.vncResolutionMode === "adaptive";
  if (!mounted || !canRun.value || document.hidden || !adaptive || size.width <= 0 || size.height <= 0) return null;
  const factor = props.hiDpi ? devicePixelRatioValue.value : 1;
  let width = Math.min(8192, Math.max(200, Math.floor(size.width * factor)));
  let height = Math.min(8192, Math.max(200, Math.floor(size.height * factor)));
  if (width * height > 16_777_216) {
    const scale = Math.sqrt(16_777_216 / (width * height));
    width = Math.floor(width * scale);
    height = Math.floor(height * scale);
  }
  if (profile.protocol === "rdp") width -= width % 2;
  // Only RDP Display Control carries a UI scale; VNC ignores it and always receives 100.
  const scalePercent = profile.protocol === "rdp" && props.hiDpi ? Math.min(500, Math.max(100, Math.round(factor * 100))) : 100;
  return { width, height, scalePercent, key: `${identity.value}:${width}x${height}@${scalePercent}` };
}
function scheduleResolution() {
  clearTimeout(resolutionTimer);
  const desired = desiredResolution();
  if (!desired || desired.key === attemptedResolution) return;
  resolutionTimer = setTimeout(() => { void requestResolution(); }, 200);
}
async function requestResolution() {
  const desired = desiredResolution();
  if (!desired || resolutionBusy || desired.key === attemptedResolution) return;
  const session = props.session;
  const key = identity.value;
  // Deduplicate rejected sizes too; retries require a new size or connection, not a tight loop.
  attemptedResolution = desired.key;
  const currentWidth = frameSize.value.width || session.width;
  const currentHeight = frameSize.value.height || session.height;
  // A matching pixel size still needs a request when a HiDPI scale must be applied remotely.
  if (currentWidth === desired.width && currentHeight === desired.height && desired.scalePercent === 100) return;
  resolutionBusy = true;
  try {
    await desktopClient.resolution(session, desired.width, desired.height, desired.scalePercent);
    // Only server frames and snapshots may change the displayed remote size.
  } catch (error) {
    if (mounted && key === identity.value && canRun.value && desiredResolution()?.key === desired.key) emit("resolutionError", error);
  } finally {
    resolutionBusy = false;
    scheduleResolution();
  }
}
watch([identity, canRun, () => props.session.profile.rdpResolutionMode, () => props.session.profile.vncResolutionMode, () => props.hiDpi, viewportSize], scheduleResolution);
let observer: MutationObserver | null = null;
let resizeObserver: ResizeObserver | null = null;
let ratioQuery: MediaQueryList | null = null;
function watchPixelRatio() {
  ratioQuery?.removeEventListener("change", measureViewport);
  ratioQuery = typeof window.matchMedia === "function" ? window.matchMedia(`(resolution: ${window.devicePixelRatio || 1}dppx)`) : null;
  ratioQuery?.addEventListener?.("change", measureViewport);
}
function measureViewport() {
  const ratio = window.devicePixelRatio || 1;
  // Moving to a display with another scale keeps the window size, so resize events alone miss it.
  if (ratio !== devicePixelRatioValue.value || !ratioQuery) { devicePixelRatioValue.value = ratio; if (mounted) watchPixelRatio(); }
  if (viewport.value) viewportSize.value = { width: viewport.value.clientWidth, height: viewport.value.clientHeight };
}
watch(identity, () => {
  attemptedResolution = "";
  scheduleResolution();
  stopPanning();
  reportedFrameError = false;
  after = 0n;
  afterCursor = 0n;
  cursorShape.value = { kind: "default" };
  frameSize.value = { width: 0, height: 0 };
  canvas.value?.getContext("2d")?.clearRect(0, 0, canvas.value.width, canvas.value.height);
  void invalidate();
});
watch([() => props.fit, () => props.panning, () => props.active], () => {
  stopPanning();
  void invalidate();
});
watch(canRun, (value) => {
  syncFramePolling();
  if (!value) {
    stopPanning();
    void invalidate();
  }
});

onMounted(async () => {
  if (!root.value) return;
  preparePluginProtectedMount(root.value);
  ready.value = true;
  mounted = true;
  await nextTick();
  measureViewport();
  if (typeof ResizeObserver !== "undefined" && viewport.value) {
    resizeObserver = new ResizeObserver(measureViewport);
    resizeObserver.observe(viewport.value);
  }
  window.addEventListener("resize", measureViewport);
  syncFramePolling();
  window.addEventListener("blur", visibility);
  document.addEventListener("visibilitychange", visibility);
  observer = new MutationObserver(visibility);
  observer.observe(document.body, {
    childList: true,
    subtree: true,
    attributes: true,
    attributeFilter: ["aria-modal"],
  });
});

onBeforeUnmount(() => {
  mounted = false;
  cancelScheduledPull();
  clearTimeout(resolutionTimer);
  void invalidate();
  observer?.disconnect();
  resizeObserver?.disconnect();
  window.removeEventListener("resize", measureViewport);
  ratioQuery?.removeEventListener?.("change", measureViewport);
  ratioQuery = null;
  window.removeEventListener("blur", visibility);
  document.removeEventListener("visibilitychange", visibility);
});

async function clipboard(text: string) {
  if (props.panning) return false;
  await acquire();
  return send({ kind: "clipboard", text });
}

/** Sends a prepared key sequence in order after acquiring control the same way the clipboard path does. */
async function sendKeys(events: DesktopInputEvent[]) {
  if (props.panning || !events.length) return false;
  await acquire();
  if (!epoch) return false;
  flushPendingInput();
  const results = await Promise.all(events.map((event) => send(event)));
  return results.every(Boolean);
}

defineExpose({ invalidate, clipboard, sendKeys });
</script>
<template>
  <div
    ref="root"
    class="desktop-display"
    data-plugin-protected
  >
    <template v-if="ready">
      <div
        ref="viewport"
        class="desktop-display__viewport"
        :class="{
          'desktop-display__viewport--fit': fit,
          'desktop-display__viewport--pan': panning,
          'desktop-display__viewport--panning': panningNow,
        }"
      >
        <canvas
          ref="canvas"
          :style="canvasStyle"
          tabindex="0"
          :aria-label="t(panning ? 'desktop.panFocus' : 'desktop.focus')"
          @pointerdown="pointer"
          @pointerup="pointer"
          @pointermove="pointer"
          @pointercancel="pointerCancel"
          @keydown="key($event, true)"
          @keyup="key($event, false)"

          @wheel="wheel"
          @contextmenu.prevent
          @focus="!panning && inputSink?.focus()"
        />
      </div>
      <textarea
        ref="inputSink"
        class="desktop-display__input"
        data-desktop-input
        :aria-label="t('desktop.focus')"
        autocomplete="off"
        autocapitalize="off"
        :spellcheck="false"
        @focus="acquire"
        @keydown="key($event, true)"
        @keyup="key($event, false)"
        @compositionend="composed"
        @blur="invalidate"
      />
      <span
        v-if="!controlled"
        class="desktop-display__hint"
      >{{ t(panning ? 'desktop.panFocus' : 'desktop.focus') }}</span>
    </template>
  </div>
</template>
<style scoped>
.desktop-display {
  position: relative;
  min-height: 0;
  height: 100%;
  overflow: hidden;
  background: var(--nvx-color-terminal-bg);
}

.desktop-display__input {
  position: absolute;
  left: 0;
  bottom: 0;
  width: 1px;
  height: 1px;
  padding: 0;
  opacity: 0;
  resize: none;
}

.desktop-display:focus-within {
  outline: 2px solid var(--nvx-color-accent);
  outline-offset: -2px;
}

.desktop-display__viewport {
  width: 100%;
  height: 100%;
  overflow: auto;
  display: grid;
  place-items: start center;
}

.desktop-display__viewport--fit {
  place-items: center;
}

.desktop-display__viewport--pan {
  display: block;
}

.desktop-display__viewport--pan canvas {
  cursor: grab;
}

.desktop-display__viewport--panning canvas {
  cursor: grabbing;
}

canvas {
  display: block;
  outline: none;
}

canvas:focus-visible {
  outline: 2px solid var(--nvx-color-accent);
  outline-offset: -2px;
}

.desktop-display__hint {
  position: absolute;
  bottom: var(--nvx-space-3);
  left: 50%;
  transform: translateX(-50%);
  padding: var(--nvx-space-1) var(--nvx-space-3);
  border-radius: var(--nvx-radius-md);
  background: var(--nvx-color-bg-surface);
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-xs);
  pointer-events: none;
}
</style>
