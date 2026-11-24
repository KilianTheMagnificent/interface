import { readFileSync } from "node:fs";
import { compositeHex, contrastRatio } from "../contrast";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import fixture from "../../fixtures/threads-v34.json";
import fixtureV45 from "../../fixtures/threads-v45.json";
import type { SessionSummary } from "../../shared/engine";
import type { ThreadsView } from "../../shared/threads";
import { projectHueClass } from "../project-hue";
import { ALL_THREAD_PROJECTS, buildThreadInbox, NO_THREAD_PROJECT, threadScopeFor, type ThreadScope } from "../thread-inbox";
import type { ThreadInboxController } from "../use-thread-inbox";
import { Sidebar, type SidebarProps } from "./Sidebar";
import { positionThreadSubmenu, THREAD_DRAG_MIME, ThreadInbox, type ThreadInboxProps } from "./ThreadInbox";

const source = readFileSync(new URL("./ThreadInbox.tsx", import.meta.url), "utf8");
const sidebarSource = readFileSync(new URL("./Sidebar.tsx", import.meta.url), "utf8");
const themeCss = readFileSync(new URL("../theme.css", import.meta.url), "utf8");
/** The inbox's own stylesheet, the template string in ThreadInbox.tsx. */
const inboxCss = source.slice(source.indexOf("const THREAD_INBOX_CSS = `"),
  source.indexOf("`;", source.indexOf("const THREAD_INBOX_CSS = `")));
