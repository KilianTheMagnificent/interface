import { readFileSync } from "node:fs";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { approvalVerdictKey } from "./approval-keys";
import { Conversation, type ConversationProps } from "./components/Conversation";
import { SteeringQueue } from "./components/SteeringQueue";
import { compositeHex, contrastRatio, evaluateCssColor } from "./contrast";
import type { ConvItem } from "./conversation-reducer";
import { createDraftStore } from "./draft-store";
import { PROJECT_HUE_COUNT } from "./project-hue";
import {
  THREAD_DOCK_DIFF_LINES, threadApprovalPending, threadChangesCue, threadDockApprovals,
  threadDockCommand, threadDockDiff, threadDockFiles, threadDockKind, threadDockMeta,
  threadDockQuestionTitle, threadDockTitle, threadWaitingClock, type ThreadApproval,
} from "./thread-frame";

/* ==========================================================================
   THE FRAME AROUND A THREADS PANE'S LIVE TURN (owner, 2026-10-07): the
   request docked above the composer, what it says and draws, the compact
   queue, and the colours they spend. The composer's own states are in
   components/ThreadComposer.test.ts; the head's in AgentTile.test.ts and
   chrome-mode.test.ts; thread-composer-fixture presses all of it in a real DOM.
   ========================================================================== */

const ask = (tool: string, input: unknown, over: Partial<ThreadApproval> = {}): ThreadApproval => ({
  kind: "approval", id: over.id ?? `ap-${tool}`, tool, summary: over.summary ?? tool, input, ...over,
});

