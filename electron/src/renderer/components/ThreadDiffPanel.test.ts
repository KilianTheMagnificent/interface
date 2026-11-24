import { readFileSync } from "node:fs";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it, vi } from "vitest";
import { THEME_KIND } from "../../shared/appearance";
import type { ThreadDiff, ThreadView } from "../../shared/threads";
import fixture from "../../fixtures/threads-v34.json";
import { compositeHex, contrastRatio } from "../contrast";
import {
  appendDiffQuote, defaultThreadDiffChoice, diffPathParts, diffQuoteFor, diffSummaryText, filePatchState, formatDiffQuote,
  omittedSentence, parsePatch, parseThreadDiffLayout, shownThreadDiffLayout, SPLIT_DIFF_MIN_WIDTH, splitDiffFits,
  splitHunkRows, THREAD_DIFF_LAYOUT_KEY, threadDiffChoiceFromValue, threadDiffCommand, threadDiffScopeLabel,
  threadDiffScopes, threadDiffScopeValue, TRUNCATED_SENTENCE, turnUndoAllowed,
  type SplitRow, type ThreadDiffQuote,
} from "../thread-diff";
import { THREAD_DIFF_PANEL_CSS, ThreadDiffPanel, type ThreadDiffPanelProps } from "./ThreadDiffPanel";
import { THREAD_SHIP_CSS } from "./ThreadShipBar";

/* ============================================================================
   The thread diff panel: its scopes, its hunks and the unchanged lines between
   them, the words for what Rust left out, what a commented line puts into the
   prompt, and the empty answer. Every diff comes from Rust; these pin what is
   drawn.
   ========================================================================= */

const view = fixture.view as unknown as { threads: ThreadView[] };
const worktree = view.threads.find(thread => thread.workspace?.mode === "worktree")!;
const local = view.threads.find(thread => thread.workspace?.mode === "local")!;
const diff = fixture.response.diff as unknown as ThreadDiff;
const noop = (): void => undefined;
const clock = (sentAt: number): string => `t${sentAt % 1000}`;

function render(patch: Partial<ThreadDiffPanelProps> = {}): string {
  return renderToStaticMarkup(createElement(ThreadDiffPanel, {
    thread: worktree, title: "Fix the flaky login test", choice: { scope: "thread" },
    load: { status: "ready", diff }, onChoose: noop, onRefresh: noop, onClose: noop, ...patch,
  }));
}

describe("thread diff scopes", () => {
  it("lists every turn newest first, then Whole thread / Uncommitted / Branch", () => {
    const scopes = threadDiffScopes(worktree, { scope: "thread" }, clock);
    expect(scopes.map(entry => entry.label))
      .toEqual(["Turn 2", "Turn 1", "Whole thread", "Uncommitted", "Branch"]);
    expect(scopes.map(entry => entry.value)).toEqual(["turn:1", "turn:0", "thread", "uncommitted", "branch"]);
    expect(threadDiffScopeLabel(scopes[0])).toBe(`Turn 2 · ${clock(worktree.turns[1].sentAt)}`);
    expect(threadDiffScopeLabel(scopes[2])).toBe("Whole thread");
  });

  it("has no Branch scope for a local thread", () => {
    expect(threadDiffScopes(local, { scope: "thread" }, clock).map(entry => entry.value))
      .toEqual(["turn:1", "turn:0", "thread", "uncommitted"]);
  });

  it("lists a turn without a checkpoint but cannot choose it, and opens on Uncommitted", () => {
    const bare: ThreadView = { ...local, turns: [{ ...local.turns[0], hasStart: false }] };
    const scopes = threadDiffScopes(bare, defaultThreadDiffChoice(bare), clock);
    expect(scopes.filter(entry => entry.disabled !== null).map(entry => entry.value))
      .toEqual(["turn:0", "thread"]);
    expect(defaultThreadDiffChoice(bare)).toEqual({ scope: "uncommitted" });
    expect(defaultThreadDiffChoice(worktree)).toEqual({ scope: "thread" });
  });

  it("always names the turn the panel shows, even one the thread no longer lists", () => {
    expect(threadDiffScopes(local, { scope: "turn", ordinal: 7 }, clock)[0])
      .toMatchObject({ value: "turn:7", label: "Turn 8", disabled: null });
  });

  it("round-trips a choice through the menu's value", () => {
    for (const choice of [{ scope: "turn", ordinal: 3 }, { scope: "thread" }, { scope: "branch" }] as const) {
      expect(threadDiffChoiceFromValue(threadDiffScopeValue(choice))).toEqual(choice);
    }
    expect(threadDiffChoiceFromValue("turn:x")).toBeNull();
  });

  it("sends turnDiff for a turn and threadDiff for the rest", () => {
    expect(threadDiffCommand("k", { scope: "turn", ordinal: 1 }))
      .toEqual({ kind: "turnDiff", key: "k", ordinal: 1 });
    expect(threadDiffCommand("k", { scope: "branch" }))
      .toEqual({ kind: "threadDiff", key: "k", scope: "branch" });
  });

  it("keeps Undo on a local thread and an owned worktree, off a shared worktree and a chat", () => {
    expect(turnUndoAllowed(local)).toBe(true);
    expect(turnUndoAllowed(worktree)).toBe(true);
    expect(turnUndoAllowed({ ...worktree, workspace: { ...worktree.workspace!, owned: false } })).toBe(false);
    expect(turnUndoAllowed({ ...local, workspace: { mode: "none", projectRoot: "", owned: false } })).toBe(false);
  });
});

