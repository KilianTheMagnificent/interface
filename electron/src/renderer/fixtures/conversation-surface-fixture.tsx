import { useState } from "react";
import { flushSync } from "react-dom";
import { createRoot, type Root } from "react-dom/client";
import { FRESH_HOLD_MS, Markdown } from "../markdown";
import { AgentTile } from "../components/AgentTile";
import { ConversationPaneSurface } from "../components/ConversationPaneSurface";
import { ProjectedConversation } from "../components/ProjectedConversation";
import type { ConversationProps } from "../components/Conversation";
import type { EngineEvent, SessionSummary } from "../../shared/engine";
import { APP_SELECT_CSS } from "../components/AppSelect";
import { createDraftStore } from "../draft-store";
import { assert, frame, frames as settled, noop, wait } from "./fixture-support";

const titles = ["Refine the conversation panes", "Review the keyboard flow", "Check narrow layouts", "Inspect the changed files"];
const summaries: SessionSummary[] = titles.map((title, i) => ({
  title, engine: i % 2 ? "claude" : "codex", sessionId: `surface-${i}`, kind: "code",
  model: i % 2 ? "claude-opus-4-6" : "gpt-6-astra", status: i === 1 ? "waiting" : "working",
  cwd: "/fixture/project", cloud: false, archived: false,
  firstSeen: "2026-09-07T10:00:00.000Z", lastActivity: "2026-09-07T10:02:00.000Z",
}));
const events: EngineEvent[][] = summaries.map((_, i) => [
  { kind: "user.message", text: titles[i] + ". Keep the existing project behavior." },
  ...Array.from({ length: 12 }, (_, j): EngineEvent[] => [
    { kind: "message.delta", text: `I’m checking ${j === 0 ? "the pane layout" : "the next interaction"}. Each conversation should keep its own draft and reading position while the other panes update.\n\n` },
    { kind: "tool.call", callId: `call-${i}-${j}`, tool: "Read", summary: "Read src/panes.tsx", input: { path: "src/panes.tsx" } },
    { kind: "tool.result", callId: `call-${i}-${j}`, ok: true, summary: "Read the component" },
  ]).flat(),
  { kind: "message.delta", text: "The pane spacing is consistent. Code remains distinct from prose:\n\n```ts\nconst pane = conversations.get(sessionId);\n```\n\nI’m checking the final interaction states." },
  ...(i === 2 ? [
    { kind: "subagent.started", agentKey: "surface-scout", subagentType: "Scout", description: "Check the pane layout" },
    { kind: "tool.call", agentKey: "surface-scout", callId: "scout-read", tool: "Read", summary: "src/panes.tsx", input: { path: "src/panes.tsx" } },
    { kind: "tool.result", agentKey: "surface-scout", callId: "scout-read", ok: true, summary: "Read the pane component" },
    { kind: "approval.requested", agentKey: "surface-scout", approvalId: "surface-nested-approval", tool: "Bash", summary: "Run the nested layout check", input: { command: "npm test" } },
    { kind: "subagent.started", agentKey: "surface-reviewer", subagentType: "Reviewer", description: "Review keyboard focus" },
    { kind: "message.delta", agentKey: "surface-reviewer", text: "Focus stays with the selected pane." },
    { kind: "subagent.finished", agentKey: "surface-reviewer", ok: true, summary: "Keyboard review complete" },
    { kind: "subagent.started", agentKey: "surface-checker", subagentType: "Checker", description: "Verify the build" },
    { kind: "message.delta", agentKey: "surface-checker", text: "The build check reported an error." },
    { kind: "subagent.finished", agentKey: "surface-checker", ok: false, summary: "Build check failed" },
  ] as EngineEvent[] : []),
  ...(i === 1 ? [{ kind: "approval.requested", approvalId: "surface-approval", tool: "Bash", summary: "Run the focused checks", input: { command: "npm test" } } as EngineEvent] : []),
]);

// Tick 1 is one short sentence. Later ticks stream a paragraph long enough to
// scroll the pane and to leave a paced reveal visibly behind; tick 3 also lands
// a notice, which a covered pane must draw at once.
const COVERED_TEXT = "Streamed while the pane was covered. ".repeat(14) + "Covered stream ends here.";
function streamedEvents(tick: number): EngineEvent[] {
  if (tick === 0) return [];
  const delta: EngineEvent = { kind: "message.delta",
    text: tick === 1 ? " Stream update 1." : ` Stream update ${tick}.\n\n${COVERED_TEXT}` };
  return tick < 3 ? [delta] : [delta, { kind: "notice", tone: "info", message: "Covered notice" }];
}

