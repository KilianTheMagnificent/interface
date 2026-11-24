import { createElement, type ComponentProps } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { CollaborationStatusDto } from "../../shared/ipc";
import type { ProjectFileEntry } from "../../shared/ipc";
import { isPreviewableOutput, outputPreviewTarget } from "../output-preview";
import type { ThreadOpinionCard } from "../thread-opinion";
import { AppSelect } from "./AppSelect";
import { CommandPalette } from "./CommandPalette";
import { TerminalLauncher } from "./TerminalLauncher";
import { ThreadChangesCard } from "./ThreadTurnParts";
import { ThreadOpinionCards } from "./ThreadOpinionCards";
import { lineageWords, ThreadPaneHead, ThreadPaneMenuItems } from "./ThreadPaneHead";

const STATUS = {
  receipt: {
    version: 1, collaborationId: "c1", revision: 3, state: "awaiting_peer_review",
    mode: "consult", sourceEngine: "claude", targetEngine: "codex", approvalRequired: true,
    cancellationSafe: false, createdAt: "2026-10-14T11:58:00.000Z",
    updatedAt: "2026-10-14T11:59:00.000Z",
  },
  sourceSessionKey: "claude#a",
} satisfies CollaborationStatusDto;

function card(over: Partial<ThreadOpinionCard>): ThreadOpinionCard {
  return {
    id: "c1", stage: "ready", engine: "codex", label: "Codex", sourceLabel: "Claude Code",
    cancellable: false, status: STATUS, ...over,
  };
}

function cards(props: Partial<ComponentProps<typeof ThreadOpinionCards>>): string {
  return renderToStaticMarkup(createElement(ThreadOpinionCards, {
    cards: [], limit: null, pending: [], onAction: () => undefined, onSwitch: () => undefined,
    ...props,
  }));
}

describe("a second opinion in the thread (review B4)", () => {
  it("is the other provider's card, never a turn: attributed, held until sent on", () => {
    const html = cards({ cards: [card({ body: "The retry ceiling is never read." })] });
    expect(html).toContain('aria-label="Second opinion from Codex"');
    expect(html).toContain("Second opinion from Codex</span>");
    expect(html).toContain("Not sent to Claude Code");
    expect(html).toContain("The retry ceiling is never read.");
    expect(html).toContain(">Send to Claude Code</button>");
    expect(html).toContain(">Dismiss</button>");
    expect(html).not.toContain("thread-user");
  });

  it("shows what goes out before anything leaves: Ask or Cancel, with the packet", () => {
    const html = cards({ cards: [card({
      stage: "review", cancellable: true,
      packet: { bytes: 9_626, omissions: 1, preview: "interface collaboration packet v2" },
    })] });
    expect(html).toContain("reads the latest answer, that turn&#x27;s changes and the recent conversation");
    expect(html).toContain("(9.4 KB, older parts left out). It can&#x27;t change files.");
    expect(html).toContain("<summary>What is sent</summary>");
    expect(html).toContain("interface collaboration packet v2");
    expect(html).toContain(">Ask Codex</button>");
    expect(html).toContain(">Cancel</button>");
    expect(cards({ cards: [card({ stage: "review", packet: { bytes: 10, omissions: 0, preview: "" } })],
      pending: ["c1"] })).toContain("disabled");
  });

  it("says when another provider is reading, sending, or why it failed", () => {
    expect(cards({ cards: [card({ stage: "running", cancellable: true })] }))
      .toContain("Codex is reading the thread…");
    expect(cards({ cards: [card({ stage: "sending", body: "x" })] })).toContain("Sending to Claude Code…");
    const failed = cards({ cards: [card({ stage: "failed", error: "Restore the target provider." })] });
    expect(failed).toContain('role="alert"');
    expect(failed).toContain("Restore the target provider.");
    expect(cards({})).toBe("");
  });

  it("offers Switch to when the thread's provider hit its limit, never by itself", () => {
    const html = cards({ limit: {
      message: "Claude Code hit its usage limit; it resets in 2h.", engine: "codex",
      label: "Codex", headroom: "62% left",
    } });
    expect(html).toContain('role="status"');
    expect(html).toContain("Claude Code hit its usage limit; it resets in 2h.");
    expect(html).toContain(">Switch to Codex</button>");
    expect(html).not.toContain("Continue with");
    expect(html).toContain("62% left");
  });

  it("also offers the same provider's other account, and the pane head names the thread's account", () => {
    const html = cards({ onContinue: () => undefined, limit: {
      message: "Claude Code hit its usage limit.", engine: "codex", label: "Codex", headroom: "90% left",
      also: [{ engine: "claude", label: "Claude Code (Work)", headroom: "38% left", account: "acct-1" }],
    } });
    expect(html).toContain(">Switch to Codex</button>");
    expect(html).toContain(">Continue with Claude Code (Work)</button>");
    expect(html.indexOf("Switch to Codex")).toBeLessThan(html.indexOf("Continue with Claude Code (Work)"));
    expect(html).toContain("38% left");
    // Without a Continue handler no account is offered: only the switch.
    const switchOnly = cards({ limit: {
      message: "m", engine: "codex", label: "Codex",
      also: [{ engine: "claude", label: "Claude Code (Work)", headroom: "38% left", account: "acct-1" }],
    } });
    expect(switchOnly).not.toContain("Claude Code (Work)");
    const head = (account: string | null): string => renderToStaticMarkup(createElement(ThreadPaneHead,
      { workspace: { mode: "local", projectRoot: "/p", owned: false, branch: "main" }, account }));
    expect(head("Work")).toContain('<span class="thread-head-place thread-head-account" title="Provider account: Work">Work</span>');
    expect(head("account removed")).toContain(">account removed</span>");
    expect(head(null)).not.toContain("thread-head-account");
  });

    it("puts Second opinion and Switch to in the pane menu, waiting while a turn runs", () => {
    const items = (refusal: string | null): string => renderToStaticMarkup(createElement(
      ThreadPaneMenuItems, {
        title: "Fix it", maximized: false, splitLabel: "Open a new pane",
        choose: () => undefined, onToggleMaximize: () => undefined, onClose: () => undefined,
        providers: {
          offers: { opinion: [{ engine: "codex", label: "Codex" }],
            switchTo: [{ engine: "codex", label: "Codex" }] },
          refusal, onSecondOpinion: () => undefined, onSwitchTo: () => undefined,
        },
      }));
    const idle = items(null);
    expect(idle).toContain("<span>Second opinion from Codex</span>");
    expect(idle).toContain("<span>Switch to Codex</span>");
    expect(idle).not.toContain("Continue with");
    expect(idle).not.toContain("disabled");
    expect(items("Let the turn finish or stop it first.")).toContain('disabled="" title="Let the turn finish');
    expect(lineageWords("handoff")).toBe("Continued from ");
    expect(lineageWords("fork")).toBe("Forked from ");
  });
});

