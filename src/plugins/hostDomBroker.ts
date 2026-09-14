import type {
  PluginHostDomNodeSnapshot,
  PluginHostDomOperation,
  PluginHostDomOperationBatch,
  PluginHostDomSnapshot,
  PluginHostStyleProperty,
  PluginTargetContextHandle,
} from "../core-api/generated/core-api";

const MAX_NODES = 2_048;
const SNAPSHOT_TTL_MS = 30_000;
const DOM_SURFACE_SELECTOR = "[data-plugin-dom-surface]";
const ALLOWED_TAGS = new Set([
  "a", "article", "aside", "button", "dd", "details", "div", "dl", "dt", "footer",
  "h1", "h2", "h3", "h4", "header", "li", "main", "nav", "ol", "output", "p",
  "pre", "section", "small", "span", "strong", "summary", "table", "tbody", "td", "th",
  "thead", "tr", "ul",
]);
const PROTECTED_SELECTOR = [
  "[data-plugin-protected]",
  ".xterm",
  "input",
  "textarea",
  "select",
  "iframe",
  "webview",
  "canvas",
  "[contenteditable]",
].join(",");
const INTERACTIVE_SELECTOR = [
  "a", "button", "input", "textarea", "select", "summary", "iframe", "webview",
  "[role='button']", "[role='link']", "[role='menuitem']", "[role='option']",
  "[tabindex]", "[contenteditable]",
].join(",");
const LANDMARK_TAGS = new Set(["main", "nav", "header", "footer"]);
const STYLE_PROPERTY: Record<PluginHostStyleProperty, string> = {
  color: "color",
  backgroundColor: "background-color",
  borderColor: "border-color",
  borderRadius: "border-radius",
  fontFamily: "font-family",
  fontSize: "font-size",
  fontWeight: "font-weight",
  fontStyle: "font-style",
  letterSpacing: "letter-spacing",
  lineHeight: "line-height",
  textAlign: "text-align",
  textDecoration: "text-decoration",
  padding: "padding",
  margin: "margin",
  gap: "gap",
  width: "width",
  maxWidth: "max-width",
  minWidth: "min-width",
  height: "height",
  maxHeight: "max-height",
  minHeight: "min-height",
};

interface RetainedSnapshot {
  contextHandle: PluginTargetContextHandle;
  revision: string;
  createdAt: number;
  elements: Map<string, HTMLElement>;
}

interface ElementRollback {
  text?: string;
  pluginTextNode?: Text;
  hidden?: boolean;
  attributes: Map<string, string | null>;
  classes: Map<string, boolean>;
  styles: Map<string, string>;
}

const retainedSnapshots = new Map<string, RetainedSnapshot>();
const rollbackByPlugin = new Map<string, Map<HTMLElement, ElementRollback>>();
const pluginApplicationOrder: string[] = [];
const permissionEpochByPlugin = new Map<string, number>();
let nextRevision = 0n;

function snapshotKey(contextHandle: string, revision: string) {
  return `${contextHandle}\u0000${revision}`;
}

function cleanExpiredSnapshots(now = Date.now()) {
  for (const [key, snapshot] of retainedSnapshots) {
    if (now - snapshot.createdAt > SNAPSHOT_TTL_MS) retainedSnapshots.delete(key);
  }
}

function safeToken(value: string | null, maximum: number) {
  if (!value || value.length > maximum || !/^[A-Za-z0-9._:-]+$/.test(value)) return null;
  return value;
}

function safeText(value: string, maximum: number, multiline: boolean) {
  if (value.length > maximum) return null;
  for (const character of value) {
    const code = character.codePointAt(0) ?? 0;
    const bidi = code === 0x61c
      || code === 0x200e
      || code === 0x200f
      || (code >= 0x202a && code <= 0x202e)
      || (code >= 0x2066 && code <= 0x2069);
    const control = code < 0x20 || (code >= 0x7f && code <= 0x9f);
    if (bidi || (control && !(multiline && (character === "\t" || character === "\n")))) {
      return null;
    }
  }
  return value;
}