function ConversationSurfaceFixture({ base, onSend, onApprove }: {
  base: ConversationProps; onSend?: (pane: number, text: string) => void; onApprove?: (id: string) => void;
}) {
  const [workspaceKind, setWorkspaceKind] = useState<"conversation" | "terminal">("conversation");
  const [focused, setFocused] = useState(0);
  // The shell's one draft store, as App holds it: each pane reads its own session.
  const [drafts] = useState(() => createDraftStore());
  const [maximized, setMaximized] = useState<number | null>(null);
  const [tick, setTick] = useState(0);
  return <div style={{ height: "100vh", display: "flex", flexDirection: "column", minWidth: 0 }}>
    <style>{APP_SELECT_CSS}</style>
    <div style={{ display: "flex", flexWrap: "wrap", gap: 12, padding: "8px 12px", alignItems: "center" }}>
      <nav aria-label="Fixture workspaces" style={{ display: "flex", gap: 8 }}>
        <button className="btn fixture-workspace" onClick={() => { setWorkspaceKind("conversation"); }}>Conversation workspace</button>
        <button className="btn fixture-workspace" onClick={() => { setWorkspaceKind("terminal"); }}>Terminal workspace</button>
      </nav>
      <button className="btn" onClick={() => { setTick(value => value + 1); }}>Stream update</button>
    </div>
    <div className="workspace-content" style={{ flex: 1, minHeight: 0 }}>
      <ConversationPaneSurface visible={workspaceKind === "conversation"}>
        <div className={`workspace-grid workspace-count-4${maximized === null ? "" : " has-maximized"}`}>
          {summaries.map((summary, i) => <AgentTile key={summary.sessionId} session={summary}
            engineLabel={i % 2 ? "Claude Code" : "Codex"} live selected={focused === i}
            maximized={maximized === i} onFocus={() => { setFocused(i); }}
            onToggleMaximize={() => { setFocused(i); setMaximized(maximized === i ? null : i); }}
            onClose={noop} onSplit={noop} splitDisabled>
              <ProjectedConversation {...base} sessionKey={`${summary.engine}#${summary.sessionId}`}
                events={i === 3 && tick ? [...events[i], ...streamedEvents(tick)] : events[i]}
                drafts={drafts}
                focused={workspaceKind === "conversation" && focused === i}
                thinking={false} live workingLabel="Reviewing changes"
                config={{ ...base.config, engine: summary.engine, engineLabel: i % 2 ? "Claude Code" : "Codex",
                  model: summary.model, effort: "high", showEffort: true,
                  models: [{ value: summary.model!, displayName: summary.model!, supportsEffort: true, effortLevels: ["low", "high"] }],
                  canSetModel: true, canSetEffort: true, effortLevels: ["low", "high"] }}
                activeApprovalIds={new Set(i === 1 ? ["surface-approval"] : i === 2 ? ["surface-nested-approval"] : [])}
                onApprove={id => { onApprove?.(id); }}
                onSend={text => { onSend?.(i, text); }} />
          </AgentTile>)}
        </div>
      </ConversationPaneSurface>
      {workspaceKind === "terminal" && <div className="empty">Terminal workspace preview</div>}
    </div>
  </div>;
}

