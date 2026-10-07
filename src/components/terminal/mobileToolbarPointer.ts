import { isMobileLayout } from "../../platform-host";

/**
 * Pointer-down guard for Pane toolbars on phones. A tap on a toolbar button would
 * move focus away from the xterm textarea and close an open soft keyboard, and the
 * focus restoration afterwards would then reopen it. Cancelling the pointer-down
 * keeps the terminal focused (the button's click still fires), exactly like the
 * helper-key row and the touch-selection toolbar. Text fields inside the toolbar
 * (the search input) must still take focus, and desktop pointer behavior is
 * untouched.
 */
export function keepTerminalFocusOnPointerDown(event: Event) {
  if (!isMobileLayout()) return;
  const target = event.target;
  if (target instanceof Element && target.closest("input, textarea, select, [contenteditable]")) return;
  event.preventDefault();
}