describe("what a docked request asks", () => {
  it("names each kind of request as a question", () => {
    expect(threadDockTitle(ask("Bash", { command: "npm test" }))).toBe("Run a command?");
    // Codex calls its shell approvals `command`.
    expect(threadDockTitle(ask("command", { command: ["npm", "test"] }))).toBe("Run a command?");
    expect(threadDockTitle(ask("Edit", { file_path: "/w/a.ts" }))).toBe("Edit a file?");
    expect(threadDockTitle(ask("MultiEdit", { file_path: "/w/a.ts", edits: [] }))).toBe("Edit a file?");
    expect(threadDockTitle(ask("Write", { file_path: "/w/a.ts", content: "x" }))).toBe("Write a file?");
    expect(threadDockTitle(ask("fileChange", { interfacePaths: ["/w/a.ts", "/w/b.ts"] })))
      .toBe("Edit 2 files?");
    expect(threadDockTitle(ask("WebFetch", { url: "https://example.com" }))).toBe("Allow web fetch?");
    expect(threadDockQuestionTitle(1)).toBe("Answer a question?");
    expect(threadDockQuestionTitle(3)).toBe("Answer 3 questions?");
  });

  it("sorts tools into the body each kind draws", () => {
    expect(threadDockKind("Bash")).toBe("command");
    expect(threadDockKind("command")).toBe("command");
    expect(threadDockKind("Write")).toBe("write");
    expect(threadDockKind("write_file")).toBe("write");
    expect(threadDockKind("Edit")).toBe("edit");
    expect(threadDockKind("fileChange")).toBe("edit");
    expect(threadDockKind("WebFetch")).toBe("other");
    expect(threadDockKind("permissions")).toBe("other");
  });

  it("reads a command whole, as a string or as shell words", () => {
    expect(threadDockCommand({ command: "npm test -- auth\n" })).toBe("npm test -- auth");
    expect(threadDockCommand({ command: ["git", "commit", "-m", "it's done"] }))
      .toBe("git commit -m 'it'\\''s done'");
    expect(threadDockCommand({ cmd: "ls" })).toBe("ls");
    // Codex's stdin writes carry no command: the body shows the input instead.
    expect(threadDockCommand({ kind: "writeStdin" })).toBe(null);
    expect(threadDockCommand(undefined)).toBe(null);
  });

  it("names an edit's file project-relative, with its exact line counts", () => {
    const edit = ask("Edit", { file_path: "/w/src/routes/login.ts",
      old_string: "const a = 1;\nconst b = 2;\n", new_string: "const a = 1;\nconst b = 3;\nconst c = 4;\n" });
    expect(threadDockFiles(edit, "/w")).toEqual([{ path: "/w/src/routes/login.ts",
      shown: "src/routes/login.ts", stat: { added: 2, removed: 1 } }]);
    const multi = ask("MultiEdit", { file_path: "/w/a.ts",
      edits: [{ old_string: "x", new_string: "y" }, { old_string: "p\nq", new_string: "r" }] });
    expect(threadDockFiles(multi, "/w")[0].stat).toEqual({ added: 2, removed: 3 });
  });

  it("never claims a Write removed nothing, and gives Codex's paths no counts", () => {
    const write = ask("Write", { file_path: "/w/new.md", content: "# Title\n\nBody\n" });
    expect(threadDockFiles(write, "/w")).toEqual([{ path: "/w/new.md", shown: "new.md",
      stat: { added: 3 } }]);
    const codex = ask("fileChange", { itemId: "i1", interfacePaths: ["/w/a.ts", "/w/b.ts", "/w/a.ts"] });
    expect(threadDockFiles(codex, "/w")).toEqual([
      { path: "/w/a.ts", shown: "a.ts" }, { path: "/w/b.ts", shown: "b.ts" }]);
    expect(threadDockFiles(ask("Edit", {}), "/w")).toEqual([]);
  });

  it("previews an edit as its removed then added lines, without the shared context", () => {
    const diff = threadDockDiff({ old_string: "keep\nold one\nold two\ntail", new_string: "keep\nnew\ntail" },
      "edit");
    expect(diff).toEqual({ lines: [{ sign: "−", text: "old one" }, { sign: "−", text: "old two" },
      { sign: "+", text: "new" }], more: 0 });
    expect(threadDockDiff({ old_string: "same", new_string: "same" }, "edit")).toBe(null);
    expect(threadDockDiff({ interfacePaths: ["/w/a.ts"] }, "edit")).toBe(null);
  });

  it("previews at most twelve lines and says how many more", () => {
    const content = Array.from({ length: 20 }, (_, index) => `line ${index + 1}`).join("\n");
    const diff = threadDockDiff({ content }, "write");
    expect(THREAD_DOCK_DIFF_LINES).toBe(12);
    expect(diff?.lines).toHaveLength(12);
    expect(diff?.lines[0]).toEqual({ sign: "+", text: "line 1" });
    expect(diff?.more).toBe(8);
  });

  it("counts the wait as m:ss, and says the project when it is known", () => {
    expect(threadWaitingClock(0)).toBe("0:00");
    expect(threadWaitingClock(42_400)).toBe("0:42");
    expect(threadWaitingClock(725_000)).toBe("12:05");
    expect(threadWaitingClock(3_723_000)).toBe("1:02:03");
    expect(threadDockMeta("website", 42_000)).toBe("in website · waiting 0:42");
    expect(threadDockMeta(undefined, 42_000)).toBe("waiting 0:42");
    expect(threadDockMeta(" ", 42_000)).toBe("waiting 0:42");
  });
});

