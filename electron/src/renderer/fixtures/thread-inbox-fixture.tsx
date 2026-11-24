import { flushSync } from "react-dom";
import type { Root } from "react-dom/client";
import { themeKind, type ResolvedTheme } from "../../shared/appearance";
import type { SessionSummary } from "../../shared/engine";
import type { JournalHit } from "../../shared/thread-journal";
import type { ThreadCommand, ThreadView, ThreadsView } from "../../shared/threads";
import { APP_SELECT_CSS } from "../components/AppSelect";
import { Sidebar } from "../components/Sidebar";
import { THREAD_FOLDS_KEY } from "../components/ThreadInbox";
import { contrastRatio } from "../contrast";
import { projectHueClass } from "../project-hue";
import type { ThreadLiveFacts } from "../session-activity";
import { ALL_THREAD_PROJECTS, buildThreadInbox, IN_MESSAGES_LABEL, type ThreadScope } from "../thread-inbox";
import type { ThreadInboxController } from "../use-thread-inbox";
import { assert, frame, noop, until, wait } from "./fixture-support";

const ATLAS = "/fixture/threads/atlas";
// Three more projects, so the rows show the hues side by side.
const DOCS = "/fixture/threads/docs";
const WEBSITE = "/fixture/threads/website";
const API = "/fixture/threads/api";
const NOW = Date.now();
const HOUR = 3_600_000;
// When the one session Rust has not recorded yet appeared. The inbox is read a
// minute later, so it is still on its way; an older unrecorded one is a hidden
// worker and is left out.
const UNKNOWN_SEEN = NOW - 72 * HOUR;

function session(key: string, title: string, over: Partial<SessionSummary> = {}): SessionSummary {
  const [engine, sessionId] = key.split("#") as [SessionSummary["engine"], string];
  return {
    engine, sessionId, title, cwd: ATLAS, projectRoot: ATLAS, status: "idle", cloud: false, origin: "app",
    firstSeen: new Date(NOW - 48 * HOUR).toISOString(), lastActivity: new Date(NOW - HOUR).toISOString(),
    archived: false, kind: "code", ...over,
  };
}

/** A finished turn; without counts it is one Rust could not measure. */
function turn(ordinal: number, added?: number, removed?: number): ThreadView["turns"][number] {
  const base = { ordinal, sentAt: NOW - 5 * HOUR, promptDigest: "0000000000000000", hasStart: true, hasEnd: true,
    state: "done" as const };
  return added === undefined || removed === undefined ? base : { ...base, filesChanged: 1, added, removed };
}

function record(key: string, over: Partial<ThreadView>): ThreadView {
  return { key, section: "active", createdAt: NOW - 24 * HOUR, woke: false, unread: false, autoSettle: true, turns: [], ...over };
}

const SESSIONS: SessionSummary[] = [
  session("claude#pin-release", "Release checklist", { status: "working", cwd: DOCS, projectRoot: DOCS }),
  session("codex#pin-docs", "Docs pass"),
  session("codex#active-done", "Fix the flaky login test", { cwd: WEBSITE, projectRoot: WEBSITE }),
  session("claude#active-waiting", "Approve the migration plan", { status: "waiting" }),
  session("codex#active-woke", "Benchmark the cache", { cwd: API, projectRoot: API }),
  session("claude#active-failed", "Port the importer"),
  session("claude#active-unknown", "Explain the session store", { kind: "chat", projectRoot: undefined,
    cwd: "/fixture/private", firstSeen: new Date(UNKNOWN_SEEN).toISOString() }),
  session("codex#hidden-worker", "A worker Rust leaves out", { firstSeen: new Date(UNKNOWN_SEEN - HOUR).toISOString() }),
  session("codex#snoozed-retry", "Regression test for retry", { cwd: WEBSITE, projectRoot: WEBSITE }),
  session("claude#settled-notes", "Draft release notes", { kind: "chat", projectRoot: undefined, cwd: "/fixture/private" }),
  session("codex#settled-old", "Old spike"),
  session("claude#agent-task", "An agent task", { kind: "agent" }),
];

const VIEW: ThreadsView = {
  version: 1, revision: 7, now: NOW,
  settings: { autoSettleDays: 3, settleOnMerge: true, generatedTitles: false, trackPullRequests: true },
  threads: [
    record("claude#pin-release", { section: "pinned", pinnedAt: NOW - HOUR, pinOrder: "a0",
      lastRun: { startedAt: NOW - 161_000 },
      workspace: { mode: "local", projectRoot: DOCS, branch: "dev", owned: false } }),
    record("codex#pin-docs", { section: "pinned", pinnedAt: NOW - 2 * HOUR, pinOrder: "a1",
      turns: [turn(0, 6, 6)] }),
    record("codex#active-done", { activeOrder: "Zz", unread: true,
      lastRun: { startedAt: NOW - 2 * HOUR, endedAt: NOW - HOUR, outcome: "completed" },
      workspace: { mode: "worktree", projectRoot: WEBSITE, worktreePath: "/fixture/threads/.interface-worktrees/flaky-3f9a2c",
        branch: "iface/flaky-3f9a2c", owned: true },
      pullRequest: { url: "https://github.com/owner/website/pull/12", number: 12, state: "open", checkedAt: NOW - HOUR } }),
    record("claude#active-waiting", { activeOrder: "a0",
      workspace: { mode: "worktree", projectRoot: ATLAS, branch: "iface/migrate-v2", owned: true } }),
    record("codex#active-woke", { createdAt: NOW - 2 * HOUR, woke: true }),
    record("claude#active-failed", { createdAt: NOW - 3 * HOUR, unread: true,
      lastRun: { startedAt: NOW - 4 * HOUR, endedAt: NOW - 3 * HOUR, outcome: "failed" },
      workspace: { mode: "worktree", projectRoot: ATLAS, branch: "iface/importer", owned: true },
      turns: [turn(0, 180, 30), turn(1, 30, 8), turn(2)] }),
    record("codex#snoozed-retry", { section: "snoozed", snoozedUntil: NOW + 20 * HOUR }),
    record("claude#settled-notes", { section: "settled", settledAt: NOW - HOUR, settledReason: "idle",
      workspace: { mode: "none", projectRoot: "", owned: false } }),
    record("codex#settled-old", { section: "settled", settledAt: NOW - 30 * HOUR, settledReason: "manual" }),
  ],
};

// As drawn. The thread that needs you floats to the top of Active (owner,
// 2026-10-07); Rust keeps it second (RUST_ACTIVE), and Move up / Move down and
// drops still mean places in Rust's order.
const RUST_ACTIVE = ["codex#active-done", "claude#active-waiting", "codex#active-woke", "claude#active-failed",
  "claude#active-unknown"];
const ORDER = {
  pinned: ["claude#pin-release", "codex#pin-docs"],
  active: ["claude#active-waiting", "codex#active-done", "codex#active-woke", "claude#active-failed",
    "claude#active-unknown"],
  snoozed: ["codex#snoozed-retry"],
  settled: ["claude#settled-notes", "codex#settled-old"],
};

const MARKS: Record<string, string> = {
  "claude#pin-release": "working", "codex#pin-docs": "none", "codex#active-done": "unread",
  "claude#active-waiting": "needs-you", "codex#active-woke": "woke", "claude#active-failed": "failed",
  "claude#active-unknown": "none", "codex#snoozed-retry": "none", "claude#settled-notes": "none",
  "codex#settled-old": "none",
};

/**
 * The Threads inbox in the real Sidebar: Pinned and Active in two-line rows
 * at the top, Snoozed and Settled shelved at the foot in slim rows, one slot
 * per row, the hover actions, folds, the scope picker, the row menu and its
 * submenus, drag to reorder and across sections, Undo by toast and ⌘Z, and
 * the chords. The Threads look (owner, 2026-10-23): sentence-case captions
 * with the count at the right, the theme's own row fills, the project's hue
 * only as a small square on line two, the Pair for a working run, the
 * provider's mark ending line two, and no header band.
 */