describe("what Rust left out", () => {
  it("says why a patch is missing, in words", () => {
    expect(filePatchState(diff.files[2])).toEqual({ kind: "omitted", sentence: omittedSentence("sensitive") });
    expect(filePatchState(diff.files[3])).toEqual({ kind: "omitted", sentence: omittedSentence("binary") });
    expect(omittedSentence("large")).toContain("256 KiB");
    expect(omittedSentence("sensitive")).toContain("credentials");
  });
});

describe("parsePatch", () => {
  it("numbers context and added lines on the new side and removed lines on the old", () => {
    const { hunks } = parsePatch("@@ -13,3 +13,4 @@ export function login\n context\n-old\n+new\n+more\n tail\n");
    expect(hunks).toHaveLength(1);
    expect(hunks[0]).toMatchObject({ oldStart: 13, oldCount: 3, newStart: 13, newCount: 4,
      section: "export function login", skippedBefore: 12 });
    expect(hunks[0].rows).toEqual([
      { kind: "context", text: "context", oldLine: 13, newLine: 13 },
      { kind: "del", text: "old", oldLine: 14, newLine: null },
      { kind: "add", text: "new", oldLine: null, newLine: 14 },
      { kind: "add", text: "more", oldLine: null, newLine: 15 },
      { kind: "context", text: "tail", oldLine: 15, newLine: 16 },
    ]);
  });

  it("counts the unchanged lines before the first hunk and between hunks from their headers", () => {
    const { hunks } = parsePatch("@@ -3,2 +3,3 @@\n a\n+b\n c\n@@ -20,2 +21,2 @@\n-d\n+e\n f\n");
    expect(hunks.map(hunk => hunk.skippedBefore)).toEqual([2, 15]);
    // A new file has nothing before its one hunk; a hunk at line 1 neither.
    expect(parsePatch("@@ -0,0 +1,2 @@\n+a\n+b\n").hunks[0].skippedBefore).toBe(0);
    expect(parsePatch("@@ -1 +1 @@\n-a\n+b\n").hunks[0].skippedBefore).toBe(0);
    // A pure insertion names the line it follows: after line 5 leaves 5 unchanged above it.
    expect(parsePatch("@@ -5,0 +6,1 @@\n+x\n").hunks[0].skippedBefore).toBe(5);
  });

  it("starts a git patch at its first hunk, and reads removed `--` lines as deletions", () => {
    // What Rust sends for a new file (hands-free run, 6 Oct 2026): git's header
    // repeated the file row and drew `diff --git` and `index` as content.
    const created = parsePatch("diff --git a/hello.txt b/hello.txt\nnew file mode 100644\n"
      + "index 0000000..45b983b\n--- /dev/null\n+++ b/hello.txt\n@@ -0,0 +1 @@\n+hi\n");
    expect(created.header).toEqual([]);
    expect(created.hunks[0].rows).toEqual([{ kind: "add", text: "hi", oldLine: null, newLine: 1 }]);
    expect(parsePatch("@@ -1,2 +1 @@\n--- a SQL comment\n-x\n+++ counter\n\\ No newline at end of file\n")
      .hunks[0].rows.map(row => row.kind)).toEqual(["del", "del", "add", "note"]);
    // A mode change has no hunk: its header is all there is.
    expect(parsePatch("diff --git a/run.sh b/run.sh\nold mode 100644\nnew mode 100755\n"))
      .toEqual({ hunks: [], header: ["diff --git a/run.sh b/run.sh", "old mode 100644", "new mode 100755"] });
  });

  it("reads a trimmed blank context line as a line, and numbers nothing past the hunk's counts", () => {
    const { hunks } = parsePatch("@@ -1,2 +1,2 @@\n\n-a\n+b\n\n");
    expect(hunks[0].rows).toEqual([
      { kind: "context", text: "", oldLine: 1, newLine: 1 },
      { kind: "del", text: "a", oldLine: 2, newLine: null },
      { kind: "add", text: "b", oldLine: null, newLine: 2 },
    ]);
  });
});

