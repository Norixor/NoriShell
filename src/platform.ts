export type DesktopPlatform = "macos" | "windows" | "linux" | "other";

export function detectDesktopPlatform(userAgent = navigator.userAgent): DesktopPlatform {
  if (/Macintosh|Mac OS X/i.test(userAgent)) return "macos";
  if (/Windows/i.test(userAgent)) return "windows";
  if (/Linux|X11/i.test(userAgent) && !/Android/i.test(userAgent)) return "linux";
  return "other";
}

/** Shortcut profiles exist only for macOS and Windows; Linux shares the Ctrl-based Windows profile. */
export function shortcutProfilePlatform(platform: DesktopPlatform): "macos" | "windows" {
  return platform === "macos" ? "macos" : "windows";
}