describe("which requests dock, and when the pane is blocked on its owner", () => {
  const nested: ConvItem = { kind: "subagent", id: "agent-1", subagentType: "Explore",
    description: "Look", status: "running", toolCount: 0, subagentCount: 0,
    children: [ask("Bash", { command: "ls" }, { id: "ap-nested" })] };
  const items: ConvItem[] = [ask("Bash", { command: "a" }, { id: "ap-1" }), nested,
    ask("Edit", {}, { id: "ap-2" }), ask("Bash", { command: "old" }, { id: "ap-gone" })];

  it("docks every open request in transcript order, subagents' included", () => {
    const active = new Set(["ap-1", "ap-nested", "ap-2"]);
    expect(threadDockApprovals(items, active).map(item => item.id))
      .toEqual(["ap-1", "ap-nested", "ap-2"]);
    expect(threadDockApprovals(items, new Set())).toEqual([]);
    // History and the live list can both carry one request: it docks once.
    expect(threadDockApprovals([...items, items[0]], active)).toHaveLength(3);
  });

  it("is blocked only on a request the owner may answer and has not", () => {
    const dock = threadDockApprovals(items, new Set(["ap-1"]));
    expect(threadApprovalPending(dock, new Set(["ap-1"]), {}, "claude#a")).toBe(true);
    // The approval policy is still deciding: not the owner's turn yet.
    expect(threadApprovalPending(dock, new Set(), {}, "claude#a")).toBe(false);
    const key = approvalVerdictKey("claude#a", "ap-1");
    expect(threadApprovalPending(dock, new Set(["ap-1"]),
      { [key]: { verdict: "allow" } }, "claude#a")).toBe(false);
    // A failed send is not a verdict: the request still waits.
    expect(threadApprovalPending(dock, new Set(["ap-1"]),
      { [key]: { verdict: "allow", error: "the bridge refused it" } }, "claude#a")).toBe(true);
  });
});

describe("the Changes chip's cue", () => {
  it("moves nothing on the first look, pops when totals arrive, flashes on each change", () => {
    expect(threadChangesCue(undefined, "+3−1", "")).toBe("");
    expect(threadChangesCue(undefined, null, "")).toBe("");
    expect(threadChangesCue(null, null, "")).toBe("");
    expect(threadChangesCue(null, "+3−1", "")).toBe("pop");
    expect(threadChangesCue("+3−1", "+3−1", "pop")).toBe("pop");
    expect(threadChangesCue("+3−1", "+5−1", "pop")).toBe("flash-a");
    expect(threadChangesCue("+5−1", "+6−1", "flash-a")).toBe("flash-b");
    expect(threadChangesCue("+6−1", "+7−1", "flash-b")).toBe("flash-a");
    // Totals going unknown keep the cue; coming back pops again.
    expect(threadChangesCue("+7−1", null, "flash-a")).toBe("flash-a");
    expect(threadChangesCue(null, "+7−1", "flash-a")).toBe("pop");
  });
});

/* ---- the dock, as the pane draws it ------------------------------------ */

const base: ConversationProps = {
  items: [], thinking: true, live: true, sessionKey: "claude#a", threadLayout: true,
  pathRoot: "/w",
  config: { engine: "claude", engineLabel: "Claude", effort: null, models: [], canSetModel: false,
    effortLevels: [], showEffort: false, canSetEffort: false, pendingModel: false,
    pendingEffort: false, error: null },
  onSend: () => undefined, onStop: () => undefined, onSetModel: () => undefined,
  onSetEffort: () => undefined, onApprove: () => undefined, onDeny: () => undefined,
  approvals: {}, activeApprovalIds: new Set<string>(), drafts: createDraftStore({}),
};
const paint = (items: ConvItem[], over: Partial<ConversationProps> = {}): string =>
  renderToStaticMarkup(createElement(Conversation, {
    ...base, items, activeApprovalIds: new Set(threadDockApprovals(items, new Set(
      items.flatMap(item => (item.kind === "approval" ? [item.id] : [])))).map(item => item.id)),
    ...over,
  })).replace(/<style[^>]*>[\s\S]*?<\/style>/gu, "");
const dockOf = (html: string): string =>
  html.slice(html.indexOf('class="thread-dock"'), html.indexOf('class="thread-composer'));

