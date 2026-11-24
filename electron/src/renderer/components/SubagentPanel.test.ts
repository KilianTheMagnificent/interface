import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { findSubagentItem } from "../tool-activity";
import type { ConvItem } from "../conversation-reducer";
import {
  SUBAGENT_PANEL_CSS,
  SubagentPanel,
  subagentActivityRows,
  subagentCountsText,
  subagentPanelKeyAction,
  type SubagentPanelAgent,
  type SubagentPanelProps,
} from "./SubagentPanel";
import { subagentGlyph, subagentStatusSentence, subagentStatusWord } from "../tool-activity";

/* ============================================================================
   Every branch of the panel, rendered and mutation-verified: each `toContain`
   is paired with the `not.toContain` that fails if the branch is inverted, so
   a heading over missing data cannot pass as "it renders".
   ========================================================================= */

const ROOT = "/Users/a/Projects/Interface";

function agentWith(patch: Partial<SubagentPanelAgent> = {}): SubagentPanelAgent {
  return {
    id: "toolu_01",
    subagentType: "interface_ui_code_audit",
    description: "Audit the renderer for duplicated status text",
    status: "running",
    toolCount: 0,
    subagentCount: 0,
    ...patch,
  };
}

/** The panel's markup WITHOUT its own stylesheet: every rule name also appears
 *  in the `<style>` block, so an un-stripped string makes "this class is not
 *  rendered" impossible to assert. The CSS is checked against
 *  SUBAGENT_PANEL_CSS directly instead. */
function render(props: Partial<SubagentPanelProps> = {}): string {
  const html = renderToStaticMarkup(createElement(SubagentPanel, {
    onClose: () => undefined,
    ...props,
  }));
  return html.replace(/<style>[\s\S]*?<\/style>/u, "");
}

