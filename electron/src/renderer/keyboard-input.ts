type KeyInput = Pick<KeyboardEvent, "type" | "key" | "code" | "repeat" | "isComposing"
  | "shiftKey" | "ctrlKey" | "altKey" | "metaKey" | "location">;

const EDITING_KEYS = new Set(["Backspace", "Delete", "Enter", "Tab", "Escape",
  "ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End", "PageUp", "PageDown"]);

/** A physical press can arrive twice on macOS: two non-repeat keydowns,
 * but only one keyup. Filter before app/xterm handlers and default text insertion.
 * Key releases, not a time debounce, distinguish intentional repeated letters. */
export class KeyboardPresses {
  private readonly held = new Map<string, string>();

  reset(): void { this.held.clear(); }

  suppress(input: KeyInput): boolean {
    if (input.type === "keyup") {
      this.held.delete(input.code);
      return false;
    }
    if (input.type !== "keydown") return false;
    // Leave shortcuts, dead keys and IME composition to the native input method.
    // macOS may omit keyup while Command is held, so do not retain that state.
    if (input.isComposing || input.metaKey || input.ctrlKey || input.altKey
        || (input.key.length !== 1 && !EDITING_KEYS.has(input.key))
        || input.code === "" || input.code === "Unidentified") {
      this.reset();
      return false;
    }
    const signature = JSON.stringify([input.key, input.shiftKey, input.location]);
    const duplicate = input.repeat === false && this.held.get(input.code) === signature;
    this.held.set(input.code, signature);
    return duplicate;
  }
}

export function installKeyboardInputGuard(target: Window): () => void {
  const presses = new KeyboardPresses();
  const reset = (): void => { presses.reset(); };
  const handle = (event: KeyboardEvent): void => {
    if (presses.suppress(event)) {
      event.preventDefault();
      event.stopImmediatePropagation();
    }
  };
  target.addEventListener("keydown", handle, true);
  target.addEventListener("keyup", handle, true);
  target.addEventListener("blur", reset);
  target.addEventListener("pagehide", reset);
  target.document.addEventListener("visibilitychange", reset);
  return () => {
    reset();
    target.removeEventListener("keydown", handle, true);
    target.removeEventListener("keyup", handle, true);
    target.removeEventListener("blur", reset);
    target.removeEventListener("pagehide", reset);
    target.document.removeEventListener("visibilitychange", reset);
  };
}
