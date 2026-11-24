import type { TitleSource } from "../shared/threads";

/** About how long a short title runs before it is cut on a word. */
export const SHORT_TITLE_CHARS = 60;

/**
 * A prompt as a short title (owner, 2026-11-15): its first line with the white
 * space collapsed, cut at about 60 characters on a word boundary with an
 * ellipsis. A word longer than the whole budget is cut where the budget ends.
 */
export function shortPromptTitle(prompt: string, max = SHORT_TITLE_CHARS): string {
  const firstLine = prompt.split(/\r?\n/u).find(line => line.trim() !== "") ?? "";
  const line = firstLine.replace(/\s+/gu, " ").trim();
  if ([...line].length <= max) return line;
  const chars = [...line];
  const head = chars.slice(0, max + 1).join("");
  const space = head.lastIndexOf(" ");
  // Cut on the last word boundary unless that throws away most of the budget.
  const cut = space >= max / 2 ? head.slice(0, space) : chars.slice(0, max).join("");
  return `${cut.replace(/[\s,;:.\-–—]+$/u, "")}…`;
}

/**
 * The title a thread is shown by, in its pane head and its inbox row. A title
 * the owner set or one Claude generated is shown as it is; a thread with no
 * title of its own (its title is still its first prompt) shows that prompt
 * shortened. A session Rust has no title source for keeps a provider's own
 * summary as it is and shortens a prompt it fell back to.
 */
export function threadDisplayTitle(title: string, source: TitleSource | undefined,
    fromSummary?: boolean): string {
  if (source === "user" || source === "generated") return title;
  if (source === undefined && fromSummary === true) return title;
  const short = shortPromptTitle(title);
  return short === "" ? title : short;
}