describe("quoting diff lines into the prompt", () => {
  const rows = parsePatch("@@ -13,3 +13,4 @@\n keep\n-gone\n+came\n+also\n tail\n").hunks[0].rows;

  it("quotes one added line on the new side", () => {
    expect(diffQuoteFor("src/auth.ts", rows.slice(2, 3), "  Clear the cookie too. "))
      .toEqual({ path: "src/auth.ts", startLine: 14, endLine: 14, side: "new", code: "+came",
        comment: "Clear the cookie too." });
  });

  it("numbers a mixed range by the lines it keeps or adds, and a removed-only range on the old side", () => {
    expect(diffQuoteFor("a.ts", rows.slice(0, 4), "")).toMatchObject(
      { startLine: 13, endLine: 15, side: "new", code: " keep\n-gone\n+came\n+also" });
    expect(diffQuoteFor("a.ts", rows.slice(1, 2), "")).toMatchObject(
      { startLine: 14, endLine: 14, side: "old", code: "-gone" });
    expect(diffQuoteFor("a.ts", [{ kind: "note", text: "\\ No newline", oldLine: null, newLine: null }], ""))
      .toBeNull();
  });

  it("formats a quote as path:line, a fenced block, the comment and a trailing blank line", () => {
    const one: ThreadDiffQuote = { path: "src/auth.ts", startLine: 21, endLine: 21, side: "new",
      code: "+  session.clear();", comment: "Also drop the remember-me cookie." };
    expect(formatDiffQuote(one)).toBe(
      "src/auth.ts:21\n```diff\n+  session.clear();\n```\nAlso drop the remember-me cookie.\n\n");
    expect(formatDiffQuote({ ...one, startLine: 14, endLine: 17 })).toMatch(/^src\/auth\.ts:14-17\n/u);
  });

  it("says when the numbers are the old file's, leaves out an empty comment, and outfences the code", () => {
    const quote: ThreadDiffQuote = { path: "README.md", startLine: 3, endLine: 4, side: "old",
      code: "-```js\n-x\n-```", comment: "  " };
    expect(formatDiffQuote(quote)).toBe("README.md:3-4 (removed)\n````diff\n-```js\n-x\n-```\n````\n\n");
  });
});

