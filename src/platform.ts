export type DesktopPlatform = "macos" | "windows" | "other";

export function detectDesktopPlatform(userAgent = navigator.userAgent): DesktopPlatform {
  if (/Macintosh|Mac OS X/i.test(userAgent)) return "macos";
  if (/Windows/i.test(userAgent)) return "windows";
  return "other";
}
