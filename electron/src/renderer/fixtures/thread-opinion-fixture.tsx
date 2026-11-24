import { flushSync } from "react-dom";
import { useState } from "react";
import type { Root } from "react-dom/client";
import type { SessionSummary } from "../../shared/engine";
import type { CollaborationStatusDto } from "../../shared/ipc";
import type { OutputPreviewSnapshot } from "../../shared/output-preview";
import type { ThreadTurn, ThreadView } from "../../shared/threads";
import { AgentTile } from "../components/AgentTile";
import { APP_SELECT_CSS } from "../components/AppSelect";
import { Conversation, type ConversationProps } from "../components/Conversation";
import { OutputPreviewPanel, type OutputPreviewPanelProps } from "../components/OutputPreviewPanel";
import { ThreadOpinionCards } from "../components/ThreadOpinionCards";
import { ThreadDiffToggle, ThreadPaneHead } from "../components/ThreadPaneHead";
import type { ConvItem } from "../conversation-reducer";
import { outputPreviewTarget } from "../output-preview";
import { conversationThread } from "../thread-pane";
import type { ThreadLimitOffer, ThreadOpinionCard } from "../thread-opinion";
import { threadPromptDigest } from "../thread-turns";
import { assert, frames, noop, until, wait } from "./fixture-support";

/**
 * Review B4, B5 and E in a real Electron DOM: a project thread that was
 * continued from another provider ("Continued from …") and later switched
 * ("Switched from Codex", protocol 59), its pane ⋯ offering Second opinion
 * from Codex and Switch to Codex, two second opinions above its composer (one
 * waiting for Ask, one reply held until Send to), the offer to switch at the
 * provider's limit, the plan headroom on
 * the model chip, and a Markdown file the turn wrote previewed beside the
 * thread from the changes card's Preview.
 */

const ROOT = "/Users/owner/Developer/Projects/atlas";
const KEY = "claude#5b0c1d2e-3f40-4a51-8b62-c7d8e9f0a1b2";
const SENT = Date.parse("2026-10-14T14:15:00Z");
const NOTES = `${ROOT}/docs/RELEASE.md`;

const turn = (ordinal: number, prompt: string, over: Partial<ThreadTurn> = {}): ThreadTurn => ({
  ordinal, sentAt: SENT + ordinal * 60_000, promptDigest: threadPromptDigest(prompt), hasStart: true,
  hasEnd: true, state: "done", ...over,
});

function status(id: string, state: CollaborationStatusDto["receipt"]["state"],
    extra: Partial<CollaborationStatusDto> = {}): CollaborationStatusDto {
  return {
    receipt: { version: 1, collaborationId: id, revision: 3, state, mode: "consult",
      sourceEngine: "claude", targetEngine: "codex", approvalRequired: true,
      cancellationSafe: state === "awaiting_approval", createdAt: "2026-10-14T14:20:00.000Z",
      updatedAt: "2026-10-14T14:21:00.000Z" },
    sourceSessionKey: KEY, ...extra,
  };
}

const CARDS: readonly ThreadOpinionCard[] = [
  { id: "c-ready", stage: "ready", engine: "codex", label: "Codex", sourceLabel: "Claude Code",
    cancellable: false, status: status("c-ready", "awaiting_peer_review"),
    body: "Two things before this ships:\n\n- `RETRIES` is read before `loadConfig()` runs, so the "
      + "override never applies.\n- The release notes promise five attempts; the test still expects three." },
  { id: "c-review", stage: "review", engine: "codex", label: "Codex", sourceLabel: "Claude Code",
    cancellable: true, status: status("c-review", "awaiting_approval"),
    packet: { bytes: 9_626, omissions: 1, preview: "interface collaboration packet v2\nmode: consult\n…" } },
];

const LIMIT: ThreadLimitOffer = {
  message: "Claude Code hit its usage limit; it resets in 2h 5m.", engine: "codex", label: "Codex",
  headroom: "62% left",
};

const PREVIEW_TEXT = "# Release 0.3.158\n\n## Threads\n\n- **Second opinion** from another provider, "
  + "held until you send it on.\n- **Switch to** another provider at its limit.\n"
  + "- Preview PDFs, images and Markdown beside the thread.\n";

export function clearThreadOpinionFixture(root: Root): void {
  flushSync(() => root.render(null));
}

