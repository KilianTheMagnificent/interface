/**
 * Composer references (T3 section 9): text the owner points at goes into a
 * thread's draft as plain text, after whatever is already typed, one blank
 * line apart, as `appendDiffQuote` does for a diff line. Selected text is
 * data: it is never parsed as HTML or Markdown here, only bounded.
 */

/** A quoted reply selection, in characters, before "…". */
export const REPLY_QUOTE_MAX_CHARS = 4_000;
/** A terminal selection, in characters, before the "…" line at its top. */
export const TERMINAL_EXCERPT_MAX_CHARS = 8_000;
/** A terminal pane's title in the excerpt's heading. */
const TERMINAL_TITLE_MAX_CHARS = 80;

/** A draft with `block` after it, one blank line apart, and the caret at its end. */
export interface DraftWrite {
  text: string;
  /** Where the caret goes: just after the block, on the empty line. */
  cursor: number;
}

function appended(draft: string, block: string): DraftWrite {
  const current = draft.replace(/\s+$/u, "");
  const text = current === "" ? block : `${current}\n\n${block}`;
  return { text, cursor: text.length };
}

/** Line endings as one kind, so a selection's lines quote the same everywhere. */
function plainLines(text: string): string {
  return text.replace(/\r\n?/gu, "\n").replace(/ /gu, " ");
}

/**
 * `selection` as a quote block: each line led by "> " (an empty line by ">"),
 * at most {@link REPLY_QUOTE_MAX_CHARS} characters of it (a longer one ends
 * "…"), then an empty line. Null when nothing but whitespace was selected.
 */
export function replyQuoteBlock(selection: string): string | null {
  const text = plainLines(selection).replace(/^\s*\n/u, "").trimEnd();
  if (text.trim() === "") return null;
  const chars = [...text];
  const kept = chars.length > REPLY_QUOTE_MAX_CHARS
    ? `${chars.slice(0, REPLY_QUOTE_MAX_CHARS).join("").trimEnd()}…` : text;
  const quoted = kept.split("\n").map(line => (line.trim() === "" ? ">" : `> ${line}`));
  return `${quoted.join("\n")}\n\n`;
}

/** The thread's draft with `selection` quoted after it; null when it is empty. */
export function appendReplyQuote(draft: string, selection: string): DraftWrite | null {
  const block = replyQuoteBlock(selection);
  return block === null ? null : appended(draft, block);
}

/** A pane's title as one bounded line, without its quotes. */
function terminalTitle(title: string): string {
  const line = title.replace(/[\u0000-\u001f\u007f"]/gu, " ").replace(/\s+/gu, " ").trim();
  const chars = [...line];
  if (chars.length === 0) return "Terminal";
  return chars.length > TERMINAL_TITLE_MAX_CHARS
    ? `${chars.slice(0, TERMINAL_TITLE_MAX_CHARS - 1).join("").trimEnd()}…` : line;
}

/**
 * A terminal selection as a fenced block under `From the terminal "<title>":`.
 * Past {@link TERMINAL_EXCERPT_MAX_CHARS} it is cut from the TOP, so the end
 * (where an error usually is) survives, and the block starts with a "…" line.
 * The fence is longer than any backtick run inside, so output cannot close it.
 */
export function terminalExcerptBlock(title: string, selection: string): string | null {
  const text = plainLines(selection).replace(/^\s*\n/u, "").trimEnd();
  if (text.trim() === "") return null;
  const chars = [...text];
  let body = text;
  if (chars.length > TERMINAL_EXCERPT_MAX_CHARS) {
    const tail = chars.slice(chars.length - TERMINAL_EXCERPT_MAX_CHARS).join("");
    // Start on a whole line when the cut fell inside one.
    const newline = tail.indexOf("\n");
    const whole = newline >= 0 && newline < tail.length - 1 ? tail.slice(newline + 1) : tail;
    body = `…\n${whole}`;
  }
  const longest = Math.max(0, ...[...body.matchAll(/`+/gu)].map(match => match[0].length));
  const fence = "`".repeat(Math.max(3, longest + 1));
  return `From the terminal "${terminalTitle(title)}":\n${fence}\n${body}\n${fence}\n\n`;
}

/** The thread's draft with the terminal selection after it; null when it is empty. */
export function appendTerminalExcerpt(draft: string, title: string,
    selection: string): DraftWrite | null {
  const block = terminalExcerptBlock(title, selection);
  return block === null ? null : appended(draft, block);
}