describe("output preview in Threads (review B5)", () => {
  it("previews PDFs, images and Markdown from inside the thread's folder only", () => {
    expect(["a.pdf", "shot.PNG", "b.jpeg", "c.webp", "README.md", "notes.markdown", "g.gif"]
      .every(isPreviewableOutput)).toBe(true);
    expect(["a.ts", "b.svg", "c.html", ".env"].some(isPreviewableOutput)).toBe(false);
    expect(outputPreviewTarget("/repo/docs/report.pdf", "/repo/")).toEqual(
      { root: "/repo", path: "docs/report.pdf" });
    expect(outputPreviewTarget("docs/plot.png", "/repo")).toEqual({ root: "/repo", path: "docs/plot.png" });
    expect(outputPreviewTarget("/elsewhere/report.pdf", "/repo")).toBeNull();
    expect(outputPreviewTarget("/repo2/report.pdf", "/repo")).toBeNull();
    expect(outputPreviewTarget("../secret.md", "/repo")).toBeNull();
    expect(outputPreviewTarget("/repo/src/main.ts", "/repo")).toBeNull();
  });

  it("offers Preview on a changes card's previewable files", () => {
    const html = renderToStaticMarkup(createElement(ThreadChangesCard, {
      view: { files: 2, stat: { added: 3, removed: 0 }, more: 0, rows: [
        { path: "/repo/out/report.pdf", shown: "out/report.pdf", letter: "A", verb: "add", stat: null },
        { path: "/repo/src/a.ts", shown: "src/a.ts", letter: "M", verb: "update", stat: null },
      ] },
      onReview: () => undefined, onPreview: () => undefined,
    }));
    expect(html).toContain('aria-label="Preview out/report.pdf"');
    expect(html).not.toContain('aria-label="Preview src/a.ts"');
    expect(renderToStaticMarkup(createElement(ThreadChangesCard, {
      view: { files: 1, stat: null, more: 0, rows: [
        { path: "/repo/out/report.pdf", shown: "out/report.pdf", letter: "A", verb: "add", stat: null },
      ] },
      onReview: () => undefined,
    }))).not.toContain("Preview");
  });

  it("lets Quick Open open the highlighted PDF, image or Markdown in the preview", () => {
    const files: ProjectFileEntry[] = [{ path: "/repo/docs/report.pdf", relativePath: "docs/report.pdf" }];
    const palette = (onPreviewFile?: (file: ProjectFileEntry) => void): string =>
      renderToStaticMarkup(createElement(CommandPalette, {
        open: true, query: "", commands: [], mode: "files", files,
        onQuery: () => undefined, onRun: () => undefined, onOpenFile: () => undefined,
        onClose: () => undefined, ...(onPreviewFile === undefined ? {} : { onPreviewFile }),
      }));
    const html = palette(() => undefined);
    expect(html).toContain('aria-label="Command Enter opens it in the preview"');
    expect(html).toContain("⌘↵ opens a PDF, image or Markdown in the preview");
    expect(palette()).not.toContain("Preview");
  });
});

describe("plan headroom where a provider is chosen (review E)", () => {
  it("says it quietly on the model chip, in its accessible name and tooltip", () => {
    const html = renderToStaticMarkup(createElement(AppSelect, {
      label: "Model", value: "opus", options: [{ value: "opus", label: "Opus" }],
      onChange: () => undefined, trailing: "38% left",
      trailingTitle: "Claude Code plan: 62% of the 5-hour window used",
    }));
    expect(html).toContain('aria-label="Model: Opus, 38% left"');
    expect(html).toContain('class="app-select-trailing" aria-hidden="true">38% left</span>');
    expect(html).toContain("Claude Code plan: 62% of the 5-hour window used");
  });

  it("puts it under an agent tile's name in the launcher", () => {
    const html = renderToStaticMarkup(createElement(TerminalLauncher, {
      profiles: [
        { id: "claude", label: "Claude Code", engine: "claude", available: true },
        { id: "codex", label: "Codex", engine: "codex", available: true },
      ],
      profilesLoading: false, cwd: "/repo", launching: false, notice: null,
      onLaunch: () => undefined, onBrowse: () => Promise.resolve(null),
      onRetryProfiles: () => undefined,
      headroom: { claude: { text: "38% left", title: "Claude Code plan: 62% used" } },
    }));
    expect(html).toContain('aria-label="Claude Code, 38% left"');
    expect(html).toContain("terminal-launcher-headroom");
    expect(html).toContain(">38% left</span>");
    expect(html).toContain('aria-label="Codex"');
  });
});
