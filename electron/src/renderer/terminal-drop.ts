/**
 * Text dragged onto a terminal, made safe to hand to the terminal's paste.
 *
 * A drag's text is set by whatever page or app the drag started in, so it is
 * treated like a paste from a stranger: tabs and line breaks stay, every other
 * control character goes. The terminal's own paste then decides how line
 * breaks reach the program (bracketed when the program asked for that), so a
 * line ending in Return is shown at the prompt, not run.
 */
export function droppedTextAsPaste(text: string): string {
  let kept = "";
  for (const character of text) {
    const code = character.codePointAt(0) ?? 0;
    const control = code < 0x20 || (code >= 0x7f && code <= 0x9f);
    if (!control || character === "\t" || character === "\n" || character === "\r") {
      kept += character;
    }
  }
  return kept;
}
