import { useMemo, useState } from "react";
import type { ProviderCommand } from "../shared/engine";
import { providerCommandDraft } from "../shared/provider-commands";
import {
  providerCommandEnterSubmits, providerCommandMatches, providerCommandMenuCommit,
  providerCommandMenuKeyAction, type ProviderCommandKeyEvent,
} from "./provider-commands";

/** A prompt textarea's keydown, as far as the slash menu reads it. */
export interface ProviderCommandMenuKeyEvent extends ProviderCommandKeyEvent {
  preventDefault(): void;
  stopPropagation(): void;
}

export interface ProviderCommandMenuControl {
  /** The rows the draft matches, or null when the draft is not a command query. */
  matches: readonly ProviderCommand[] | null;
  open: boolean;
  activeIndex: number;
  setActiveIndex: (index: number) => void;
  /** Show the menu again at its first row: an edit, a new catalogue, a new session. */
  reset: () => void;
  /** Hide it until the next edit. */
  close: () => void;
  /** Put a row's text in the draft for the user to finish. */
  insert: (command: ProviderCommand) => void;
  /** Send a row as its canonical command now. */
  run: (command: ProviderCommand) => void;
  /** The menu's share of the textarea's keydown: true when it took the key. */
  onKeyDown: (event: ProviderCommandMenuKeyEvent) => boolean;
}

/**
 * The slash menu both prompt surfaces draw (the launch card and a session's
 * composer): which rows match the draft, which one is active, whether it is
 * showing, its keys, and choosing a row. Each surface keeps its own catalogue,
 * draft, submit and refusal sentences; `setDraft` and `submit` are theirs.
 *
 * Enter on a draft that is already a complete command belongs to the submit,
 * not to the menu, so a typed `/review` runs rather than re-inserting itself.
 */
export function useProviderCommandMenu(options: {
  commands: readonly ProviderCommand[];
  draft: string;
  /** The surface has a catalogue to offer at all. */
  available: boolean;
  setDraft: (text: string) => void;
  submit: (text: string) => void;
  /** Where the caret goes once a row is inserted. */
  focus: () => void;
}): ProviderCommandMenuControl {
  const { commands, draft } = options;
  const [suppressed, setSuppressed] = useState(false);
  const [index, setIndex] = useState(0);
  const matches = useMemo(() => providerCommandMatches(commands, draft), [commands, draft]);
  const open = options.available && !suppressed && matches !== null;
  const activeIndex = matches === null || matches.length === 0
    ? 0 : Math.min(index, matches.length - 1);
  const insert = (command: ProviderCommand): void => {
    options.setDraft(providerCommandDraft(command));
    setSuppressed(true);
    setIndex(0);
    window.requestAnimationFrame(() => { options.focus(); });
  };
  const run = (command: ProviderCommand): void => {
    setSuppressed(true);
    setIndex(0);
    options.submit(providerCommandMenuCommit(command, draft));
  };
  return {
    matches,
    open,
    activeIndex,
    setActiveIndex: setIndex,
    reset: () => { setSuppressed(false); setIndex(0); },
    close: () => { setSuppressed(true); },
    insert,
    run,
    onKeyDown: event => {
      if (!open || (event.key === "Enter" && providerCommandEnterSubmits(commands, draft))) {
        return false;
      }
      const action = providerCommandMenuKeyAction(event, matches?.length ?? 0, activeIndex);
      if (action === null) return false;
      event.preventDefault();
      event.stopPropagation();
      if (action.type === "move") setIndex(action.index);
      else if (action.type === "close") setSuppressed(true);
      else {
        const command = matches?.[action.index];
        if (command !== undefined) {
          if (action.type === "submit") run(command);
          else insert(command);
        }
      }
      return true;
    },
  };
}