export async function threadInboxFixture(root: Root, theme: ResolvedTheme, chrome: string, width: number) {
  document.documentElement.dataset.theme = theme;
  document.documentElement.dataset.chrome = chrome;
  document.documentElement.style.colorScheme = themeKind(theme);
  // A fresh inbox: the folds are read once, when it mounts.
  flushSync(() => root.render(<div />));
  localStorage.removeItem(THREAD_FOLDS_KEY);
  let selected = "codex#active-done";
  let scope: ThreadScope = ALL_THREAD_PROJECTS;
  // The listing, Rust's view and the live logs' readings: the "under load"
  // checks change them as the host would.
  let sessions = SESSIONS;
  let view = VIEW;
  let live: Record<string, ThreadLiveFacts> = {};
  // The search and the journal's matches for it (protocol 56).
  let query = "";
  let messageHits: readonly JournalHit[] | undefined;
  const sent: ThreadCommand[][] = [];
  const selections: string[] = [];
  const archived: string[] = [];
  const unarchived: string[] = [];
  // Whether the next archive succeeds, and when it answers.
  let archiveAnswer: () => Promise<boolean> = () => Promise.resolve(true);
  const render = (): void => {
    const model = buildThreadInbox({ sessions, view, scope, query, selectedKey: selected,
      now: UNKNOWN_SEEN + 60_000, messageHits });
    const inbox: ThreadInboxController = {
      model,
      setScope: next => { scope = next; render(); },
      send: commands => { sent.push([...commands]); return Promise.resolve(true); },
      unarchive: key => { unarchived.push(key); },
      shortcuts: true,
    };
    flushSync(() => root.render(<div style={{ width, height: 720, display: "flex", padding: 8, boxSizing: "border-box" }}>
      <style>{APP_SELECT_CSS}</style>
      <Sidebar threadInbox={inbox} threadLive={live} sectionLabel="Threads"
        newLabel="New thread"
        zeroStateText="No threads yet."
        rateLimits={{}} hidden={{ external: 0, trivial: 0 }} hideable={0}
        onSelect={key => { selections.push(key); selected = key; render(); }}
        onNew={noop} onNewInProject={noop} onToggleFilters={noop} onQuery={noop} query=""
        onArchive={key => { archived.push(key); return archiveAnswer(); }} onSettings={noop} />
    </div>));
  };
  const settle = async (): Promise<void> => { await frame(); await frame(); };
  const rowKeys = (section: string): string[] => [...document.querySelectorAll<HTMLElement>(
    `section[data-thread-section="${section}"] .thread-inbox-item`)].map(item => item.dataset.threadKey ?? "");
  const item = (key: string): HTMLElement => document.querySelector<HTMLElement>(`.thread-inbox-item[data-thread-key="${key}"]`)!;
  const rowButton = (key: string): HTMLButtonElement => item(key).querySelector<HTMLButtonElement>(".thread-inbox-row")!;
  const fold = (section: string): HTMLButtonElement =>
    document.querySelector<HTMLButtonElement>(`section[data-thread-section="${section}"] .thread-inbox-fold`)!;
  const action = (rowKey: string, name: string): HTMLButtonElement =>
    item(rowKey).querySelector<HTMLButtonElement>(`[data-thread-action="${name}"]`)!;
  const last = (): ThreadCommand[] => sent[sent.length - 1] ?? [];
  // Any computed colour (oklch, relative colours, color-mix) as the sRGB a canvas paints it,
  // composited over `under` when it is not opaque.
  const paint = document.createElement("canvas").getContext("2d", { willReadFrequently: true })!;
  const painted = (value: string, under = "#000000"): string => {
    paint.clearRect(0, 0, 1, 1);
    paint.fillStyle = under;
    paint.fillRect(0, 0, 1, 1);
    paint.fillStyle = value;
    paint.fillRect(0, 0, 1, 1);
    const [r, g, b] = paint.getImageData(0, 0, 1, 1).data;
    return `#${[r, g, b].map(v => v.toString(16).padStart(2, "0")).join("")}`;
  };
  const reduced = matchMedia("(prefers-reduced-motion: reduce)").matches;
  // A menu item's own words, without the chord hint beside them.
  const labelOf = (button: Element): string => [...button.childNodes]
    .filter(node => !(node instanceof Element && node.classList.contains("thread-menu-key")))
    .map(node => node.textContent ?? "").join("").trim();
  const menuItem = (label: string): HTMLButtonElement | undefined => [...document.querySelectorAll<HTMLButtonElement>(
    ".thread-menu [role=menuitem], .thread-submenu [role=menuitem], .thread-submenu [role=menuitemradio]")]
    .find(button => labelOf(button) === label);
  const menuOpened = (what: string) => until(
    () => document.querySelector(".thread-menu")?.contains(document.activeElement) === true,
    () => `${what} did not open with focus: ${JSON.stringify({
      menu: document.querySelector(".thread-menu")?.outerHTML.slice(0, 300),
      focus: document.activeElement?.outerHTML.slice(0, 200),
    })}`);
  const key = (target: EventTarget, init: KeyboardEventInit): void => {
    target.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init }));
  };
  // The row menu's keyboard path: ⇧F10 on the row (the menu key does the same).
  const openMenu = async (rowKey: string): Promise<void> => {
    rowButton(rowKey).focus();
    key(rowButton(rowKey), { key: "F10", shiftKey: true });
    await menuOpened(`The menu for ${rowKey}`);
  };

  render(); await settle();

  // Sections in order, Snoozed and Settled folded by default.
  const sections = [...document.querySelectorAll<HTMLElement>("section[data-thread-section]")]
    .map(section => section.dataset.threadSection);
  assert(sections.join() === "pinned,active,snoozed,settled", `Wrong section order: ${sections.join()}`);
  assert(fold("snoozed").getAttribute("aria-expanded") === "false"
    && fold("settled").getAttribute("aria-expanded") === "false", "Snoozed and Settled were not folded by default");
  // The shelves sit at the foot of the list, not under the last Active row:
  // 34px folds ("Snoozed · 1", the chevron at the right) under one hairline
  // that runs the list's width, 18px clear of the rows above it.
  const list = document.querySelector<HTMLElement>(".thread-inbox")!.getBoundingClientRect();
  const activeBox = document.querySelector('section[data-thread-section="active"]')!.getBoundingClientRect();
  const snoozedShelf = fold("snoozed").getBoundingClientRect();
  const settledShelf = fold("settled").getBoundingClientRect();
  assert(settledShelf.bottom >= list.bottom - 12 && snoozedShelf.top > activeBox.bottom + 24,
    `The shelves were not pushed to the foot of the list: ${JSON.stringify({ list, activeBox, snoozedShelf, settledShelf })}`);
  assert(Math.round(snoozedShelf.height) === 30 && Math.round(settledShelf.height) === 30,
    `A shelf fold is ${snoozedShelf.height}px, not 30px`);
  const shelves = document.querySelector<HTMLElement>(".thread-inbox-shelves")!;
  const hairline = getComputedStyle(shelves, "::before");
  const shelvesBox = shelves.getBoundingClientRect();
  assert(hairline.height === "1px" && hairline.top === "18px" && Number.parseFloat(hairline.width) >= settledShelf.width - 1
    && Math.round(snoozedShelf.top - shelvesBox.top) === 27,
  `The shelves' hairline is ${hairline.width} × ${hairline.height} at ${hairline.top}, ${snoozedShelf.top - shelvesBox.top}px above the first shelf`);
  assert(fold("settled").querySelector(".thread-inbox-label")?.textContent === "Settled"
    && fold("settled").querySelector(".thread-inbox-count")?.textContent === "· 2",
  `The Settled shelf reads "${fold("settled").textContent}"`);
  const shelfChevron = fold("settled").querySelector(".thread-inbox-chevron")!.getBoundingClientRect();
  assert(Math.round(settledShelf.right - shelfChevron.right) <= 12 && shelfChevron.left > settledShelf.left + settledShelf.width / 2,
    "The shelf's chevron is not at its right end");
  // Captions (owner, 2026-11-16, the TN Code look): a small mono label in
  // capitals in the dim tier, the count at the right edge, the chevron hidden
  // until hover or focus while open.
  const caption = fold("active");
  const captionCount = caption.querySelector(".thread-inbox-count")!.getBoundingClientRect();
  const fgDim = getComputedStyle(document.documentElement).getPropertyValue("--fg-dim").trim();
  assert(getComputedStyle(caption).textTransform === "uppercase" && caption.querySelector(".thread-inbox-label")?.textContent === "Active"
    && painted(getComputedStyle(caption).color) === painted(fgDim)
    && Math.abs(captionCount.right - (caption.getBoundingClientRect().right - 12)) <= 1,
  `The Active caption is ${getComputedStyle(caption).textTransform} "${caption.textContent}", its count ending at ${captionCount.right}`);
  assert(getComputedStyle(caption.querySelector(".thread-inbox-chevron")!).opacity === "0",
    "An open caption shows its chevron at rest");
  assert(rowKeys("pinned").join() === ORDER.pinned.join(), `Wrong Pinned order: ${rowKeys("pinned").join()}`);
  assert(rowKeys("active").join() === ORDER.active.join(), `Wrong Active order: ${rowKeys("active").join()}`);
  assert(!document.body.textContent?.includes("An agent task"), "An Agent task leaked into Threads");
  assert(!document.body.textContent?.includes("A worker Rust leaves out"),
    "A session Rust left out of its view was listed after the grace period");
  fold("snoozed").click(); await settle();
  fold("settled").click(); await settle();
  assert(rowKeys("snoozed").join() === ORDER.snoozed.join() && rowKeys("settled").join() === ORDER.settled.join(),
    "Snoozed or Settled lost its order when unfolded");
  assert(JSON.parse(localStorage.getItem(THREAD_FOLDS_KEY) ?? "{}").settled === false, "The fold was not remembered");
  // A folded caption keeps its chevron up, turned, and shows its section's
  // highest mark before the count: Pinned holds a working run.
  fold("pinned").click(); await settle();
  for (const animation of document.getAnimations()) if (animation instanceof CSSTransition) animation.finish();
  const foldedChevron = getComputedStyle(fold("pinned").querySelector(".thread-inbox-chevron")!);
  const foldedMark = fold("pinned").querySelector<HTMLElement>(".thread-inbox-fold-end [data-thread-mark]");
  assert(foldedChevron.opacity === "1" && foldedChevron.transform !== "none" && rowKeys("pinned").length === 0
    && foldedMark?.dataset.threadMark === "working" && foldedMark.querySelector(".thinking-mark") !== null
    && fold("pinned").getAttribute("aria-label") === "Pinned, 2 threads, working",
  `Folded Pinned shows ${foldedChevron.opacity} / ${foldedChevron.transform}, ${foldedMark?.outerHTML ?? "no mark"}, "${fold("pinned").getAttribute("aria-label")}"`);
  fold("pinned").click(); await settle();
  assert(rowKeys("pinned").join() === ORDER.pinned.join(), "Pinned did not open again");

  // One slot per row, from the ladder.
  for (const [rowKey, mark] of Object.entries(MARKS)) {
    const marks = item(rowKey).querySelectorAll("[data-thread-mark]");
    assert(marks.length === 1 && (marks[0] as HTMLElement).dataset.threadMark === mark,
      `${rowKey} drew ${[...marks].map(node => (node as HTMLElement).dataset.threadMark).join()} instead of ${mark}`);
  }
  // Unread: a dot in the done blue, no glow and no project hue.
  const unread = getComputedStyle(item("codex#active-done").querySelector(".thread-inbox-unread")!);
  const doneInk = getComputedStyle(document.documentElement).getPropertyValue("--status-done").trim();
  assert(painted(unread.backgroundColor) === painted(doneInk) && unread.boxShadow === "none"
    && unread.width === "7px",
  `The unread dot is ${unread.width} ${unread.backgroundColor} / ${unread.boxShadow}, not 7px of ${doneInk} with no glow`);
  const slotText = (rowKey: string): string => item(rowKey).querySelector(".thread-inbox-slot")?.textContent ?? "";
  // Working (owner, 2026-10-07; the Pair, 2026-10-23): no pill, the Pair at
  // 13px in the working colour, then the run's clock in the slot's own
  // tabular figures.
  assert(/^\d+:\d\d$/u.test(slotText("claude#pin-release")), `The working slot said "${slotText("claude#pin-release")}"`);
  assert(item("claude#pin-release").querySelector(".thread-inbox-pill") === null, "A working row still wears a pill");
  assert(item("claude#pin-release").querySelector(".working-wave") === null, "A working row still draws the Wave");
  const pair = item("claude#pin-release").querySelector<SVGSVGElement>(".thread-inbox-slot > .thinking-mark")!;
  const pairBox = pair.getBoundingClientRect();
  const workingInk = getComputedStyle(document.documentElement).getPropertyValue("--status-working").trim();
  const bead = getComputedStyle(pair.querySelector(".thinking-mark-a")!);
  assert(Math.round(pairBox.width) === 13 && Math.round(pairBox.height) === 13
    && painted(getComputedStyle(pair).color) === painted(workingInk) && pair.querySelectorAll("circle").length === 2
    && bead.animationName === (reduced ? "none" : "thinking-mark-a"),
  `The working Pair is ${pairBox.width}x${pairBox.height} in ${getComputedStyle(pair).color}, moving ${bead.animationName}, not 13px in ${workingInk}`);
  const clock = getComputedStyle(item("claude#pin-release").querySelector(".thread-inbox-clock")!);
  const rowFace = getComputedStyle(item("claude#pin-release").querySelector(".thread-inbox-title")!).fontFamily;
  assert(clock.fontVariantNumeric.includes("tabular-nums") && clock.fontFamily === rowFace
    && painted(clock.color) === painted(fgDim),
  `The clock is not the row's dim tabular figures: ${clock.fontFamily} (row ${rowFace}) / ${clock.fontVariantNumeric} / ${clock.color}`);
  assert(slotText("claude#active-waiting") === "Needs you" && slotText("claude#active-failed") === "Failed"
    && slotText("codex#active-woke") === "Back" && slotText("codex#active-done") === "1h",
  `Slots said ${["claude#active-waiting", "claude#active-failed", "codex#active-woke", "codex#active-done"].map(slotText).join(" / ")}`);
  // Needs you is the one filled pill, dark ink on the amber; Failed an outline
  // pill in the failed colour, no fill. Both 18px, words only, at 4.5:1.
  const sheet = painted(getComputedStyle(document.querySelector(".sidebar")!).backgroundColor);
  const needsPill = item("claude#active-waiting").querySelector<HTMLElement>(".thread-inbox-pill.is-needs-you")!;
  const needsStyle = getComputedStyle(needsPill);
  const needsFill = painted(needsStyle.backgroundColor, sheet);
  const needsRatio = contrastRatio(painted(needsStyle.color, needsFill), needsFill);
  assert(needsFill !== sheet && needsRatio >= 4.5 && Math.round(needsPill.getBoundingClientRect().height) === 18
    && needsPill.querySelector("svg") === null && needsPill.textContent === "Needs you",
  `Needs you is ${needsPill.getBoundingClientRect().height}px, ${needsStyle.color} on ${needsStyle.backgroundColor} at ${needsRatio.toFixed(2)}:1`);
  assert(needsStyle.animationName === (reduced ? "none" : "thread-inbox-ring")
    && (reduced || (needsStyle.animationIterationCount === "2" && needsStyle.animationDuration === "1.1s")),
  `Needs you rings ${needsStyle.animationName} ×${needsStyle.animationIterationCount} over ${needsStyle.animationDuration}`);
  const failedPill = item("claude#active-failed").querySelector<HTMLElement>(".thread-inbox-pill.is-failed")!;
  const failedStyle = getComputedStyle(failedPill);
  const failedRatio = contrastRatio(painted(failedStyle.color, sheet), sheet);
  const failedInk = getComputedStyle(document.documentElement).getPropertyValue("--status-failed").trim();
  assert(painted(failedStyle.backgroundColor, sheet) === sheet && failedStyle.borderTopWidth === "1px"
    && failedStyle.borderTopStyle === "solid" && painted(failedStyle.borderTopColor) === painted(failedInk)
    && failedRatio >= 4.5 && Math.round(failedPill.getBoundingClientRect().height) === 18
    && failedPill.querySelector("svg") === null,
  `Failed is not a readable 18px outline: ${failedStyle.backgroundColor} / ${failedStyle.borderTopWidth} ${failedStyle.borderTopColor} / ${failedRatio.toFixed(2)}:1`);
  assert(item("codex#active-woke").querySelector(".thread-inbox-pill") === null
    && item("codex#active-done").querySelector(".thread-inbox-slot .thread-inbox-pill") === null,
  "Back or the age was drawn as a pill");
  // Two lines in Pinned and Active, a slim line on the shelves; nothing wraps.
  for (const row of document.querySelectorAll<HTMLElement>(".thread-inbox-row")) {
    const height = Math.round(row.getBoundingClientRect().height);
    const want = row.classList.contains("is-slim") ? 32 : 52;
    assert(height === want, `A ${row.classList.contains("is-slim") ? "slim" : "two-line"} row is ${height}px, not ${want}px`);
    assert(row.scrollWidth <= row.clientWidth + 1, "A thread row overflows");
    // The provider's mark ends line two of a two-line row; a slim row has none.
    const logos = row.querySelectorAll(".provider-logo");
    if (row.classList.contains("is-slim")) assert(logos.length === 0, "A slim row drew a provider logo");
    else {
      const logo = logos[0]?.getBoundingClientRect();
      const trail = row.querySelector(".thread-inbox-trail")!.getBoundingClientRect();
      assert(logos.length === 1 && logo !== undefined && Math.round(logo.width) === 12
        && Math.round(logo.right) === Math.round(trail.right) && logo.top >= row.getBoundingClientRect().top + 28,
      `${row.getAttribute("aria-label")}: the provider's mark is not 12px at the end of line two`);
    }
    // The hue lives on one mark alone, a full row's tile or a slim row's
    // square: the row wears none.
    const marks = row.querySelectorAll(".thread-inbox-tile, .thread-inbox-hue");
    assert(![...row.classList].some(name => name.startsWith("project-hue-")) && marks.length === 1
      && marks[0].classList.contains(row.classList.contains("is-slim") ? "thread-inbox-hue" : "thread-inbox-tile")
      && [...marks[0].classList].filter(name => name.startsWith("project-hue-")).length === 1
      && row.querySelector(".project-tile") === null,
    `${row.getAttribute("aria-label")}: the hue is not on the row's one mark alone`);
  }
  assert(document.querySelectorAll(".thread-inbox-row.is-slim").length === 3, "The shelves' rows are not slim");
  // The row leads with its tile, the thread's initials on the project's hue;
  // line two is project · branch · the PR or the diff, with the provider's
  // mark in its brand colour at the right.
  const logo = (rowKey: string): string => getComputedStyle(item(rowKey).querySelector(".provider-logo")!).color;
  assert(logo("codex#active-done") === "rgb(18, 128, 90)" && logo("claude#active-failed") === "rgb(184, 79, 49)",
    `Provider marks are ${logo("codex#active-done")} and ${logo("claude#active-failed")}, not the brand colours`);
  const lead = rowButton("codex#active-done").firstElementChild;
  const leadBox = lead?.getBoundingClientRect();
  const doneBox = rowButton("codex#active-done").getBoundingClientRect();
  // Its corner rides the radius system, so the Bare CLI form squares it.
  assert(lead?.classList.contains("thread-inbox-tile") === true && lead.classList.contains(projectHueClass(WEBSITE))
    && leadBox !== undefined && Math.round(leadBox.width) === 28 && Math.round(leadBox.height) === 28
    && Math.round(leadBox.left - doneBox.left) === 9
    && Math.abs((leadBox.top + leadBox.bottom) / 2 - (doneBox.top + doneBox.bottom) / 2) <= 1
    && getComputedStyle(lead).borderTopLeftRadius === (chrome === "cli" ? "0px" : "8px")
    && /^[A-Z0-9]{1,2}$/u.test(lead.textContent ?? "")
    && item("codex#active-done").querySelector(".thread-inbox-meta > .thread-inbox-hue") === null,
  `The row does not lead with its 28px tile: ${lead?.outerHTML} ${getComputedStyle(lead!).borderTopLeftRadius}`);
  const hues = new Set(["claude#pin-release", "codex#active-done", "codex#active-woke", "claude#active-failed"]
    .map(rowKey => getComputedStyle(item(rowKey).querySelector(".thread-inbox-tile")!).backgroundImage));
  assert(hues.size === 4, `Four projects drew ${hues.size} tile colours`);
  const slimSquare = item("codex#snoozed-retry").querySelector<HTMLElement>(".thread-inbox-hue")!;
  assert(getComputedStyle(slimSquare).opacity === "0.5", "A slim row's square is not at half strength");
  const words = (rowKey: string): string => item(rowKey).querySelector(".thread-inbox-words")?.textContent ?? "";
  assert(item("codex#active-done").querySelector(".thread-inbox-project")?.textContent === "website"
    && item("codex#active-done").querySelector(".thread-inbox-branch")?.textContent === "iface/flaky-3f9a2c"
    && words("codex#active-done") === "website·iface/flaky-3f9a2c·#12",
  `Line two reads "${words("codex#active-done")}"`);
  // The pull request: its glyph and number in its state's colour, legible on the selected row.
  const pr = item("codex#active-done").querySelector<HTMLElement>(".thread-inbox-words > .thread-inbox-pr.is-open");
  const chosenFill = painted(getComputedStyle(rowButton("codex#active-done")).backgroundColor, sheet);
  assert(pr !== null && pr.textContent === "#12" && pr.querySelector("svg") !== null
    && painted(getComputedStyle(pr).backgroundColor, chosenFill) === chosenFill
    && contrastRatio(painted(getComputedStyle(pr).color, chosenFill), chosenFill) >= 4.5,
  `The pull request is not a readable mark on line two: ${pr?.outerHTML}`);
  assert(item("claude#active-failed").querySelector(".thread-inbox-words > .thread-inbox-diff")?.textContent === "+210−38"
    && words("claude#active-failed") === "atlas·iface/importer·+210−38",
  `Line two reads "${words("claude#active-failed")}"`);
  assert(item("codex#active-woke").querySelector(".thread-inbox-diff, .thread-inbox-pr") === null, "Unknown counts drew a diff");
  // A plain chat's tile is the dim tier, never a project's hue; a slim plain
  // row keeps the hollow ring.
  const plainTile = item("claude#active-unknown").querySelector<HTMLElement>(".thread-inbox-tile.project-hue-none");
  assert(item("claude#active-unknown").querySelector(".thread-inbox-project")?.textContent === "No project"
    && plainTile !== null && !hues.has(getComputedStyle(plainTile).backgroundImage)
    && item("claude#active-unknown").querySelector(".thread-inbox-branch") === null,
  "A plain chat is not the dim tile and No project");
  // The selection is the selection colour's wash, worked out from the theme's
  // own tokens: no gradient, no hued ring.
  const chosen = getComputedStyle(rowButton("codex#active-done"));
  const washProbe = document.createElement("span");
  washProbe.style.backgroundColor = "var(--tn-row-selected)";
  document.body.append(washProbe);
  const rowSelected = getComputedStyle(washProbe).backgroundColor;
  washProbe.remove();
  assert(chosen.backgroundImage === "none" && chosen.backgroundColor === rowSelected && !chosen.boxShadow.includes("inset"),
    `The selected row is ${chosen.backgroundImage} / ${chosen.backgroundColor} / ${chosen.boxShadow}, not ${rowSelected}`);

  // No header band (owner, 2026-10-23): the hue glow went, and the sidebar is
  // its plain ground whichever thread is focused.
  for (const focused of ["codex#active-done", "claude#active-unknown"]) {
    selected = focused; render(); await settle();
    assert(document.querySelector(".thread-glow") === null,
      `Focusing ${focused} drew a header band`);
  }
  selected = "codex#active-done"; render(); await settle();
  assert(rowButton("codex#snoozed-retry").querySelector(".session-title")!.getAttribute("title")?.includes("Wakes") === true
    && /^(?:\d{1,2}(?::\d\d)? [ap]m|Sun|Mon|Tue|Wed|Thu|Fri|Sat)$/u.test(slotText("codex#snoozed-retry")),
  `A snoozed row does not say when it wakes: "${slotText("codex#snoozed-retry")}"`);
  // Titles: unread strongest; a read row with nothing to say recedes to the dim tier.
  const weight = (rowKey: string): number => Number(getComputedStyle(item(rowKey).querySelector(".thread-inbox-title")!).fontWeight);
  const titleInk = (rowKey: string): string => painted(getComputedStyle(item(rowKey).querySelector(".thread-inbox-title")!).color);
  const fgStrong = getComputedStyle(document.documentElement).getPropertyValue("--fg-strong").trim();
  assert(weight("claude#active-failed") > weight("claude#active-waiting")
    && titleInk("claude#active-waiting") === painted(fgStrong) && titleInk("codex#pin-docs") === painted(fgDim),
  `Titles: ${weight("claude#active-failed")} / ${weight("claude#active-waiting")} weights, ${titleInk("claude#active-waiting")} / ${titleInk("codex#pin-docs")} inks`);

  // The hover actions sit in line one's slot, at rest invisible, each a named 24px target.
  const slot = item("claude#active-waiting").querySelector<HTMLElement>(".thread-inbox-slot")!;
  const actions = item("claude#active-waiting").querySelector<HTMLElement>(".thread-inbox-actions")!;
  assert(getComputedStyle(slot).visibility === "visible" && getComputedStyle(actions).opacity === "0",
    "At rest the row shows its actions instead of its slot");
  const actionsBox = actions.getBoundingClientRect();
  const rowBox = rowButton("claude#active-waiting").getBoundingClientRect();
  const slotBox = slot.getBoundingClientRect();
  assert(Math.round(actionsBox.height) === 24 && actionsBox.right <= rowBox.right && actionsBox.top >= rowBox.top
    && actionsBox.bottom <= rowBox.top + 30 && actionsBox.left >= slotBox.left - 10
    && Math.abs((actionsBox.top + actionsBox.bottom) / 2 - (slotBox.top + slotBox.bottom) / 2) <= 1,
  `The actions are not in line one's slot: ${JSON.stringify({ actionsBox, rowBox, slotBox })}`);
  for (const button of item("claude#active-waiting").querySelectorAll<HTMLElement>(".thread-inbox-action")) {
    const box = button.getBoundingClientRect();
    assert(box.width >= 24 && box.height >= 24 && button.getAttribute("aria-label") !== null,
      "A hover action is under 24px or unnamed");
  }

  // Selecting a thread goes through onSelect (App visits it).
  rowButton("claude#active-waiting").click(); await settle();
  assert(selections.at(-1) === "claude#active-waiting", "Clicking a row did not select its thread");

  // The row menu opens on a right-click too, at the pointer.
  const woke = rowButton("codex#active-woke").getBoundingClientRect();
  rowButton("codex#active-woke").dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true,
    clientX: woke.left + 40, clientY: woke.top + 10 }));
  await menuOpened("A right-click on a row");
  key(document.activeElement!, { key: "Escape" });
  await until(() => document.querySelector(".thread-menu") === null
    && document.activeElement === rowButton("codex#active-woke"), "Escape did not hand focus back to the row");

  // Row menu: Pin sends pin. Entries with a real chord say it.
  await openMenu("codex#active-woke");
  const labels = [...document.querySelectorAll<HTMLElement>(".thread-menu > [role=menuitem]")].map(labelOf);
  for (const label of ["Pin", "Settle", "Snooze", "Mark unread", "Regenerate title", "Auto-settle", "Archive"]) {
    assert(labels.includes(label), `The row menu lacks ${label}: ${labels.join(" / ")}`);
  }
  const hint = (label: string): string => menuItem(label)?.querySelector(".thread-menu-key")?.textContent ?? "";
  assert(hint("Pin") === "⌘⌥P" && hint("Settle") === "⌘⇧S" && hint("Mark unread") === "",
    `Menu hints were Pin ${hint("Pin")}, Settle ${hint("Settle")}, Mark unread ${hint("Mark unread")}`);
  assert(!labels.includes("Refresh pull request"), "A thread with no tracked branch offers Refresh pull request");
  menuItem("Pin")!.click(); await settle();
  assert(JSON.stringify(last()) === JSON.stringify([{ kind: "pin", key: "codex#active-woke" }]),
    `Pin sent ${JSON.stringify(last())}`);
  assert(document.querySelector(".thread-menu") === null, "The menu stayed open after Pin");

  // A tracked worktree thread: its hover card names the pull request, and its
  // menu refreshes it.
  rowButton("codex#active-done").focus();
  key(rowButton("codex#active-done"), { key: " " });
  await until(() => document.querySelector(".session-peek") !== null, "Space did not open the hover card");
  const peekText = document.querySelector(".session-peek")?.textContent ?? "";
  assert(peekText.includes("PR#12 · open"), `The hover card does not name the pull request: ${peekText}`);
  key(rowButton("codex#active-done"), { key: "Escape" }); await settle();
  await openMenu("codex#active-done");
  menuItem("Refresh pull request")!.click(); await settle();
  assert(JSON.stringify(last()) === JSON.stringify([{ kind: "refreshPullRequest", key: "codex#active-done" }]),
    `Refresh pull request sent ${JSON.stringify(last())}`);

  // Snooze ▸ 1 hour, then Undo from the toast.
  await openMenu("codex#active-woke");
  menuItem("Snooze")!.click();
  await until(() => document.querySelector(".thread-submenu")?.contains(document.activeElement) === true,
    "The Snooze submenu did not take focus");
  const presetLabels = [...document.querySelectorAll(".thread-submenu [role=menuitem]")].map(node => node.textContent?.trim());
  assert(presetLabels.includes("1 hour") && presetLabels.includes("Tomorrow 9 am") && presetLabels.includes("Next Monday 9 am"),
    `Snooze presets were ${presetLabels.join(" / ")}`);
  const before = Date.now();
  menuItem("1 hour")!.click(); await settle();
  const snooze = last()[0] as Extract<ThreadCommand, { kind: "snooze" }>;
  assert(snooze.kind === "snooze" && snooze.key === "codex#active-woke"
    && Math.abs(snooze.until - (before + HOUR)) < 5_000, `Snooze sent ${JSON.stringify(last())}`);
  await until(() => document.querySelector(".thread-inbox-toast") !== null, "Snooze offered no Undo");
  assert(document.querySelector(".thread-inbox-toast-text")?.textContent === "Snoozed “Benchmark the cache”.",
    "The snooze toast named the wrong thing");
  document.querySelector<HTMLButtonElement>(".thread-inbox-toast-undo")!.click(); await settle();
  assert(JSON.stringify(last()) === JSON.stringify([{ kind: "unsnooze", key: "codex#active-woke" }]),
    `Undo of a snooze sent ${JSON.stringify(last())}`);
  assert(document.querySelector(".thread-inbox-toast") === null, "Undo left the toast up");

  // Auto-settle ▸ Disabled, keyboard-driven into the submenu.
  await openMenu("codex#active-woke");
  const autoSettle = document.querySelector<HTMLButtonElement>('[data-thread-submenu="autoSettle"]')!;
  autoSettle.focus();
  key(autoSettle, { key: "ArrowRight" });
  await until(() => document.querySelector(".thread-submenu")?.contains(document.activeElement) === true,
    "ArrowRight did not open Auto-settle");
  assert(menuItem("Enabled")?.getAttribute("aria-checked") === "true", "Auto-settle did not show its current choice");
  menuItem("Disabled")!.click(); await settle();
  assert(JSON.stringify(last()) === JSON.stringify([{ kind: "setAutoSettle", key: "codex#active-woke", enabled: false }]),
    `Auto-settle sent ${JSON.stringify(last())}`);

  // Settle from the menu, then ⌘Z with focus in the sidebar puts it back in place.
  await openMenu("claude#active-waiting");
  menuItem("Settle")!.click(); await settle();
  assert(JSON.stringify(last()) === JSON.stringify([{ kind: "settle", key: "claude#active-waiting" }]),
    `Settle sent ${JSON.stringify(last())}`);
  await until(() => document.querySelector(".thread-inbox-toast") !== null, "Settle offered no Undo");
  rowButton("codex#active-done").focus();
  key(document.activeElement!, { key: "z", code: "KeyZ", metaKey: true });
  await settle();
  assert(JSON.stringify(last()) === JSON.stringify([{ kind: "unsettle", key: "claude#active-waiting" },
    { kind: "reorder", key: "claude#active-waiting", section: "active", beforeKey: "codex#active-woke" }]),
  `⌘Z sent ${JSON.stringify(last())}`);

  // Archive is the session's own hide; Undo is its unarchive, offered only
  // once the archive has answered that it happened.
  if (document.querySelector(".thread-inbox-toast") !== null) {
    document.querySelector<HTMLButtonElement>(".thread-inbox-toast-undo")!.click(); await settle();
  }
  let answerArchive: (archivedIt: boolean) => void = noop;
  archiveAnswer = () => new Promise<boolean>(resolve => { answerArchive = resolve; });
  await openMenu("codex#settled-old");
  menuItem("Archive")!.click(); await settle();
  assert(archived.at(-1) === "codex#settled-old", "Archive did not hide the session");
  assert(document.querySelector(".thread-inbox-toast") === null, "Undo was offered before the archive answered");
  // Focus goes to the row that takes the archived row's place.
  await until(() => document.activeElement === rowButton("claude#settled-notes"),
    () => `Archive left focus on ${document.activeElement?.outerHTML.slice(0, 160) ?? "nothing"}`);
  answerArchive(true);
  await until(() => document.querySelector(".thread-inbox-toast-text")?.textContent === "Archived “Old spike”.",
    "Archive offered no Undo");
  document.querySelector<HTMLButtonElement>(".thread-inbox-toast-undo")!.click(); await settle();
  assert(unarchived.at(-1) === "codex#settled-old", "Undo did not unarchive the session");
  // A refused archive has nothing to undo.
  archiveAnswer = () => Promise.resolve(false);
  await openMenu("codex#settled-old");
  menuItem("Archive")!.click(); await settle(); await settle();
  assert(document.querySelector(".thread-inbox-toast") === null, "A refused archive offered Undo");
  archiveAnswer = () => Promise.resolve(true);

  // Move up / Move down: the keyboard's reorder within Active.
  await openMenu("claude#active-waiting");
  menuItem("Move up")!.click(); await settle();
  assert(JSON.stringify(last()) === JSON.stringify([{ kind: "reorder", key: "claude#active-waiting",
    section: "active", beforeKey: "codex#active-done" }]), `Move up sent ${JSON.stringify(last())}`);
  await until(() => document.activeElement === rowButton("claude#active-waiting"),
    "Move up did not hand focus back to the row");
  await openMenu("claude#active-waiting");
  menuItem("Move down")!.click(); await settle();
  assert(JSON.stringify(last()) === JSON.stringify([{ kind: "reorder", key: "claude#active-waiting",
    section: "active", beforeKey: "claude#active-failed" }]), `Move down sent ${JSON.stringify(last())}`);
  await openMenu("codex#active-done");
  assert(menuItem("Move up") === undefined && menuItem("Move down") !== undefined,
    "Rust's top Active row offers Move up");
  document.body.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true })); await settle();

  // Drag within Active: drop on the upper half of a row places it before that row.
  const transfer = new DataTransfer();
  const drag = async (from: string, onto: Element, upper: boolean): Promise<void> => {
    rowButton(from).dispatchEvent(new DragEvent("dragstart", { bubbles: true, cancelable: true, dataTransfer: transfer }));
    await settle();
    const box = onto.getBoundingClientRect();
    const init = { bubbles: true, cancelable: true, dataTransfer: transfer, clientX: box.left + 20,
      clientY: upper ? box.top + 3 : box.bottom - 3 };
    onto.dispatchEvent(new DragEvent("dragover", init));
    await settle();
    onto.dispatchEvent(new DragEvent("drop", init));
    rowButton(from)?.dispatchEvent(new DragEvent("dragend", { bubbles: true, dataTransfer: transfer }));
    await settle();
  };
  rowButton("claude#active-failed").dispatchEvent(new DragEvent("dragstart", { bubbles: true, cancelable: true, dataTransfer: transfer }));
  await settle();
  const target = item("codex#active-done");
  const targetBox = target.getBoundingClientRect();
  target.dispatchEvent(new DragEvent("dragover", { bubbles: true, cancelable: true, dataTransfer: transfer,
    clientX: targetBox.left + 20, clientY: targetBox.top + 3 }));
  await settle();
  assert(target.classList.contains("is-drop-before"), "Dragging over a row drew no insertion line");
  target.dispatchEvent(new DragEvent("drop", { bubbles: true, cancelable: true, dataTransfer: transfer,
    clientX: targetBox.left + 20, clientY: targetBox.top + 3 }));
  rowButton("claude#active-failed").dispatchEvent(new DragEvent("dragend", { bubbles: true, dataTransfer: transfer }));
  await settle();
  assert(JSON.stringify(last()) === JSON.stringify([{ kind: "reorder", key: "claude#active-failed", section: "active",
    beforeKey: "codex#active-done" }]), `A drag within Active sent ${JSON.stringify(last())}`);
  assert(!document.querySelector(".is-drop-before, .is-drop-after, .is-dragging"), "The drag left its marks behind");

  // The floated row's lower half: before the row Rust keeps after it
  // (codex#active-woke), not the one drawn under it (codex#active-done).
  await drag("claude#active-failed", item("claude#active-waiting"), false);
  assert(JSON.stringify(last()) === JSON.stringify([{ kind: "reorder", key: "claude#active-failed", section: "active",
    beforeKey: RUST_ACTIVE[RUST_ACTIVE.indexOf("claude#active-waiting") + 1] }]),
  `A drop under the floated row sent ${JSON.stringify(last())}`);
  // Across sections: the lower half of the last pinned row pins at the end.
  await drag("codex#active-woke", item("codex#pin-docs"), false);
  assert(JSON.stringify(last()) === JSON.stringify([{ kind: "pin", key: "codex#active-woke" },
    { kind: "reorder", key: "codex#active-woke", section: "pinned" }]), `A drag to Pinned sent ${JSON.stringify(last())}`);
  // Onto Settled settles, with Undo.
  await drag("codex#active-done", document.querySelector('section[data-thread-section="settled"] .thread-inbox-shelf-head')!, true);
  assert(JSON.stringify(last()) === JSON.stringify([{ kind: "settle", key: "codex#active-done" }]),
    `A drag to Settled sent ${JSON.stringify(last())}`);
  await until(() => document.querySelector(".thread-inbox-toast-text")?.textContent === "Settled “Fix the flaky login test”.",
    "A drag to Settled offered no Undo");
  // A settled thread dragged into Active un-settles there.
  await drag("claude#settled-notes", item("claude#active-waiting"), true);
  assert(JSON.stringify(last()) === JSON.stringify([{ kind: "unsettle", key: "claude#settled-notes" },
    { kind: "reorder", key: "claude#settled-notes", section: "active", beforeKey: "claude#active-waiting" }]),
  `A drag from Settled sent ${JSON.stringify(last())}`);
  const sentBefore = sent.length;
  await drag("codex#pin-docs", document.querySelector('section[data-thread-section="snoozed"] .thread-inbox-item')!, true);
  assert(sent.length === sentBefore, "Snoozed accepted a drop without a time");

  // The hover actions. Snooze opens the presets anchored to its button...
  if (document.querySelector(".thread-inbox-toast") !== null) {
    document.querySelector<HTMLButtonElement>(".thread-inbox-toast-undo")!.click(); await settle();
  }
  const wokeTitle = item("codex#active-woke").querySelector(".thread-inbox-title")!.getBoundingClientRect().width;
  action("codex#active-woke", "snooze").click();
  await menuOpened("The Snooze button's menu");
  assert(document.querySelector(".thread-snooze-menu") !== null
    && action("codex#active-woke", "snooze").getAttribute("aria-expanded") === "true",
  "The Snooze button did not open its own menu");
  // While its menu is open the row keeps the actions up, in the slot's box: nothing moves.
  for (const animation of document.getAnimations()) if (animation instanceof CSSTransition) animation.finish();
  assert(getComputedStyle(item("codex#active-woke").querySelector(".thread-inbox-slot")!).visibility === "hidden"
    && getComputedStyle(item("codex#active-woke").querySelector(".thread-inbox-actions")!).opacity === "1",
  "The open Snooze menu did not keep the actions in place of the slot");
  assert(item("codex#active-woke").querySelector(".thread-inbox-title")!.getBoundingClientRect().width === wokeTitle,
    "Swapping the slot for the actions moved the title");
  const snoozeBox = document.querySelector(".thread-snooze-menu")!.getBoundingClientRect();
  const snoozeButtonBox = action("codex#active-woke", "snooze").getBoundingClientRect();
  assert(Math.abs(snoozeBox.top - snoozeButtonBox.bottom) <= 12 || Math.abs(snoozeBox.bottom - snoozeButtonBox.top) <= 12,
    "The Snooze menu is not anchored to its button");
  const snoozeLabels = [...document.querySelectorAll(".thread-snooze-menu [role=menuitem]")].map(labelOf);
  assert(snoozeLabels.includes("1 hour") && snoozeLabels.includes("Tomorrow 9 am") && snoozeLabels.includes("Next Monday 9 am"),
    `The Snooze button offered ${snoozeLabels.join(" / ")}`);
  // ...Escape gives focus back to the button...
  key(document.activeElement!, { key: "Escape" });
  await until(() => document.querySelector(".thread-menu") === null
    && document.activeElement === action("codex#active-woke", "snooze"), "Escape did not hand focus back to Snooze");
  // ...and a preset snoozes, with Undo.
  action("codex#active-woke", "snooze").click();
  await menuOpened("The Snooze button's menu, again");
  menuItem("Tomorrow 9 am")!.click(); await settle();
  assert(last().length === 1 && last()[0].kind === "snooze" && (last()[0] as { key: string }).key === "codex#active-woke",
    `The Snooze button sent ${JSON.stringify(last())}`);
  await until(() => document.querySelector(".thread-inbox-toast-text")?.textContent === "Snoozed “Benchmark the cache”.",
    "The Snooze button offered no Undo");
  // Settle settles, with Undo, and focus takes the row's place.
  action("claude#active-waiting", "settle").click(); await settle();
  assert(JSON.stringify(last()) === JSON.stringify([{ kind: "settle", key: "claude#active-waiting" }]),
    `The Settle button sent ${JSON.stringify(last())}`);
  await until(() => document.querySelector(".thread-inbox-toast-text")?.textContent === "Settled “Approve the migration plan”.",
    "The Settle button offered no Undo");
  // Focus goes to the row drawn under it: the floated row's neighbour on screen.
  await until(() => document.activeElement === rowButton(ORDER.active[1]),
    () => `Settle left focus on ${document.activeElement?.outerHTML.slice(0, 160) ?? "nothing"}`);
  // Wake on a snoozed row, Un-settle on a settled one.
  action("codex#snoozed-retry", "unsnooze").click(); await settle();
  assert(JSON.stringify(last()) === JSON.stringify([{ kind: "unsnooze", key: "codex#snoozed-retry" }]),
    `Wake sent ${JSON.stringify(last())}`);
  action("claude#settled-notes", "unsettle").click(); await settle();
  assert(JSON.stringify(last()) === JSON.stringify([{ kind: "unsettle", key: "claude#settled-notes" }]),
    `Un-settle sent ${JSON.stringify(last())}`);
  assert(item("codex#snoozed-retry").querySelectorAll(".thread-inbox-action").length === 1
    && item("claude#settled-notes").querySelectorAll(".thread-inbox-action").length === 1,
  "A shelved row offers more than its one action");

  // The chords: ⌘⇧] / ⌘⇧[ walk the drawn order, ⌘⇧S settles, ⌘⌥P pins.
  selected = "claude#pin-release"; render(); await settle();
  key(document.body, { key: "}", code: "BracketRight", metaKey: true, shiftKey: true });
  await settle();
  assert(selections.at(-1) === "codex#pin-docs", `⌘⇧] selected ${selections.at(-1)}`);
  key(document.body, { key: "{", code: "BracketLeft", metaKey: true, shiftKey: true });
  key(document.body, { key: "{", code: "BracketLeft", metaKey: true, shiftKey: true });
  await settle();
  assert(selections.at(-1) === "codex#settled-old", `⌘⇧[ did not wrap to the last drawn thread: ${selections.at(-1)}`);
  selected = "codex#active-done"; render(); await settle();
  (document.activeElement as HTMLElement | null)?.blur();
  key(document.body, { key: "s", code: "KeyS", metaKey: true, shiftKey: true });
  await settle();
  assert(JSON.stringify(last()) === JSON.stringify([{ kind: "settle", key: "codex#active-done" }]),
    `⌘⇧S sent ${JSON.stringify(last())}`);
  key(document.body, { key: "π", code: "KeyP", metaKey: true, altKey: true });
  await settle();
  assert(JSON.stringify(last()) === JSON.stringify([{ kind: "pin", key: "codex#active-done" }]),
    `⌘⌥P sent ${JSON.stringify(last())}`);
  const beforePalette = sent.length;
  key(document.body, { key: "p", code: "KeyP", metaKey: true, shiftKey: true });
  assert(sent.length === beforePalette, "⌘⇧P (the command palette) pinned a thread");

  // The scope picker narrows to plain chats.
  document.querySelector<HTMLButtonElement>(".thread-scope-picker .app-select-trigger")!.click();
  await until(() => document.querySelector('[role="listbox"] [role="option"]') !== null, "The scope picker did not open");
  const scopes = [...document.querySelectorAll<HTMLElement>('[role="listbox"] [role="option"]')]
    .map(option => option.getAttribute("aria-label") ?? option.textContent?.trim() ?? "");
  assert(scopes[0]?.startsWith("All projects") && scopes.some(label => label.startsWith("atlas"))
    && scopes.at(-1)?.startsWith("No project") === true, `Scope choices were ${scopes.join(" / ")}`);
  [...document.querySelectorAll<HTMLElement>('[role="listbox"] [role="option"]')].at(-1)!.click();
  await settle();
  const plain = [...document.querySelectorAll<HTMLElement>(".thread-inbox-item")].map(node => node.dataset.threadKey);
  assert(plain.join() === "claude#active-unknown,claude#settled-notes", `No project listed ${plain.join()}`);
  scope = ALL_THREAD_PROJECTS; render(); await settle();

  // Inbox under load (owner, 2026-10-07): line two says what a live thread is
  // doing, what it asks, or why it failed, in the project line's place.
  const LIVE: Record<string, ThreadLiveFacts> = {
    "claude#pin-release": { running: true, step: { tool: "Edit", summary: `${DOCS}/src/routes/login.ts`,
      input: { file_path: `${DOCS}/src/routes/login.ts` } } },
    "claude#active-waiting": { running: false, ask: { tool: "Bash", summary: "Run the tests", input: { command: "npm test" } } },
    "claude#active-failed": { running: false, error: "Rate limit reached for claude-opus" },
  };
  live = LIVE; render(); await settle();
  const lineOf = (rowKey: string): HTMLElement | null =>
    item(rowKey).querySelector<HTMLElement>(".thread-inbox-words > .thread-inbox-line");
  for (const [rowKey, kind, text] of [["claude#pin-release", "is-step", "Editing login.ts"],
    ["claude#active-waiting", "is-ask", "Run npm test?"],
    ["claude#active-failed", "is-error", "Rate limit reached for claude-opus"]] as const) {
    const line = lineOf(rowKey);
    assert(line?.textContent === text && line.classList.contains(kind),
      `${rowKey}'s second line is ${line?.outerHTML ?? "missing"}, not ${kind} "${text}"`);
    const row = rowButton(rowKey);
    assert(Math.round(row.getBoundingClientRect().height) === 52 && row.scrollWidth <= row.clientWidth + 1,
      `${rowKey} with a live line is ${row.getBoundingClientRect().height}px or overflows`);
    // The tile and the provider's mark stay; the project, branch and diff give way.
    assert(rowButton(rowKey).querySelector(":scope > .thread-inbox-tile") !== null
      && item(rowKey).querySelector(".thread-inbox-trail .provider-logo") !== null
      && item(rowKey).querySelector(".thread-inbox-project, .thread-inbox-branch, .thread-inbox-diff, .thread-inbox-pr") === null,
    `${rowKey}'s live line did not take the project line's place`);
    // Every live line reads in line two's dim tier (the slot says the status), legibly.
    const ratio = contrastRatio(painted(getComputedStyle(line).color, sheet), sheet);
    assert(painted(getComputedStyle(line).color) === painted(fgDim) && ratio >= 4.5,
      `${rowKey}'s ${kind} line is ${getComputedStyle(line).color} at ${ratio.toFixed(2)}:1`);
  }
  // The step holds still with motion on or off (its shimmer was cut).
  const step = getComputedStyle(lineOf("claude#pin-release")!);
  assert(step.animationName === "none" && step.backgroundImage === "none",
    `The step moves: ${step.animationName} / ${step.backgroundImage}`);

  // A row that comes to need you floats to the top of Active and slides
  // there; the rows it passes slide down. Under reduced motion it lands at once.
  const floats = (): Animation[] => document.getAnimations().filter(animation => animation.id === "thread-inbox-float");
  const shift = (animation: Animation): number => Number(/translateY\((-?[\d.]+)px\)/u.exec(
    String((animation.effect as KeyframeEffect).getKeyframes()[0]?.transform ?? ""))?.[1] ?? 0);
  const slideOf = (rowKey: string): Animation | undefined =>
    floats().find(animation => (animation.effect as KeyframeEffect).target === item(rowKey));
  sessions = SESSIONS.map(entry => entry.sessionId === "active-woke" ? { ...entry, status: "waiting" as const } : entry);
  render();
  assert(reduced ? floats().length === 0
    : (shift(slideOf("codex#active-woke") ?? new Animation()) > 0 && shift(slideOf("codex#active-done") ?? new Animation()) < 0
      && slideOf("claude#active-unknown") === undefined),
  `The float slid ${floats().map(animation => `${(animation.effect as KeyframeEffect).target?.getAttribute("data-thread-key")} ${shift(animation)}`).join(", ") || "nothing"}`);
  await settle();
  assert(rowKeys("active").join() === ["claude#active-waiting", "codex#active-woke", "codex#active-done",
    "claude#active-failed", "claude#active-unknown"].join(), `The floated order is ${rowKeys("active").join()}`);
  // Answered, it settles back into Rust's place.
  sessions = SESSIONS; render(); await settle();
  assert(rowKeys("active").join() === ORDER.active.join(), `Answered, Active reads ${rowKeys("active").join()}`);
  await until(() => floats().length === 0, "A float outlived its 240ms");

  // A working row that finishes unread: the dot pops in and line two fades
  // from the step back to the project line. A dot already there on first paint does not pop.
  sessions = SESSIONS.map(entry => entry.sessionId === "pin-release" ? { ...entry, status: "idle" as const } : entry);
  view = { ...VIEW, threads: VIEW.threads.map(record => record.key === "claude#pin-release"
    ? { ...record, unread: true, lastRun: { startedAt: NOW - 161_000, endedAt: NOW, outcome: "completed" as const } } : record) };
  live = { ...LIVE, "claude#pin-release": { running: false } };
  render(); await settle();
  const dot = item("claude#pin-release").querySelector<HTMLElement>(".thread-inbox-unread");
  assert(dot !== null && dot.classList.contains("is-arriving")
    && getComputedStyle(dot).animationName === (reduced ? "none" : "thread-inbox-pop"),
  `The finished row's dot did not pop in: ${dot?.outerHTML ?? "no dot"} ${dot === null ? "" : getComputedStyle(dot).animationName}`);
  assert(!item("codex#active-done").querySelector(".thread-inbox-unread")!.classList.contains("is-arriving"),
    "An unread dot popped on its first paint");
  const leaving = item("claude#pin-release").querySelector<HTMLElement>(".thread-inbox-leaving");
  assert(item("claude#pin-release").querySelector(".thread-inbox-words > .thread-inbox-project")?.textContent === "docs"
    && (reduced ? leaving === null : leaving?.textContent === "Editing login.ts"
      && getComputedStyle(leaving).animationName === "thread-inbox-fade-out"),
  `Line two did not fade back to the project line: ${item("claude#pin-release").querySelector(".thread-inbox-meta")?.outerHTML}`);
  await until(() => item("claude#pin-release").querySelector(".thread-inbox-leaving") === null, "The step never left");
  sessions = SESSIONS; view = VIEW; live = LIVE; render(); await settle();
  await wait(0);

  // Protocol 56: a search lists threads whose messages match under "In
  // messages", after Active, as ordinary rows with the excerpt where the
  // project was; a thread whose row already matched is not listed twice.
  query = "zeppelin";
  messageHits = [
    { key: "codex#settled-old", seq: 9, turn: 1, role: "message", excerpt: "…the Zeppelin cache was cold", at: NOW },
    { key: "codex#settled-old", seq: 4, role: "prompt", excerpt: "an older Zeppelin match", at: NOW },
    { key: "claude#active-failed", seq: 2, role: "prompt", excerpt: "Port the Zeppelin importer", at: NOW },
  ];
  sessions = SESSIONS.map(session => session.sessionId === "active-failed"
    ? { ...session, title: "Port the Zeppelin importer" } : session);
  render(); await settle();
  const foundSection = document.querySelector<HTMLElement>("section[data-thread-found]");
  assert(foundSection !== null
    && foundSection.querySelector(".thread-inbox-label")?.textContent === IN_MESSAGES_LABEL
    && foundSection.querySelector(".thread-inbox-count")?.textContent === "1",
  `No "In messages" caption: ${foundSection?.outerHTML.slice(0, 300)}`);
  // Its caption reads like Active's: the count at the right, inside the list.
  const foundHead = foundSection.querySelector<HTMLElement>(".thread-inbox-found-head")!.getBoundingClientRect();
  const foundCount = foundSection.querySelector<HTMLElement>(".thread-inbox-count")!.getBoundingClientRect();
  const listBox = document.querySelector<HTMLElement>(".thread-inbox")!;
  assert(foundHead.right <= listBox.getBoundingClientRect().right + 0.5 && listBox.scrollWidth <= listBox.clientWidth
    && Math.abs(foundCount.right - (foundHead.right - 12)) <= 1,
  `The In messages caption overflows: ${JSON.stringify({ foundHead, foundCount, scroll: listBox.scrollWidth, client: listBox.clientWidth })}`);
  const foundKeys = [...foundSection.querySelectorAll<HTMLElement>(".thread-inbox-item")].map(found => found.dataset.threadKey);
  assert(foundKeys.join() === "codex#settled-old", `Wrong threads in messages: ${foundKeys.join()}`);
  assert(rowKeys("active").join() === "claude#active-failed", `The title match moved: ${rowKeys("active").join()}`);
  const foundWords = foundSection.querySelector<HTMLElement>(".thread-inbox-words > .thread-inbox-project");
  assert(foundWords?.textContent === "…the Zeppelin cache was cold"
    && foundSection.querySelector(".thread-inbox-title")?.textContent === "Old spike",
  `The match is not drawn in the project's place: ${foundSection.querySelector(".thread-inbox-meta")?.outerHTML}`);
  const activeRowHeight = document.querySelector('section[data-thread-section="active"] .thread-inbox-item')!
    .getBoundingClientRect().height;
  const foundRowHeight = foundSection.querySelector(".thread-inbox-item")!.getBoundingClientRect().height;
  assert(Math.abs(foundRowHeight - activeRowHeight) < 1,
    `A thread found in messages is ${foundRowHeight}px tall, an Active row ${activeRowHeight}px`);
  const activeSection = document.querySelector('section[data-thread-section="active"]')!.getBoundingClientRect();
  assert(foundSection.getBoundingClientRect().top >= activeSection.bottom, "In messages is not under Active");
  // Two characters do not search messages.
  query = "ze"; render(); await settle();
  assert(document.querySelector("section[data-thread-found]") === null, "Two characters searched messages");
  query = ""; messageHits = undefined; sessions = SESSIONS; render(); await settle();

  // The resting picture for the screenshot: every section open, nothing armed.
  if (document.querySelector(".thread-inbox-toast") !== null) {
    document.querySelector<HTMLButtonElement>(".thread-inbox-toast-undo")!.click();
  }
  selected = "codex#active-done";
  flushSync(() => root.render(<div />));
  localStorage.setItem(THREAD_FOLDS_KEY, JSON.stringify({ pinned: false, active: false, snoozed: false, settled: false }));
  render(); await settle();
  for (const animation of document.getAnimations()) if (animation instanceof CSSTransition) animation.finish();
  await settle();
  return { theme, chrome, width, sections: 4, marks: Object.keys(MARKS).length, commands: sent.length,
    undo: true, drag: true, chords: true, scope: true, shelves: true, actions: true, live: true, float: true,
    inMessages: true,
    motion: reduced ? "reduce" : "no-preference" };
}