// Every shipped palette, and a token's value in one of them (var() chains followed).
const rootBlock = themeCss.match(/^:root \{([\s\S]*?)\n\}/mu)?.[1] ?? "";
const themes = [{ id: ":root", body: rootBlock }, ...[...themeCss.matchAll(/:root\[data-theme="([^"]+)"\] \{([\s\S]*?)\n\}/gu)]
  .map(([, id, body]) => ({ id, body })).filter(({ id }) => !id.startsWith("custom-"))];
function resolve(body: string, name: string, depth = 0): string {
  const value = (body.match(new RegExp(`--${name}:\\s*([^;]+);`, "u"))
    ?? rootBlock.match(new RegExp(`--${name}:\\s*([^;]+);`, "u")))?.[1].trim() ?? "";
  const reference = /^var\(--([a-z0-9-]+)\)$/u.exec(value)?.[1];
  return reference !== undefined && depth < 8 ? resolve(body, reference, depth + 1) : value;
}
const channels = (hex: string): number[] => [1, 3, 5].map(at => Number.parseInt(hex.slice(at, at + 2), 16));
/** color-mix(in srgb, front weight, back) for two opaque #rrggbb colours. */
const mix = (front: string, back: string, weight: number): string => `#${channels(front)
  .map((value, index) => Math.round(value * weight + channels(back)[index] * (1 - weight)).toString(16).padStart(2, "0"))
  .join("")}`;
/** A row fill as painted on the sidebar's sheet: an #rrggbbaa token composited, an opaque one as it is. */
const onSheet = (fill: string, sheet: string): string =>
  /^#[0-9a-f]{8}$/iu.test(fill) ? compositeHex(fill, sheet) : fill.slice(0, 7);
/** What a row's words can sit on in a palette: the sheet, the hover fill and the selection. */
const rowFills = (body: string, sheet: string): string[] =>
  [sheet, onSheet(resolve(body, "row-hover"), sheet), onSheet(resolve(body, "row-selected"), sheet)];
// sRGB <-> OKLCH (CSS Color 4), for the relative colours the Needs you pill reads.
const toLinear = (v: number): number => (v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4);
const fromLinear = (v: number): number => (v <= 0.0031308 ? v * 12.92 : 1.055 * v ** (1 / 2.4) - 0.055);
function toOklch(hex: string): [number, number, number] {
  const [r, g, b] = channels(hex).map(v => toLinear(v / 255));
  const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
  const m = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
  const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
  const lightness = 0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s;
  const a = 1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s;
  const bb = 0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s;
  return [lightness, Math.hypot(a, bb), Math.atan2(bb, a)];
}
function fromOklch(lightness: number, chroma: number, hue: number): string {
  const [a, b] = [chroma * Math.cos(hue), chroma * Math.sin(hue)];
  const l = (lightness + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const m = (lightness - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const s = (lightness - 0.0894841775 * a - 1.2914855480 * b) ** 3;
  return `#${[4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
    -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
    -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s]
    .map(v => Math.round(Math.min(1, Math.max(0, fromLinear(v))) * 255).toString(16).padStart(2, "0")).join("")}`;
}
const noop = (): void => undefined;
const ATLAS = "/Users/owner/Developer/Projects/atlas";
const ATLAS_HUE = projectHueClass(ATLAS);

function session(key: string, title: string, over: Partial<SessionSummary> = {}): SessionSummary {
  const [engine, sessionId] = key.split("#") as [SessionSummary["engine"], string];
  return {
    engine, sessionId, title, cwd: ATLAS, projectRoot: ATLAS, status: "idle", cloud: false, origin: "app",
    firstSeen: "2026-10-01T00:00:00.000Z", lastActivity: "2026-10-01T00:00:00.000Z", archived: false,
    kind: "code", ...over,
  };
}

const SESSIONS = [
  session("claude#5f0c1d2e-8a4b-4c6d-9e7f-1a2b3c4d5e6f", "Session cache invalidation", { status: "working" }),
  session("codex#019a3c5e-7d21-7c30-9f4e-2b6d8a1c4e90", "Fix the flaky login test"),
  session("codex#019a3d70-1b42-7e55-8c11-5f3a9d2e7b04", "Regression test for retry"),
  session("claude#c8e1f2a3-4b5c-4d6e-8f70-91a2b3c4d5e6", "Draft release notes", { kind: "chat", projectRoot: undefined }),
  session("claude#waiting", "Approve the migration", { status: "waiting" }),
];

function controller(scope: ThreadScope = ALL_THREAD_PROJECTS, sessions = SESSIONS, query = ""): ThreadInboxController {
  return {
    // A minute after the sessions appeared: "claude#waiting" has no record yet
    // and is still listed, as a brand-new session is.
    model: buildThreadInbox({ sessions, view: fixture.view as ThreadsView, scope, query,
      selectedKey: "codex#019a3c5e-7d21-7c30-9f4e-2b6d8a1c4e90", now: Date.parse("2026-10-01T00:01:00.000Z") }),
    setScope: noop, send: () => Promise.resolve(true), unarchive: noop, shortcuts: false,
  };
}

// The fixture's clock: 6 Oct 2026, 17:00 UTC, while the pinned thread's run is open.
const NOW = (fixture.view as ThreadsView).now;

function inboxMarkup(over: Partial<ThreadInboxProps> = {}): string {
  return renderToStaticMarkup(createElement(ThreadInbox, {
    inbox: controller(), onSelect: noop, onArchive: noop, diffStats: {}, peeks: {},
    emptyText: "No threads yet.", emptyHint: null, filterQuery: "", now: () => NOW, ...over,
  }));
}

/** One row's markup, from its <li> to the next. */
function rowMarkup(html: string, key: string): string {
  const start = html.indexOf(`data-thread-key="${key}"`);
  expect(start).toBeGreaterThan(-1);
  const end = html.indexOf("</li>", start);
  return html.slice(start, end);
}

function sidebarMarkup(over: Partial<SidebarProps> = {}): string {
  return renderToStaticMarkup(createElement(Sidebar, {
    groups: [{ projectPath: ATLAS, projectName: "atlas", rows: [], counts: { total: 1, waiting: 0, working: 0 } }],
    projectCount: 1, rateLimits: {}, onSelect: noop, onNew: noop, onNewInProject: noop, onArchive: noop,
    hidden: { external: 0, trivial: 0 }, hideable: 0, onToggleFilters: noop, onQuery: noop, query: "",
    sectionLabel: "Threads", newLabel: "New thread", zeroStateText: "No threads yet.", ...over,
  }));
}

/* Review B3 (2026-10-14): an Away loop's thread is marked with Away's own
   coffee glyph before its title, in the dim tier; no colour of its own. */
describe("an Away loop's row", () => {
  it("carries Away's coffee before the title and says Away loop", () => {
    const away = fixtureV45.view as ThreadsView;
    const key = away.threads[0]!.key;
    const inbox: ThreadInboxController = {
      model: buildThreadInbox({ sessions: [session(key, "Away · Fix the flaky login test")], view: away,
        scope: ALL_THREAD_PROJECTS, query: "", selectedKey: null, now: away.now }),
      setScope: noop, send: () => Promise.resolve(true), unarchive: noop, shortcuts: false,
    };
    const row = rowMarkup(inboxMarkup({ inbox, now: () => away.now }), key);
    expect(row).toContain('<span class="session-title thread-inbox-title" title="Away · Fix the flaky login test">'
      + '<span class="codicon codicon-sm codicon-coffee thread-inbox-away" aria-hidden="true" title="Away loop"></span>'
      + "Away · Fix the flaky login test</span>");
    expect(row).toMatch(/aria-label="Away · Fix the flaky login test, Away loop, /u);
    expect(inboxMarkup()).not.toContain("thread-inbox-away\"");
    expect(source).toContain(".thread-inbox-away { margin-inline-end: 4px; color: var(--fg-dim);");
  });
});

describe("a row's short title (owner, 2026-11-15)", () => {
  it("prints a title that is still its prompt short, on one line, with the whole title in the tooltip", () => {
    const key = "claude#long-prompt";
    const prompt = "burst write plan.md Draft the release checklist for atlas and then check every link in it";
    const html = inboxMarkup({ inbox: controller(ALL_THREAD_PROJECTS, [session(key, prompt)]) });
    const row = rowMarkup(html, key);
    expect(row).toContain(`<span class="session-title thread-inbox-title" title="${prompt}">`
      + "burst write plan.md Draft the release checklist for atlas…</span>");
  });
});

describe("the Threads inbox", () => {
  it("draws Pinned and Active open and Snoozed and Settled folded, in order", () => {
    const html = inboxMarkup();
    const sections = [...html.matchAll(/<section[^>]*data-thread-section="(\w+)"/gu)].map(match => match[1]);
    expect(sections).toEqual(["pinned", "active", "snoozed", "settled"]);
    expect(html).toContain('aria-label="Pinned, 1 thread"');
    expect(html).toContain('aria-label="Active, 2 threads"');
    expect(html).toMatch(/aria-expanded="false"[^>]*aria-label="Snoozed, 1 thread"/u);
    expect(html).toMatch(/aria-expanded="false"[^>]*aria-label="Settled, 1 thread"/u);
    // Folded sections draw no rows.
    expect(html).not.toContain("Regression test for retry");
    expect(html).not.toContain("Draft release notes");
    const rows = [...html.matchAll(/<li class="session-item thread-inbox-item[^"]*" data-thread-key="([^"]+)"/gu)]
      .map(match => match[1]);
    // The thread that needs you floats to the top of Active (owner,
    // 2026-10-07); Rust keeps it after the keyed row, and the menu says so.
    expect(rows).toEqual(["claude#5f0c1d2e-8a4b-4c6d-9e7f-1a2b3c4d5e6f",
      "claude#waiting", "codex#019a3c5e-7d21-7c30-9f4e-2b6d8a1c4e90"]);
  });

  it("draws Pinned and Active as two lines: title and one slot, then the hue square, project · branch · PR, and the provider", () => {
    const html = inboxMarkup();
    const active = rowMarkup(html, "codex#019a3c5e-7d21-7c30-9f4e-2b6d8a1c4e90");
    // Line one: the title, then the one slot. An unread failed run says
    // Failed, as an outline pill of words alone (owner, 2026-10-23).
    expect(active).toContain('<span class="session-title thread-inbox-title" title="Fix the flaky login test">Fix the flaky login test</span>'
      + '<span class="thread-inbox-slot is-failed" data-thread-mark="failed" title="Failed"><span class="thread-inbox-pill is-failed">Failed</span></span>');
    // Line two: the project's 8px hue square where the letter tile was, then
    // project · branch · the pull request, its glyph and number in its state's colour.
    expect(active).toContain(`<span class="thread-inbox-tile ${ATLAS_HUE}" aria-hidden="true">`);
    expect(active).toMatch(new RegExp('<span class="thread-inbox-meta">'
      + '<span class="thread-inbox-words"><span class="thread-inbox-project" title="/Users/owner/Developer/Projects/atlas">atlas</span>'
      + '<span class="thread-inbox-sep" aria-hidden="true">·</span>'
      + '<span class="thread-inbox-branch" title="iface/fix-the-flaky-login-test-3f9a2c">iface/fix-the-flaky-login-test-3f9a2c</span>'
      + '<span class="thread-inbox-sep" aria-hidden="true">·</span>'
      + '<span class="thread-inbox-pr is-open" title="Pull request #482 · open"><svg class="thread-glyph" width="12" height="12"[^>]*>[\\s\\S]*?</svg>#482</span>'
      + "</span></span>", "u"));
    // Then at the right of line two, as before: the provider's own mark.
    expect(active).toMatch(/<\/span><\/span><span class="thread-inbox-trail"><svg class="provider-logo provider-logo-codex thread-inbox-provider"[^>]*width="12" height="12"/u);
    // Unread is the strongest title; the selection is carried; the row itself wears no hue.
    expect(active).toContain('class="session-row thread-inbox-row is-unread is-selected" aria-label="Fix the flaky login test, Failed, atlas, branch iface/fix-the-flaky-login-test-3f9a2c, pull request #482 open" aria-current="true"');
    // Working: no pill (owner, 2026-10-07). The Pair at 13px where the Wave
    // was (owner, 2026-10-23), then the run's elapsed time (open five minutes
    // before the view's clock).
    const working = rowMarkup(html, "claude#5f0c1d2e-8a4b-4c6d-9e7f-1a2b3c4d5e6f");
    expect(working).toMatch(/<span class="thread-inbox-slot is-working" data-thread-mark="working" title="Working"><svg class="thinking-mark" width="13" height="13"[^>]*><circle class="thinking-mark-a"[^>]*><\/circle><circle class="thinking-mark-b"[^>]*><\/circle><\/svg><span class="thread-inbox-clock">5:00<\/span><\/span>/u);
    expect(working).not.toContain("thread-inbox-pill");
    expect(working).not.toContain("working-wave");
    expect(source).toContain("<ThinkingMark size={13} />");
    expect(source).not.toContain("WorkingIndicator");
    // A measured 0/0 draws no diff, and a local thread with no branch draws
    // none; the provider's mark is still there.
    expect(working).not.toContain("thread-inbox-diff");
    expect(working).not.toContain("thread-inbox-branch");
    expect(working).toMatch(/<span class="thread-inbox-trail"><svg class="provider-logo provider-logo-claude thread-inbox-provider"/u);
    // Needs you: the one filled pill, words alone.
    expect(rowMarkup(html, "claude#waiting")).toContain('class="thread-inbox-slot is-needs-you" data-thread-mark="needs-you" title="Needs attention"><span class="thread-inbox-pill is-needs-you">Needs you</span></span>');
    // No letter tile, no provider pip, and no ⋯: the menu is the row's context menu.
    expect(html).not.toContain("project-tile");
    expect(html).not.toContain("thread-inbox-pip");
    expect(html).not.toContain("session-menu");
    expect(html).toContain('draggable="true"');
  });

  it("puts a thread's diff after its branch on line two, where the pull request would be", () => {
    // Without its pull request, the active thread's two measured turns (+41 −12, +18 −0) read as one total.
    const view = fixture.view as ThreadsView;
    const counted = { ...view, threads: view.threads.map(({ pullRequest: _, ...thread }) => thread) };
    const html = renderToStaticMarkup(createElement(ThreadInbox, {
      inbox: { ...controller(), model: buildThreadInbox({ sessions: SESSIONS, view: counted, scope: ALL_THREAD_PROJECTS,
        query: "", selectedKey: null, now: Date.parse("2026-10-01T00:01:00.000Z") }) },
      onSelect: noop, onArchive: noop, diffStats: {}, peeks: {}, emptyText: "", emptyHint: null, filterQuery: "", now: () => NOW,
    }));
    expect(rowMarkup(html, "codex#019a3c5e-7d21-7c30-9f4e-2b6d8a1c4e90")).toContain(
      '<span class="thread-inbox-branch" title="iface/fix-the-flaky-login-test-3f9a2c">iface/fix-the-flaky-login-test-3f9a2c</span>'
      + '<span class="thread-inbox-sep" aria-hidden="true">·</span><span class="thread-inbox-diff">'
      + '<span class="thread-inbox-added">+59</span><span class="thread-inbox-removed">−12</span></span></span></span>'
      + '<span class="thread-inbox-trail"><svg class="provider-logo provider-logo-codex thread-inbox-provider"');
    expect(source).toContain(".thread-inbox-diff { display: inline-flex; flex: none; gap: 4px; font-variant-numeric: tabular-nums; }");
    // The branch gives way before the project does; the totals never do.
    expect(source).toContain(".thread-inbox-project { flex: 0 0 auto; max-width: 60%;");
    expect(source).toContain(".thread-inbox-branch { flex: 0 1 auto; min-width: 0; overflow: hidden; text-overflow: ellipsis; }");
  });

  it("keeps Back a muted word and the age plain text, with the unread dot in the done blue", () => {
    // With its run completed rather than failed, the unread thread says its age after the dot.
    const sessions = SESSIONS.map(item => item.title === "Session cache invalidation" ? { ...item, status: "idle" as const } : item);
    const view = fixture.view as ThreadsView;
    const done = { ...view, threads: view.threads.map(thread => thread.lastRun?.outcome === "failed"
      ? { ...thread, lastRun: { ...thread.lastRun, outcome: "completed" as const } } : thread) };
    const html = renderToStaticMarkup(createElement(ThreadInbox, {
      inbox: { ...controller(), model: buildThreadInbox({ sessions, view: done, scope: ALL_THREAD_PROJECTS, query: "",
        selectedKey: null, now: Date.parse("2026-10-01T00:01:00.000Z") }) },
      onSelect: noop, onArchive: noop, diffStats: {}, peeks: {}, emptyText: "", emptyHint: null, filterQuery: "", now: () => NOW,
    }));
    expect(rowMarkup(html, "codex#019a3c5e-7d21-7c30-9f4e-2b6d8a1c4e90"))
      .toMatch(/<span class="thread-inbox-slot is-age" data-thread-mark="unread" title="Done"><span class="thread-inbox-unread"><\/span>\d+[mhdw]<\/span>/u);
    // The done blue, 7px, unlit: the project's hue no longer reaches it (owner, 2026-10-23).
    expect(source).toContain(".thread-inbox-unread { flex: none; width: 7px; height: 7px; border-radius: 50%; background: var(--status-done); }");
    expect(inboxCss).not.toMatch(/\.thread-inbox-unread \{[^}]*(?:project-h|box-shadow)/u);
    // Back keeps its alarm clock and muted word, outside any pill.
    expect(source).toMatch(/<span className="thread-inbox-slot is-woke"[^>]*>\s*<InboxGlyph name="alarm" size=\{13\} \/>\{slot\.word\}\s*<\/span>/u);
    expect(source).toContain(".thread-inbox-slot.is-woke { color: var(--fg-muted); }");
  });

  it("swaps the slot for Snooze and Settle on hover, Wake on a snoozed row, Un-settle on a settled one", () => {
    // A search opens every section, so the shelves' rows draw too.
    const html = inboxMarkup({ inbox: controller(ALL_THREAD_PROJECTS, SESSIONS, "e") });
    const active = rowMarkup(html, "codex#019a3c5e-7d21-7c30-9f4e-2b6d8a1c4e90");
    expect(active).toMatch(/<span class="thread-inbox-actions"><button type="button" class="thread-inbox-action" data-thread-action="snooze" data-thread-menu-trigger="" aria-label="Snooze Fix the flaky login test" title="Snooze" aria-haspopup="menu" aria-expanded="false">[\s\S]*?<button type="button" class="thread-inbox-action" data-thread-action="settle" aria-label="Settle Fix the flaky login test" aria-keyshortcuts="Meta\+Shift\+S" title="Settle \(⌘⇧S\)">/u);
    expect(rowMarkup(html, "codex#019a3d70-1b42-7e55-8c11-5f3a9d2e7b04"))
      .toContain('data-thread-action="unsnooze" aria-label="Wake Regression test for retry" title="Wake now"');
    expect(rowMarkup(html, "claude#c8e1f2a3-4b5c-4d6e-8f70-91a2b3c4d5e6"))
      .toContain('data-thread-action="unsettle" aria-label="Un-settle Draft release notes"');
    // The swap is in place: the slot is hidden, not removed, under the buttons.
    expect(source).toContain(".thread-inbox-item:hover .thread-inbox-slot,");
    expect(source).toContain(".thread-inbox-item:has(:focus-visible) .thread-inbox-actions,");
    expect(source).toMatch(/\.thread-inbox-slot \{[^}]*min-width: 40px;/u);
    expect(source).toContain("{ visibility: hidden; }");
  });

  it("captions Pinned and Active in sentence case with the count at the right, the whole caption the fold", () => {
    // Owner, 2026-10-23 (the approved canvas): "Pinned … 2", not the
    // uppercase micro-label; the chevron shows on hover or focus, and stays
    // while the section is folded.
    const html = inboxMarkup();
    expect(html).toMatch(/<h3 class="thread-inbox-heading"><button type="button" id="[^"]+" class="thread-inbox-fold" aria-expanded="true" aria-controls="[^"]+" aria-label="Active, 2 threads"><span class="thread-inbox-label">Active<\/span><svg class="thread-glyph thread-inbox-chevron" width="12" height="12"[^>]*>[\s\S]*?<\/svg><span class="thread-inbox-fold-end"><span class="thread-inbox-count" aria-hidden="true">2<\/span><\/span><\/button><\/h3>/u);
    expect(html).not.toContain("sidebar-list-heading");
    const captionRules = [...inboxCss.replace(/\/\*[\s\S]*?\*\//gu, "").matchAll(/([^{}]+)\{([^{}]*)\}/gu)]
      .filter(([, selector]) => /thread-inbox-(?:heading|fold|label|count)/u.test(selector));
    expect(captionRules.length).toBeGreaterThan(4);
    // Owner, 2026-11-16 (the TN Code look): a caption is a small mono label
    // in capitals, tracked by the one caps amount; a shelf's fold turns both
    // off again and reads as plain words.
    expect(captionRules.filter(([, , body]) => /uppercase|--track-caps/u.test(body)).map(([, selector]) => selector.trim()))
      .toEqual([".thread-inbox-heading"]);
    expect(source).toMatch(/\.thread-inbox-heading \{[^}]*margin: 12px 0 2px; font-family: var\(--tn-font-mono\); font-size: var\(--text-meta\);[^}]*letter-spacing: var\(--track-caps\); text-transform: uppercase;/u);
    expect(source).toMatch(/\.thread-inbox-shelf-head \{[^}]*letter-spacing: 0; text-transform: none;/u);
    expect(source).toMatch(/\.thread-inbox-fold \{[^}]*height: 22px; padding: 0 12px;[^}]*color: var\(--tn-label-3\);/u);
    expect(source).toContain(".thread-inbox-fold-end { display: inline-flex; flex: none; align-items: center; gap: 6px; margin-left: auto; }");
    expect(source).toContain(".thread-inbox-chevron { opacity: 0;");
    expect(source).toContain(".thread-inbox-fold:hover .thread-inbox-chevron, .thread-inbox-fold:focus-visible .thread-inbox-chevron,\n"
      + ".thread-inbox-chevron.is-folded { opacity: 1; }");
    expect(source).toContain(".thread-inbox-chevron.is-folded { transform: rotate(-90deg); }");
  });

  it("shelves Snoozed and Settled at the foot of the list in slim rows", () => {
    const html = inboxMarkup({ inbox: controller(ALL_THREAD_PROJECTS, SESSIONS, "e") });
    // Pinned and Active keep their captions; the shelves follow them.
    expect(html).toMatch(/<h3 class="thread-inbox-heading">[\s\S]*<div class="thread-inbox-shelves"><section class="thread-inbox-section thread-inbox-shelf-section" data-thread-section="snoozed"/u);
    // A shelf is the canvas's slim fold: "Snoozed · 1", the chevron at the right, turned down while open.
    expect(html).toMatch(/<h3 class="thread-inbox-shelf-head"><button type="button" id="[^"]+" class="thread-inbox-fold thread-inbox-shelf" aria-expanded="true"[^>]*aria-label="Snoozed, 1 thread"><span class="thread-inbox-label">Snoozed<\/span><span class="thread-inbox-count" aria-hidden="true">· 1<\/span><span class="thread-inbox-fold-end"><svg class="thread-glyph thread-inbox-chevron is-open" width="14" height="14"[^>]*>[\s\S]*?<\/svg><\/span><\/button><\/h3>/u);
    // Slim: the project's square at half strength, the title, then the wake time or the age.
    const snoozed = rowMarkup(html, "codex#019a3d70-1b42-7e55-8c11-5f3a9d2e7b04");
    expect(snoozed).toMatch(new RegExp(`class="session-row thread-inbox-row is-read is-slim"[^>]*><span class="thread-inbox-hue ${ATLAS_HUE}" aria-hidden="true"></span><span class="session-title thread-inbox-title" title="Regression test for retry · Wakes [^"]+">Regression test for retry</span><span class="thread-inbox-slot is-wake" data-thread-mark="none" title="Wakes [^"]+">[^<]+</span></button>`, "u"));
    // No project: the hollow ring.
    const settled = rowMarkup(html, "claude#c8e1f2a3-4b5c-4d6e-8f70-91a2b3c4d5e6");
    expect(settled).toMatch(/<span class="thread-inbox-hue project-hue-none" aria-hidden="true"><\/span><span class="session-title thread-inbox-title"[^>]*>Draft release notes<\/span><span class="thread-inbox-slot is-age" data-thread-mark="none">\d+[mhdw]<\/span>/u);
    // The shelves sit at the bottom of the scroll area under one hairline 18px
    // clear of the rows above, and the geometry is the canvas's: 54px rows,
    // 8px above and below their two lines, 2px apart.
    expect(source).toMatch(/\.thread-inbox-shelves \{[^}]*margin-top: auto; padding-top: 27px;/u);
    expect(source).toMatch(/\.thread-inbox-shelves::before \{[^}]*top: 18px;[^}]*height: var\(--border-w\); background: var\(--border\);/u);
    expect(source).toMatch(/\.sidebar \.session-row\.thread-inbox-row \{[^}]*grid-template-rows: 18px 14px;[^}]*row-gap: 2px;[^}]*height: 52px; padding: 0 10px 0 46px;[^}]*border-radius: calc\(var\(--radius-container\) \+ 1px\);/u);
    expect(source).toMatch(/\.sidebar \.session-row\.thread-inbox-row\.is-slim \{[^}]*height: 32px;/u);
    expect(source).toMatch(/\.thread-inbox-fold\.thread-inbox-shelf \{[^}]*height: 30px;/u);
    expect(source).toContain(".thread-inbox .session-items > li + li { margin-top: 2px; }");
    expect(source).toContain(".thread-inbox-row.is-slim .thread-inbox-hue { opacity: 0.5; }");
    expect(source).not.toContain("thread-inbox-rule");
    // Nothing in the inbox turns: the Pair is its one working mark.
    expect(source).not.toContain("thread-inbox-spin");
  });

  it("keeps the project's hue to its square: none on the row, its selection, its unread dot or the header", () => {
    // Owner, 2026-11-16 (the TN Code look): a full row's hue is its 28px tile,
    // the thread's initials on it; a slim row keeps the 8px square of
    // 2026-10-23 (a hollow ring for No project). Nothing else takes the hue.
    expect(source).toContain('<span className={`thread-inbox-hue ${projectHueClass(root)}`} aria-hidden="true" />');
    expect(source).toContain('<span className={`thread-inbox-tile ${projectHueClass(root)}`} aria-hidden="true">');
    expect(source).toMatch(/\.thread-inbox-tile \{[^}]*width: 28px; height: 28px;[^}]*border-radius: var\(--radius-container\);/u);
    expect(inboxMarkup()).toMatch(/<span class="thread-inbox-tile project-hue-[^"]*" aria-hidden="true">[A-Z0-9]{1,2}<\/span>/u);
    expect(source).toContain(".thread-inbox-hue {\n  flex: none; width: 8px; height: 8px; border-radius: calc(var(--radius-control) - 1.5px); background: var(--project-h);\n}");
    expect(source).toContain(".thread-inbox-hue.project-hue-none { border-radius: 50%; background: transparent; box-shadow: inset 0 0 0 1.5px var(--fg-faint); }");
    // The square, and the tile's gloss from a lighter mix down to the hue.
    expect(inboxCss.match(/--project-h/gu)).toHaveLength(3);
    expect(inboxCss.match(/gradient/gu)).toHaveLength(2);
    expect(source).not.toContain("rowClasses.push(projectHueClass");
    expect(inboxMarkup()).not.toMatch(/class="session-row thread-inbox-row[^"]*project-hue-/u);
    // The square's hue classes load with the inbox itself.
    expect(source).toContain('import "./ProjectHue.css";');
    // Selection is the selection colour's wash and hover a soft fill, both
    // worked out from the theme's own tokens (theme.css's --tn-*).
    expect(source).toContain(".sidebar .thread-inbox .session-row.thread-inbox-row.is-selected { background: var(--tn-row-selected); }");
    expect(source).toContain(".sidebar .thread-inbox .session-row.thread-inbox-row:hover:not(.is-selected) { background: var(--tn-fill); }");
    // The provider's mark ends line two in its brand colour; the diff takes the diff tokens.
    expect(source).toContain('<ProviderLogo engine={engine} label={engine} size={12} className="thread-inbox-provider" />');
    expect(source).toContain(".thread-inbox-added { color: var(--diff-added); }");
    expect(source).toContain(".thread-inbox-removed { color: var(--diff-removed); }");
    expect(source).not.toMatch(/#[0-9a-f]{6}\b/iu);
  });

  it("draws its glyphs from the Threads icon set, keeping Away's coffee", () => {
    expect(source).toContain('import { ThreadGlyph } from "./ThreadGlyph";');
    expect(source.match(/codicon-[a-z-]+/gu)?.filter(name => name !== "codicon-sm")).toEqual(["codicon-coffee"]);
    // The hover actions at 15px: the alarm clock (Snooze) and Settle's check
    // on an open row, the sun (Wake) on a snoozed one, Undo's arrow (Un-settle) on a settled one.
    const html = inboxMarkup({ inbox: controller(ALL_THREAD_PROJECTS, SESSIONS, "e") });
    const active = rowMarkup(html, "codex#019a3c5e-7d21-7c30-9f4e-2b6d8a1c4e90");
    expect(active).toMatch(/data-thread-action="snooze"[^>]*><svg class="thread-glyph" width="15" height="15"[^>]*data-inbox-glyph="alarm"/u);
    expect(active).toMatch(/data-thread-action="settle"[^>]*><svg class="thread-glyph" width="15" height="15"[^>]*><path d="M3\.5 8\.5l2\.75 2\.75L12\.5 5"/u);
    expect(rowMarkup(html, "codex#019a3d70-1b42-7e55-8c11-5f3a9d2e7b04"))
      .toMatch(/data-thread-action="unsnooze"[^>]*><svg class="thread-glyph" width="15" height="15"[^>]*data-inbox-glyph="wake"/u);
    expect(rowMarkup(html, "claude#c8e1f2a3-4b5c-4d6e-8f70-91a2b3c4d5e6"))
      .toMatch(/data-thread-action="unsettle"[^>]*><svg class="thread-glyph" width="15" height="15"[^>]*><path d="M5\.75 3\.75 2\.5 7l3\.25 3\.25"/u);
    // The two the set has no word for, drawn in its grammar: 16px grid, 1.5 stroke, round ends.
    expect(source).toMatch(/<svg className="thread-glyph" width=\{size\} height=\{size\} viewBox="0 0 16 16" fill="none" stroke="currentColor"\s*strokeWidth=\{1\.5\} strokeLinecap="round" strokeLinejoin="round"/u);
  });

  it("says on line two what a live thread is doing, asking or failed on, in the project line's place", () => {
    // Owner, 2026-10-07, "Inbox under load": the step a working thread is on
    // (project-relative, short), what a thread that needs you asks, the first
    // line of a failed thread's newest error. The hue square and the
    // provider's mark stay; the project, branch, pull request and diff come
    // back when it ends. All three read in line two's dim tier (owner,
    // 2026-10-23): the slot's pill or mark already says the status.
    const html = inboxMarkup({ live: {
      "claude#5f0c1d2e-8a4b-4c6d-9e7f-1a2b3c4d5e6f": { running: true,
        step: { tool: "Edit", summary: `${ATLAS}/src/routes/login.ts`, input: { file_path: `${ATLAS}/src/routes/login.ts` } } },
      "claude#waiting": { running: false, ask: { tool: "Bash", summary: "Run the tests", input: { command: "npm test" } } },
      "codex#019a3c5e-7d21-7c30-9f4e-2b6d8a1c4e90": { running: false, error: "Rate limit reached for gpt-5" },
    } });
    const working = rowMarkup(html, "claude#5f0c1d2e-8a4b-4c6d-9e7f-1a2b3c4d5e6f");
    expect(working).toMatch(new RegExp(`<span class="thread-inbox-meta"><span class="thread-inbox-words"><span class="thread-inbox-line is-step" title="Editing login.ts">Editing login.ts</span></span></span><span class="thread-inbox-trail"><svg class="provider-logo`, "u"));
    expect(source).toMatch(/\.thread-inbox-meta \{[^}]*color: var\(--tn-label-3\);/u);
    expect(inboxCss).not.toMatch(/\.thread-inbox-line\.is-/u);
    // The step changes at every tool call, so the row's name leaves it out.
    expect(working).toContain('aria-label="Session cache invalidation, Working, atlas"');
    const waiting = rowMarkup(html, "claude#waiting");
    expect(waiting).toContain('<span class="thread-inbox-line is-ask" title="Run npm test?">Run npm test?</span>');
    expect(waiting).toContain('aria-label="Approve the migration, Needs you, Run npm test?, atlas"');
    const failed = rowMarkup(html, "codex#019a3c5e-7d21-7c30-9f4e-2b6d8a1c4e90");
    expect(failed).toContain('<span class="thread-inbox-line is-error" title="Rate limit reached for gpt-5">Rate limit reached for gpt-5</span>');
    expect(failed).not.toContain('class="thread-inbox-pr ');
    expect(failed).not.toContain("thread-inbox-branch");
    expect(failed).toContain('aria-label="Fix the flaky login test, Failed, Rate limit reached for gpt-5, atlas, branch iface/fix-the-flaky-login-test-3f9a2c, pull request #482 open"');
    // Without a live log a row keeps its project line.
    expect(rowMarkup(inboxMarkup(), "claude#waiting")).not.toContain("thread-inbox-line");
  });

  it("moves with the board's timings, and holds every motion still for reduced motion", () => {
    // The step's words never move. Needs you rings twice as it arrives; the
    // unread dot pops; a live line fades out over the project line.
    expect(source).not.toContain("thread-inbox-shine");
    expect(source).toContain("animation: thread-inbox-ring 1.1s var(--ease-out) 2;");
    expect(source).toContain(".thread-inbox-unread.is-arriving { animation: thread-inbox-pop 240ms cubic-bezier(.34, 1.56, .64, 1); }");
    expect(source).toContain("@keyframes thread-inbox-pop { 0% { transform: scale(.3); } 60% { transform: scale(1.18); } 100% { transform: scale(1); } }");
    expect(source).toContain("animation: thread-inbox-fade-out var(--motion-base) linear forwards;");
    expect(source).toMatch(/@media \(prefers-reduced-motion: reduce\) \{\s*\.thread-inbox-pill\.is-needs-you, \.thread-inbox-unread\.is-arriving, \.thread-inbox-leaving,\s*\.thread-inbox-words\.is-entering > \* \{ animation: none; \}/u);
    // The Pair moves on the app's loop budget (ThinkingMark.css) and rests with the sidebar hidden.
    expect(source).toContain(".app-grid.no-sidebar > .sidebar .thinking-mark circle { animation-play-state: paused; }");
    // Nothing arrives on a row's first paint, and nothing fades under reduced motion.
    expect(source).toContain("if (before === null) return;");
    expect(source).toContain('if (before.line.kind !== "meta" && line.kind === "meta" && motionAllowed()) setLeaving(before.line);');
    // The float: a FLIP translate, 240ms on the board's curve, only when the
    // drawn order moved and Rust's did not; reduced motion lands at once.
    expect(source).toContain("const THREAD_FLOAT_MS = 240;");
    expect(source).toContain('const THREAD_FLOAT_EASE = "cubic-bezier(.16, 1, .3, 1)";');
    expect(source).toContain("if (previous.drawnOrder === this.props.drawnOrder || previous.rustOrder !== this.props.rustOrder) return null;");
    expect(source).toMatch(/document\.hidden \|\| !motionAllowed\(\)/u);
    expect(source).toContain("node.animate([{ transform: `translateY(${dy}px)` }, { transform: \"none\" }],");
  });

  it("keeps the pull request's words at 4.5:1 in every palette, on any row state", () => {
    // The pull request (owner, 2026-10-23): its glyph and number in its
    // state's colour on line two, with no wash under them, so they sit on
    // whatever the row has: the sidebar's sheet, the hover fill or the
    // selection. The ink leans 15% toward --fg-strong; measured for every
    // tone it takes on all three.
    expect(source).toContain("color: color-mix(in srgb, var(--thread-inbox-pr) 85%, var(--fg-strong)); font-variant-numeric: tabular-nums;");
    expect(source).toContain(".thread-inbox-pr.is-open { --thread-inbox-pr: var(--diff-added); }");
    expect(source).toContain(".thread-inbox-pr.is-merged { --thread-inbox-pr: var(--review-accent); }");
    expect(source).toMatch(/\.thread-inbox-pr \{\s*--thread-inbox-pr: var\(--fg-dim\);/u);
    expect(themes.length).toBeGreaterThan(8);
    const failures: string[] = [];
    for (const { id, body } of themes) {
      const sheet = resolve(body, "material-chrome").slice(0, 7);
      const strong = resolve(body, "fg-strong").slice(0, 7);
      for (const tone of ["diff-added", "review-accent", "fg-dim"]) {
        const ink = mix(resolve(body, tone).slice(0, 7), strong, 0.85);
        const worst = Math.min(...rowFills(body, sheet).map(row => contrastRatio(ink, row)));
        if (worst < 4.5) failures.push(`${id} ${tone} ${worst.toFixed(2)}`);
      }
    }
    expect(failures).toEqual([]);
  });

  it("keeps Needs you's dark ink, Failed's outline words and line two's dim words at 4.5:1 in every palette", () => {
    // Needs you (owner, 2026-10-07): the amber fill, lifted to oklch L 0.8
    // where a paper palette's amber is a deep text colour, under an ink of the
    // same hue at L 0.27. Failed's outline (in the failed colour, owner,
    // 2026-10-23) and line two's words have no wash of their own, so they are
    // measured on every row state: the sheet, the hover fill and the selection.
    expect(source).toContain("--thread-inbox-amber: oklch(from var(--thread-inbox-waiting) max(l, 0.8) c h);");
    expect(source).toContain("color: oklch(from var(--thread-inbox-waiting) 0.27 calc(c * 0.7) h);");
    expect(source).toContain("color: color-mix(in srgb, var(--status-failed) 80%, var(--fg-strong)); background: transparent;");
    expect(source).toContain("border: var(--border-w) solid var(--status-failed); font-weight: var(--wght-medium);");
    const failures: string[] = [];
    for (const { id, body } of themes) {
      const sheet = resolve(body, "material-chrome").slice(0, 7);
      const strong = resolve(body, "fg-strong").slice(0, 7);
      const amber = resolve(body, "status-waiting").slice(0, 7);
      const [l, c, h] = toOklch(amber);
      const fill = fromOklch(Math.max(l, 0.8), c, h);
      const ink = fromOklch(0.27, c * 0.7, h);
      if (contrastRatio(ink, fill) < 4.5) failures.push(`${id} needs-you ${contrastRatio(ink, fill).toFixed(2)}`);
      const inks = {
        failed: mix(resolve(body, "status-failed").slice(0, 7), strong, 0.8),
        dim: resolve(body, "fg-dim").slice(0, 7),
      };
      for (const [name, word] of Object.entries(inks)) {
        const worst = Math.min(...rowFills(body, sheet).map(row => contrastRatio(word, row)));
        if (worst < 4.5) failures.push(`${id} ${name} ${worst.toFixed(2)}`);
      }
    }
    expect(failures).toEqual([]);
  });

  it("shows a folded section's highest mark on its header", () => {
    const waitingSettled = SESSIONS.map(item => item.sessionId.startsWith("c8e1") ? { ...item, status: "waiting" as const } : item);
    const html = inboxMarkup({ inbox: controller(ALL_THREAD_PROJECTS, waitingSettled) });
    expect(html).toContain('aria-label="Settled, 1 thread, needs attention"');
    // Needs you is a dot in the sidebar's amber, before the shelf's chevron.
    expect(html).toMatch(/<span class="thread-inbox-label">Settled<\/span><span class="thread-inbox-count" aria-hidden="true">· 1<\/span><span class="thread-inbox-fold-end"><span aria-hidden="true"><span class="thread-inbox-mark session-wait" role="img" aria-label="Needs attention" title="Needs attention" data-thread-mark="needs-you"><span class="thread-inbox-mark-dot"><\/span><\/span><\/span><svg class="thread-glyph thread-inbox-chevron"/u);
    // Working is the Pair and Failed the set's error glyph, so the three differ by shape as well as colour.
    expect(source).toContain('case "working": return { glyph: <ThinkingMark size={13} />, tone: null };');
    expect(source).toContain('case "failed": return { glyph: <ThreadGlyph name="error" size={14} />, tone: "session-fail" };');
  });

  it("says when the scope, not the search, emptied the inbox", () => {
    const scoped = inboxMarkup({ inbox: controller(threadScopeFor("/elsewhere/web"), SESSIONS) });
    expect(scoped).toContain("No threads in web.");
    expect(scoped).toContain("Show all projects");
    const none = inboxMarkup({ inbox: controller(ALL_THREAD_PROJECTS, []), emptyHint: "Start one with +." });
    expect(none).toContain("No threads yet.");
    expect(none).toContain("Start one with +.");
    const searched = inboxMarkup({ inbox: controller(ALL_THREAD_PROJECTS, []), filterQuery: "zebra",
      onSearchTranscripts: noop });
    expect(searched).toContain("Search transcripts for “zebra”");
    expect(inboxMarkup({ inbox: controller(NO_THREAD_PROJECT) })).toContain('data-thread-section="settled"');
  });

  it("puts a submenu beside its item, flipping left at the window's edge", () => {
    expect(positionThreadSubmenu({ top: 100, left: 200, right: 400 }, { width: 160, height: 120 },
      { width: 1200, height: 800 })).toEqual({ top: 96, left: 404 });
    expect(positionThreadSubmenu({ top: 100, left: 200, right: 400 }, { width: 160, height: 120 },
      { width: 500, height: 800 })).toEqual({ top: 96, left: 36 });
    expect(positionThreadSubmenu({ top: 780, left: 200, right: 400 }, { width: 160, height: 120 },
      { width: 1200, height: 800 })).toEqual({ top: 672, left: 404 });
  });
});

describe("the inbox's commands and keys", () => {
  it("sends menu, drop and Undo commands through the controller, never directly", () => {
    expect(source).not.toContain("window.iface");
    expect(source).toContain("void inbox.send(commands)");
    expect(source).toContain("act(row, entry.commands, entry.undo)");
    expect(source).toContain("const commands = threadDropCommands(row, drop);");
    expect(source).toContain("act(row, commands, threadDropUndo(commands));");
    expect(source).toContain("else void inbox.send(threadUndoCommands(toast, now()));");
    expect(source).toContain('if (toast.action === "archive") inbox.unarchive(toast.key);');
    expect(THREAD_DRAG_MIME).toBe("application/x-interface-thread");
  });

  it("keeps the row menu on right-click and the menu key, with the real chords hinted", () => {
    expect(source).toContain('actions.toggleMenu(row.key, new DOMRect(event.clientX, event.clientY, 0, 0), event.currentTarget);');
    expect(source).toContain('} else if (event.key === "ContextMenu" || (event.key === "F10" && event.shiftKey)) {');
    expect(source).toContain("const chord = THREAD_MENU_CHORDS[entry.id];");
    expect(source).toContain('{chord !== undefined && <span className="thread-menu-key" aria-hidden="true">{chord.hint}</span>}');
  });

  it("opens the Snooze presets as a menu anchored to the hover button, and gives focus back to it", () => {
    expect(source).toContain("menuAnchor.current = trigger.getBoundingClientRect();");
    expect(source).toContain('setMenu({ key, kind: "snooze", submenu: null });');
    expect(source).toContain('className="row-menu thread-menu thread-snooze-menu is-floating"');
    expect(source).toContain("returnFocus.current = trigger;");
    // Settle, Wake and Un-settle from the hover buttons take the row away: focus takes its place.
    expect(source).toMatch(/quick: \(key, kind\) => \{[\s\S]*?focusInPlaceOf\(key\);\s*if \(kind === "settle"\) act\(row, \[\{ kind: "settle", key \}\], "settle"\);/u);
  });

  it("takes ⌘Z only while the sidebar has focus and nothing is being typed", () => {
    expect(source).toMatch(/if \(toast === null \|\| active === null \|\| active\.closest\("\.sidebar"\) === null\s*\|\| editingText\(active\)\) return;/u);
    expect(source).toContain("if (document.querySelector(FOREGROUND_DIALOG_SELECTOR) !== null) return;");
    expect(source).toContain('if (target !== null && target.closest(".terminal-center") !== null) return;');
    expect(source).toContain("if (!inbox.shortcuts) return;");
  });

  it("offers Undo in a short toast for settle, snooze and archive", () => {
    expect(source).toContain('<div className="thread-inbox-toast" role="status">');
    expect(source).toContain('title="Undo (⌘Z)"');
    expect(source).toContain("export const THREAD_UNDO_MS = 6000;");
    // Undo waits for the archive's answer and never follows a refusal
    // (review, 6 Oct 2026: it was offered before the archive had happened).
    expect(source).toMatch(/const archive = \(row: ThreadInboxRow\): void => \{\s*const undo = threadUndo\("archive", row, model\);[\s\S]*?void Promise\.resolve\(onArchive\(row\.key\)\)\.then\(archived => \{\s*if \(archived === true\) offerUndo\(undo\);/u);
  });

  it("hands focus to the row that takes an archived row's place", () => {
    expect(source).toContain("onClick={() => { closeMenuInPlaceOf(menuRow.key); archive(menuRow); }}");
    expect(source).toContain("const neighbour = threadFocusAfterRemoval(order, key);");
  });

  it("keeps focus in the list when a settle or snooze takes the row away, so ⌘Z still undoes it", () => {
    // Hands-free run, 6 Oct 2026: the settled row's trigger vanished with
    // focus on it, focus fell to the page and ⌘Z found no focused sidebar.
    expect(source).toContain("if (threadActionLeavesSection(undo, row.section)) closeMenuInPlaceOf(row.key);");
    expect(source).toContain("closeMenuAfter(row, entry.undo);\n          act(row, entry.commands, entry.undo);");
    expect(source).toContain("closeMenuAfter(menuRow, item.undo);\n                  act(menuRow, item.commands, item.undo);");
    expect(source).toMatch(/if \(focused === row\.key\) focusInPlaceOf\(row\.key\);\s*act\(row, \[\{ kind: "settle", key: row\.key \}\], "settle"\);/u);
  });
});

describe("the Sidebar on the Threads surface", () => {
  it("draws the inbox and the scope picker only when given an inbox", () => {
    const threads = sidebarMarkup({ threadInbox: controller() });
    expect(threads).toContain('<span class="sidebar-section-label">Threads</span>');
    expect(threads).toMatch(/<span class="sidebar-section-label">Threads<\/span><span class="app-select app-select-compact thread-scope-picker/u);
    expect(threads).toContain("All projects");
    expect(threads).toContain('class="session-list session-list-flat thread-inbox"');
    // No project folds, no Collapse all, no lifted Needs attention section.
    expect(threads).not.toContain('class="group-header');
    expect(threads).not.toContain("Collapse all");
    expect(threads).not.toContain("Needs attention</div>");
    const code = sidebarMarkup({ sectionLabel: "Workspaces", codeWorkspaces: true });
    expect(code).not.toContain("thread-");
    expect(code).toContain('class="group-header');
  });

  it("draws no header band: the Threads sidebar sits on its plain ground", () => {
    // Owner, 2026-10-23: the band of the focused thread's hue and its dot
    // grid went with the rest of the hued glow, whichever thread is focused.
    const threads = sidebarMarkup({ threadInbox: controller() });
    expect(threads).not.toContain("thread-glow");
    expect(threads).toContain('<aside class="pane sidebar is-floating"');
    const code = sidebarMarkup({ sectionLabel: "Workspaces", codeWorkspaces: true });
    expect(code).not.toContain("thread-glow");
    expect(source).not.toContain("THREAD_GLOW_CSS");
    expect(source).not.toContain("ThreadHeaderGlow");
    expect(source).not.toContain("radial-gradient");
    expect(sidebarSource).not.toContain("threadFocusedProjectRoot");
  });

  it("keeps the Code and Agent panes' markup free of the inbox", () => {
    expect(sidebarSource).toContain("{threadInbox !== undefined && <ThreadScopePicker inbox={threadInbox} />}");
    expect(sidebarSource).toContain("const lifting = !loading && attentionRows.length > 0 && threadInbox === undefined;");
    expect(sidebarSource).toContain("threadInbox === undefined && showHeaders && !loading && listedGroups.length > 0");
  });
});
