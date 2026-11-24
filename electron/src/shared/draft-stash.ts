/**
 * `iface:draftStash` (protocol 63, T3 section 9): a thread's stashed
 * drafts. Rust owns `draft-stash.v1.json` and every rule (its bounds, which
 * thread a key means, pop's "stash the composer first"); the renderer asks
 * and draws. Mirrors `crates/interface-contracts/src/draft_stash.rs`; both
 * read `fixtures/composer-references-v63.json`.
 */

import type { PromptContextDescriptor } from "./ipc.js";

/** A draft as the composer holds it: its text and its chips. */
export interface StashDraft {
  text: string;
  items: PromptContextDescriptor[];
}

export type DraftStashCommand =
  | { command: "list"; key: string }
  | { command: "push"; key: string; draft: StashDraft }
  /** Takes `id` back; a non-empty `current` is stashed in the same commit. */
  | { command: "pop"; key: string; id: string; current?: StashDraft }
  | { command: "delete"; key: string; id: string };

export interface DraftStashSummary {
  id: string;
  /** The first non-blank line, at most 120 characters; "" for attachments only. */
  preview: string;
  /** Milliseconds since the Unix epoch. */
  at: number;
  /** How many attachments it keeps. */
  items: number;
}

export interface DraftStashEntry {
  id: string;
  at: number;
  draft: StashDraft;
}

export interface DraftStashView {
  version: 1;
  /** The thread (its head) the stashes belong to. */
  key: string;
  /** Newest first. */
  stashes: DraftStashSummary[];
  popped?: DraftStashEntry;
}