describe("SubagentPanel — a selected subagent", () => {
  it("renders every field a fully populated subagent can fill", () => {
    const html = render({
      agent: agentWith({
        status: "done",
        model: "claude-opus-4-6",
        toolCount: 17,
        subagentCount: 2,
        lastActivity: "wrote the audit to /Users/a/Projects/Interface/notes.md",
        result: "Three defects: the name prints twice, the status prints twice, "
          + "and the cell is a card in a column of rows.",
        children: [
          { kind: "tool", id: "t1", tool: "Grep", summary: "sub-desc in theme.css",
            result: { ok: true, summary: "12 matches" }, input: {} },
        ],
      }),
      prompt: "Audit src/renderer for repeated strings in the subagent cell.",
      parentType: "orchestrator",
      timing: "1m 34s",
      pathRoot: ROOT,
    });

    expect(html).toContain("interface_ui_code_audit");
    expect(html).toContain("Audit the renderer for duplicated status text");
    expect(html).toContain(">Prompt sent</h3>");
    expect(html).toContain("Audit src/renderer for repeated strings");
    expect(html).toContain("claude-opus-4-6");
    expect(html).toContain(">Done</span>");
    expect(html).toContain(subagentStatusSentence("done"));
    expect(html).toContain("1m 34s");
    expect(html).toContain(">17</span>");
    expect(html).toContain(">tools</span>");
    expect(html).toContain(">2</span>");
    expect(html).toContain(">subagents</span>");
    expect(html).toContain(">Result</h3>");
    expect(html).toContain("Three defects: the name prints twice");
    expect(html).toContain("Spawned by");
    expect(html).toContain("orchestrator");
    expect(html).toContain("Grep");
    // Nothing anywhere may print the absence of a field.
    expect(html).not.toContain("undefined");
    expect(html).not.toContain("null");
    expect(html).not.toContain("NaN");
  });

  it("omits every block whose data is absent rather than drawing an empty row", () => {
    // The author's photographed case: a spawn with nothing behind it yet.
    const html = render({ agent: agentWith() });

    expect(html).toContain("interface_ui_code_audit");
    expect(html).toContain(">Running</span>");
    // Gated blocks, all absent.
    expect(html).not.toContain("Prompt sent");
    expect(html).not.toContain(">Work</h3>");
    expect(html).not.toContain(">Result</h3>");
    expect(html).not.toContain("Latest activity");
    expect(html).not.toContain(">Activity</h3>");
    expect(html).not.toContain("Spawned by");
    expect(html).not.toContain("undefined");
    // The one field that never blanks: an absent model is a fact, not a gap.
    expect(html).toContain("Inherits the parent");
    expect(html).not.toContain(">Model</h3><p class=\"subp-text subp-mono\"></p>");
  });

  it("prints the untruncated result instead of its own 80-character clip", () => {
    const clip = "Three defects: the name prints twice, the status prints twic…";
    const full = "Three defects: the name prints twice, the status prints twice, and the "
      + "cell is a card in a column of borderless rows.";
    const both = render({
      agent: agentWith({ status: "done", lastActivity: clip, result: full }),
    });
    expect(both).toContain(">Result</h3>");
    expect(both).toContain(full);
    expect(both).not.toContain("Latest activity");
    expect(both).not.toContain(clip);

    // Without the untruncated field the heading tells the truth about what it
    // is showing instead of promising a result it does not have.
    const onlyClip = render({ agent: agentWith({ status: "done", lastActivity: clip }) });
    expect(onlyClip).toContain(">Latest activity</h3>");
    expect(onlyClip).toContain(clip);
    expect(onlyClip).not.toContain(">Result</h3>");
  });

  it("draws the app's four state glyphs and never a second live region", () => {
    const running = render({ agent: agentWith({ status: "running" }) });
    expect(running).toContain("dot dot-working dot-pulse");
    expect(running).not.toContain("codicon-check");

    const done = render({ agent: agentWith({ status: "done" }) });
    expect(done).toContain("codicon-check");
    expect(done).not.toContain("dot-pulse");

    const failed = render({ agent: agentWith({ status: "failed" }) });
    expect(failed).toContain("codicon-error");
    expect(failed).toContain(subagentStatusSentence("failed"));
    expect(failed).not.toContain("codicon-check");

    // The sentence that lives only in a `title` attribute on the transcript
    // row, said out loud here.
    const ended = render({ agent: agentWith({ status: "ended" }) });
    expect(ended).toContain("codicon-debug-stop");
    expect(ended).toContain("The stream stopped before this subagent reported a result.");

    // The cell already owns the aria-live announcement for this agent.
    for (const html of [running, done, failed, ended]) {
      expect(html).not.toContain("aria-live");
      expect(html).not.toContain('role="status"');
    }
  });

  it("is a labelled, focusable region with a close control", () => {
    const html = render({ agent: agentWith() });
    expect(html).toContain('role="complementary"');
    expect(html).toContain('aria-label="interface_ui_code_audit subagent details"');
    expect(html).toContain('tabindex="-1"');
    expect(html).toContain('aria-label="Close subagent details"');
    expect(html).toContain("codicon-close");
  });

  it("draws Reveal in transcript only when the caller can honour it", () => {
    const wired = render({ agent: agentWith(), onReveal: () => undefined });
    expect(wired).toContain('aria-label="Reveal in transcript"');
    expect(wired).toContain("codicon-list-tree");

    const unwired = render({ agent: agentWith() });
    expect(unwired).not.toContain("Reveal in transcript");

    // No agent, nothing to reveal — even with the callback supplied.
    const nothing = render({ onReveal: () => undefined });
    expect(nothing).not.toContain("Reveal in transcript");
  });

  it("makes the tree line a breadcrumb only when the parent is selectable", () => {
    const clickable = render({
      agent: agentWith(), parentType: "orchestrator", onSelectParent: () => undefined,
    });
    expect(clickable).toContain('class="subp-link"');
    expect(clickable).toContain(">orchestrator</button>");

    const flat = render({ agent: agentWith(), parentType: "orchestrator" });
    expect(flat).toContain("Spawned by");
    expect(flat).toContain("<span>orchestrator</span>");
    expect(flat).not.toContain("subp-link");
  });

  it("shortens absolute paths with the same root the transcript rows use", () => {
    const withRoot = render({
      agent: agentWith({ result: `edited ${ROOT}/src/renderer/theme.css` }),
      pathRoot: ROOT,
    });
    expect(withRoot).toContain("edited src/renderer/theme.css");
    expect(withRoot).not.toContain(`${ROOT}/src/renderer/theme.css`);

    // No root is not a failure mode: the raw string renders unchanged.
    const noRoot = render({
      agent: agentWith({ result: `edited ${ROOT}/src/renderer/theme.css` }),
    });
    expect(noRoot).toContain(`edited ${ROOT}/src/renderer/theme.css`);
  });
});

