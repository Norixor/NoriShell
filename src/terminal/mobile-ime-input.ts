/**
 * Phone-only text input pipeline for the xterm helper textarea.
 *
 * Android soft keyboards (Gboard and others) deliver almost every character as
 * a keyCode-229 keydown followed by `beforeinput`/`input`, and the order of
 * those events relative to keyup varies between keyboards and versions. xterm's
 * own 229 path reads the textarea before and after a timer and sends the string
 * difference, which duplicates or drops text when the IME and xterm disagree on
 * the textarea contents, and its composition state can stay "composing" when a
 * compositionend never arrives, swallowing every later key.
 *
 * On phones this controller owns the text path instead: it intercepts input and
 * composition events on an ancestor in the capture phase (so xterm's textarea
 * listeners never see them), computes the exact edit of each event from the
 * textarea contents captured just before it, and hands the committed text to
 * `deliver` once. Composition text is only previewed until the IME commits it.
 * The textarea is emptied whenever the IME is idle, so autocorrect or
 * prediction never has earlier words to rewrite. Physical keys with real key
 * codes (Enter, Backspace, arrows, Ctrl combinations) still go through xterm's
 * keydown path, so desktop-style key encoding is unchanged.
 */
export interface MobileImeInputOptions {
  /** Ancestor that receives capture-phase listeners before xterm's textarea handlers. */
  eventRoot: HTMLElement;
  /** Resolves the xterm helper textarea; only events targeting it are handled. */
  textarea: () => HTMLTextAreaElement | null | undefined;
  /** Receives committed terminal text: typed characters, backspaces and `\r`. */
  deliver: (data: string) => void;
  /** Byte(s) for one deleted character, following the configured Backspace mode. */
  backspace: () => string;
  /**
   * Receives text that must be treated as a paste: anything the IME inserts in one
   * edit that contains a line break or another control character (a keyboard
   * clipboard chip, drag and drop, a multi-line suggestion), and every
   * insertFromPaste/insertFromDrop. The caller routes it through the paste guard
   * so multi-line confirmation and bracketed paste apply; it is never typed.
   */
  paste: (text: string) => void;
  /** Shows the in-progress composition, or hides it with `null`. */
  preview: (text: string | null) => void;
}

export interface MobileImeInputController {
  /** Whether an IME composition is in progress; physical keys must wait for it. */
  isComposing(): boolean;
  /** Commits any pending composition and empties the textarea, e.g. before blur or readOnly changes. */
  flush(): void;
  dispose(): void;
}

/** Splits a string into code points so surrogate pairs are never cut in half. */
function codePoints(value: string) {
  return Array.from(value);
}

/**
 * The terminal edit that turns `before` into `after`: how many characters to
 * delete from the end of the common prefix and which text to type afterwards.
 * Editing in the middle of the textarea cannot happen because the caret always
 * stays at the end of a buffer the controller keeps short.
 */
export function textareaEdit(before: string, after: string): { deleted: number; inserted: string } {
  const previous = codePoints(before);
  const next = codePoints(after);
  let common = 0;
  while (common < previous.length && common < next.length && previous[common] === next[common]) common += 1;
  return { deleted: previous.length - common, inserted: next.slice(common).join("") };
}

// Any C0 control or DEL in IME text means it is not ordinary typing.
// eslint-disable-next-line no-control-regex
const CONTROL_CHARACTER = /[\x00-\x1f\x7f]/;
const LINE_BREAK_ONLY = /^(?:\r\n|\r|\n)$/;
const PASTE_INPUT_TYPES = new Set(["insertFromPaste", "insertFromPasteAsQuotation", "insertFromDrop", "insertFromYank"]);

