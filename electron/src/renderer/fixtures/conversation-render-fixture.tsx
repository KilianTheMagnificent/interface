import type { ReactNode } from "react";
import { flushSync } from "react-dom";
import type { Root } from "react-dom/client";
import type { EngineEvent, ModelOption, SessionSummary } from "../../shared/engine";
import { ConversationComposer, type ConversationComposerProps } from "../components/ConversationComposer";
import {
  ConversationTranscript, type ConversationTranscriptProps,
} from "../components/ConversationTranscript";
import { SearchableItem, ToolRun } from "../components/ConversationRows";
import { ProjectedConversation } from "../components/ProjectedConversation";
import { sessionConfigFor } from "../app-session-projection";
import type { SessionConfigView } from "../conversation-types";
import { createDraftStore } from "../draft-store";
import type { PromptContextItem } from "../prompt-context";
import { assert, frames } from "./fixture-support";

/**
 * The conversation pane's render fan-out, counted per half with real DOM
 * events, the way the sidebar row fixture counts rows: a keystroke draws the
 * composer and no transcript (not the transcript, not one row), and a streamed
 * token draws the transcript's tail and no composer. The parent mints fresh
 * callbacks, a fresh approval Set and a fresh capability object on every
 * render and derives the config through sessionConfigFor, as App's tiles do. Drafts live in one store, as App's do: they
 * survive a session switch and a remount, and two panes of one session share
 * the same draft.
 */
