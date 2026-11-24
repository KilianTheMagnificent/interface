import { flushSync } from "react-dom";
import { useState } from "react";
import type { Root } from "react-dom/client";
import type { DraftStashCommand, DraftStashView } from "../../shared/draft-stash";
import { APP_SELECT_CSS } from "../components/AppSelect";
import { ThreadReferenceDialog } from "../components/ComposerReferenceDialogs";
import { StartedThreads } from "../components/StartedThreads";
import { ThreadOpinionCards } from "../components/ThreadOpinionCards";
import { ThreadScheduleLine } from "../components/ThreadScheduleLine";
import type { ThreadScheduleView } from "../../shared/threads";
import { Conversation, type ConversationProps, type ConversationStash } from "../components/Conversation";
import type { ConvItem } from "../conversation-reducer";
import { createDraftStore } from "../draft-store";
import { normalizePromptContextItems, type PromptContextItem } from "../prompt-context";
import { threadReferenceItem } from "../thread-reference";
import { useDraftStash } from "../use-draft-stash";
import { assert, frames, noop, until } from "./fixture-support";

/**
 * Composer references and stash (T3 section 9) in a real Electron DOM: a
 * reply's text selected with a real Selection and quoted into the draft (the
 * caret after it); ⌘S stashes a draft with an image chip, "Stashed (1)"
 * restores it over a half-typed composer (which is stashed in its place);
 * and the + menu's Thread kind adds a thread chip. The stash is the real
 * hook over a fake `draftStash` that keeps Rust's semantics.
 */
const SESSION = "claude#composer-refs";
const REPLY = "Added login(), which trims the name, and logout(), which clears the session.";
const IMAGE = "/private/tmp/Interface/pasted-images/paste-1.png";
const PREVIEW = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=";

/** Protocol 62's "Next:" line and protocol 57's "Started threads", which sit
 *  above the same composer: the stash control must sit with them sensibly. */
const SCHEDULE: ThreadScheduleView = {
  id: "sched-1", prompt: "Check the build and report failures.", when: { kind: "interval", minutes: 60 },
  enabled: true, createdAt: Date.UTC(2026, 10, 15, 8), nextAt: Date.UTC(2026, 10, 15, 13), setBy: "owner",
  paused: false,
};
/** Protocol 64's Resume at reset: its "Resumes at …" row and the limit line
 *  with Snooze until reset sit above the same composer too. */
const RESUME: ThreadScheduleView = {
  id: "sched-resume", prompt: "Continue from where you stopped.",
  when: { kind: "once", at: Date.UTC(2026, 10, 15, 12, 31) }, enabled: true,
  createdAt: Date.UTC(2026, 10, 15, 11), nextAt: Date.UTC(2026, 10, 15, 12, 31), setBy: "owner",
  paused: false, kind: "resume",
};
const LIMIT_RESET = {
  resetsAt: Date.UTC(2026, 10, 15, 12, 30), message: "Claude Code hit its usage limit; it resets in 30m.",
  resume: false, snooze: true,
};

const items: ConvItem[] = [
  { kind: "user", id: "u0", text: "Add login and logout to src/auth.ts" },
  { kind: "text", id: "a1", streaming: false, text: REPLY },
];

/** Rust's stash, in memory: newest first, pop stashes a non-empty current. */
function fakeStash(calls: DraftStashCommand[]) {
  let next = 1;
  let rows: { id: string; at: number; text: string; items: PromptContextItem[] }[] = [];
  const view = (popped?: DraftStashView["popped"]): DraftStashView => ({
    version: 1, key: SESSION,
    stashes: [...rows].reverse().map(row => ({
      id: row.id, at: row.at, items: row.items.length,
      preview: row.text.split("\n").find(line => line.trim() !== "")?.trim() ?? "",
    })),
    ...(popped === undefined ? {} : { popped }),
  });
  return async (command: DraftStashCommand): Promise<DraftStashView> => {
    calls.push(command);
    await Promise.resolve();
    if (command.command === "push") {
      rows.push({ id: `stash-${next++}`, at: Date.now(), text: command.draft.text, items: command.draft.items });
      return view();
    }
    if (command.command === "pop") {
      const taken = rows.find(row => row.id === command.id);
      if (taken === undefined) throw new Error("That stashed draft is gone.");
      rows = rows.filter(row => row !== taken);
      if (command.current !== undefined) {
        rows.push({ id: `stash-${next++}`, at: Date.now(), text: command.current.text, items: command.current.items });
      }
      return view({ id: taken.id, at: taken.at, draft: { text: taken.text, items: taken.items } });
    }
    if (command.command === "delete") rows = rows.filter(row => row.id !== command.id);
    return view();
  };
}

