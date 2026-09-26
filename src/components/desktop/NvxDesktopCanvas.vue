<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { desktopClient } from "../../core-api/desktop-client";
import type { DesktopInputEvent, DesktopSessionSummary } from "../../core-api/generated/core-api";
import { preparePluginProtectedMount } from "../../plugins/hostDomBroker";
import { decodeDesktopFrame, desktopPointerButtons, desktopKey, reservedDesktopKey } from "./input";
const props = defineProps<{
  session: DesktopSessionSummary;
  active: boolean;
  fit: boolean;
  panning: boolean;
}>();
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
const canvasStyle = computed(() => {
  if (!props.fit || props.panning || !frameSize.value.width || !frameSize.value.height
    || !viewportSize.value.width || !viewportSize.value.height) return undefined;
  const scale = Math.min(
    viewportSize.value.width / frameSize.value.width,
    viewportSize.value.height / frameSize.value.height,
  );
  return {
    width: `${frameSize.value.width * scale}px`,
    height: `${frameSize.value.height * scale}px`,
  };
});

let reportedFrameError = false;
let mounted = false;
let frameBusy = false;
let after = 0n;
let timer: ReturnType<typeof setTimeout> | undefined;

let intent = 0;
let epoch: string | null = null;
let inputSequence = 0n;
let pending = 0;
let chain = Promise.resolve();
let focusChain = Promise.resolve();
let pan: {
  pointerId: number;
  x: number;
  y: number;
  left: number;
  top: number;
} | null = null;

const canRun = computed(() => props.active && props.session.state === "running");
const identity = computed(() => `${props.session.id}:${props.session.generation}`);

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

function invalidate() {
  stopPanning();
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

function send(input: DesktopInputEvent) {
  if (!available() || !epoch) return Promise.resolve(false);
  if (pending >= 32) {
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
    .catch(() => {
      if (ticket === intent) {
        void invalidate();
        emit("error");
      }
      return false;
    })
    .finally(() => {
      pending--;
    });
  chain = result.then(() => undefined);
  return result;
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
  if (event.type === "pointermove" && pending > 1) return;
  void send({ kind: "pointer", ...point(event), buttons: desktopPointerButtons(event) });
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
  const input = desktopKey(event, down);
  if (input) void send(input);
}

function composed(event: CompositionEvent) {
  if (props.panning) return;
  if (event.data) void send({ kind: "text", text: event.data });
  if (inputSink.value) inputSink.value.value = "";
}

function wheel(event: WheelEvent) {
  if (props.panning) return;
  event.preventDefault();
  const scale = event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? 400 : 1;
  void send({
    kind: "wheel",
    ...point(event),
    deltaX: Math.max(-32768, Math.min(32767, Math.round(event.deltaX * scale))),
    deltaY: Math.max(-32768, Math.min(32767, Math.round(event.deltaY * scale))),
  });
}

async function pull() {
  if (!mounted) return;
  timer = setTimeout(() => void pull(), 16);
  if (!frameBusy && canRun.value && !document.hidden) {
    frameBusy = true;
    const key = identity.value;
    const session = props.session;
    try {
      const data = await desktopClient.frame(session, String(after));
      if (!mounted || !canRun.value || document.hidden || key !== identity.value) return;
      const frame = decodeDesktopFrame(data, after);
      if (frame && canvas.value) {
        reportedFrameError = false;
        const context = canvas.value.getContext("2d");
        if (!context) return;
        if (frame.base !== 0n && (frame.base !== after || canvas.value.width !== frame.width || canvas.value.height !== frame.height)) {
          after = 0n;
          return;
        }
        if (canvas.value.width !== frame.width || canvas.value.height !== frame.height) {
          canvas.value.width = frame.width;
          canvas.value.height = frame.height;
        }
        if (frameSize.value.width !== frame.width || frameSize.value.height !== frame.height) {
          frameSize.value = { width: frame.width, height: frame.height };
        }
        context.putImageData(new ImageData(frame.rgba, frame.rectWidth, frame.rectHeight), frame.x, frame.y);
        after = frame.sequence;
      }
    } catch {
      if (key === identity.value && canRun.value && !reportedFrameError) {
        reportedFrameError = true;
        emit("error");
      }
    } finally {
      frameBusy = false;
    }
  }
}

function visibility() {
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
  let width = Math.min(8192, Math.max(200, Math.floor(size.width)));
  let height = Math.min(8192, Math.max(200, Math.floor(size.height)));
  if (width * height > 16_777_216) {
    const scale = Math.sqrt(16_777_216 / (width * height));
    width = Math.floor(width * scale);
    height = Math.floor(height * scale);
  }
  if (profile.protocol === "rdp") width -= width % 2;
  return { width, height, key: `${identity.value}:${width}x${height}` };
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
  if (currentWidth === desired.width && currentHeight === desired.height) return;
  resolutionBusy = true;
  try {
    await desktopClient.resolution(session, desired.width, desired.height);
    // Only server frames and snapshots may change the displayed remote size.
  } catch (error) {
    if (mounted && key === identity.value && canRun.value && desiredResolution()?.key === desired.key) emit("resolutionError", error);
  } finally {
    resolutionBusy = false;
    scheduleResolution();
  }
}
watch([identity, canRun, () => props.session.profile.rdpResolutionMode, () => props.session.profile.vncResolutionMode, viewportSize], scheduleResolution);
let observer: MutationObserver | null = null;
let resizeObserver: ResizeObserver | null = null;
function measureViewport() {
  if (viewport.value) viewportSize.value = { width: viewport.value.clientWidth, height: viewport.value.clientHeight };
}
watch(identity, () => {
  attemptedResolution = "";
  scheduleResolution();
  stopPanning();
  reportedFrameError = false;
  after = 0n;
  frameSize.value = { width: 0, height: 0 };
  canvas.value?.getContext("2d")?.clearRect(0, 0, canvas.value.width, canvas.value.height);
  void invalidate();
});
watch([() => props.fit, () => props.panning, () => props.active], () => {
  stopPanning();
  void invalidate();
});
watch(canRun, (value) => {
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
  void pull();
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
  clearTimeout(timer);
  clearTimeout(resolutionTimer);
  void invalidate();
  observer?.disconnect();
  resizeObserver?.disconnect();
  window.removeEventListener("resize", measureViewport);
  window.removeEventListener("blur", visibility);
  document.removeEventListener("visibilitychange", visibility);
});

async function clipboard(text: string) {
  if (props.panning) return false;
  await acquire();
  return send({ kind: "clipboard", text });
}

defineExpose({ invalidate, clipboard });
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
