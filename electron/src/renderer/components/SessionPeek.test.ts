import { readFileSync } from "node:fs";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import {
  positionSessionPeek, SESSION_PEEK_CSS, SessionPeek, type SessionPeekProps,
} from "./SessionPeek";

const source = readFileSync(new URL("./SessionPeek.tsx", import.meta.url), "utf8");

function card(over: Partial<SessionPeekProps> = {}): string {
  return renderToStaticMarkup(createElement(SessionPeek, {
    id: "peek",
    title: "Qwen catalog fix",
    badge: "waiting",
    status: "Needs attention",
    age: "3m",
    diff: { added: 18, removed: 4 },
    worktree: "qwen-catalog-2",
    facts: {
      activity: "Ran qwen model probe — catalog empty on 2nd boot",
      files: 6,
      approval: "Allow Bash: rm -rf ~/.qwen/cache ?",
    },
    box: { top: 44, left: 300 },
    ...over,
  }));
}

describe("session peek — the read-only preview", () => {
  it("answers the three questions a hover asks, in one card", () => {
    const html = card();
    expect(html).toContain('id="peek"');
    expect(html).toContain('role="tooltip"');
    expect(html).toContain("Qwen catalog fix");
    expect(html).toContain("waiting 3m");
    expect(html).toContain("Ran qwen model probe — catalog empty on 2nd boot");
    expect(html).toContain('class="session-peek-added">+18<');
    expect(html).toContain('class="session-peek-removed">−4<');
    expect(html).toContain("6 files");
    expect(html).toContain("qwen-catalog-2");
    expect(html).toContain(">Worktree<");
    // Never "Branch": the app has the checkout's name, not the ref.
    expect(html).not.toContain(">Branch<");
  });

  it("quotes the pending request verbatim under its own heading", () => {
    // A paraphrase of a permission prompt is how someone approves the wrong
    // thing. The card shows the engine's own words or shows nothing.
    const html = card();
    expect(html).toContain("Waiting on you");
    expect(html).toContain("Allow Bash: rm -rf ~/.qwen/cache ?");
  });

  it("stays read-only: nothing in it can answer the request", () => {
    const html = card();
    expect(html).not.toContain("<button");
    expect(html).not.toContain("Allow<");
    expect(html).not.toContain("Deny");
    expect(html).toContain("Click opens the session");
    expect(html).toContain("Space");
    // A card under the pointer must never eat the click meant for the row.
    expect(SESSION_PEEK_CSS).toContain("pointer-events: none;");
  });

  it("shows the request only while the session is actually blocked", () => {
    const working = card({
      badge: "working", status: "Working",
      facts: { activity: "Editing mapper.ts", approval: "Allow Bash: rm -rf /" },
    });
    expect(working).not.toContain("Waiting on you");
    expect(working).not.toContain("Allow Bash");
    expect(working).toContain("Working · 3m");
  });

  it("omits honestly instead of drawing placeholder rows", () => {
    const bare = card({
      diff: null, worktree: "", facts: {},
      badge: "idle", status: "Idle",
    });
    expect(bare).not.toContain("session-peek-facts");
    expect(bare).not.toContain("Changes");
    expect(bare).not.toContain("Worktree");
    expect(bare).not.toContain("Last");
    expect(bare).not.toContain("Waiting on you");
    // The header and the hint are all a card with nothing to report shows.
    expect(bare).toContain("Idle · 3m");
  });

  it("drops the file count rather than claiming zero files", () => {
    const html = card({ facts: { files: 0 } });
    expect(html).toContain('class="session-peek-added">+18<');
    expect(html).not.toContain("0 file");
    expect(card({ facts: { files: 1 } })).toContain("1 file");
  });

  it("hides itself until it has been measured", () => {
    expect(card({ box: null })).toContain("visibility:hidden");
    expect(card()).toContain("top:44px");
    expect(card()).toContain("left:300px");
  });
});

describe("positionSessionPeek", () => {
  const size = { width: 296, height: 200 };
  const viewport = { width: 1440, height: 900 };
  const rowAt = (top: number) => ({ top, bottom: top + 42, left: 8, right: 280 });

  it("sits beside the row, clear of the pane", () => {
    expect(positionSessionPeek(rowAt(120), size, viewport)).toEqual({ top: 120, left: 288 });
  });

  it("flips to the other side when the card would not fit", () => {
    const narrow = { width: 900, height: 900 };
    const anchor = { top: 100, bottom: 142, left: 600, right: 876 };
    expect(positionSessionPeek(anchor, size, narrow).left).toBe(600 - 8 - size.width);
  });

  it("stays inside the window even when neither side has room", () => {
    const tiny = { width: 320, height: 900 };
    expect(positionSessionPeek({ top: 40, bottom: 82, left: 8, right: 300 }, size, tiny).left)
      .toBe(8);
  });

  it("keeps a whole card on screen for a row near the bottom", () => {
    const placed = positionSessionPeek(rowAt(860), size, viewport);
    expect(placed.top).toBe(900 - 8 - 200);
    expect(placed.top + size.height).toBeLessThanOrEqual(viewport.height);
  });

  it("never leaves the window on a viewport shorter than the card", () => {
    const placed = positionSessionPeek(rowAt(40), size, { width: 1440, height: 120 });
    expect(placed.top).toBe(8);
  });
});

describe("session peek — the overlay idiom, in tokens", () => {
  it("draws itself the way every other floating surface does", () => {
    for (const token of ["var(--widget-bg)", "var(--border-strong)",
      "var(--radius-overlay)", "var(--overlay-shadow)"]) {
      expect(SESSION_PEEK_CSS).toContain(token);
    }
    expect(SESSION_PEEK_CSS).toContain("var(--diff-added)");
    expect(SESSION_PEEK_CSS).toContain("var(--diff-removed)");
    // The amber border IS the blocked-on-you meaning.
    expect(SESSION_PEEK_CSS).toMatch(/\.session-peek-ask \{[^}]*var\(--status-waiting\)/u);
  });

  it("appears without motion for anyone who asked for none", () => {
    expect(SESSION_PEEK_CSS).toContain("@media (prefers-reduced-motion: reduce)");
    expect(SESSION_PEEK_CSS).toMatch(
      /@media \(prefers-reduced-motion: reduce\) \{\s*\.session-peek \{ animation: none; \}/u);
  });

  it("spends no literal colour", () => {
    // Every hue in this file must come from a token, or the card stops being
    // theme-aware the first time someone switches palettes.
    expect(source).not.toMatch(/#[0-9a-fA-F]{3,8}\b/u);
    expect(source).not.toMatch(/\b(?:rgb|hsl)a?\(/u);
  });
});