describe("SubagentPanel — the activity list", () => {
  const children: ConvItem[] = [
    { kind: "text", id: "c1", text: "Read the theme and found the rail.", streaming: false },
    { kind: "user", id: "c2", text: "Focus on the duplicated strings." },
    { kind: "tool", id: "c3", tool: "Read", summary: `${ROOT}/src/renderer/theme.css`,
      result: { ok: true, summary: "1694 lines" }, input: {} },
    { kind: "tool", id: "c4", tool: "Bash", summary: "npm run typecheck",
      result: { ok: false, summary: "2 errors" }, input: {} },
    { kind: "tool", id: "c5", tool: "Grep", summary: "sub-desc", input: {} },
    { kind: "approval", id: "c6", tool: "Write", summary: "theme.css", input: {} },
    { kind: "approval", id: "c7", tool: "Bash", summary: "rm -rf dist", input: {},
      resolved: "deny" },
    { kind: "diff", id: "c8", callId: "c1", path: `${ROOT}/src/renderer/App.tsx` },
    { kind: "notice", id: "c9", text: "context trimmed", tone: "warning" },
    { kind: "subagent", id: "c10", subagentType: "nested_reviewer", description: "Review",
      status: "done", children: [], toolCount: 1, subagentCount: 0 },
  ];

  it("derives one read-only row per child, in the app's state vocabulary", () => {
    const rows = subagentActivityRows(children, ROOT);
    expect(rows).toHaveLength(10);

    expect(rows[0]).toMatchObject({ icon: "comment", state: null,
      prose: "Read the theme and found the rail." });
    expect(rows[1]).toMatchObject({ icon: "comment-discussion",
      prose: "Focus on the duplicated strings." });
    // A settled tool reports its RESULT summary, with the root applied.
    expect(rows[2]).toMatchObject({ state: "ok", stateWord: "Done", name: "Read",
      summary: "1694 lines" });
    expect(rows[3]).toMatchObject({ state: "failed", stateWord: "Failed", name: "Bash",
      summary: "2 errors" });
    // No result yet is "running", never a silent blank.
    expect(rows[4]).toMatchObject({ state: "running", stateWord: "Running", name: "Grep",
      summary: "sub-desc" });
    // The one row worth a visible flag, and the one that must NOT offer a verdict.
    expect(rows[5]).toMatchObject({ icon: "warning", attention: true,
      stateWord: "Needs you", name: "Write" });
    expect(rows[6]).toMatchObject({ attention: false, stateWord: "Denied", name: "Bash" });
    expect(rows[7]).toMatchObject({ icon: "git-compare", name: "src/renderer/App.tsx" });
    expect(rows[8]).toMatchObject({ icon: "warning", summary: "context trimmed" });
    // A nested spawn carries the key that re-targets the panel.
    expect(rows[9]).toMatchObject({ icon: "robot", state: "ok", name: "nested_reviewer",
      agentKey: "c10", summary: "1 tool" });
  });

  it("renders the list without a second Allow/Deny or a live transcript row", () => {
    const html = render({ agent: agentWith({ children, toolCount: 3 }), pathRoot: ROOT });
    expect(html).toContain(">Activity</h3>");
    // Said once, visibly. The hidden state word is the glyph's voice, so the
    // one row that prints its state does not also whisper it.
    expect(html).toContain('<span class="subp-act-flag">Needs you</span>');
    expect(html).not.toContain('<span class="subp-sr">Needs you</span>');
    expect(html).toContain('<span class="subp-sr">Denied</span>');
    expect(html).toContain("Read the theme and found the rail.");
    // Read-only: no verdict controls, no nested approval card, no live region.
    expect(html).not.toContain(">Allow<");
    expect(html).not.toContain(">Deny<");
    expect(html).not.toContain("aria-live");
    expect(html).not.toContain("conv-row");
    expect(html).not.toContain("class=\"sub ");
  });

  it("makes a nested spawn clickable only when the caller can re-target", () => {
    const wired = render({
      agent: agentWith({ children }), onSelectChild: () => undefined,
    });
    expect(wired).toContain('aria-label="Inspect nested_reviewer subagent"');
    expect(wired).toContain("subp-act-open");

    const unwired = render({ agent: agentWith({ children }) });
    expect(unwired).toContain("nested_reviewer");
    expect(unwired).not.toContain("subp-act-open");
    expect(unwired).not.toContain("Inspect nested_reviewer");
  });

  it("draws no Activity block at all when the subagent has done nothing", () => {
    expect(subagentActivityRows([])).toEqual([]);
    const html = render({ agent: agentWith({ children: [] }) });
    expect(html).not.toContain(">Activity</h3>");
  });

  it("derives a child's row once, again only when the child, root or parent status changes", () => {
    const text: ConvItem = { kind: "text", id: "t", text: "Edited /repo/src/a.ts", streaming: true };
    const tool: ConvItem = { kind: "tool", id: "pending", tool: "Read", summary: "/repo/b.ts",
      input: {} };
    const [first, pending] = subagentActivityRows([text, tool], "/repo", "running");
    expect(first).toMatchObject({ prose: "Edited src/a.ts" });
    // A streamed token replaces the text child only; the tool keeps its row.
    const grown: ConvItem = { ...text, text: "Edited /repo/src/a.ts and more" };
    const [second, samePending] = subagentActivityRows([grown, tool], "/repo", "running");
    expect(second).not.toBe(first);
    expect(second).toMatchObject({ prose: "Edited src/a.ts and more" });
    expect(samePending).toBe(pending);
    expect(subagentActivityRows([text], "/repo", "running")[0]).toBe(first);
    expect(subagentActivityRows([text], "/elsewhere", "running")[0])
      .toMatchObject({ prose: "Edited /repo/src/a.ts" });
    expect(subagentActivityRows([tool], "/repo", "done")[0])
      .toMatchObject({ state: "ended", stateWord: "No result" });
  });

  it("does not call an unresolved child tool running after its parent stopped", () => {
    const tool: ConvItem = { kind: "tool", id: "pending", tool: "Read",
      summary: "source", input: {} };
    expect(subagentActivityRows([tool], undefined, "done")[0])
      .toMatchObject({ state: "ended", stateWord: "No result" });
    expect(subagentActivityRows([tool], undefined, "running")[0])
      .toMatchObject({ state: "running", stateWord: "Running" });
  });
});