describe("the dock", () => {
  const bash = ask("Bash", { command: "npm test -- auth", description: "Run the auth tests" },
    { id: "ap-1", summary: "npm test -- auth" });

  it("draws a waiting command above the composer, and not in the transcript", () => {
    const html = paint([{ kind: "user", id: "u0", text: "Fix it" }, bash]);
    const dock = dockOf(html);
    expect(dock).toContain('class="thread-dock-card is-pending" role="alert" aria-label="Approval required"'
      + ' data-voice-guard="approval"');
    // Its tile says what it is about: a command, in the Threads icons, amber while it waits.
    expect(dock).toContain('<span class="thread-dock-tile thread-dock-glyph" aria-hidden="true"><svg class="thread-glyph"');
    expect(dock).toContain('<rect x="1.75" y="2.75" width="12.5" height="10.5" rx="2.25"></rect>');
    expect(dock).toContain('<span class="thread-dock-title">Run a command?</span>');
    expect(dock).toContain('<span class="thread-dock-prompt" aria-hidden="true">$ </span>npm test -- auth</pre>');
    // The description is the transcript's business, not a second band here.
    expect(dock).not.toContain("conv-input-label");
    expect(dock).toContain('<span class="thread-dock-meta" aria-hidden="true">waiting 0:00</span>');
    const acts = dock.slice(dock.indexOf('class="thread-dock-acts"'));
    expect(acts.indexOf(">Deny<")).toBeLessThan(acts.indexOf(">Allow once<"));
    expect(acts).toContain('class="btn thread-dock-ghost"');
    expect(acts).toContain('class="btn btn-primary thread-dock-allow"');
    expect(html.slice(0, html.indexOf('class="conv-strip"'))).not.toMatch(/conv-approval|thread-dock/u);
  });

  it("says the project in its footnote when the shell passes it", () => {
    expect(dockOf(paint([bash], { projectLabel: "website" })))
      .toContain(">in website · waiting 0:00</span>");
  });

  it("prints the keys on the card that receives them, and only in the pane that owns them", () => {
    const keyed = dockOf(paint([bash], { focused: true }));
    expect(keyed).toContain('data-approval-key="deny" aria-keyshortcuts="Meta+Backspace Control+Backspace"');
    expect(keyed).toContain('Deny<span class="kbd-hint" aria-hidden="true">⌘⌫</span>');
    expect(keyed).toContain('Allow once<span class="kbd-hint" aria-hidden="true">⌘↩</span>');
    expect(dockOf(paint([bash]))).not.toContain("data-approval-key");
    // Two open requests: the lower card takes the keys, as the listener does.
    const two = dockOf(paint([bash, ask("Bash", { command: "npm run lint" }, { id: "ap-2" })],
      { focused: true }));
    expect(two.match(/data-approval-key="allow"/gu)).toHaveLength(1);
    expect(two.indexOf('data-approval-key="allow"')).toBeGreaterThan(two.indexOf("npm run lint"));
  });

  it("draws an edit as its file, its counts and a small diff — never a content band", () => {
    const dock = dockOf(paint([ask("Edit", { file_path: "/w/src/auth.ts",
      old_string: "return null;", new_string: "return user;\n// signed in" }, { id: "ap-e" })]));
    expect(dock).toContain('<span class="thread-dock-title">Edit a file?</span>');
    expect(dock).toContain('<li class="thread-dock-file" title="/w/src/auth.ts">'
      + '<span class="file-badge kind-ts" aria-hidden="true">TS</span>'
      + '<span class="thread-dock-path">src/auth.ts</span>');
    expect(dock).toContain('<span class="thread-dock-added">+2</span><span class="thread-dock-removed">−1</span>');
    expect(dock).toContain('aria-label="Proposed change"');
    expect(dock).toContain('<div class="thread-dock-line is-removed"><span class="thread-dock-sign">−</span>'
      + '<span class="thread-dock-code">return null;</span></div>');
    expect(dock).toContain('<div class="thread-dock-line is-added"><span class="thread-dock-sign">+</span>'
      + '<span class="thread-dock-code">return user;</span></div>');
    expect(dock).not.toMatch(/conv-input-label|>new_string<|>old_string</u);
  });

  it("draws a Write as its file and the start of what it writes", () => {
    const content = Array.from({ length: 15 }, (_, index) => `# ${index}`).join("\n");
    const dock = dockOf(paint([ask("Write", { file_path: "/w/NOTES.md", content }, { id: "ap-w" })]));
    expect(dock).toContain("Write a file?");
    expect(dock).toContain('<span class="thread-dock-path">NOTES.md</span>');
    expect(dock).toContain('<span class="thread-dock-added">+15</span>');
    expect(dock).not.toContain("thread-dock-removed");
    expect(dock.match(/thread-dock-line is-added/gu)).toHaveLength(12);
    expect(dock).toContain('<div class="thread-dock-more">+3 more lines</div>');
    expect(dock).not.toContain(">content<");
  });

  it("draws Codex's patch request as the files it names", () => {
    const dock = dockOf(paint([ask("fileChange", { itemId: "i", interfacePaths: ["/w/a.rs", "/w/b.rs"] },
      { id: "ap-c" })]));
    expect(dock).toContain("Edit 2 files?");
    expect(dock).toContain('aria-label="Files"');
    expect(dock).toContain('<span class="thread-dock-path">b.rs</span>');
    expect(dock).not.toContain("thread-dock-diff");
  });

  it("shows any other request's own input", () => {
    const dock = dockOf(paint([ask("WebFetch", { url: "https://example.com", prompt: "Summarise" },
      { id: "ap-o" })]));
    expect(dock).toContain("Allow web fetch?");
    expect(dock).toContain('class="conv-pre"');
    expect(dock).toContain("Summarise");
  });

  it("docks a question with its choices, Skip then Send answers", () => {
    const dock = dockOf(paint([ask("AskUserQuestion", {}, { id: "q1", questions: [{
      question: "Which library?", header: "Library", multiSelect: false,
      options: [{ label: "date-fns", description: "small" }] }] })], { focused: true }));
    expect(dock).toContain('class="thread-dock-card is-pending" role="alert" aria-label="The agent is asking a question"');
    expect(dock).toContain('<span class="thread-dock-title">Answer a question?</span>');
    expect(dock).toContain("Which library?");
    expect(dock.indexOf(">Skip<")).toBeLessThan(dock.indexOf(">Send answers<"));
    expect(dock).toMatch(/disabled=""[^>]*data-approval-key="allow"/u);
  });

  it("draws a request the approval policy is checking without amber or verdicts", () => {
    const dock = dockOf(paint([bash], { approvalGate: () => "checking" }));
    expect(dock).toContain('class="thread-dock-card is-checking"');
    expect(dock).toContain("Checking with Interface&#x27;s approval policy…");
    expect(dock).toContain('<span class="thread-dock-sum" title="npm test -- auth">Run npm test -- auth</span>');
    expect(dock).not.toMatch(/is-pending|thread-dock-glyph|>Deny<|>Allow once</u);
  });

  it("keeps an answered card until the engine drops it, then the transcript has it", () => {
    const sent = dockOf(paint([bash], {
      approvals: { [approvalVerdictKey("claude#a", "ap-1")]: { verdict: "allow" } } }));
    expect(sent).toContain('<div tabindex="-1" class="thread-dock-card" data-voice-guard="approval">');
    expect(sent).toContain("Allow sent");
    expect(sent).not.toContain("thread-dock-meta");
    const gone = paint([bash], { activeApprovalIds: new Set() });
    expect(gone).not.toContain("thread-dock");
  });

  it("is not drawn outside the Threads layout", () => {
    const plain = paint([bash], { threadLayout: false });
    expect(plain).not.toContain("thread-dock");
    expect(plain).toContain("conv-approval conv-approval-pending");
  });
});

