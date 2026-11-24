import { readFileSync } from "node:fs";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import {
  highlightSearchText, librarySessionTitle, libraryProjectLabel,
  SESSION_LIBRARY_CSS,
  SessionLibrary,
  type SessionLibraryProps,
} from "./SessionLibrary";

const props = (over: Partial<SessionLibraryProps> = {}): SessionLibraryProps => ({
  open: true, tab: "search", query: "needle", onTabChange: () => undefined,
  onQueryChange: () => undefined, onSearch: () => undefined,
  searchHits: [], archived: [], worktrees: [], onRefresh: () => undefined,
  onOpenSession: () => undefined, onRestoreSession: () => undefined,
  onOpenWorktree: () => undefined, onOpenTerminal: () => undefined,
  onCleanWorktree: () => undefined, onClose: () => undefined, ...over,
});

describe("SessionLibrary", () => {
  it("provides one accessible browser for search, archives and worktrees", () => {
    const html = renderToStaticMarkup(createElement(SessionLibrary, props()));
    expect(html).toContain('role="dialog"');
    expect(html).toContain('aria-modal="true"');
    expect(html).toContain('role="tablist"');
    expect(html).toContain("Search transcripts");
    expect(html).toContain("Archived");
    expect(html).toContain("Worktrees");
    expect(html.match(/role="tab"/g)).toHaveLength(3);
    expect(html.match(/role="tab"[^>]*tabindex="0"/g)).toHaveLength(1);
    expect(html.match(/role="tab"[^>]*tabindex="-1"/g)).toHaveLength(2);
    expect(html.match(/role="tabpanel"/g)).toHaveLength(3);
  });

  it("renders bounded-search incompleteness instead of implying full results", () => {
    const html = renderToStaticMarkup(createElement(SessionLibrary, props({
      searchIncomplete: ["Grok history is unavailable", "Codex deadline reached"],
    })));
    expect(html).toContain("Partial results");
    expect(html).toContain("Grok history is unavailable");
  });

  it("highlights literal queries case-insensitively and escapes provider text", () => {
    const html = renderToStaticMarkup(createElement("span", null,
      highlightSearchText("<script> [A+B] and [a+b]", "[a+b]")));
    expect(html).toBe("<span>&lt;script&gt; <mark>[A+B]</mark> and <mark>[a+b]</mark></span>");
    expect(highlightSearchText("Unchanged", "  ")).toBe("Unchanged");
  });

  it("shows two unique excerpts first and leaves extra matches in a closed disclosure", () => {
    const html = renderToStaticMarkup(createElement(SessionLibrary, props({
      searchHits: [{ key: "codex#one", engine: "codex", title: "/tmp/long/provider/transcript",
        cwd: "/work/project", excerpts: ["needle one", "needle one", "needle two", "needle three"] }],
    })));
    expect(html).toContain('class="session-library-result-count"');
    expect(html).toContain('title="/tmp/long/provider/transcript">Codex session');
    expect(html).toContain('title="/work/project">Codex · project');
    expect(html.match(/<mark>needle<\/mark>/g)).toHaveLength(3);
    expect(html).toContain('<details class="session-library-more"><summary>Show 1 more match</summary>');
    expect(html).not.toContain("<details open");
    expect(html).toContain("needle</mark> three");
  });

  it("keeps useful titles and full path fallbacks available", () => {
    expect(librarySessionTitle("Fix the renderer", "codex")).toBe("Fix the renderer");
    expect(librarySessionTitle("<local-command-caveat>...", "claude")).toBe("Claude Code session");
    expect(libraryProjectLabel("/work/project/")).toBe("project");
    expect(libraryProjectLabel("/")).toBe("/");
  });

  it("offers backend-rechecked cleanup only for retained worktrees", () => {
    const html = renderToStaticMarkup(createElement(SessionLibrary, props({
      tab: "worktrees",
      worktrees: [
        { key: "claude#live", path: "/tmp/live", sourceCwd: "/repo/src",
          state: "active", dirty: false },
        { key: "claude#dirty", path: "/tmp/dirty", sourceCwd: "/repo",
          state: "retained", dirty: true },
        { key: "claude#clean", path: "/tmp/clean", sourceCwd: "/repo",
          state: "retained", dirty: false },
      ],
    })));
    expect(html.match(/>Clean up</g)).toHaveLength(2);
    expect(html).toContain("Retained · changed");
  });

  it("confirms Clean up before a managed worktree leaves the disk", () => {
    const source = readFileSync(new URL("./SessionLibrary.tsx", import.meta.url), "utf8");
    // The row button only arms the dialog; the caller hears from its confirm alone.
    expect(source).toContain("onClick={() => { setCleanupTarget(item); }}>Clean up</button>");
    expect(source).toContain('confirmLabel="Clean up" tone="danger"');
    expect(source.match(/onCleanWorktree\(/g)).toHaveLength(1);
    // A confirm left armed when the library closes must not greet the next open.
    expect(source).toContain("if (!open) setCleanupTarget(null);");
    const html = renderToStaticMarkup(createElement(SessionLibrary, props({
      tab: "worktrees",
      worktrees: [{ key: "claude#clean", path: "/tmp/clean", sourceCwd: "/repo",
        state: "retained", dirty: false }],
    })));
    expect(html).not.toContain('role="alertdialog"');
  });

  it("ages archived rows relatively and keeps the exact instant on hover", () => {
    const html = renderToStaticMarkup(createElement(SessionLibrary, props({
      tab: "archives",
      now: Date.parse("2026-08-15T12:00:00.000Z"),
      archived: [{
        key: "claude#one", engine: "claude", title: "Create a file named hello.txt",
        cwd: "/private/tmp/iface-walk", lastActivity: "2026-08-15T04:32:03.309Z",
      }],
    })));
    expect(html).toContain("<time ");
    expect(html).toContain('title="2026-08-15T04:32:03.309Z">7h</time>');
    // The machine timestamp must not be what the row prints.
    expect(html).not.toContain(">2026-08-15T04:32:03.309Z<");
  });

  it("never says a finished session was active 'now' for a stamp ahead of this clock", () => {
    const html = renderToStaticMarkup(createElement(SessionLibrary, props({
      tab: "archives",
      now: Date.parse("2026-08-15T12:00:00.000Z"),
      archived: [{
        key: "claude#skewed", engine: "claude", title: "Imported from a fast machine",
        cwd: "/repo", lastActivity: "2027-01-01T00:00:00.000Z",
      }],
    })));
    expect(html).not.toContain(">now<");
    expect(html).toContain(">2027-01-01T00:00:00.000Z</time>");
  });

  it("prints an unparseable timestamp rather than a blank where the age goes", () => {
    const html = renderToStaticMarkup(createElement(SessionLibrary, props({
      tab: "archives",
      now: Date.parse("2026-08-15T12:00:00.000Z"),
      archived: [{
        key: "claude#odd", engine: "claude", title: "Imported session",
        cwd: "/repo", lastActivity: "unknown",
      }],
    })));
    expect(html).not.toContain("<time");
    expect(html).toContain("unknown");
  });

  it("renders nothing while closed", () => {
    expect(renderToStaticMarkup(createElement(SessionLibrary, props({ open: false })))).toBe("");
  });

  it("uses the shared focus lifecycle and retains logical row focus after mutations", () => {
    const source = SessionLibrary.toString();
    expect(source).toContain("useModalFocus");
    expect(source).toContain("rovingTabKeyAction");
    expect(source).toContain("logicalListFocusIndex");
    expect(source).toContain("pendingRowFocus");
  });

  it("uses a compact, flat library palette without hiding keyboard actions", () => {
    expect(SESSION_LIBRARY_CSS).toContain("place-items:start center");
    expect(SESSION_LIBRARY_CSS).toContain("width:min(760px, 100%)");
    // The app's ONE horizontal-tab selected look, in its one selection hue:
    // this drew the underline in bronze while the toolbar's switch two feet
    // away drew the same underline in blue.
    expect(SESSION_LIBRARY_CSS).toContain("box-shadow:inset 0 -2px 0 var(--selection-accent)");
    // And the palette's overlay recipe, so three sibling dialogs stop being
    // three: same 60% scrim, same offset, same 38px row, same backdrop fade.
    expect(SESSION_LIBRARY_CSS).toContain("var(--chrome-bg) 60%");
    expect(SESSION_LIBRARY_CSS).toContain("padding:max(64px, 10vh) 24px 24px");
    expect(SESSION_LIBRARY_CSS).toContain("min-height:38px; padding:5px 8px");
    expect(SESSION_LIBRARY_CSS).toContain("prefers-reduced-motion: reduce");
    expect(SESSION_LIBRARY_CSS).toContain("border-radius:0");
    // Row actions are always visible: no rule dims them, so none has to reveal
    // them on hover or keyboard focus.
    expect(SESSION_LIBRARY_CSS).not.toMatch(/\.session-library-actions[^{}]*\{[^}]*opacity/u);
    expect(SESSION_LIBRARY_CSS).toContain(".session-library-action { min-height:28px;");
    expect(SESSION_LIBRARY_CSS).toContain(
      ".session-library-state { padding:34px 16px; color:var(--fg-dim);",
    );
    expect(SESSION_LIBRARY_CSS).toContain("@container (max-width:750px)");
    expect(SESSION_LIBRARY_CSS).toContain("@media (max-width:760px)");
    expect(SESSION_LIBRARY_CSS).toContain(":not(:disabled):hover");
  });

  it("leaves the attention hue to agents that are actually blocked on you", () => {
    // A 2px amber left rail is the app's "an agent needs you" shape. This
    // banner says things like "Partial results", and a dirty worktree is a
    // version-control fact, so neither may wear it.
    expect(SESSION_LIBRARY_CSS).not.toContain("var(--status-waiting)");
    expect(SESSION_LIBRARY_CSS).toContain("padding:5px 8px; color:var(--fg-muted);");
    expect(SESSION_LIBRARY_CSS).toContain(".session-library-dirty { color:var(--vcs-modified); }");
    // The rail follows the text, so an error notice still rails in danger.
    expect(SESSION_LIBRARY_CSS).toContain("border-left:2px solid currentColor;");
    expect(SESSION_LIBRARY_CSS).toContain(".session-library-error { color:var(--danger-fg); }");
  });
});
