import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { DraftStashView, StashDraft } from "../shared/draft-stash";
import type { DraftStashPorts } from "./use-draft-stash";
import { useDraftStash } from "./use-draft-stash";

const KEY = "claude#thread";

/**
 * The hook's commands, with a draft store the test owns and a host whose
 * answer the test releases: words typed while Rust works land in between.
 */
function harness(initial: string) {
  let draft = initial;
  let release: (() => void) | null = null;
  const pushed: StashDraft[] = [];
  const view = (popped?: StashDraft): DraftStashView => ({
    version: 1, key: KEY, stashes: [],
    ...(popped === undefined ? {} : { popped: { id: "stash-1", preview: "", at: 0, items: 0, draft: popped } }),
  } as DraftStashView);
  const ports: DraftStashPorts = {
    api: {
      draftStash: (command => new Promise<DraftStashView>(resolve => {
        if (command.command === "push") pushed.push(command.draft);
        if (command.command === "pop" && command.current !== undefined) pushed.push(command.current);
        release = () => { resolve(command.command === "pop" ? view({ text: "the stashed words", items: [] }) : view()); };
      })) as DraftStashPorts["api"]["draftStash"],
    },
    getDraft: () => draft,
    setDraft: (_key, text) => { draft = text; },
    getChips: () => [],
    dropChips: () => undefined,
    restoreChips: () => Promise.resolve(null),
    reportError: message => { throw new Error(message); },
  };
  let actions: ReturnType<typeof useDraftStash> | null = null;
  function Probe() { actions = useDraftStash(ports); return null; }
  renderToStaticMarkup(createElement(Probe));
  return {
    actions: actions!,
    type: (text: string) => { draft += text; },
    answer: async () => { release?.(); for (let i = 0; i < 5; i++) await Promise.resolve(); },
    draft: () => draft,
    pushed,
  };
}

describe("stash while the owner keeps typing", () => {
  it("keeps only the words typed while Rust stored the draft, so nothing is in two places", async () => {
    const stash = harness("my long draft about the bug");
    stash.actions.stash(KEY);
    stash.type(" and more");
    await stash.answer();
    expect(stash.pushed.map(draft => draft.text)).toEqual(["my long draft about the bug"]);
    expect(stash.draft()).toBe("and more");
  });

  it("empties the composer when nothing was typed meanwhile", async () => {
    const stash = harness("park this");
    stash.actions.stash(KEY);
    await stash.answer();
    expect(stash.draft()).toBe("");
  });
});

describe("restore while the owner keeps typing", () => {
  it("keeps the words typed while a restore was on its way, after the restored draft", async () => {
    const restore = harness("half written");
    restore.actions.restore(KEY, "stash-1");
    restore.type(" TYPED-DURING-RESTORE");
    await restore.answer();
    expect(restore.pushed.map(draft => draft.text)).toEqual(["half written"]);
    expect(restore.draft()).toBe("the stashed words\n\nTYPED-DURING-RESTORE");
  });

  it("puts the stash back as it was when nothing was typed meanwhile", async () => {
    const over = harness("half written");
    over.actions.restore(KEY, "stash-1");
    await over.answer();
    expect(over.draft()).toBe("the stashed words");
    const empty = harness("");
    empty.actions.restore(KEY, "stash-1");
    await empty.answer();
    expect(empty.draft()).toBe("the stashed words");
  });
});