function directText(element: HTMLElement) {
  const value = Array.from(element.childNodes)
    .filter((node) => node.nodeType === Node.TEXT_NODE)
    .map((node) => node.textContent ?? "")
    .join("");
  return value ? safeText(value, 4_096, true) : null;
}

function attributes(element: HTMLElement) {
  return ["id", "role", "aria-label", "aria-current", "data-plugin-target"]
    .flatMap((name) => {
      const value = element.getAttribute(name);
      const safe = value === null ? null : safeText(value, 512, false);
      return safe === null ? [] : [{ name, value: safe }];
    });
}

function isProtected(element: Element) {
  return element.matches(PROTECTED_SELECTOR)
    || element.closest("[data-plugin-protected]") !== null
    || element.matches(INTERACTIVE_SELECTOR)
    || element.closest(INTERACTIVE_SELECTOR) !== null;
}

function hasProtectedDescendant(element: Element) {
  return element.querySelector(PROTECTED_SELECTOR) !== null
    || element.querySelector(INTERACTIVE_SELECTOR) !== null;
}

function crossesProtectedBoundary(element: Element) {
  return isProtected(element) || hasProtectedDescendant(element);
}

function intersects(element: Element, root: Element) {
  return element === root || element.contains(root) || root.contains(element);
}

function collectNodes(
  element: Element,
  parentHandle: string | null,
  snapshotRevision: string,
  nodes: PluginHostDomNodeSnapshot[],
  elements: Map<string, HTMLElement>,
) {
  if (nodes.length >= MAX_NODES || isProtected(element)) return;
  const tagName = element.tagName.toLowerCase();
  let nextParent = parentHandle;
  // Do not issue a writable handle for an ancestor of a protected subtree.
  // Continue walking its safe siblings, but do not aggregate its text or labels.
  if (element instanceof HTMLElement && ALLOWED_TAGS.has(tagName) && !hasProtectedDescendant(element)) {
    const nodeHandle = `node:${snapshotRevision}:${nodes.length}`;
    const rect = element.getBoundingClientRect();
    nodes.push({
      nodeHandle,
      parentHandle,
      tagName,
      role: safeToken(element.getAttribute("role"), 80),
      directText: directText(element),
      attributes: attributes(element),
      rect: {
        x: Math.round(rect.x),
        y: Math.round(rect.y),
        width: Math.max(0, Math.round(rect.width)),
        height: Math.max(0, Math.round(rect.height)),
      },
    });
    elements.set(nodeHandle, element);
    nextParent = nodeHandle;
  }
  for (const child of element.children) {
    if (nodes.length >= MAX_NODES) break;
    collectNodes(child, nextParent, snapshotRevision, nodes, elements);
  }
}

export function capturePluginHostDomSnapshot(
  contextHandle: PluginTargetContextHandle,
): PluginHostDomSnapshot | null {
  const root = document.getElementById("app");
  if (!root) return null;
  cleanExpiredSnapshots();
  nextRevision += 1n;
  const revision = nextRevision.toString();
  const nodes: PluginHostDomNodeSnapshot[] = [];
  const elements = new Map<string, HTMLElement>();
  const roots = Array.from(root.querySelectorAll(DOM_SURFACE_SELECTOR))
    .filter((candidate) => candidate.parentElement?.closest(DOM_SURFACE_SELECTOR) === null);
  for (const surface of roots) {
    if (nodes.length >= MAX_NODES) break;
    collectNodes(surface, null, revision, nodes, elements);
  }
  if (nodes.length === 0) return null;
  retainedSnapshots.set(snapshotKey(contextHandle, revision), {
    contextHandle,
    revision,
    createdAt: Date.now(),
    elements,
  });
  return {
    contextHandle,
    snapshotRevision: revision,
    nodes,
    truncated: nodes.length >= MAX_NODES,
  };
}