describe("ThreadDiffPanel", () => {
  it("names the scope in its menu button and the scope's totals beside it", () => {
    const markup = render();
    expect(markup).toContain('aria-label="Diff scope: Whole thread"');
    expect(markup).toContain('aria-haspopup="listbox"');
    expect(markup).toContain("4 files · +19 −3");
    expect(markup).toContain('<span class="tdiff-add">+19</span>');
    expect(render({ choice: { scope: "turn", ordinal: 1 } })).toMatch(/Diff scope: Turn 2 · [^"]+"/u);
  });

  it("stacks every file under a header with its status, path and counts", () => {
    const markup = render();
    expect(markup.match(/class="tdiff-file"/gu)).toHaveLength(diff.files.length);
    expect(markup.match(/class="tdiff-fh-toggle" aria-expanded="true"/gu)).toHaveLength(diff.files.length);
    // The folders (and a rename's old path) read dimmer than the file's name.
    expect(markup).toContain('<span class="tdiff-dir">src/auth/backoff.ts → src/auth/</span>retry.ts');
    expect(markup).toContain('<span class="tdiff-path"><bdi>.env.local</bdi></span>');
    expect(markup).toContain('<span class="tdiff-add">+6</span>');
    // Both patches are drawn at once, numbered, with their marker columns.
    expect(markup).toContain('class="tdiff-dl is-del"');
    expect(markup).toContain('class="tdiff-dl is-add"');
    // Each line's code, lightly highlighted as spans, reads exactly as it was.
    expect(markup.replace(/<[^>]+>/gu, "")).toContain("import { login } from &quot;../src/auth&quot;;");
    expect(markup).toContain('<span class="tdiff-code"><span class="tdiff-tok-keyword">import</span> '
      + '<span class="tdiff-tok-punctuation">{</span> login <span class="tdiff-tok-punctuation">}</span> '
      + '<span class="tdiff-tok-keyword">from</span> <span class="tdiff-tok-string">&quot;../src/auth&quot;</span>'
      + '<span class="tdiff-tok-punctuation">;</span></span>');
    // The omitted files say why in place of a patch.
    expect(markup).toContain(omittedSentence("sensitive"));
    expect(markup).toContain(omittedSentence("binary"));
  });

  it("leads each file with its type badge and ends the totals with the diffstat", () => {
    const markup = render();
    expect(markup).toMatch(/<span class="tdiff-sr">Renamed <\/span><span class="file-badge kind-ts" aria-hidden="true">TS<\/span><span class="tdiff-path">/u);
    expect(markup).toContain('<span class="file-badge kind-other" aria-hidden="true">LOC</span>');
    // +19 −3 of 22 lines: four added squares and one removed, after the figures.
    expect(markup).toMatch(/<span class="tdiff-del">−3<\/span><\/span><span class="diffstat-bar" aria-hidden="true">(?:<i class="is-added"><\/i>){4}<i class="is-removed"><\/i><\/span>/u);
    // No line changes: no figures and no squares.
    const quiet: ThreadDiff = { ...diff, files: [diff.files[3]] };
    expect(render({ load: { status: "ready", diff: quiet } })).not.toContain("diffstat-bar");
  });

  it("marks added and removed rows with a 3px bar at the left edge, under the selection's", () => {
    const css = THREAD_DIFF_PANEL_CSS.replace(/\s+/gu, " ");
    expect(css).toContain(".tdiff-dl.is-add { background:var(--diff-added-bg); box-shadow:inset 3px 0 0 var(--diff-added); }");
    expect(css).toContain(".tdiff-dl.is-del { background:var(--diff-removed-bg); box-shadow:inset 3px 0 0 var(--diff-removed); }");
    // A quoted or selected line's focus bar comes later, so it wins.
    expect(css.indexOf(".tdiff-dl.is-selected {")).toBeGreaterThan(css.indexOf(".tdiff-dl.is-add {"));
    expect(css.indexOf(".tdiff-dl.is-quoted {")).toBeGreaterThan(css.indexOf(".tdiff-dl.is-del {"));
  });

  it("draws a line it does not colour, or one Rust notes, as plain text", () => {
    const plain: ThreadDiff = { ...diff, truncated: false, files: [{ path: "notes.txt", status: "modified",
      added: 1, removed: 0, binary: false, patch: "@@ -1 +1,2 @@\n keep <b>\n+if (x) { y(); }\n\\ No newline at end of file\n" }] };
    const markup = render({ load: { status: "ready", diff: plain } });
    expect(markup).toContain('<span class="tdiff-code">keep &lt;b&gt;</span>');
    expect(markup).toContain('<span class="tdiff-code">if (x) { y(); }</span>');
    expect(markup).toContain('<span class="tdiff-code">\\ No newline at end of file</span>');
    expect(markup.replace(/<style>[\s\S]*?<\/style>/u, "")).not.toContain("tdiff-tok-");
  });

  it("heads a hunk with its range, its function and the unchanged lines it leaves out, on a strip", () => {
    const shifted: ThreadDiff = { ...diff, truncated: false, files: [{ ...diff.files[0],
      patch: "@@ -12,2 +12,2 @@ function retry\n-a\n+b\n" }] };
    const markup = render({ load: { status: "ready", diff: shifted } });
    expect(markup).toContain('<div class="tdiff-hunk"><span class="tdiff-hunk-range">@@ -12,2 +12,2 @@</span>'
      + '<span class="tdiff-hunk-section">function retry</span>'
      + '<span class="tdiff-hunk-skip">11 unchanged lines</span></div>');
    // A hunk that starts the file and names no function has nothing to say: no strip.
    expect(render()).not.toContain('class="tdiff-hunk"');
  });

  it("heads the panel with its name and tools, then the scope and its totals", () => {
    const point = { label: "Undo · 2 files", description: "Restores 2 files." };
    const markup = render({ choice: { scope: "turn", ordinal: 1 }, onBrowseOutputs: noop,
      turnUndo: () => ({ point, busy: false, onUndo: noop }) });
    const head = markup.slice(markup.indexOf('<header class="tdiff-head">'), markup.indexOf("</header>"));
    expect(head).toMatch(/^<header class="tdiff-head"><div class="tdiff-head-row"><h2 class="tdiff-title">Changes<\/h2>/u);
    // The tools in the canvas's order, each named; then, on the second row, the scope.
    const named = [...head.matchAll(/<button [^>]*aria-label="([^"]+)"/gu)].map(match => match[1]);
    expect(named.slice(0, 6)).toEqual(
      ["Browse outputs", "Stacked", "Split", "Undo turn", "Refresh changes", "Close changes"]);
    expect(named[6]).toMatch(/^Diff scope: Turn 2 · /u);
    expect(head).toContain('title="Restore files to before this message"');
    expect(head).toContain('title="Close (Esc)"');
    // What the scope changed follows it.
    expect(head).toContain('<span aria-hidden="true">4 files</span>');
    expect(head.indexOf("tdiff-scope")).toBeLessThan(head.indexOf("4 files"));
  });

  it("lists the changed files above the diffs, the first one marked and holding the tab stop", () => {
    const markup = render();
    const list = markup.slice(markup.indexOf('<ul class="tdiff-files" aria-label="Changed files">'),
      markup.indexOf("</ul>"));
    const rows = [...list.matchAll(
      /<button type="button" class="tdiff-frow" tabindex="(-?\d)"( aria-current="true")? aria-label="([^"]+)"/gu)]
      .map(([, tab, current, label]) => `${tab}${current === undefined ? "" : "*"} ${label}`);
    expect(rows).toEqual([
      "0* src/auth/backoff.ts → src/auth/retry.ts, renamed, +6 −2",
      "-1 test/login.test.ts, added, +12 −0",
      "-1 .env.local, +1 −1",
      "-1 assets/logo.png, deleted",
    ]);
    // Each row: the badge, the folders dimmer than the name, a new or deleted file said so.
    expect(list).toContain('<span class="file-badge kind-ts" aria-hidden="true">TS</span><span class="tdiff-frow-path"><bdi>'
      + '<span class="tdiff-dir">test/</span><span class="tdiff-name">login.test.ts</span>'
      + '<span class="tdiff-suffix"> · new</span></bdi></span>');
    expect(list).toContain('<span class="tdiff-suffix"> · deleted</span>');
    // No files, no list.
    expect(render({ load: { status: "ready", diff: { ...diff, files: [], truncated: false } } }))
      .not.toContain('class="tdiff-files"');
    expect(render({ load: { status: "loading" } })).not.toContain('class="tdiff-files"');
  });

  it("puts the comment + on every quotable line, and none where nothing can be quoted", () => {
    const quotable = render({ onQuote: noop });
    expect(quotable.match(/class="tdiff-cplus"/gu))
      .toHaveLength(quotable.match(/class="tdiff-ln" data-line/gu)!.length);
    expect(quotable).toMatch(
      /<button type="button" class="tdiff-ln" data-line="0:0:0"[^>]*><span class="tdiff-cplus" aria-hidden="true"><svg/u);
    expect(render()).not.toContain('class="tdiff-cplus"');
    expect(THREAD_DIFF_PANEL_CSS.replace(/\s+/gu, " ")).toContain(".tdiff-lines.is-quotable .tdiff-dl:hover .tdiff-cplus, "
      + ".tdiff-ln:focus-visible .tdiff-cplus { visibility:visible; }");
  });

  it("draws its corners from the radius tokens, so the CLI chrome squares them", () => {
    for (const css of [THREAD_DIFF_PANEL_CSS, THREAD_SHIP_CSS]) {
      const radii = [...css.matchAll(/border-radius:([^;]+);/gu)].map(match => match[1].trim());
      expect(radii.length).toBeGreaterThan(5);
      for (const radius of radii) {
        expect(radius).toMatch(/^(?:var\(--radius-[a-z]+\)|calc\(var\(--radius-control\) \* [\d.]+\))$/u);
      }
    }
  });

  it("makes lines selectable only when there is somewhere to quote them", () => {
    expect(render()).not.toContain('class="tdiff-ln" data-line');
    const quotable = render({ onQuote: noop });
    expect(quotable).toContain('aria-label="Removed line 1"');
    expect(quotable).toContain('aria-label="Added line 1"');
    // One tab stop for every line, on the first one drawn.
    expect(quotable.match(/class="tdiff-ln" data-line="[^"]+" tabindex="0"/gu)).toHaveLength(1);
  });

  it("offers Undo turn on a turn of a local or owned-worktree thread, never a shared worktree's", () => {
    const point = { label: "Undo · 2 files", description: "Restores 2 files." };
    const turnUndo = () => ({ point, busy: false, onUndo: noop });
    const turn = { scope: "turn" as const, ordinal: 1 };
    expect(render({ thread: local, choice: turn, turnUndo })).toContain('aria-label="Undo turn"');
    expect(render({ thread: worktree, choice: turn, turnUndo })).toContain('aria-label="Undo turn"');
    const shared = { ...worktree, workspace: { ...worktree.workspace!, owned: false } };
    expect(render({ thread: shared, choice: turn, turnUndo })).not.toContain("Undo turn");
    expect(render({ thread: local, turnUndo })).not.toContain("Undo turn");
    expect(render({ thread: local, choice: turn, turnUndo: () => null })).not.toContain("Undo turn");
    expect(render({ thread: local, choice: turn, turnUndo: () => ({ point, busy: true, onUndo: noop }) }))
      .toMatch(/<button type="button" class="tdiff-iconbtn" disabled="" aria-label="Undo turn"/u);
  });

  it("names a truncated answer, and stays quiet when it is whole", () => {
    expect(render()).toContain(TRUNCATED_SENTENCE);
    expect(render({ load: { status: "ready", diff: { ...diff, truncated: false } } }))
      .not.toContain(TRUNCATED_SENTENCE);
  });

  it("says No changes for an empty answer and draws no file", () => {
    const markup = render({ load: { status: "ready", diff: { ...diff, files: [], truncated: false } } });
    expect(markup).toContain(diffSummaryText({ ...diff, files: [] }));
    expect(markup).toContain('<p class="tdiff-note">No changes</p>');
    expect(markup).not.toContain('class="tdiff-file"');
  });

  it("shows Rust's refusal and the loading state", () => {
    expect(render({ load: { status: "error", message: "Not a git repository." } }))
      .toContain("Not a git repository.");
    expect(render({ load: { status: "loading" } })).toContain("Loading changes…");
  });
});

describe("a line comment lands in the thread's draft", () => {
  const quote: ThreadDiffQuote = {
    path: "src/auth.ts", startLine: 21, endLine: 21, side: "new", code: "+  session.clear();",
    comment: "Also drop the remember-me cookie.",
  };
  it("is the whole draft when nothing is typed yet", () => {
    expect(appendDiffQuote("", quote)).toBe(formatDiffQuote(quote));
    expect(appendDiffQuote("  \n", quote)).toBe(formatDiffQuote(quote));
  });
  it("follows the owner's words, one blank line apart, and never replaces them", () => {
    expect(appendDiffQuote("Please tidy this up.\n\n", quote))
      .toBe(`Please tidy this up.\n\n${formatDiffQuote(quote)}`);
  });
});

describe("a file header's path", () => {
  it("splits at the last separator, so the name reads first", () => {
    expect(diffPathParts("src/auth.ts")).toEqual({ dir: "src/", name: "auth.ts" });
    expect(diffPathParts("README.md")).toEqual({ dir: "", name: "README.md" });
    expect(diffPathParts("a/b.ts → c/d.ts")).toEqual({ dir: "a/b.ts → c/", name: "d.ts" });
    expect(diffPathParts("C:\\repo\\main.rs")).toEqual({ dir: "C:\\repo\\", name: "main.rs" });
  });
});

/* The syntax colours are light-dark() pairs: each must read at 4.5:1 on the
   file's card (--surface, Threads look 2026-10-23) and on the added and
   removed tints over it, wherever the theme's own text does (Solarized
   Light's body text does not on its tints; that is its palette, not this
   one's). */
describe("the diff's syntax colours", () => {
  const theme = readFileSync(new URL("../theme.css", import.meta.url), "utf8");
  const root = theme.match(/:root \{([\s\S]*?)\n\}/u)?.[1] ?? "";
  const blocks = [...theme.matchAll(/:root\[data-theme="([^"]+)"\] \{([\s\S]*?)\n\}/gu)]
    .filter(([, id]) => Object.hasOwn(THEME_KIND, id))
    .map(([, id, body]) => ({ id: id as keyof typeof THEME_KIND, body }));
  const token = (body: string, name: string): string => {
    const value = (body.match(new RegExp(`--${name}:\\s*([^;]+);`, "u"))
      ?? root.match(new RegExp(`--${name}:\\s*([^;]+);`, "u")))?.[1].trim();
    if (value === undefined) throw new Error(`Missing --${name}`);
    const reference = /^var\(--([a-z0-9-]+)\)$/u.exec(value)?.[1];
    return reference === undefined ? value : token(body, reference);
  };
  const css = THREAD_DIFF_PANEL_CSS;
  const kinds = ["keyword", "function", "type", "string", "number", "literal", "comment", "punctuation"];
  /** The colour a token kind paints in one theme. */
  const colour = (kind: string, body: string, dark: boolean): string => {
    const rule = new RegExp(`\\.tdiff-tok-${kind}\\b[^{]*\\{ color:([^;]+);`, "u").exec(css)?.[1].trim();
    if (rule === undefined) throw new Error(`No colour for ${kind}`);
    const pair = /^light-dark\((#[0-9a-f]{6}), (#[0-9a-f]{6})\)$/u.exec(rule);
    if (pair !== null) return dark ? pair[2] : pair[1];
    const reference = /^var\(--([a-z0-9-]+)\)$/u.exec(rule)?.[1];
    if (reference === undefined) throw new Error(`Unreadable colour for ${kind}: ${rule}`);
    return token(body, reference);
  };

  it("reads at 4.5:1 on the card and both tints in dark and light", () => {
    const measured = blocks.filter(({ id }) => id === "dark" || id === "light");
    expect(measured.map(({ id }) => id).sort()).toEqual(["dark", "light"]);
    // The code sits on the file's card.
    expect(THREAD_DIFF_PANEL_CSS.replace(/\s+/gu, " "))
      .toMatch(/\.tdiff-file \{ overflow:clip; background:var\(--surface\);/u);
    for (const { id, body } of blocks) {
      const panel = token(body, "surface");
      const grounds = [panel, compositeHex(token(body, "diff-added-bg"), panel),
        compositeHex(token(body, "diff-removed-bg"), panel)];
      const textReads = grounds.every(ground => contrastRatio(token(body, "fg"), ground) >= 4.5);
      if (id !== "dark" && id !== "light" && !textReads) continue;
      for (const kind of kinds) {
        for (const ground of grounds) {
          expect(contrastRatio(colour(kind, body, THEME_KIND[id] === "dark"), ground), `${id} ${kind} on ${ground}`)
            .toBeGreaterThanOrEqual(4.5);
        }
      }
    }
  });
});

/* Split: the old file on the left, the new on the right. Every cell carries
   its parsed row's index, which is what selection, quoting and the syntax
   colours key on, so a split line is the same line as its stacked row. */
describe("splitHunkRows", () => {
  /** A split row as `left | right`: each side's kind, number and index, or `·` for a filler. */
  const side = (cell: SplitRow["left"], number: "oldLine" | "newLine"): string =>
    cell === null ? "·" : `${cell.row.kind}:${cell.row[number] ?? "-"}@${cell.index}`;
  const read = (split: SplitRow[]): string[] =>
    split.map(row => `${side(row.left, "oldLine")} | ${side(row.right, "newLine")}`);

  it("puts a pure addition on the right with an empty left", () => {
    const { rows } = parsePatch("@@ -0,0 +1,2 @@\n+first\n+second\n").hunks[0];
    expect(read(splitHunkRows(rows))).toEqual(["· | add:1@0", "· | add:2@1"]);
  });

  it("puts a pure deletion on the left with an empty right", () => {
    const { rows } = parsePatch("@@ -4,2 +3,0 @@\n-gone\n-also gone\n").hunks[0];
    expect(read(splitHunkRows(rows))).toEqual(["del:4@0 | ·", "del:5@1 | ·"]);
  });

  it("pairs three deletions with one addition row by row, then fills the right", () => {
    const { rows } = parsePatch("@@ -1,5 +1,3 @@\n keep\n-a\n-b\n-c\n+x\n tail\n").hunks[0];
    expect(read(splitHunkRows(rows))).toEqual([
      "context:1@0 | context:1@0",
      "del:2@1 | add:2@4",
      "del:3@2 | ·",
      "del:4@3 | ·",
      "context:5@5 | context:3@5",
    ]);
  });

  it("draws context on both sides, numbered by each file", () => {
    const { rows } = parsePatch("@@ -10,3 +12,3 @@ function retry\n a\n-b\n+B\n c\n").hunks[0];
    const split = splitHunkRows(rows);
    expect(read(split)).toEqual(["context:10@0 | context:12@0", "del:11@1 | add:13@2", "context:12@3 | context:14@3"]);
    // A context line is one cell drawn twice, not two lines.
    expect(split[0].left).toBe(split[0].right);
  });

  it("keeps every row's index, in order on each side, so each cell maps back to its parsed row", () => {
    const { rows } = parsePatch("@@ -1,6 +1,7 @@\n a\n-b\n-c\n+B\n+C\n+D\n d\n-e\n+E\n f\n").hunks[0];
    const split = splitHunkRows(rows);
    for (const row of split) {
      for (const cell of [row.left, row.right]) if (cell !== null) expect(cell.row).toBe(rows[cell.index]);
    }
    const lefts = split.flatMap(row => row.left === null ? [] : [row.left.index]);
    const rights = split.flatMap(row => row.right === null ? [] : [row.right.index]);
    // The old file reads context and removals, the new context and additions, each in patch order.
    expect(lefts).toEqual([0, 1, 2, 6, 7, 9]);
    expect(rights).toEqual([0, 3, 4, 5, 6, 8, 9]);
    expect(new Set([...lefts, ...rights])).toEqual(new Set(rows.map((_, index) => index)));
  });

  it("starts a new pairing when a removal follows an addition, and keeps a note on its line's side", () => {
    const { rows } = parsePatch("@@ -1,2 +1,2 @@\n-a\n+b\n-c\n+d\n").hunks[0];
    expect(read(splitHunkRows(rows))).toEqual(["del:1@0 | add:1@1", "del:2@2 | add:2@3"]);
    const noted = parsePatch("@@ -1 +1 @@\n-z\n\\ No newline at end of file\n+w\n\\ No newline at end of file\n")
      .hunks[0].rows;
    expect(read(splitHunkRows(noted))).toEqual(["del:1@0 | add:1@2", "note:-@1 | note:-@3"]);
  });
});

describe("the Changes panel's layout choice", () => {
  it("reads only Split back from storage; anything else is Stacked", () => {
    expect(THREAD_DIFF_LAYOUT_KEY).toBe("interface.threads.diff-layout");
    expect(parseThreadDiffLayout("split")).toBe("split");
    expect(parseThreadDiffLayout("stacked")).toBe("stacked");
    expect(parseThreadDiffLayout(null)).toBe("stacked");
    expect(parseThreadDiffLayout("columns")).toBe("stacked");
  });

  it("draws Split only when it is chosen and the panel is wide enough", () => {
    expect(splitDiffFits(SPLIT_DIFF_MIN_WIDTH)).toBe(true);
    expect(splitDiffFits(SPLIT_DIFF_MIN_WIDTH - 1)).toBe(false);
    // The Changes column is 400–560px: Split must fit at its widest.
    expect(splitDiffFits(560)).toBe(true);
    expect(splitDiffFits(400)).toBe(false);
    expect(shownThreadDiffLayout("split", true)).toBe("split");
    expect(shownThreadDiffLayout("split", false)).toBe("stacked");
    expect(shownThreadDiffLayout("split", null)).toBe("split");
    expect(shownThreadDiffLayout("stacked", true)).toBe("stacked");
  });
});

describe("ThreadDiffPanel in Split", () => {
  afterEach(() => { vi.unstubAllGlobals(); });
  /** The panel as a viewer who chose `stored` sees it before it is measured. */
  function renderStored(stored: string | null, patch: Partial<ThreadDiffPanelProps> = {}): string {
    vi.stubGlobal("window", { localStorage: {
      getItem: (key: string) => (key === THREAD_DIFF_LAYOUT_KEY ? stored : null), setItem: noop } });
    return render(patch);
  }
  const shifted: ThreadDiff = { ...diff, truncated: false, files: [{ path: "src/auth/retry.ts", status: "modified",
    added: 2, removed: 3, binary: false, patch: "@@ -10,5 +12,4 @@ function retry\n keep\n-a\n-b\n-c\n+x\n+y\n tail\n" }] };

  it("opens Stacked with no stored choice, and when storage is unavailable", () => {
    for (const markup of [render(), renderStored(null), renderStored("bogus")]) {
      expect(markup).toContain('aria-label="Diff layout"');
      expect(markup).toMatch(/aria-label="Stacked" aria-pressed="true"/u);
      expect(markup).toMatch(/aria-label="Split" aria-pressed="false"/u);
      expect(markup).not.toContain('class="tdiff-sl"');
    }
  });

  it("draws the old file on the left and the new on the right, each line numbered by its own file", () => {
    const markup = renderStored("split", { load: { status: "ready", diff: shifted } });
    expect(markup).toMatch(/aria-label="Split" aria-pressed="true"/u);
    expect(markup.match(/class="tdiff-sl"/gu)).toHaveLength(5);
    const rows = [...markup.matchAll(/<div class="tdiff-sl">([\s\S]*?)<\/div><\/div>(?=<div class="tdiff-sl">|<\/div>)/gu)]
      .map(match => match[1].replace(/<[^>]+>/gu, "|").replace(/\|+/gu, "|"));
    // Each cell: its number (a removed or added one named for screen readers), marker and code.
    expect(rows).toEqual([
      "|10|keep|12|keep|",
      "|Removed |11|−|a|Added |13|+|x|",
      "|Removed |12|−|b|Added |14|+|y|",
      "|Removed |13|−|c|",
      "|14|tail|15|tail|",
    ]);
    // The side with no line is a filler, kept out of the reading order.
    expect(markup).toContain('<div class="tdiff-dl is-empty" aria-hidden="true"></div>');
    // The same row colours as stacked, one cell per side.
    expect(markup.match(/class="tdiff-dl is-del"/gu)).toHaveLength(3);
    expect(markup.match(/class="tdiff-dl is-add"/gu)).toHaveLength(2);
  });

  it("names each line once, row by row, with one tab stop, so a comment quotes the line it was made on", () => {
    const markup = renderStored("split", { load: { status: "ready", diff: shifted }, onQuote: noop });
    const lines = [...markup.matchAll(/data-line="([^"]+)"[^>]*aria-label="([^"]+)"/gu)]
      .map(match => `${match[1]} ${match[2]}`);
    // Left to right, top to bottom; a context line's button is its right copy.
    expect(lines).toEqual([
      "0:0:0 Line 12",
      "0:0:1 Removed line 11", "0:0:4 Added line 13",
      "0:0:2 Removed line 12", "0:0:5 Added line 14",
      "0:0:3 Removed line 13",
      "0:0:6 Line 15",
    ]);
    // The left copy of a context line shows the old file's number, as text.
    expect(markup).toContain('<div class="tdiff-dl is-context"><span class="tdiff-ln">10</span>');
    expect(markup.match(/class="tdiff-ln" data-line="[^"]+" tabindex="0"/gu)).toHaveLength(1);
  });

  it("splits into two equal columns and keeps the stacked rows' look", () => {
    const css = THREAD_DIFF_PANEL_CSS.replace(/\s+/gu, " ");
    expect(css).toContain(".tdiff-sl { display:grid; grid-template-columns:minmax(0, 1fr) minmax(0, 1fr); }");
    expect(css).toContain(".tdiff-lines.is-quotable .tdiff-dl:not(.is-note):not(.is-empty) { cursor:pointer; }");
    expect(css).toContain(".tdiff-lines { content-visibility:auto;");
  });
});