export function createMobileImeInput(options: MobileImeInputOptions): MobileImeInputController {
  let composing = false;
  // Textarea contents before the current composition started, or before the current input event.
  let compositionBase = "";
  let inputBase: string | null = null;
  // Contents already accounted for after the last delivered edit.
  let delivered = "";
  let clearTimer: ReturnType<typeof setTimeout> | null = null;

  function target(event: Event) {
    const textarea = options.textarea();
    return textarea && event.target === textarea ? textarea : null;
  }

  function stop(event: Event) {
    // Capture on an ancestor runs before xterm's at-target listeners; stopping
    // here keeps xterm's CompositionHelper and input handler out of the text path.
    event.stopImmediatePropagation();
    event.stopPropagation();
  }

  /**
   * Sends one textarea edit. Only a lone line break (the IME's Enter key) maps to
   * `\r`; any other text with line breaks or control characters is a paste, so a
   * keyboard clipboard chip can never run commands line by line without the
   * paste guard's confirmation.
   */
  function deliverEdit(before: string, after: string, forcePaste = false) {
    const { deleted, inserted } = textareaEdit(before, after);
    delivered = after;
    const erase = options.backspace().repeat(deleted);
    if (!inserted) {
      if (erase) options.deliver(erase);
      return;
    }
    if (!forcePaste && LINE_BREAK_ONLY.test(inserted)) {
      options.deliver(erase + "\r");
      return;
    }
    if (forcePaste || CONTROL_CHARACTER.test(inserted)) {
      if (erase) options.deliver(erase);
      options.paste(inserted);
      return;
    }
    options.deliver(erase + inserted);
  }

  function clearNow(textarea: HTMLTextAreaElement) {
    if (clearTimer !== null) clearTimeout(clearTimer);
    clearTimer = null;
    textarea.value = "";
    delivered = "";
  }

  function scheduleClear() {
    if (clearTimer !== null) clearTimeout(clearTimer);
    // Wait for the IME's current batch to finish: clearing between a commit and
    // the next composition would make the keyboard drop its own state.
    clearTimer = setTimeout(() => {
      clearTimer = null;
      const textarea = options.textarea();
      if (!textarea || composing) return;
      if (textarea.value) textarea.value = "";
      delivered = "";
    }, 0);
  }

  function finishComposition(textarea: HTMLTextAreaElement) {
    composing = false;
    options.preview(null);
    deliverEdit(compositionBase, textarea.value);
    scheduleClear();
  }

  function handleCompositionStart(event: Event) {
    const textarea = target(event);
    if (!textarea) return;
    stop(event);
    if (clearTimer !== null) {
      clearTimeout(clearTimer);
      clearTimer = null;
    }
    composing = true;
    // xterm may have cleared the textarea itself (Enter, blur); the DOM is the
    // source of truth for what the composition will replace.
    compositionBase = textarea.value;
    options.preview("");
  }

  function handleCompositionUpdate(event: Event) {
    const textarea = target(event);
    if (!textarea) return;
    stop(event);
    if (!composing) {
      composing = true;
      compositionBase = delivered;
    }
    options.preview((event as CompositionEvent).data ?? "");
  }

  function handleCompositionEnd(event: Event) {
    const textarea = target(event);
    if (!textarea) return;
    stop(event);
    if (!composing) return;
    finishComposition(textarea);
  }

  function handleBeforeInput(event: Event) {
    const textarea = target(event);
    if (!textarea) return;
    stop(event);
    const input = event as InputEvent;
    if (PASTE_INPUT_TYPES.has(input.inputType) && input.cancelable) {
      // Take the clipboard or dropped text directly and keep it out of the textarea.
      event.preventDefault();
      const text = input.dataTransfer?.getData("text/plain") ?? input.data ?? "";
      if (text) options.paste(text);
      return;
    }
    if (!composing && !input.isComposing) inputBase = textarea.value;
  }

  function handleInput(event: Event) {
    const textarea = target(event);
    if (!textarea) return;
    stop(event);
    const input = event as InputEvent;
    if (input.isComposing || input.inputType === "insertCompositionText") {
      if (!composing) {
        composing = true;
        compositionBase = inputBase ?? delivered;
      }
      inputBase = null;
      // The textarea is the authoritative composition text when compositionupdate data lags.
      options.preview(textareaEdit(compositionBase, textarea.value).inserted);
      return;
    }
    if (composing) {
      // A non-composition input while composing means the IME ended the
      // composition without a compositionend; never stay stuck composing.
      inputBase = null;
      finishComposition(textarea);
      return;
    }
    const before = inputBase ?? delivered;
    inputBase = null;
    if (PASTE_INPUT_TYPES.has(input.inputType)) {
      // A paste the browser did not let us cancel: send the inserted text as a paste and drop it from the textarea.
      deliverEdit(before, textarea.value, true);
      clearNow(textarea);
      return;
    }
    deliverEdit(before, textarea.value);
    scheduleClear();
  }

  function flush() {
    const textarea = options.textarea();
    if (!textarea) return;
    if (composing) finishComposition(textarea);
    if (clearTimer !== null) {
      clearTimeout(clearTimer);
      clearTimer = null;
    }
    textarea.value = "";
    delivered = "";
    inputBase = null;
  }

  function handleFocusOut(event: Event) {
    if (!target(event)) return;
    // Losing focus commits what the user saw being composed exactly once.
    flush();
  }

  const listeners: Array<[string, (event: Event) => void]> = [
    ["compositionstart", handleCompositionStart],
    ["compositionupdate", handleCompositionUpdate],
    ["compositionend", handleCompositionEnd],
    ["beforeinput", handleBeforeInput],
    ["input", handleInput],
    ["focusout", handleFocusOut],
  ];
  for (const [type, listener] of listeners) options.eventRoot.addEventListener(type, listener, true);

  return {
    isComposing: () => composing,
    flush,
    dispose() {
      for (const [type, listener] of listeners) options.eventRoot.removeEventListener(type, listener, true);
      if (clearTimer !== null) clearTimeout(clearTimer);
      clearTimer = null;
      options.preview(null);
    },
  };
}