export async function conversationSurfaceInteractions(root: Root, base: ConversationProps): Promise<unknown> {
  document.documentElement.dataset.chrome = "refined";
  let sent = ""; let approved = "";
  flushSync(() => root.render(<ConversationSurfaceFixture base={base} onSend={(pane, text) => { sent = `${pane}:${text}`; }}
    onApprove={id => { approved = id; }} />));
  await settled();
  const firstPane = document.querySelector<HTMLElement>('.workspace-agent')!;
  const draft = firstPane.querySelector<HTMLTextAreaElement>('.conv-ta')!;
  const log = firstPane.querySelector<HTMLElement>('[role="log"]')!;
  assert(log.scrollHeight > log.clientHeight, "Conversation transcript is not scrollable");
  log.scrollTop = 80; log.dispatchEvent(new Event("scroll"));
  Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!.call(draft, "Keep this draft");
  draft.dispatchEvent(new Event("input", { bubbles: true }));
  await settled();
  const scroll = log.scrollTop;
  const switchTo = (name: string): void => {
    [...document.querySelectorAll<HTMLButtonElement>('.fixture-workspace')]
      .find(button => button.textContent === name)!.click();
  };
  switchTo("Terminal workspace"); await settled();
  assert(firstPane.closest('.workspace-surface')?.hasAttribute("inert"), "Hidden conversation panes remain interactive");
  switchTo("Conversation workspace"); await settled();
  assert(firstPane === document.querySelector('.workspace-agent'), "Workspace selection remounted a pane");
  assert(draft.value === "Keep this draft", "Workspace selection lost the draft");
  assert(Math.abs(log.scrollTop - scroll) < 2, "Workspace selection lost the reading position");
  assert(![...document.querySelectorAll<HTMLTextAreaElement>('.conv-ta')].slice(1).some(input => input.value), "Draft crossed session identity");
  [...document.querySelectorAll<HTMLButtonElement>('button')].find(button => button.textContent === "Stream update")!.click();
  await settled();
  assert(Math.abs(log.scrollTop - scroll) < 2, "Sibling streaming moved the reader");
  draft.focus();
  draft.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
  await settled();
  assert(sent === "0:Keep this draft", `Composer sent to the wrong session: ${sent}`);
  const allow = [...document.querySelectorAll<HTMLButtonElement>('button')]
    .find(button => button.textContent?.trim().startsWith("Allow"));
  assert(allow && !allow.disabled, "Conversation pane lost its approval control");
  allow.click(); await settled();
  assert(approved === "surface-approval", "Approval did not reach its session callback");

  // A covered pane does no per-frame work: streamed text waits until the pane is
  // shown, then arrives whole on the showing commit rather than replaying at
  // reading pace, and a pane that was following is still at its newest line.
  // Motion is on for this part, since reduced motion never paces anything.
  const streaming = document.querySelectorAll<HTMLElement>('.workspace-agent')[3]!;
  const streamingLog = streaming.querySelector<HTMLElement>('[role="log"]')!;
  const streamUpdate = [...document.querySelectorAll<HTMLButtonElement>('button')]
    .find(button => button.textContent === "Stream update")!;
  const following = (): boolean =>
    streamingLog.scrollHeight - streamingLog.scrollTop - streamingLog.clientHeight <= 40;
  const matchMedia = window.matchMedia;
  window.matchMedia = ((query: string) => query.includes("prefers-reduced-motion")
    ? { matches: false, media: query, onchange: null, addEventListener: noop,
      removeEventListener: noop, addListener: noop, removeListener: noop, dispatchEvent: () => false }
    : matchMedia.call(window, query)) as typeof window.matchMedia;
  try {
    // The send above re-pinned the first pane; put its reader back up the page.
    log.scrollTop = 80; log.dispatchEvent(new Event("scroll"));
    streamingLog.scrollTop = streamingLog.scrollHeight;
    streamingLog.dispatchEvent(new Event("scroll"));
    await settled();
    const reading = log.scrollTop;
    assert(reading > 0 && following(), "The panes did not start reading and following");
    switchTo("Terminal workspace"); await settled();
    streamUpdate.click(); await settled();
    assert(!streaming.textContent?.includes("Stream update 2."), "A covered pane drew streamed text");
    flushSync(() => { switchTo("Conversation workspace"); });
    assert(streaming.textContent?.includes("Covered stream ends here."),
      "The shown pane replayed text that streamed while it was covered");
    await settled();
    assert(following(), "A following pane lost its newest line across hide and show");
    switchTo("Terminal workspace"); await settled();
    streamUpdate.click(); await settled();
    assert(streaming.textContent?.includes("Covered notice") && streaming.textContent.includes("Stream update 3."),
      "A covered pane held back an event that is not streamed text");
    switchTo("Conversation workspace"); await settled();
    assert(following(), "A following pane lost its newest line after a covered notice");
    assert(Math.abs(log.scrollTop - reading) < 2, "Covered streaming moved a sibling's reader");
    // A reader up the page in the streaming pane itself stays where they were.
    streamingLog.scrollTop = 40; streamingLog.dispatchEvent(new Event("scroll"));
    await settled();
    switchTo("Terminal workspace"); await settled();
    streamUpdate.click(); await settled();
    switchTo("Conversation workspace"); await settled();
    assert(Math.abs(streamingLog.scrollTop - 40) < 2 && streaming.textContent?.includes("Stream update 4."),
      "Hide and show moved a reader in the streaming pane");
  } finally {
    window.matchMedia = matchMedia;
  }
  return { panes: 4, draft: "preserved", scroll: "preserved", send: "scoped", approval: "passed",
    covered: "held, then whole and pinned" };
}