describe("the queued follow-ups card in a Threads pane", () => {
  const queued = (id: string, text: string) => ({ id, text, createdAt: 1 });
  const callbacks = { onMove: () => undefined, onEdit: () => undefined, onRemove: () => undefined,
    onSendNow: () => undefined, onClear: () => undefined };
  const compact = (count: number): string => renderToStaticMarkup(createElement(SteeringQueue, {
    items: Array.from({ length: count }, (_, index) => queued(`q${index}`, `Prompt ${index + 1}`)),
    label: "Queued follow-ups", compact: true, ...callbacks,
  }));

  it("says QUEUED n and keeps the card's accessible name", () => {
    const html = compact(2);
    expect(html).toContain('class="steering-queue is-compact"');
    expect(html).toContain('<span aria-hidden="true">Queued</span>'
      + '<span class="steering-queue-sr">Queued follow-ups</span>');
    expect(html).toContain('<span class="steering-queue-count" title="2 of 20">2</span>');
  });

  it("says Send now · Edit · Remove in words, every name and key unchanged", () => {
    const html = compact(2);
    expect(html).toContain('aria-label="Send queued prompt 1 now" title="Send now">Send now</button>');
    expect(html).toContain('aria-label="Edit queued prompt 1" title="Edit">Edit</button>');
    expect(html).toContain('aria-label="Remove queued prompt 1" title="Remove">Remove</button>');
    expect(html.indexOf("Send queued prompt 1 now")).toBeLessThan(html.indexOf("Edit queued prompt 1"));
    expect(html.indexOf("Edit queued prompt 1")).toBeLessThan(html.indexOf("Remove queued prompt 1"));
    // Reordering stays, on hover or focus, and so do the row's own keys.
    expect(html).toContain('class="steering-queue-iconbtn steering-queue-move" disabled=""'
      + ' aria-label="Move queued prompt 1 earlier"');
    expect(html).toContain('aria-label="Move queued prompt 1 later"');
    expect(html.match(/aria-keyshortcuts="Alt\+ArrowUp Alt\+ArrowDown Alt\+Home Alt\+End"/gu))
      .toHaveLength(2);
  });

  it("offers Clear only past one prompt", () => {
    expect(compact(1)).not.toContain(">Clear<");
    expect(compact(2)).toContain(">Clear<");
  });

  it("is what the Threads pane draws, and only there", () => {
    const transcript = readFileSync(new URL("./components/ConversationTranscript.tsx", import.meta.url), "utf8");
    expect(transcript).toContain('label="Queued follow-ups"\n              compact={threadLayout}');
    const full = renderToStaticMarkup(createElement(SteeringQueue, {
      items: [queued("q0", "Prompt 1")], label: "Queued follow-ups", ...callbacks }));
    expect(full).toContain('class="steering-queue"');
    expect(full).toContain("1/20");
  });
});