function rollbackRecord(pluginId: string, element: HTMLElement) {
  let plugin = rollbackByPlugin.get(pluginId);
  if (!plugin) {
    plugin = new Map();
    rollbackByPlugin.set(pluginId, plugin);
    pluginApplicationOrder.push(pluginId);
  }
  let record = plugin.get(element);
  if (!record) {
    record = { attributes: new Map(), classes: new Map(), styles: new Map() };
    plugin.set(element, record);
  }
  return record;
}

function canChangeVisibility(element: HTMLElement) {
  return element.id !== "app"
    && !LANDMARK_TAGS.has(element.tagName.toLowerCase())
    && !crossesProtectedBoundary(element);
}

function canChangeText(element: HTMLElement) {
  return canChangeVisibility(element)
    && !element.children.length
    && !["button", "a", "summary"].includes(element.tagName.toLowerCase());
}

function operationElement(
  operation: PluginHostDomOperation,
  retained: RetainedSnapshot,
) {
  const element = retained.elements.get(operation.nodeHandle);
  return element?.isConnected && !crossesProtectedBoundary(element) ? element : null;
}

function touchesInteractiveBoundary(element: HTMLElement) {
  return element.matches(INTERACTIVE_SELECTOR)
    || element.closest(INTERACTIVE_SELECTOR) !== null
    || element.querySelector(INTERACTIVE_SELECTOR) !== null;
}

export function applyPluginHostDomOperations(
  pluginId: string,
  batch: PluginHostDomOperationBatch,
  expectedPermissionEpoch: number,
) {
  if (pluginHostDomEpoch(pluginId) !== expectedPermissionEpoch) return false;
  cleanExpiredSnapshots();
  const key = snapshotKey(batch.contextHandle, batch.snapshotRevision);
  const retained = retainedSnapshots.get(key);
  if (!retained || retained.contextHandle !== batch.contextHandle) return false;
  if ([...retained.elements.values()].some((element) => (
    !element.isConnected || crossesProtectedBoundary(element)
  ))) {
    retainedSnapshots.delete(key);
    return false;
  }
  const resolved = batch.operations.map((operation) => ({
    operation,
    element: operationElement(operation, retained),
  }));
  if (resolved.some(({ operation, element }) => (
    !element
    || touchesInteractiveBoundary(element)
    || (operation.kind === "setText" && !canChangeText(element))
    || (operation.kind === "setHidden" && !canChangeVisibility(element))
    || (operation.kind === "setAttribute" && !["title", "aria-label"].includes(operation.name))
    || ((operation.kind === "addClass" || operation.kind === "removeClass")
      && !operation.className.startsWith("norishell-plugin-"))
  ))) return false;

  for (const { operation, element } of resolved as Array<{
    operation: PluginHostDomOperation;
    element: HTMLElement;
  }>) {
    const rollback = rollbackRecord(pluginId, element);
    if (operation.kind === "setText") {
      rollback.text ??= element.textContent ?? "";
      element.textContent = operation.text;
      rollback.pluginTextNode = element.firstChild instanceof Text ? element.firstChild : undefined;
    } else if (operation.kind === "setAttribute") {
      if (!rollback.attributes.has(operation.name)) {
        rollback.attributes.set(operation.name, element.getAttribute(operation.name));
      }
      element.setAttribute(operation.name, operation.value);
    } else if (operation.kind === "addClass" || operation.kind === "removeClass") {
      if (!rollback.classes.has(operation.className)) {
        rollback.classes.set(operation.className, element.classList.contains(operation.className));
      }
      element.classList.toggle(operation.className, operation.kind === "addClass");
    } else if (operation.kind === "setStyle") {
      const property = STYLE_PROPERTY[operation.property];
      if (!rollback.styles.has(property)) {
        rollback.styles.set(property, element.style.getPropertyValue(property));
      }
      element.style.setProperty(property, operation.value);
    } else {
      rollback.hidden ??= element.hasAttribute("hidden");
      element.toggleAttribute("hidden", operation.hidden);
    }
  }
  retainedSnapshots.delete(key);
  return true;
}

