/** A late xterm replay callback only focuses the exact current, visible view. */
export function focusTerminalViewIfCurrent<T extends { focus(): void }>(
  handle: T, current: T | undefined, visible: boolean, focused: boolean,
): void {
  if (current === handle && visible && focused) handle.focus();
}