/* ---- the colours, legible in every palette ------------------------------ */

const read = (path: string): string => readFileSync(new URL(path, import.meta.url), "utf8");
const theme = read("./theme.css");
const hueCss = read("./components/ProjectHue.css");
const rootBlock = theme.match(/:root \{([\s\S]*?)\n\}/u)?.[1] ?? "";
const palettes = [
  { id: ":root", body: rootBlock },
  ...[...theme.matchAll(/:root\[data-theme="([^"]+)"\] \{([\s\S]*?)\n\}/gu)]
    .filter(([, id]) => !id.startsWith("custom-"))
    .map(([, id, body]) => ({ id, body })),
].map(({ id, body }) => ({ id, body, dark: /(?<![-\w])color-scheme:\s*dark;/u.test(body) }));

function token(body: string, name: string, seen = new Set<string>()): string {
  if (seen.has(name)) throw new Error(`Circular --${name}`);
  const value = (body.match(new RegExp(`--${name}:\\s*([^;]+);`, "u"))
    ?? rootBlock.match(new RegExp(`--${name}:\\s*([^;]+);`, "u")))?.[1].trim();
  if (value === undefined) throw new Error(`Missing --${name}`);
  const reference = /^var\(--([a-z0-9-]+)\)$/u.exec(value)?.[1];
  return reference === undefined ? value : token(body, reference, new Set([...seen, name]));
}
const pick = (pair: string, dark: boolean): string => {
  const match = /^light-dark\((#[0-9a-f]{6}),\s*(#[0-9a-f]{6})\)$/u.exec(pair.trim());
  if (match === null) throw new Error(`Not a light-dark pair: ${pair}`);
  return dark ? match[2] : match[1];
};
const hues = Array.from({ length: PROJECT_HUE_COUNT }, (_, index) => {
  const rule = new RegExp(`\\.project-hue-${index} \\{ --project-h: ([^;]+); --project-h2: ([^;]+); \\}`, "u")
    .exec(hueCss);
  if (rule === null) throw new Error(`Missing .project-hue-${index}`);
  return rule[1];
});
const over = (expression: string, backdrop: string): string => {
  const colour = evaluateCssColor(expression);
  return colour.length === 9 ? compositeHex(colour, backdrop) : colour;
};

describe("the live turn's colours", () => {
  it("measures both kinds of palette", () => {
    expect(palettes.filter(palette => palette.dark).length).toBeGreaterThanOrEqual(4);
    expect(palettes.filter(palette => !palette.dark).length).toBeGreaterThanOrEqual(4);
  });

  it("writes Needs you in ink that reads on its amber fill, in every palette", () => {
    // The pill's rule, as theme.css spells it.
    expect(theme).toMatch(/\.workspace-tile-needs \{[^}]*color: var\(--surface\);[^}]*background: var\(--tile-waiting\);/u);
    expect(theme).toContain('.workspace-agent[data-status="waiting"] .workspace-tile-head { --tile-waiting: var(--status-waiting); }');
    const failures: string[] = [];
    for (const palette of palettes) {
      const ink = token(palette.body, "surface");
      const fill = token(palette.body, "status-waiting");
      const ratio = contrastRatio(ink, fill);
      if (ratio < 4.5) failures.push(`${palette.id}: ${ink} on ${fill} is ${ratio.toFixed(2)}`);
      // Dark palettes write it in dark ink on their bright amber.
      if (palette.dark && contrastRatio(ink, "#000000") > contrastRatio(ink, "#ffffff")) {
        failures.push(`${palette.id}: the ink ${ink} is not dark`);
      }
    }
    expect(failures).toEqual([]);
  });

  it("keeps the docked card's words legible on its amber tint", () => {
    const failures: string[] = [];
    for (const palette of palettes) {
      const surface = token(palette.body, "surface");
      const card = over(`color-mix(in srgb, ${token(palette.body, "status-waiting")} 7%, ${surface})`, surface);
      for (const name of ["fg-strong", "fg", "fg-dim"]) {
        const ratio = contrastRatio(token(palette.body, name), card);
        if (ratio < 4.5) failures.push(`${palette.id} --${name}: ${ratio.toFixed(2)}`);
      }
    }
    expect(failures).toEqual([]);
  });

  it("keeps the Steer chip and its key hint legible, hued or neutral", () => {
    const failures: string[] = [];
    for (const palette of palettes) {
      const surface = token(palette.body, "surface");
      const fgStrong = token(palette.body, "fg-strong");
      const pairs = [{ what: "neutral", colour: token(palette.body, "fg-dim") },
        ...hues.map((pair, index) => ({ what: `hue ${index}`, colour: pick(pair, palette.dark) }))];
      for (const { what, colour } of pairs) {
        const chip = over(`color-mix(in srgb, ${colour} 10%, transparent)`, surface);
        const label = contrastRatio(fgStrong, chip);
        if (label < 4.5) failures.push(`${palette.id} ${what} label: ${label.toFixed(2)}`);
        const kbd = contrastRatio(over(`color-mix(in srgb, ${colour} 70%, ${fgStrong})`, chip), chip);
        if (kbd < 4.5) failures.push(`${palette.id} ${what} key hint: ${kbd.toFixed(2)}`);
      }
    }
    expect(failures).toEqual([]);
  });
});