export function pluginHostDomEpoch(pluginId: string) {
  return permissionEpochByPlugin.get(pluginId) ?? 0;
}

function restoreDirectTextWithoutRemovingChildren(element: HTMLElement, rollback: ElementRollback) {
  if (rollback.pluginTextNode?.parentNode === element) {
    rollback.pluginTextNode.remove();
  }
  if (rollback.text) element.insertBefore(document.createTextNode(rollback.text), element.firstChild);
}

function restoreRollback(
  element: HTMLElement,
  rollback: ElementRollback,
  mountedProtectedRoot: Element | null = null,
) {
  const intersectsMountedRoot = mountedProtectedRoot !== null && intersects(element, mountedProtectedRoot);
  const containsProtectedDescendant = hasProtectedDescendant(element);
  if (rollback.text !== undefined) {
    if (containsProtectedDescendant || intersectsMountedRoot) {
      restoreDirectTextWithoutRemovingChildren(element, rollback);
    } else {
      element.textContent = rollback.text;
    }
  }

  // Outside the explicit pre-mount hook, a later protected subtree wins over
  // stale rollback state. Do not let a plugin's old hide/style/class alter it.
  if (containsProtectedDescendant && !intersectsMountedRoot) return;
  if (rollback.hidden !== undefined) element.toggleAttribute("hidden", rollback.hidden);
  for (const [name, value] of rollback.attributes) {
    if (value === null) element.removeAttribute(name);
    else element.setAttribute(name, value);
  }
  for (const [name, present] of rollback.classes) element.classList.toggle(name, present);
  for (const [name, value] of rollback.styles) element.style.setProperty(name, value);
}

function removePluginApplication(pluginId: string) {
  const index = pluginApplicationOrder.indexOf(pluginId);
  if (index >= 0) pluginApplicationOrder.splice(index, 1);
}

/**
 * Call immediately after mounting an empty `data-plugin-protected` shell and
 * before its host-owned data is rendered. It only clears plugin state whose
 * elements intersect that shell, its ancestors, or its existing descendants.
 */
export function preparePluginProtectedMount(root: Element) {
  for (const [key, snapshot] of retainedSnapshots) {
    if ([...snapshot.elements.values()].some((element) => intersects(element, root))) {
      retainedSnapshots.delete(key);
    }
  }
  for (const pluginId of [...pluginApplicationOrder].reverse()) {
    const plugin = rollbackByPlugin.get(pluginId);
    if (!plugin) continue;
    for (const [element, rollback] of [...plugin]) {
      if (!intersects(element, root)) continue;
      restoreRollback(element, rollback, root);
      plugin.delete(element);
    }
    if (plugin.size === 0) {
      rollbackByPlugin.delete(pluginId);
      removePluginApplication(pluginId);
    }
  }
}

export function revertAllPluginHostDom() {
  for (const pluginId of [...pluginApplicationOrder].reverse()) {
    const plugin = rollbackByPlugin.get(pluginId);
    if (!plugin) continue;
    for (const [element, rollback] of plugin) {
      restoreRollback(element, rollback);
    }
  }
  rollbackByPlugin.clear();
  pluginApplicationOrder.length = 0;
  retainedSnapshots.clear();
}

export function invalidatePluginHostDom(pluginId: string) {
  permissionEpochByPlugin.set(pluginId, pluginHostDomEpoch(pluginId) + 1);
  // Host DOM changes can overlap. A global reverse rollback fails closed and
  // prevents one revoked plugin from resurrecting another plugin's old state.
  revertAllPluginHostDom();
}

export function revertPluginHostDom(pluginId: string) {
  invalidatePluginHostDom(pluginId);
}
