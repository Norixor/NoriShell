let installed = false;

export function disableDefaultWebviewContextMenu(): void {
  if (installed) return;
  installed = true;
  // Cancel only the WebView default action; application menus still receive the event.
  document.addEventListener("contextmenu", (event) => event.preventDefault(), true);
}
