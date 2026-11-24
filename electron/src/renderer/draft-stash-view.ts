/**
 * Stash (T3 section 9), the renderer's half: what a stash row says, the
 * draft a composer hands Rust, and ⌘S. Rust keeps the stashes and decides
 * everything else (bounds, which thread, pop's "stash the composer first").
 */

import type { DraftStashSummary, StashDraft } from "../shared/draft-stash";
import type { PromptContextDescriptor } from "../shared/ipc";
import { normalizePromptContextItems, type PromptContextItem } from "./prompt-context";
import { threadAgeLabel } from "./thread-inbox";

/** A stash row's words: its first line (or what it holds) and its age. */
export function stashRowWords(entry: DraftStashSummary, now: number): { label: string; detail: string } {
  const attachments = entry.items === 0 ? ""
    : `${entry.items} attachment${entry.items === 1 ? "" : "s"}`;
  const label = entry.preview !== "" ? entry.preview : attachments || "Empty draft";
  const age = threadAgeLabel(entry.at, now);
  const when = age === "" ? "" : age === "now" ? "just now" : `${age} ago`;
  return {
    label,
    detail: [when, entry.preview !== "" ? attachments : ""].filter(part => part !== "").join(" · "),
  };
}

/** Whether a composer holds anything to stash: words or chips. */
export function draftHasContent(text: string, items: readonly PromptContextItem[]): boolean {
  return text.trim() !== "" || items.length > 0;
}

/**
 * The draft as Rust stores it: the text as typed and the chips as plain
 * references (no thumbnail, no backend content; Rust rebuilds both).
 */
export function stashDraftOf(text: string, items: readonly PromptContextItem[]): StashDraft {
  return {
    text,
    items: normalizePromptContextItems(items).map(item => ({
      id: item.id, kind: item.kind, label: item.label, value: item.value,
      ...(item.detail === undefined ? {} : { detail: item.detail }),
      ...(item.estimatedTokens === undefined ? {} : { estimatedTokens: item.estimatedTokens }),
    })),
  };
}

/** A restored draft's chips that carry a path, which are granted again before they are drawn. */
export function stashPathItems(items: readonly PromptContextDescriptor[]): PromptContextDescriptor[] {
  return items.filter(item => item.kind === "file" || item.kind === "folder");
}

/** A restored draft's other chips, drawn as they were stashed. */
export function stashOtherItems(items: readonly PromptContextDescriptor[]): PromptContextItem[] {
  return normalizePromptContextItems(items.filter(item => item.kind !== "file" && item.kind !== "folder"));
}

/**
 * The words typed into the composer while Rust worked: what follows `handed`
 * (the text Rust now holds), or null when the owner changed the field some
 * other way, so it cannot be told which words Rust has.
 */
function typedSince(handed: string, now: string): string | null {
  return now.startsWith(handed) ? now.slice(handed.length).trim() : null;
}

/**
 * The composer after a stash: Rust holds `stored`, so those words go and only
 * the ones typed meanwhile stay. Edited some other way, it is left as it is
 * (a word kept twice, never one lost).
 */
export function draftAfterStash(stored: string, now: string): string {
  return typedSince(stored, now) ?? now;
}

/**
 * The composer after a restore: the restored words, then any typed while the
 * restore was on its way (Rust stashed `handed`, the composer's text when it
 * began). Edited some other way, the field's whole text follows them.
 */
export function draftAfterRestore(handed: string, now: string, restored: string): string {
  const typed = typedSince(handed, now) ?? now.trim();
  if (typed === "") return restored;
  return restored.trim() === "" ? typed : `${restored.replace(/\s+$/u, "")}\n\n${typed}`;
}

/** ⌘S (Ctrl+S off the Mac) with no other modifier: the composer's Stash. */
export function stashChord(event: Pick<KeyboardEvent, "key" | "metaKey" | "ctrlKey" | "shiftKey" | "altKey">): boolean {
  return (event.metaKey || event.ctrlKey) && !event.shiftKey && !event.altKey
    && event.key.toLowerCase() === "s";
}