export async function threadOpinionFixture(root: Root, theme: string, chrome: string,
    pose: "pane" | "menu" = "pane"): Promise<Record<string, unknown>> {
  flushSync(() => root.render(null));
  document.documentElement.dataset.theme = theme;
  document.documentElement.dataset.chrome = chrome;
  const thread: ThreadView = {
    key: KEY, section: "active", createdAt: SENT, woke: false, unread: false, autoSettle: true,
    workspace: { mode: "local", projectRoot: ROOT, branch: "main", owned: false },
    lineage: { parent: "codex#parent", relation: "handoff", keptTurns: 0, at: SENT },
    turns: [turn(0, "Write the release notes for 0.3.158 to docs/RELEASE.md",
      { filesChanged: 1, added: 9, removed: 0, endedAt: SENT + 41_000 })],
    lastRun: { startedAt: SENT, endedAt: SENT + 41_000, outcome: "completed" },
  };
  const items: ConvItem[] = [
    { kind: "user", id: "user-0", text: "Write the release notes for 0.3.158 to docs/RELEASE.md" },
    { kind: "tool", id: "tool-1", tool: "Write", summary: NOTES, input: { file_path: NOTES },
      result: { ok: true, summary: "Wrote 9 lines" } },
    { kind: "diff", id: "diff-1", path: NOTES, callId: "tool-1", added: 9, removed: 0 },
    { kind: "text", id: "text-2", text: "The notes are in `docs/RELEASE.md`.", streaming: false },
  ];
  const session: SessionSummary = {
    engine: "claude", sessionId: KEY.split("#")[1], title: "Write the release notes",
    cwd: ROOT, status: "idle", cloud: false, lastActivity: "2026-10-14T14:16:00Z",
    firstSeen: "2026-10-14T14:15:00Z", archived: false,
  };
  const reads: string[] = [];
  const actions: string[] = [];
  const snapshot = (root: string, path: string): OutputPreviewSnapshot => ({
    root, path, byteLength: PREVIEW_TEXT.length, revision: "9f2c41d07ab3e5c8",
    content: { kind: "text", text: PREVIEW_TEXT, format: "markdown" },
  });
  const api: OutputPreviewPanelProps["api"] = {
    previewOutput: request => {
      if (request.version === 1) {
        reads.push(`${request.action}:${request.path}`);
        return Promise.resolve(request.action === "list"
          ? { kind: "directory", version: 1, root: request.root, path: request.path,
            entries: [{ name: "docs", directory: true }, { name: "README.md", directory: false }] }
          : { kind: "file", version: 1, snapshot: snapshot(request.root, request.path) });
      }
      if (request.action === "listReviews") return Promise.resolve({ kind: "reviews", version: 2, reviews: [] });
      if (request.action === "captureReview") {
        return Promise.resolve({ kind: "review", version: 2, review: {
          id: "review-1", revision: 1, snapshot: snapshot(request.root, request.path), anchor: { kind: "file" },
          note: "", threadKey: null, task: "", updatedAt: 1, delivery: null } });
      }
      return Promise.reject(new Error(`Unexpected preview request ${request.action}`));
    },
  };
  function Harness() {
    const [preview, setPreview] = useState<{ path: string; nonce: number } | null>(null);
    const props: ConversationProps = {
      items, live: true, thinking: false, focused: true, sessionKey: KEY,
      config: {
        engine: "claude", engineLabel: "Claude Code", model: "claude-opus-5", effort: "high",
        models: [{ value: "claude-opus-5", displayName: "Opus 5", supportsEffort: true,
          effortLevels: ["low", "high"] }],
        canSetModel: true, effortLevels: ["low", "high"], showEffort: true, canSetEffort: true,
        pendingModel: false, pendingEffort: false, error: null,
      },
      headroom: { engine: "claude", text: "38% left", tone: "ok",
        title: "Claude Code plan: 62% of the 5-hour window used, resets in 2h 5m" },
      onSend: noop, onStop: noop, onSetModel: noop, onSetEffort: noop,
      onApprove: noop, onDeny: noop, approvals: {}, activeApprovalIds: new Set(),
      pathRoot: ROOT, threadLayout: true, thread: conversationThread(thread),
      onThreadTurnDiff: noop,
      onThreadPreview: path => {
        const target = outputPreviewTarget(path, ROOT);
        if (target !== null) setPreview(current => ({ path: target.path, nonce: (current?.nonce ?? 0) + 1 }));
      },
      beforeComposer: <ThreadOpinionCards cards={CARDS} limit={LIMIT} pending={[]}
        onAction={(card, action) => { actions.push(`${card.id}:${action}`); }}
        onSwitch={offer => { actions.push(`switch:${offer.engine}`); }} />,
    };
    return (
      <div className={`app-grid no-sidebar${preview !== null ? " has-detail has-thread-diff" : ""}`}>
        <style>{APP_SELECT_CSS}</style>
        <main className="workspace-shell" style={{ gridColumn: 2 }}>
          <div className="workspace-content">
            <div className="workspace-grid workspace-count-1">
              <AgentTile session={session} selected maximized={false} live engineLabel="Claude Code"
                projectLabel="atlas" projectRoot={ROOT}
                onFocus={noop} onToggleMaximize={noop} onClose={noop}
                providers={{ offers: { opinion: [{ engine: "codex", label: "Codex" }],
                  switchTo: [{ engine: "codex", label: "Codex" }] }, refusal: null,
                  onSecondOpinion: engine => { actions.push(`opinion:${engine}`); },
                  onSwitchTo: engine => { actions.push(`menu-switch:${engine}`); } }}
                headDetail={<ThreadPaneHead workspace={thread.workspace}
                  lineage={{ relation: "handoff", title: "Fix the login retry", onSelect: noop }}
                  switchedFrom="Codex" />}
                headActions={<ThreadDiffToggle open={false} onToggle={noop} totals={{ added: 9, removed: 0 }} />}>
                <div className="tile-stack"><Conversation {...props} /></div>
              </AgentTile>
            </div>
          </div>
        </main>
        {preview !== null && <div className="detail-pane thread-preview-pane" style={{ gridColumn: 3 }}>
          <OutputPreviewPanel root={ROOT} visible api={api} onClose={() => { setPreview(null); }}
            onOpenFolder={noop} openRequest={preview} />
        </div>}
      </div>
    );
  }
  flushSync(() => root.render(<Harness />));
  await document.fonts.ready;
  await frames(3);
  const head = document.querySelector<HTMLElement>(".workspace-tile-head");
  assert(head?.textContent?.includes("Continued from Fix the login retry"), "The handoff lineage is not said");
  assert(head?.textContent?.includes("Switched from Codex"), "The switch is not said");
  const chip = document.querySelector<HTMLElement>(".thread-composer-model .app-select-trailing");
  assert(chip?.textContent === "38% left", "The model chip does not say the plan headroom");
  const cards = [...document.querySelectorAll<HTMLElement>(".thread-opinion")];
  assert(cards.length === 2, `Expected two second opinions, saw ${String(cards.length)}`);
  assert(cards[0].textContent?.includes("Send to Claude Code"), "The reply is not held for Send to");
  assert(cards[1].textContent?.includes("Ask Codex"), "The packet is not held for Ask");
  assert(document.querySelector(".thread-limit")?.textContent?.includes("Switch to Codex"),
    "The limit offer is missing");
  const previewButton = document.querySelector<HTMLButtonElement>(".thread-file-preview");
  assert(previewButton !== null, "The changes card offers no Preview");
  previewButton.click();
  await until(() => document.querySelector(".output-preview .output-content h1")?.textContent === "Release 0.3.158",
    () => `The preview did not open the notes: ${reads.join(", ")}`);
  assert(reads.includes("read:docs/RELEASE.md"), `The preview read ${reads.join(", ")}`);
  if (pose === "menu") {
    document.querySelector<HTMLButtonElement>(".thread-pane-menu-button")?.click();
    await until(() => document.querySelector(".thread-pane-menu") !== null, "The pane menu did not open");
    const menu = document.querySelector<HTMLElement>(".thread-pane-menu")!;
    assert(menu.textContent?.includes("Second opinion from Codex"), "The menu offers no second opinion");
    assert(menu.textContent?.includes("Switch to Codex"), "The menu offers no Switch to");
    assert(!menu.textContent?.includes("Continue with"), "The menu still offers Continue with");
  }
  // The floating menu's material settles over a few frames in a hidden window.
  await Promise.allSettled(document.getAnimations().map(animation => animation.finished));
  await wait(250);
  await frames(4);
  return { reads, actions, pose };
}
