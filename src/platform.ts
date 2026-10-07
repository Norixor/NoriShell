export type DesktopPlatform = "macos" | "windows" | "linux" | "android" | "other";

/**
 * Development-only platform override for browser previews: `?nvxPlatform=android`
 * on the Vite dev server renders the Android shell without a device. Production
 * builds and native WebViews ignore it.
 */
function developmentPlatformOverride(): DesktopPlatform | null {
  if (!import.meta.env.DEV || typeof window === "undefined") return null;
  const value = new URLSearchParams(window.location.search).get("nvxPlatform");
  return value === "android" || value === "macos" || value === "windows" || value === "linux" ? value : null;
}

export function detectDesktopPlatform(userAgent = navigator.userAgent): DesktopPlatform {
  const override = developmentPlatformOverride();
  if (override) return override;
  if (/Android/i.test(userAgent)) return "android";
  if (/Macintosh|Mac OS X/i.test(userAgent)) return "macos";
  if (/Windows/i.test(userAgent)) return "windows";
  if (/Linux|X11/i.test(userAgent)) return "linux";
  return "other";
}

/** Shortcut profiles exist only for macOS and Windows; Linux and Android share the Ctrl-based Windows profile. */
export function shortcutProfilePlatform(platform: DesktopPlatform): "macos" | "windows" {
  return platform === "macos" ? "macos" : "windows";
}