describe("SubagentPanel — the states with no subagent", () => {
  it("orients rather than apologises when nothing is selected", () => {
    const html = render();
    expect(html).toContain("Select a subagent in a transcript to open it here");
    expect(html).toContain("keeps each subagent one line in the transcript");
    // It still closes, and it claims nothing about an agent it does not have.
    expect(html).toContain('aria-label="Close subagent details"');
    expect(html).toContain('aria-label="Subagent details"');
    expect(html).not.toContain(">Status</h3>");
    expect(html).not.toContain(">Model</h3>");
    expect(html).not.toContain("undefined");
  });

  it("says plainly when a selection outlived its subject", () => {
    const named = render({ missing: true, missingLabel: "interface_ui_code_audit" });
    expect(named).toContain("interface_ui_code_audit is no longer in this transcript.");
    expect(named).toContain('role="status"');
    expect(named).not.toContain("Select a subagent in a transcript");

    const unnamed = render({ missing: true });
    expect(unnamed).toContain("That subagent is no longer in this transcript.");
    expect(unnamed).not.toContain("undefined");

    // `missing` is consulted only when there is no agent to show.
    const live = render({ agent: agentWith(), missing: true });
    expect(live).not.toContain("no longer in this transcript");
    expect(live).toContain(">Status</h3>");
  });

  it("titles an unnamed missing selection rather than drawing a blank heading", () => {
    // App remembers the name as "" until a render resolves it, and `??` lets ""
    // through — so a selection that never resolved put an EMPTY <h2> above a
    // sentence that handled the same value carefully. The two agree now.
    const blank = render({ missing: true, missingLabel: "" });
    expect(blank).toContain(">Subagent</h2>");
    expect(blank).not.toContain("></h2>");
    expect(blank).toContain("That subagent is no longer in this transcript.");
    expect(render({ missing: true, missingLabel: "   " })).toContain(">Subagent</h2>");
  });
});