// Everything a message has to be drawn as while it streams. Grown in uneven
// steps into one mounted Markdown, which resumes its parse; each step is
// compared with a fresh mount of the same text, which parses it whole.
const STREAMED_MARKDOWN = [
  "# Findings", "", "The panes keep **their own** drafts. Code stays distinct:", "",
  "```ts", "const pane = panes.get(key);", "", "if (!pane) return;", "```", "",
  "| Area | State |", "| :--- | ---: |", "| Chat | Ready |", "| Review | `a | b` |", "",
  "- first", "- second", "  continued", "", "> A note", ">", "> in two parts", "",
  "Heading | with a pipe", "--- | ---", "x | y", "", "Done :codex-annotation{index=\"2\"}",
].join("\n");

export async function markdownStreamingInteractions(root: Root): Promise<unknown> {
  const reference = document.createElement("div");
  const referenceRoot = createRoot(reference);
  const streamed = document.getElementById("root")!;
  let steps = 0;
  const settling = { most: 0, kept: 0 };
  try {
    for (let end = 1; ; end = Math.min(STREAMED_MARKDOWN.length, end + 1 + steps % 6)) {
      const text = STREAMED_MARKDOWN.slice(0, end);
      flushSync(() => { root.render(<Markdown text={text} />); });
      flushSync(() => { referenceRoot.render(<Markdown key={end} text={text} />); });
      assert(streamed.innerHTML === reference.innerHTML,
        `Streamed Markdown differs from a whole parse at ${end} characters`);
      steps += 1;
      if (end === STREAMED_MARKDOWN.length) break;
    }
    // The same stream as a Threads pane paints it, every step paced: each
    // paint's new words settle in, and its one caret follows the last word
    // (never inside code or a table). Unwrapped and without the caret, the DOM
    // is exactly the whole parse; no span ever sits in code; a span keeps its
    // element from paint to paint (or its fade would restart); and once the
    // paints stop, the spans go and the words are plain text again.
    let previous: Element[] = [];
    for (let end = 1, step = 0; ; end = Math.min(STREAMED_MARKDOWN.length, end + 1 + step % 6)) {
      const text = STREAMED_MARKDOWN.slice(0, end);
      flushSync(() => { root.render(<Markdown key="settle" text={text} settleIn caret />); });
      flushSync(() => { referenceRoot.render(<Markdown key={end} text={text} />); });
      assert(!streamed.querySelector("pre .conv-md-fresh, code .conv-md-fresh"),
        `A settling-in span wrapped code at ${end} characters`);
      assert(streamed.querySelectorAll(".conv-caret").length === 1
        && !streamed.querySelector("pre .conv-caret, code .conv-caret, table .conv-caret"),
        `The Threads caret is not one mark outside code at ${end} characters`);
      const unwrapped = streamed.cloneNode(true) as HTMLElement;
      unwrapped.querySelector(".conv-caret")!.remove();
      for (const span of unwrapped.querySelectorAll(".conv-md-fresh")) span.replaceWith(...span.childNodes);
      assert(unwrapped.innerHTML === reference.innerHTML,
        `Settling in changed the Markdown at ${end} characters`);
      const spans = [...streamed.querySelectorAll(".conv-md-fresh")];
      settling.most = Math.max(settling.most, spans.length);
      settling.kept += previous.filter(span => spans.includes(span)).length;
      previous = spans;
      step += 1;
      if (end === STREAMED_MARKDOWN.length) break;
      if (step % 3 === 0) await frame();
    }
    assert(settling.most > 0, "A paced stream settled no words in");
    assert(settling.kept > 0, "A settling-in span was remounted by the next paint");
    await wait(FRESH_HOLD_MS + 120);
    assert(!streamed.querySelector(".conv-md-fresh"), "Settling-in spans outlived their fade");
  } finally {
    referenceRoot.unmount();
  }
  return { steps, characters: STREAMED_MARKDOWN.length, settling };
}
