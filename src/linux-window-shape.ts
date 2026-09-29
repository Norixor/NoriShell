import { detectDesktopPlatform } from "./platform";

export type LinuxWindowShape = "window" | "tab";

/**
 * Linux has no native rounded frame for undecorated windows, so the transparent
 * window is shaped by the page itself. The root attribute drives the CSS in
 * base.css. The maximized/fullscreen attribute (`data-nvx-maximized`) is pushed by
 * Core from the real window state, because a page cannot tell a maximized window
 * from a merely large one. Other platforms keep their native frame.
 */
export function installLinuxWindowShape(shape: LinuxWindowShape): () => void {
  if (detectDesktopPlatform() !== "linux") return () => undefined;
  const root = document.documentElement;
  root.dataset.nvxShape = shape;
  return () => {
    delete root.dataset.nvxShape;
  };
}