describe("SubagentPanel — pure helpers", () => {
  it("names each state once, in a word and in a sentence", () => {
    expect(subagentStatusWord("running")).toBe("Running");
    expect(subagentStatusWord("done")).toBe("Done");
    expect(subagentStatusWord("failed")).toBe("Failed");
    expect(subagentStatusWord("ended")).toBe("Ended");
    expect(subagentStatusSentence("ended"))
      .toBe("The stream stopped before this subagent reported a result.");
    expect(subagentStatusSentence("running")).not.toBe(subagentStatusSentence("done"));
  });

  it("draws each state with one glyph, the transcript's and the panel's", () => {
    expect(subagentGlyph("running")).toBe("running");
    expect(subagentGlyph("done")).toBe("ok");
    expect(subagentGlyph("failed")).toBe("failed");
    expect(subagentGlyph("ended")).toBe("ended");
  });

  it("counts delegated work first, and says nothing about zero", () => {
    expect(subagentCountsText(17, 2)).toBe("2 subagents · 17 tools");
    expect(subagentCountsText(1, 1)).toBe("1 subagent · 1 tool");
    expect(subagentCountsText(3, 0)).toBe("3 tools");
    expect(subagentCountsText(0, 1)).toBe("1 subagent");
    expect(subagentCountsText(0, 0)).toBe("");
  });

  it("claims Escape and no other key", () => {
    expect(subagentPanelKeyAction("Escape")).toBe("close");
    expect(subagentPanelKeyAction("Enter")).toBeNull();
    expect(subagentPanelKeyAction("Tab")).toBeNull();
    expect(subagentPanelKeyAction("f")).toBeNull();
  });

  it("handles Escape on the panel subtree, never on the document", () => {
    const source = SubagentPanel.toString();
    expect(source).toContain("onKeyDown");
    expect(source).toContain("stopPropagation");
    expect(source).not.toContain("document.addEventListener");
    expect(source).not.toContain("window.addEventListener");
    // Presentational: no IPC, no store, no fetching.
    expect(source).not.toContain("window.iface");
  });

  it("moves focus into the region only when the keyboard opened it", () => {
    // renderToStaticMarkup runs no effects and this suite has no DOM, so the
    // guard is asserted at the source. Dropping it — focusing on every open —
    // would steal the caret from a mouse user still reading the transcript.
    const source = SubagentPanel.toString();
    expect(source).toContain("focusOnOpen !== true");
    expect(source).toContain("focusRequest");
    expect(source).toContain("rootRef");
    expect(source).toContain(".focus()");
  });

  it("paints from app tokens with no colour literal", () => {
    expect(SUBAGENT_PANEL_CSS).toContain("var(--material-chrome)");
    expect(SUBAGENT_PANEL_CSS).toContain("var(--border)");
    expect(SUBAGENT_PANEL_CSS).toContain("var(--fg-muted)");
    expect(SUBAGENT_PANEL_CSS).toContain("var(--focus)");
    expect(SUBAGENT_PANEL_CSS).toContain("var(--row-hover)");
    expect(SUBAGENT_PANEL_CSS).toContain("var(--status-waiting)");
    expect(SUBAGENT_PANEL_CSS).not.toMatch(/#[0-9a-f]{3,8}/i);
    // One hairline against the workspace, and no card chrome of its own.
    expect(SUBAGENT_PANEL_CSS).toContain("border-left:var(--border-w) solid var(--border)");
    // Nothing animates here; theme.css already guards the one pulsing dot.
    expect(SUBAGENT_PANEL_CSS).not.toContain("transition:");
    expect(SUBAGENT_PANEL_CSS).not.toContain("animation:");
  });

  it("uses only codicon names declared in codicons.css", () => {
    const html = render({
      agent: agentWith({ status: "failed", children: [
        { kind: "diff", id: "d1", callId: "c1", path: "src/renderer/App.tsx" },
        { kind: "notice", id: "n1", text: "note", tone: "info" },
        { kind: "notice", id: "n2", text: "bad", tone: "error" },
        { kind: "subagent", id: "s1", subagentType: "child", description: "d",
          status: "running", children: [], toolCount: 0, subagentCount: 0 },
      ] }),
      onReveal: () => undefined,
    });
    const declared = new Set(["robot", "close", "list-tree", "check", "error",
      "debug-stop", "warning", "info", "comment", "comment-discussion", "git-compare",
      "sm"]);
    const used = [...html.matchAll(/codicon-([a-z-]+)/gu)].map(match => match[1]);
    expect(used.length).toBeGreaterThan(0);
    for (const name of used) expect(declared).toContain(name);
  });

  it("accepts a reduced ConvItem subagent with no mapping layer", () => {
    // Compile-time proof of the integrator's call site: what findSubagentItem
    // returns — including its `undefined`, which is the empty state — is what
    // the prop takes. No adapter, no mapping layer, nothing to keep in sync.
    const item: Extract<ConvItem, { kind: "subagent" }> = {
      kind: "subagent", id: "toolu_01", subagentType: "reviewer",
      description: "/root/reviewer", status: "running", children: [],
      toolCount: 0, subagentCount: 0,
    };
    const found = findSubagentItem([item], "toolu_01");
    const html = render({ agent: found });
    expect(html).toContain("reviewer");
    expect(html).toContain("/root/reviewer");

    // The panel is what a dead selection resolves to, not a crash.
    expect(render({ agent: findSubagentItem([item], "gone"), missing: true }))
      .toContain("no longer in this transcript");
  });
});