export async function conversationRenderFixture(root: Root): Promise<Record<string, unknown>> {
  document.documentElement.dataset.chrome = "refined";
  // React reads a memo's `type` when it renders, so patch before mounting.
  let transcripts = 0;
  let composers = 0;
  let rows = 0;
  const transcriptType = ConversationTranscript as unknown as {
    type: (props: ConversationTranscriptProps) => ReactNode };
  const composerType = ConversationComposer as unknown as {
    type: (props: ConversationComposerProps) => ReactNode };
  const itemType = SearchableItem as unknown as { type: (props: never) => ReactNode };
  const runType = ToolRun as unknown as { type: (props: never) => ReactNode };
  const drawTranscript = transcriptType.type;
  const drawComposer = composerType.type;
  const drawItem = itemType.type;
  const drawRun = runType.type;
  transcriptType.type = props => { transcripts++; return drawTranscript(props); };
  composerType.type = props => { composers++; return drawComposer(props); };
  itemType.type = props => { rows++; return drawItem(props); };
  runType.type = props => { rows++; return drawRun(props); };
  const reset = (): void => { transcripts = 0; composers = 0; rows = 0; };

  // The session's config comes through sessionConfigFor on every render, as
  // App's tiles get theirs: it keeps its identity while nothing it shows changed.
  const summary = (key: string): SessionSummary => ({
    title: "Render fixture", engine: "codex", sessionId: key.slice("codex#".length), kind: "code",
    model: "gpt-6-astra", status: "working", cwd: "/fixture/render", cloud: false,
    archived: false, firstSeen: "2026-09-27T06:00:00.000Z", lastActivity: "2026-09-27T06:00:00.000Z",
  });
  const models: ModelOption[] = [{ value: "gpt-6-astra", displayName: "gpt-6-astra",
    supportsEffort: true, effortLevels: ["low", "high"] }];
  const configFor = (key: string): SessionConfigView => sessionConfigFor(key, summary(key), events,
    models, "Codex", {}, {}, {}, [], ["default"], [], "default", false, false);
  const drafts = createDraftStore();
  // App's own slices of state keep their identity between renders.
  const IDLE = { status: "idle" } as const;
  const NO_VERDICTS = {};
  const NO_CONTEXT: readonly PromptContextItem[] = [];
  const sent: string[] = [];
  const history: EngineEvent[] = [
    { kind: "user.message", text: "Tidy the parser" },
    ...Array.from({ length: 30 }, (_, index): EngineEvent[] => [
      { kind: "message.delta", text: `Step ${index}: reading the next file.\n\n` },
      { kind: "tool.call", callId: `read-${index}`, tool: "Read", summary: `Read src/${index}.ts`,
        input: { path: `/fixture/render/src/${index}.ts` } },
      { kind: "tool.result", callId: `read-${index}`, ok: true, summary: "Read the file" },
    ]).flat(),
  ];
  let events = history;
  const pane = (key: string): ReactNode =>
    <ProjectedConversation events={events} sessionKey={key} drafts={drafts} config={configFor(key)}
      live thinking workingLabel="Reading" queueWhileBusy={false} focused
      history={IDLE} cwd="/fixture/render" pathRoot="/fixture/render"
      approvals={NO_VERDICTS} activeApprovalIds={new Set<string>()}
      contextItems={NO_CONTEXT} contextCapabilities={{ file: { available: true } }}
      contextOpen={false} onContextOpenChange={open => { void open; }}
      onContextRequest={kind => { void kind; }} onContextRemove={id => { void id; }}
      onSend={text => { sent.push(`${key}:${text}`); }} onStop={() => undefined}
      onSetModel={model => { void model; }} onSetEffort={effort => { void effort; }}
      onApprove={id => { void id; }} onDeny={id => { void id; }}
      onQueue={() => true} onRunCommand={text => { void text; }} />;
  const render = (node: ReactNode): void => {
    flushSync(() => root.render(<div style={{ height: "100vh", display: "flex",
      flexDirection: "column" }}>{node}</div>));
  };
  const textareas = (): HTMLTextAreaElement[] => [...document.querySelectorAll<HTMLTextAreaElement>(".conv-ta")];
  const setValue = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!;
  const type = (area: HTMLTextAreaElement, text: string): void => {
    flushSync(() => { setValue.call(area, text); area.dispatchEvent(new Event("input", { bubbles: true })); });
  };

  try {
    render(pane("codex#render-a"));
    // Let the mount settle first: its own pin fires a scroll event a frame or
    // two later, and the transcript answers that one itself.
    await frames(4);
    assert(transcripts > 0 && composers > 0 && rows > 0, "The pane did not draw both halves");

    // Keystrokes: the composer alone.
    reset();
    const area = textareas()[0]!;
    for (const text of ["F", "Fi", "Fix", "Fix the", "Fix the tests"]) type(area, text);
    await frames();
    assert(area.value === "Fix the tests" && drafts.get("codex#render-a") === "Fix the tests",
      `Typing did not reach the draft: ${area.value}`);
    const keystroke = { transcripts, rows, composers };
    assert(transcripts === 0 && rows === 0,
      `Five keystrokes drew the transcript ${transcripts} times and ${rows} rows`);
    assert(composers >= 5, `Five keystrokes drew the composer ${composers} times`);

    // Streamed tokens: the transcript's tail, and no composer. The parent
    // re-renders with new callbacks and objects on every frame, as App does.
    reset();
    for (let token = 0; token < 6; token++) {
      events = [...events, { kind: "message.delta", text: ` token ${token}` }];
      render(pane("codex#render-a"));
    }
    await frames();
    const streamed = { transcripts, rows, composers };
    assert(composers === 0, `Six streamed tokens drew the composer ${composers} times`);
    assert(transcripts >= 6, `Six streamed tokens drew the transcript ${transcripts} times`);
    assert(rows <= 6, `Six streamed tokens drew ${rows} rows, expected only the tail`);
    assert(area.value === "Fix the tests", "Streaming disturbed the draft");

    // A session switch in place shows B's own draft; switching back finds A's.
    render(pane("codex#render-b"));
    await frames();
    assert(textareas()[0]!.value === "", "Session B showed session A's draft");
    type(textareas()[0]!, "B draft");
    render(pane("codex#render-a"));
    await frames();
    assert(textareas()[0]!.value === "Fix the tests", "Switching back lost session A's draft");
    assert(drafts.get("codex#render-b") === "B draft", "Session B's draft was not kept");

    // A remount keeps it too: the draft lives outside the pane.
    render(null);
    render(pane("codex#render-a"));
    await frames();
    assert(textareas()[0]!.value === "Fix the tests", "A remount lost the draft");

    // Two panes of one session share one draft, as two views of it do in App.
    render(<>{pane("codex#render-a")}{pane("codex#render-a")}</>);
    await frames();
    const [left, right] = textareas();
    assert(left && right, "The two panes did not both draw a composer");
    type(left, "Shared words");
    await frames();
    assert(right.value === "Shared words", `The second pane showed ${right.value}`);

    // Sending still goes through the pane's own session, and empties both.
    flushSync(() => left.dispatchEvent(new KeyboardEvent("keydown",
      { key: "Enter", bubbles: true, cancelable: true })));
    await frames();
    assert(sent.at(-1) === "codex#render-a:Shared words", `The send went to ${sent.at(-1)}`);
    assert(textareas().every(node => node.value === ""), "A send left a draft behind");
    return { keystroke, streamed, drafts: "kept per session, across remounts, shared per session" };
  } finally {
    transcriptType.type = drawTranscript;
    composerType.type = drawComposer;
    itemType.type = drawItem;
    runType.type = drawRun;
    flushSync(() => root.render(null));
  }
}
