/**
 * Resolves once the page has presented the current DOM: the first animation frame
 * runs before that frame's paint, the second after it. Frames stop while a window is
 * minimized or occluded, so `timeoutMs` bounds the wait.
 */
export function afterNextPaint(timeoutMs = 100): Promise<void> {
  return new Promise((resolve) => {
    let settled = false;
    // `finish` runs only after `timer` is assigned: from a later frame, the timer itself,
    // or synchronously below once the timer exists.
    const finish = () => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      resolve();
    };
    const timer = setTimeout(finish, timeoutMs);
    if (typeof requestAnimationFrame !== "function") {
      finish();
      return;
    }
    requestAnimationFrame(() => requestAnimationFrame(finish));
  });
}

/**
 * The page canvas colour as RGB. A native Tab WebView paints it before its first
 * document frame instead of the platform default (white on WebView2).
 */
export function canvasBackgroundRgb(): [number, number, number] | null {
  if (typeof document === "undefined" || typeof getComputedStyle !== "function") return null;
  const value = getComputedStyle(document.documentElement).getPropertyValue("--nvx-color-bg-canvas").trim();
  return parseHexRgb(value);
}

export function parseHexRgb(value: string): [number, number, number] | null {
  const match = /^#([0-9a-f]{6})$/i.exec(value.trim());
  if (!match) return null;
  const hex = match[1]!;
  return [0, 2, 4].map((offset) => Number.parseInt(hex.slice(offset, offset + 2), 16)) as [number, number, number];
}
