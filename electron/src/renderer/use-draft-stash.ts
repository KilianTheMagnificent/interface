import { useCallback, useRef, useState } from "react";
import type { DraftStashSummary, DraftStashView } from "../shared/draft-stash";
import type { SessionKey } from "../shared/engine";
import type { IfaceApi, PromptContextDescriptor } from "../shared/ipc";
import { draftAfterRestore, draftAfterStash, draftHasContent, stashDraftOf } from "./draft-stash-view";
import { ipcMessage } from "./ipc-message";
import type { PromptContextItem } from "./prompt-context";

/** One thread's stashes as its composer shows them. */
export interface ThreadStashState {
  entries: readonly DraftStashSummary[];
  busy: boolean;
}

/** What the stash needs from App: the draft store, the chips, and drawing restored chips. */
export interface DraftStashPorts {
  api: Pick<IfaceApi, "draftStash">;
  getDraft: (key: SessionKey) => string;
  setDraft: (key: SessionKey, text: string) => void;
  getChips: (key: SessionKey) => readonly PromptContextItem[];
  /** Drop these chips from the composer (they are in Rust now). */
  dropChips: (key: SessionKey, ids: ReadonlySet<string>) => void;
  /** Grant a restored draft's paths again and draw its chips; resolves with a refusal to show, or null. */
  restoreChips: (key: SessionKey, items: readonly PromptContextDescriptor[]) => Promise<string | null>;
  reportError: (message: string) => void;
}

/**
 * Stash (T3 section 9): each thread's stashed drafts, as Rust answers them.
 * Every command answers the thread's whole list. The composer's words and
 * chips leave it only once Rust has them; a refusal leaves them where they
 * were and says why.
 */
export function useDraftStash(ports: DraftStashPorts) {
  const [stashes, setStashes] = useState<Readonly<Record<SessionKey, ThreadStashState>>>({});
  const latest = useRef(ports);
  latest.current = ports;
  const busy = useRef(new Set<SessionKey>());

  const settle = useCallback((key: SessionKey, view: DraftStashView | null) => {
    busy.current.delete(key);
    setStashes(current => ({
      ...current,
      [key]: { entries: view?.stashes ?? current[key]?.entries ?? [], busy: false },
    }));
  }, []);

  const run = useCallback(async (key: SessionKey,
      work: () => Promise<DraftStashView>): Promise<DraftStashView | null> => {
    if (busy.current.has(key)) return null;
    busy.current.add(key);
    setStashes(current => ({ ...current, [key]: { entries: current[key]?.entries ?? [], busy: true } }));
    try {
      const view = await work();
      settle(key, view);
      return view;
    } catch (failure: unknown) {
      settle(key, null);
      latest.current.reportError(ipcMessage(failure));
      return null;
    }
  }, [settle]);

  const load = useCallback((key: SessionKey): void => {
    void run(key, () => latest.current.api.draftStash({ command: "list", key }));
  }, [run]);

  const stash = useCallback((key: SessionKey): void => {
    const ports = latest.current;
    const text = ports.getDraft(key);
    const chips = ports.getChips(key);
    if (!draftHasContent(text, chips)) return;
    void run(key, () => ports.api.draftStash({ command: "push", key, draft: stashDraftOf(text, chips) }))
      .then(view => {
        if (view === null) return;
        // Words typed while Rust stored the draft stay; the stored ones go.
        const now = latest.current.getDraft(key);
        const left = draftAfterStash(text, now);
        if (left !== now) latest.current.setDraft(key, left);
        latest.current.dropChips(key, new Set(chips.map(chip => chip.id)));
      });
  }, [run]);

  const restore = useCallback((key: SessionKey, id: string): void => {
    const ports = latest.current;
    const text = ports.getDraft(key);
    const chips = ports.getChips(key);
    const current = draftHasContent(text, chips) ? stashDraftOf(text, chips) : undefined;
    void run(key, () => ports.api.draftStash({
      command: "pop", key, id, ...(current === undefined ? {} : { current }),
    })).then(async view => {
      const popped = view?.popped;
      if (popped === undefined) return;
      // The composer's draft is in Rust now (pop stashed it in the same
      // commit), so the restored one replaces it; words typed while the pop
      // was on its way are in neither, so they follow the restored ones.
      latest.current.dropChips(key, new Set(chips.map(chip => chip.id)));
      latest.current.setDraft(key, draftAfterRestore(text, latest.current.getDraft(key), popped.draft.text));
      const refused = await latest.current.restoreChips(key, popped.draft.items);
      if (refused !== null) latest.current.reportError(refused);
    });
  }, [run]);

  const remove = useCallback((key: SessionKey, id: string): void => {
    void run(key, () => latest.current.api.draftStash({ command: "delete", key, id }));
  }, [run]);

  return { stashes, load, stash, restore, remove };
}