export async function composerReferencesFixture(root: Root, theme: string): Promise<Record<string, unknown>> {
  flushSync(() => root.render(null));
  document.documentElement.dataset.theme = theme;
  document.documentElement.dataset.chrome = "refined";
  const calls: DraftStashCommand[] = [];
  const contextCalls: string[] = [];
  const errors: string[] = [];
  const drafts = createDraftStore({});
  const draftStash = fakeStash(calls);
  const control = { chips: (_items: PromptContextItem[]): void => undefined };
  const chipsNow = { current: [] as PromptContextItem[] };
  function Harness() {
    const [chips, setChips] = useState<PromptContextItem[]>([]);
    const [picking, setPicking] = useState(false);
    chipsNow.current = chips;
    control.chips = setChips;
    const stash = useDraftStash({
      api: { draftStash },
      getDraft: key => drafts.get(key),
      setDraft: (key, text) => { drafts.set(key, text); },
      getChips: () => chipsNow.current,
      dropChips: (_key, ids) => { setChips(current => current.filter(chip => !ids.has(chip.id))); },
      restoreChips: async (_key, restored) => {
        // App grants paths again and redraws the picture; here the grant is
        // the fixture's and the thumbnail is the one the chip had.
        setChips(current => normalizePromptContextItems([...current, ...restored.map(item =>
          item.value === IMAGE ? { ...item, previewUrl: PREVIEW } : item)]));
        return null;
      },
      reportError: message => { errors.push(message); },
    });
    const state = stash.stashes[SESSION] ?? { entries: [], busy: false };
    const paneStash: ConversationStash = {
      entries: state.entries, busy: state.busy,
      onStash: () => { stash.stash(SESSION); },
      onRestore: id => { stash.restore(SESSION, id); },
      onRemove: id => { stash.remove(SESSION, id); },
    };
    const props: ConversationProps = {
      items, live: true, thinking: false, focused: true, sessionKey: SESSION, drafts,
      threadLayout: true,
      config: {
        engine: "claude", engineLabel: "Claude", model: "claude-opus-5", effort: "high",
        models: [{ value: "claude-opus-5", displayName: "Opus 5", supportsEffort: true,
          effortLevels: ["low", "medium", "high"] }],
        canSetModel: true, effortLevels: ["low", "medium", "high"], showEffort: true,
        canSetEffort: true, pendingModel: false, pendingEffort: false,
        permissionMode: "default", permissionModes: ["default"], canSetPermission: false,
        error: null,
      },
      contextCapabilities: { file: { available: true }, chat: { available: true } },
      contextItems: chips,
      onContextOpenChange: noop,
      onContextRequest: kind => {
        contextCalls.push(kind);
        if (kind === "chat") setPicking(true);
      },
      onContextRemove: id => { setChips(current => current.filter(chip => chip.id !== id)); },
      onSend: noop, onStop: noop, onSetModel: noop, onSetEffort: noop,
      onApprove: noop, onDeny: noop, approvals: {}, activeApprovalIds: new Set(),
      pathRoot: "/repo",
      stash: paneStash,
      beforeComposer: <>
        <StartedThreads line={{ items: [{ key: "codex#helper", title: "Write the login tests",
          status: "running" }], more: 0 }} onOpen={noop} />
        <ThreadScheduleLine line={{ id: RESUME.id, what: "Continue from where you stopped.",
          when: "Resumes at 12:31", byAgent: false, more: [SCHEDULE], schedule: RESUME }}
        now={Date.UTC(2026, 10, 15, 12)} onEdit={noop} onCancel={noop} />
        <ThreadOpinionCards cards={[]} limit={null} pending={[]} onAction={noop} onSwitch={noop}
          reset={LIMIT_RESET} onResume={noop} onSnooze={noop} />
      </>,
    };
    return (
      <div style={{ height: "100vh", display: "flex", flexDirection: "column", minWidth: 0 }}>
        <style>{APP_SELECT_CSS}</style>
        <Conversation {...props} />
        <ThreadReferenceDialog open={picking}
          choices={[{ key: "codex#header", title: "Fix the header", meta: "Codex · 2h" }]}
          onCancel={() => { setPicking(false); }}
          onChoose={key => {
            setPicking(false);
            setChips(current => [...current, threadReferenceItem(
              { key, title: "Fix the header", provider: "Codex", lastActivityAt: 0 }, "chat-1")]);
          }} />
      </div>
    );
  }
  flushSync(() => root.render(<Harness />));
  await frames(3);
  const box = document.querySelector<HTMLElement>(".thread-composer");
  assert(box !== null, "The Threads composer did not mount");
  const field = box.querySelector<HTMLTextAreaElement>("textarea")!;
  const press = (target: HTMLElement): void => { flushSync(() => { target.click(); }); };
  const typeInto = (text: string): void => {
    flushSync(() => {
      Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!.call(field, text);
      field.dispatchEvent(new Event("input", { bubbles: true }));
    });
  };

  // ---- Quote: a real selection inside the reply --------------------------------
  const reply = document.querySelector<HTMLElement>(".conv-text")!;
  const words = [...reply.querySelectorAll<HTMLElement>("p, .conv-md-p")][0] ?? reply;
  const textNode = [...words.childNodes].find(node => node.nodeType === Node.TEXT_NODE
    && (node.textContent ?? "").includes("which trims the name"))!;
  assert(textNode !== undefined, "The reply's words were not drawn as text");
  const start = textNode.textContent!.indexOf("which trims the name");
  const range = document.createRange();
  range.setStart(textNode, start);
  range.setEnd(textNode, start + "which trims the name".length);
  window.getSelection()!.removeAllRanges();
  window.getSelection()!.addRange(range);
  reply.dispatchEvent(new MouseEvent("mouseup", { bubbles: true }));
  await until(() => reply.querySelector(".conv-quote-button") !== null, "Selecting reply text offered no Quote");
  const quote = reply.querySelector<HTMLButtonElement>(".conv-quote-button")!;
  const quoteRect = quote.getBoundingClientRect();
  const replyRect = reply.getBoundingClientRect();
  assert(quoteRect.top >= replyRect.top && quoteRect.height >= 24, "The Quote button is not by the selection");
  press(quote);
  await frames(2);
  assert(field.value === "> which trims the name\n\n", `Quote wrote ${JSON.stringify(field.value)}`);
  assert(document.activeElement === field && field.selectionStart === field.value.length,
    "The caret is not after the quote");
  assert(reply.querySelector(".conv-quote-button") === null, "The Quote button stayed");
  // A second quote goes after the first and what was typed.
  typeInto(`${field.value}Why?`);
  range.setStart(textNode, 0);
  range.setEnd(textNode, "Added login()".length);
  window.getSelection()!.removeAllRanges();
  window.getSelection()!.addRange(range);
  reply.dispatchEvent(new MouseEvent("mouseup", { bubbles: true }));
  await until(() => reply.querySelector(".conv-quote-button") !== null, "A second selection offered no Quote");
  press(reply.querySelector<HTMLButtonElement>(".conv-quote-button")!);
  await frames(2);
  const quoted = "> which trims the name\n\nWhy?\n\n> Added login()\n\n";
  assert(String(field.value) === quoted, `Two quotes wrote ${JSON.stringify(field.value)}`);
  // A selection outside the reply offers nothing.
  window.getSelection()!.selectAllChildren(document.body);
  reply.dispatchEvent(new MouseEvent("mouseup", { bubbles: true }));
  await frames(2);
  assert(reply.querySelector(".conv-quote-button") === null, "A selection beyond the reply offered Quote");
  window.getSelection()!.removeAllRanges();

  // ---- Stash with ⌘S: the draft and its image chip go to Rust ------------------------
  flushSync(() => {
    control.chips([{ id: "img-1", kind: "file", label: "paste-1.png", value: IMAGE, previewUrl: PREVIEW }]);
  });
  await frames();
  assert(box.querySelector(".conv-context-row") !== null, "The image chip was not drawn");
  field.focus();
  flushSync(() => {
    field.dispatchEvent(new KeyboardEvent("keydown", { key: "s", metaKey: true, bubbles: true, cancelable: true }));
  });
  await until(() => field.value === "" && box.querySelector(".thread-composer-stash") !== null,
    () => `⌘S did not stash: ${JSON.stringify(field.value)}, ${JSON.stringify(calls)}`);
  const pushed = calls.at(-1);
  assert(pushed?.command === "push" && pushed.draft.text === quoted
    && pushed.draft.items.length === 1 && pushed.draft.items[0]?.value === IMAGE
    && !("previewUrl" in pushed.draft.items[0]!), `⌘S sent ${JSON.stringify(pushed)}`);
  assert(box.querySelector(".conv-context-row") === null, "The stashed chip stayed in the composer");
  const stashed = box.querySelector<HTMLButtonElement>(".thread-composer-stash button")!;
  assert(stashed.textContent === "Stashed (1)", `The control reads ${stashed.textContent}`);
  // An empty composer's ⌘S stashes nothing.
  const before = calls.length;
  flushSync(() => {
    field.dispatchEvent(new KeyboardEvent("keydown", { key: "s", metaKey: true, bubbles: true, cancelable: true }));
  });
  await frames();
  assert(calls.length === before, "⌘S on an empty composer asked Rust");

  // ---- Restore over a half-typed composer --------------------------------------------
  typeInto("half-typed answer");
  press(stashed);
  await frames();
  const list = box.querySelector<HTMLElement>('[aria-label="Stashed drafts"]');
  assert(list !== null, "Stashed (1) did not open its list");
  const row = list.querySelector<HTMLButtonElement>(".thread-composer-menu-row")!;
  assert(row.textContent?.includes("> which trims the name") === true
    && row.textContent.includes("1 attachment"), `The row reads ${row.textContent}`);
  press(row);
  await until(() => field.value === quoted && box.querySelector(".conv-context-row") !== null,
    () => `Restore left ${JSON.stringify(field.value)}`);
  const popped = calls.at(-1);
  assert(popped?.command === "pop" && popped.current?.text === "half-typed answer",
    `Restore sent ${JSON.stringify(popped)}`);
  await until(() => box.querySelector(".thread-composer-stash button")?.textContent === "Stashed (1)",
    "The half-typed draft was not stashed in the restored one's place");
  const chip = box.querySelector<HTMLElement>(".conv-context-row");
  assert(chip?.textContent?.includes("paste-1.png") === true, "The restored draft lost its image chip");

  // ---- A thread chip from the + menu's Thread kind ---------------------------------
  const plus = box.querySelector<HTMLButtonElement>('button[aria-label="Add context, commands and options"]')!;
  press(plus);
  await frames();
  const menuRows = (): HTMLElement[] => [...box.querySelectorAll<HTMLElement>('[role="menu"] [role="menuitem"]')];
  const stashRow = menuRows().find(menuRow => menuRow.textContent?.includes("Stash draft") === true);
  assert(stashRow !== undefined && stashRow.getAttribute("aria-disabled") !== "true",
    "The + menu does not offer Stash draft");
  press(menuRows().find(menuRow => menuRow.textContent?.startsWith("Add context") === true)!);
  await frames();
  const threadKind = menuRows().find(menuRow => menuRow.textContent?.startsWith("Thread") === true);
  assert(threadKind !== undefined && threadKind.getAttribute("aria-disabled") !== "true",
    "The Thread kind is not offered");
  press(threadKind);
  await until(() => document.querySelector(".composer-ref-choice") !== null, "The thread picker did not open");
  assert(contextCalls.at(-1) === "chat", `The Thread kind asked for ${contextCalls.at(-1)}`);
  press(document.querySelector<HTMLButtonElement>(".composer-ref-choice")!);
  await until(() => box.querySelector(".conv-context-row")?.textContent?.includes("Fix the header") === true,
    "Choosing a thread drew no chip");
  assert(errors.length === 0, `The stash reported ${errors.join(" | ")}`);
  // The lines above the composer and the stash control by + do not overlap:
  // started threads, the schedule line (a resume first: "Resumes at …") and
  // the limit line with Snooze until reset (protocol 64).
  const lines = [".started-threads", ".thread-schedule-line", ".thread-limit"].map(selector =>
    document.querySelector<HTMLElement>(selector)?.getBoundingClientRect());
  assert(lines.every(rect => rect !== undefined), "The started-threads, schedule or limit line is missing");
  assert(document.querySelector(".thread-schedule-line")?.textContent?.startsWith("Resumes at") === true,
    "The resume row does not read Resumes at …");
  assert(document.querySelector(".thread-limit")?.textContent?.includes("Snooze until reset") === true,
    "The limit line offers no Snooze until reset");
  const boxTop = box.getBoundingClientRect().top;
  const stashRect = box.querySelector<HTMLElement>(".thread-composer-stash")!.getBoundingClientRect();
  assert(lines.every(rect => rect!.bottom <= boxTop + 0.5), "A line above the composer overlaps it");
  assert(lines[0]!.bottom <= lines[1]!.top + 0.5, "The started-threads and schedule lines overlap");
  assert(lines[1]!.bottom <= lines[2]!.top + 0.5, "The schedule and limit lines overlap");
  assert(stashRect.top >= boxTop, "Stashed (n) is not inside the composer");
  await frames(2);
  return { theme, quoted, stashCalls: calls.map(call => call.command), chips: chipsNow.current.map(item => item.kind) };
}

export function clearComposerReferencesFixture(root: Root): void {
  flushSync(() => root.render(null));
}
