import { readFileSync } from "node:fs";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { EngineId, SessionSummary } from "../shared/engine";
import type { EngineEvent } from "../shared/engine";
import { updateButtonLabel, type UpdateState } from "../shared/updater";
import { SurfaceSwitch } from "./components/SurfaceSwitch";
import { AgentTile } from "./components/AgentTile";
import { Sidebar } from "./components/Sidebar";
import { SubagentPanel } from "./components/SubagentPanel";
import { nextCatalogueRevision, sessionConfigFor, liveTurnProgress } from "./app-session-projection";
import { conversationItems } from "./conversation-reducer";
import { findSubagentItem } from "./tool-activity";
import { attentionSessions, buildSidebarView } from "./viewmodels";

const session: SessionSummary = {
  engine: "codex",
  sessionId: "tile-1",
  title: "Repair parser",
  cwd: "/work/parser",
  status: "waiting",
  cloud: false,
  lastActivity: "2026-08-13T12:00:00.000Z",
  firstSeen: "2026-08-13T11:00:00.000Z",
  archived: false,
};

describe("AgentTile", () => {
  it("names the owning provider, status, and persistent tile controls", () => {
    const html = renderToStaticMarkup(createElement(AgentTile, {
      session,
      selected: true,
      maximized: false,
      live: true,
      engineLabel: "OpenAI Codex",
      onFocus: () => undefined,
      onToggleMaximize: () => undefined,
      onClose: () => undefined,
      children: createElement("p", null, "Independent conversation"),
    }));
    expect(html).toContain('data-agent-key="codex#tile-1"');
    expect(html).toContain('aria-label="Repair parser — OpenAI Codex — Needs attention"');
    // The mark is the composer's now (Threads redesign, 2026-10-06).
    expect(html).not.toContain('data-provider-logo="codex"');
    expect(html).toContain('class="workspace-tile-name">Repair parser<');
    expect(html).not.toContain("workspace-tile-status");
    expect(html).toContain("workspace-agent is-selected");
    // Maximize and close live in the pane's ⋯ menu (AgentTile.test.ts).
    expect(html).toContain('aria-label="Repair parser pane actions"');
    expect(html).not.toContain("window.confirm");
    expect(html).toContain("Independent conversation");
  });

  it("exposes restore rather than a second maximize while full screen", () => {
    const html = renderToStaticMarkup(createElement(AgentTile, {
      session,
      selected: true,
      maximized: true,
      live: false,
      engineLabel: "OpenAI Codex",
      onFocus: () => undefined,
      onToggleMaximize: () => undefined,
      onClose: () => undefined,
      children: null,
    }));
    expect(html).toContain("workspace-agent is-selected is-maximized");
    expect(html).toContain('aria-label="Repair parser pane actions"');
  });
});

/* Reachability, not presence. Everything below renders the REAL component with
   the REAL selector output the shell hands it, because the failure mode this
   round exists to catch is a fix that lives in a file and never reaches a
   screen — a prop declared and never passed, a count computed and never shown. */
describe("what the user actually sees when agents are blocked", () => {
  const noop = (): void => undefined;
  const summary = (engine: EngineId, id: string,
      over: Partial<SessionSummary> = {}): SessionSummary => ({
    engine, sessionId: id, title: id, cwd: `/work/${id}`, status: "idle",
    cloud: false, lastActivity: "2026-08-14T00:00:00.000Z",
    firstSeen: "2026-08-13T00:00:00.000Z", archived: false, ...over,
  });

  // Three agents blocked on the user, and a sidebar search that matches none of
  // them — the exact state in which the pill and its destination used to
  // disagree.
  const listing = [
    summary("claude", "alpha", { status: "waiting", title: "alpha rewrite" }),
    summary("codex", "beta", { status: "waiting", title: "beta rewrite" }),
    summary("claude", "gamma", { status: "waiting", title: "gamma rewrite" }),
    summary("codex", "ledger", { status: "idle", title: "ledger notes" }),
  ];

  it("counts the blocked agents off the listing, never off the sidebar view", () => {
    // The defect: the sidebar search dropped every waiting row from
    // `view.visible`, and a count taken from that read zero under a toolbar
    // that had three to report. The pill counts the LISTING, so no query can
    // quieten it.
    const view = buildSidebarView(listing, null, new Set(), false, false, "ledger");
    expect(view.visible.map(s => s.sessionId)).toEqual(["ledger"]);
    expect(attentionSessions(listing).map(s => s.sessionId))
      .toEqual(["alpha", "beta", "gamma"]);
    expect(attentionSessions(listing)).toHaveLength(3);
  });

  it("draws row-shaped skeletons instead of claiming an empty store", () => {
    // M7 shipped dormant: `loading` was declared on Sidebar and never passed,
    // so a cold start printed "No agent sessions yet." over a store nobody had
    // read. This renders the pane the way App renders it in that state.
    const props = {
      groups: [], projectCount: 0, health: [], rateLimits: {},
      hidden: { external: 0, trivial: 0 }, hideable: 0,
      onSelect: noop, onNew: noop, onNewInProject: noop, onArchive: noop,
      onToggleFilters: noop,
    };
    const cold = renderToStaticMarkup(createElement(Sidebar, { ...props, loading: true }));
    expect(cold).toContain('class="session-skeleton"');
    expect(cold).toContain('aria-label="Loading sessions"');
    expect(cold).not.toContain("No agent sessions yet.");
    // Once the listing lands the sentence is the honest one again.
    const warm = renderToStaticMarkup(createElement(Sidebar, { ...props, loading: false }));
    expect(warm).not.toContain('class="session-skeleton"');
  });

  it("actually passes that loading state from the shell", () => {
    // The half the render test above cannot see, and the half that was missing:
    // the skeletons were reachable code inside Sidebar and unreachable in the
    // app, because App declared no `loading` at its only <Sidebar> call site.
    const app = readFileSync(new URL("./App.tsx", import.meta.url), "utf8");
    const call = app.match(/<Sidebar\n[\s\S]*?\n {6}\/>/)?.[0] ?? "";
    expect(call).not.toBe("");
    expect(call).toContain("loading={sidebarLoading}");
    // And that flag is the first-read fact, with an exit for a read that fails
    // — skeletons pulsing forever would be its own untrue claim.
    // Rows the host paints from its cached registry before the first listing
    // answers end the skeleton early; an empty cache keeps it.
    expect(app).toContain(
      "const sidebarLoading = !registryReady && !firstListingSettled && state.sessions.length === 0;");
    // That cached push can be empty or partial, so it never counts as the
    // answer that prunes saved layouts: only the first listing response does,
    // and registry pushes count only after it.
    const onRegistry = app.slice(app.indexOf("window.iface.onRegistry("),
      app.indexOf("window.iface.onEvent("));
    expect(onRegistry).toContain("if (firstListingAnswered.current) setRegistryReady(true);");
    expect(onRegistry).not.toMatch(/^\s*setRegistryReady\(true\);/m);
    const listing = app.slice(app.indexOf("window.iface.listSessionSnapshot()"),
      app.indexOf("window.iface.probeEngines("));
    expect(listing.match(/firstListingAnswered\.current = true;/g)).toHaveLength(2);
    // LATCHED, not read live off the dismissible error. Reading `refreshError`
    // directly meant dismissing it flipped the pane back into a loading state
    // it could never leave, because `registryReady` stays false after a failed
    // first read. The latch is what makes "settled" survive the dismissal.
    expect(app).toContain(
      "if (registryReady || refreshError !== null) setFirstListingSettled(true);");
    expect(app).not.toContain(
      "const sidebarLoading = !registryReady && refreshError === null;");
  });
});

describe("the agent-first shell contract", () => {
  const app = readFileSync(new URL("./App.tsx", import.meta.url), "utf8");
  const css = readFileSync(new URL("./theme.css", import.meta.url), "utf8");
  const codicons = readFileSync(new URL("./codicons.css", import.meta.url), "utf8");
  const main = readFileSync(new URL("../main/main.ts", import.meta.url), "utf8");
  const appearance = readFileSync(new URL("./appearance.ts", import.meta.url), "utf8");
  const appearanceContract = readFileSync(new URL("../shared/appearance.ts", import.meta.url), "utf8");
  const store = readFileSync(new URL("./store.ts", import.meta.url), "utf8");
  const quickOpenHook = readFileSync(new URL("./use-quick-open.ts", import.meta.url), "utf8");
  // The view models: the viewmodels.ts barrel and its four rule-set modules.
  const viewmodels = ["viewmodels", "sidebar-view", "session-activity", "session-config-view",
    "usage-view"].map(name => readFileSync(
    new URL(`./${name}.ts`, import.meta.url), "utf8")).join("\n");
  // The conversation pane: its shell, both memoized halves and the rows they draw.
  const conversation = ["Conversation", "ConversationTranscript", "ConversationComposer",
    "ConversationRows"].map(name => readFileSync(
    new URL(`./components/${name}.tsx`, import.meta.url), "utf8")).join("\n");
  const usage = readFileSync(new URL("./use-plan-usage.ts", import.meta.url), "utf8");
  const voice = readFileSync(new URL("./use-voice-assistant.ts", import.meta.url), "utf8");
  const library = readFileSync(new URL("./use-session-library.ts", import.meta.url), "utf8");
  // The shell's palette commands (labels, disabled states and reasons).
  const palette = readFileSync(new URL("./palette-commands.ts", import.meta.url), "utf8");

  it("carries the blocked-on-you count in the always-visible toolbar, as the attention tray", () => {
    // H1. The strong attention treatment used to live only inside a surface
    // you navigate TO, so agents could sit blocked while the user worked in a
    // conversation with no in-window signal at all. The tray is that signal:
    // a bell with the count, and behind it the list of what and where.
    expect(app).toContain("attentionSessions(state.sessions)");
    expect(app).toContain("const waitingSessions = attentionListing.length;");
    expect(app).toContain("<AttentionTray");
    expect(app).toContain("rows={attentionRowList}");
    expect(app).toContain("unread={attentionUnread}");
    // The live rows are built from THE attention selection, plus the panes
    // ringing for input — the one place terminals join the count.
    expect(app).toMatch(/const attentionNeeds = useMemo<LiveNeed\[\]>\(\(\) => \[\s*\.\.\.attentionListing\.map/);
    expect(app).toContain('terminal.activity === "waiting"');
    // Finished and failed turns are recorded at the edge that already chimes,
    // before the OS notification, so a denied permission cannot silence the tray.
    expect(app).toContain("if (track !== undefined) { setAttention(current => recordEvent(current, track)); persistAttention(track); }");
    expect(app).toMatch(/kind: status === "failed" \? "failed" : "finished",/);
    // The old pill and its parallel wording are gone.
    expect(app).not.toContain("workspace-toolbar-attention");
    expect(app).not.toContain("attentionPillWords(");
    expect(app).not.toContain("${attentionAnnouncement(waitingSessions)}. Show them.");
  });

  it("clears the locally recorded events when there is no harness to resolve", () => {
    // A failed first harness read leaves harnessView null while recorded
    // events still show the tray's Clear all; that button must not be inert.
    expect(app).toContain("if (harnessView === null) { setAttention(clearEvents); return; }");
  });

  it("keeps ONE attention derivation, so no two surfaces can disagree", () => {
    // The defect this replaces: the pill counted `state.sessions` while its
    // destination was built from `view.visible`, so with a search active the
    // toolbar read "3 need you" over a surface reading "Nothing needs you".
    //
    // One selection now feeds the count, the announcement and the navigation.
    // `attentionSessions` is called exactly once in App.
    expect(app.match(/attentionSessions\(/g)).toHaveLength(1);
    // No second count survives in the shell: the old parallel helper is gone
    // and the number the pill prints is assigned in exactly one place.
    expect(app).not.toContain("waitingSessionCount");
    expect(app.match(/const waitingSessions =/g)).toHaveLength(1);
    // And the one predicate is written down exactly once in the view-model.
    expect(viewmodels.match(/export function needsAttention/g)).toHaveLength(1);
    expect(viewmodels).toContain('return session.status === "waiting";');
    // Including the per-project count in the sidebar's group headers: one
    // predicate, so a project header cannot call a session blocked that the
    // toolbar does not.
    expect(viewmodels).toContain("if (needsAttention(s)) bucket.counts.waiting++;");
    // One executable occurrence survives in the whole view-model, and this
    // fails if a second is introduced. Comment lines are stripped so prose
    // about the rule does not count as a restatement of it.
    const code = viewmodels.split("\n")
      .filter(line => !/^\s*(\*|\/\/|\/\*)/.test(line)).join("\n");
    expect(code.match(/status === "waiting"/g)).toHaveLength(1);
  });

  it("sends a tray row to the exact agent or pane it names", () => {
    // A row is a place. A session row selects that session — clearing the
    // search first, because the sidebar is the one filter that can drop a row
    // the user has not archived, and the destination has to be visible. A pane
    // row shows the terminals and asks the centre to bring that pane forward.
    // Opening records viewing; explicit resolution is separate, and generation must match.
    const activate = app.match(
      /const activateAttention = useCallback\([\s\S]*?\n  \}, \[selectSession, showTerminals, terminalSummaries, harnessRequest\]\);/)?.[0] ?? "";
    expect(activate).toContain('kind: "markSeen"');
    expect(activate).not.toContain("dismissEvent");
    expect(activate).toContain("terminal.generation !== row.target.generation");
    expect(activate).toContain('setQuery("");');
    expect(activate).toContain("selectSession(row.target.key);");
    expect(activate).toContain("showTerminals();");
    expect(activate).toContain("terminalCenter.current?.focusTerminal(terminalId);");
    expect(app).not.toContain("showAttention");
    expect(app).not.toContain("setBoardEpoch");
    expect(app).not.toContain("key={`board-");
  });

  it("announces the attention count from one region, on the transition alone", () => {
    // Announced from OUTSIDE <main>: the shell takes aria-hidden whenever a
    // palette or dialog is open, and an alarm silenced by the command palette
    // is not an alarm.
    const opening = '<div className="terminal-sr-only" role="status" aria-live="polite">';
    expect(app).toContain(opening);
    expect(app.indexOf(opening))
      .toBeLessThan(app.indexOf('<main className="workspace-shell"'));
    // The content is a function of the COUNT and of nothing else. It used to
    // blank itself on the destination that listed the same agents, which cost
    // it the 0 -> 1 announcement in that view and made mere navigation away
    // fire the alarm with nothing changed.
    const region = app.match(
      /<div className="terminal-sr-only" role="status" aria-live="polite">[\s\S]*?<\/div>/)?.[0] ?? "";
    expect(region).toContain("{attentionAnnouncement(waitingSessions)}");
    expect(region).not.toContain("centerView");
    expect(region).not.toContain("settingsOpen");
  });

  it("hides the live region with the shared visually-hidden rule", () => {
    // The pill's amber rules went with the pill: the bell is the toolbar's own
    // neutral icon button and the state colours live on the rows inside the
    // tray (AttentionTray.tsx, one amber site on the allowlist). What was left
    // was one visually-hidden rule, which terminal.css already defines.
    expect(app).not.toContain("ATTENTION_CSS");
    const attentionCss = readFileSync(new URL("./terminal.css", import.meta.url), "utf8")
      .match(/(?:^|\n)\.terminal-sr-only \{[^}]*\}/u)?.[0] ?? "";
    expect(attentionCss).toContain("position: absolute;");
    expect(attentionCss).not.toContain(".workspace-toolbar-attention");
    expect(attentionCss).not.toContain("var(--status-waiting)");
    // No colour literal anywhere in it, and no hex that would dodge the theme.
    expect(attentionCss).not.toMatch(/#[0-9a-fA-F]{3,8}\b/);
  });

  it("keeps a usage number on the always-on strip instead of blanking it", () => {
    // M1. Five minutes was shorter than the fastest window being measured by a
    // factor of sixty, and nothing re-read, so the meter was three provider
    // words in five of eight captured frames.
    expect(viewmodels).toContain("export const USAGE_STALE_MS = 5 * MINUTE_MS;");
    expect(viewmodels).toContain('`${stale ? "~" : ""}${Math.round(percent)}%`');
    // One catch-up read on the tick the snapshot crosses the line — measured
    // against the strip's own clock, so the crossing is observable at all, and
    // no longer gated on the menu being open. A finished turn also kicks a
    // coalesced re-read, because that is when quota actually moves; its timing
    // (a minute's floor, Refresh never floored) is use-plan-usage.test.ts.
    expect(usage).toContain("const usageStale = usageFreshness(usage.readAt, now).stale;");
    expect(usage).toContain('if (state.status === "ready" && !state.stale) return "none";');
    expect(usage).not.toContain('if (state.status === "ready" && !state.open) return "none";');
    expect(usage).toContain("usageCatchUp({ status: usage.status, stale: usageStale, open: usageOpen, hidden });");
    expect(usage).toContain("}, [usageOpen, usage.status, usageStale, hidden, refreshUsage]);");
    expect(usage).toContain("const noteUsageTurnDone = useCallback(() => { reads.turnDone(); }, [reads]);");
    expect(app).toContain("noteUsageTurnDone();");
    // Still not a poll: the only interval in this area advances the clock.
    expect(usage).toContain("const id = setInterval(() => { setNow(Date.now()); }, USAGE_TICK_MS);");
  });

  it("mounts one real Conversation per explicit workspace pane", () => {
    expect(app).toContain("gridKeys.map(key =>");
    expect(app).toContain('surface !== "agent"');
    expect(app).toMatch(/<AgentTile[\s\S]*?\{tileConversation\(key, summary,[\s\S]*?<\/AgentTile>/);
    expect(app).toMatch(/function tileConversation\([\s\S]*?<SessionTile/);
    expect(readFileSync(new URL("./components/SessionTile.tsx", import.meta.url), "utf8"))
      .toMatch(/const SessionTile = memo\(function SessionTile\([\s\S]*?<ProjectedConversation/);
    expect(app).toContain('const WORKSPACE_STATE = "interface.workspace.v2"');
    expect(app).toContain("JSON.stringify(workspaces[kind])");
    // Chat and Agent keep their keys; Code's slot and its v1 source are swept.
    expect(app).toContain("return `${WORKSPACE_STATE}.${kind}`");
    expect(app).toContain('const RETIRED_WORKSPACE_STATES = [WORKSPACE_STATE, "interface.workspace.v1"]');
    expect(app).not.toContain("loadInitialWorkspaceState");
    expect(app).not.toContain("LEGACY_WORKSPACE_STATE");
    // Chat or Agent opens on its launch sheet only when its OWN layout is empty.
    expect(app).toContain(
      "parseWorkspaceState(readStored(workspaceStorageKey(stored))).openKeys.length === 0");
  });

  it("uses a flexible two-column shell instead of the legacy fixed chat rail", () => {
    expect(css).toContain("grid-template-columns: var(--pane-left) minmax(0, 1fr)");
    expect(css).not.toContain("1fr var(--pane-right)");
    expect(app).not.toContain('className="activitybar"');
    expect(css).toContain("workspace-count-4");
    expect(css).toContain("container: workspace / inline-size");
    // Nothing queries the shell by name, and its only child is itself an
    // inline-size container, so the shell declares no container of its own.
    expect(css).not.toContain("container: workspace-shell");
    expect(css).toContain("@container workspace (max-width: 719px)");
    // A single narrow pane fills the area; only several stack and scroll.
    expect(css).toContain(".workspace-grid:not(.has-maximized):not(.workspace-count-1) > .workspace-tile");
    expect(css).toContain("@container workspace (min-width: 1140px)");
    expect(css).toContain("@media (max-width: 1080px)");
    expect(css).not.toContain("@container workspace-shell");
    expect(css).toContain(".workspace-grid.workspace-count-3 > .workspace-tile:last-child");
    expect(css).not.toContain(
      ".workspace-grid.workspace-count-1 > .workspace-tile.workspace-agent > .workspace-tile-head");
    // The grid is now spaced panes rather than hairline-separated cells.
    expect(css).toContain("--tile-radius: 12px");
    expect(css).toContain("--tile-gap: 12px");
    expect(css).toContain("--tile-head-h: 30px");
    expect(css).toMatch(/\.workspace-tile \{[\s\S]*?border-radius: var\(--tile-radius\)/);
  });

  it("blocks direct and queued delivery until provider configuration settles", () => {
    // The lane's refusals (settings pending, provider commands in the queue)
    // are exercised in outbound-lane.test.ts; this pins App's half: the lane
    // reads the in-flight config tokens, and the tile locks follow it.
    // ConfigAttempts itself is exercised in session-config-tokens.test.ts.
    expect(app).toContain("configPending: key => configTokens.pending(key),");
    expect(app).toContain("const tileConfigPending = configTokens.pending(key);");
    expect(app).toContain("sendLocked={tileConfigPending}");
    expect(app).toContain("queueControlsLocked={tileOutboundPending || tileConfigPending}");
    expect(app).toContain("queueWhileBusy={tileProgress.active || tileOutboundPending}");
    expect(app).not.toContain('summary.status === "working" || summary.status === "waiting"');
  });

  it("does not treat an ACP engine without a store listing as a refresh failure", () => {
    expect(app).toContain("isProviderListingFailure");
    expect(app).toContain("if (!isProviderListingFailure(provider)) continue;");
  });

  it("re-reads sessions on focus but probes providers only on mount and Refresh", () => {
    // A probe launches every provider's CLI, so returning to the window skips it.
    expect(app).toContain('const onFocus = (): void => { void refreshAll("focus"); };');
    expect(app).toMatch(
      /if \(document\.visibilityState === "visible"\) \{\s*void refreshAll\("focus"\);/u);
    expect(app).toContain('case "workspace.refresh": void refreshAll("explicit"); break;');
    expect(app).toContain('onRefreshProviders={() => { void refreshAll("explicit"); }}');
    expect(app).toContain(
      "const REFRESH_RANK: Record<RefreshKind, number> = { focus: 0, full: 1, explicit: 2 };");
    const refresh = app.slice(app.indexOf("const refreshAll = useCallback("),
      app.indexOf("const freezeHistoryFor = useCallback("));
    // An explicit Refresh first finds a CLI installed since start (protocol 41).
    expect(refresh).toMatch(
      /kind === "focus"\s*\? undefined\s*: window\.iface\.probeEngines\(kind === "explicit" \? \{ refresh: true \} : undefined\)/u);
    // A running refresh never swallows a later one that asks for more.
    expect(refresh).toContain(
      "if (running !== null && REFRESH_RANK[running.kind] >= REFRESH_RANK[kind]) return running.run;");
    // The host's probe restarts the listings it keeps (Codex, for a minute):
    // it is asked first, and an explicit Refresh lists again after it and
    // reports that listing.
    expect(refresh.indexOf("window.iface.probeEngines("))
      .toBeLessThan(refresh.indexOf("const listing = listSessions();"));
    expect(refresh).toContain(
      'kind === "explicit" ? Promise.all([listing, probe]).then(listSessions) : listing,');
    expect(refresh).toContain(".then(() => { listingFailures = found; });");
  });

  it("never releases a durable pause before a Send now is known deliverable", () => {
    // The data-loss fix: Send now must NOT commit a resume (un-pause) up front
    // and then let deliverQueued refuse; the premature resumeSteeringPrompt
    // call is gone entirely. outbound-lane.test.ts drives a Send now on a
    // paused row of a session that is not live and checks it stays paused.
    expect(app).not.toContain("resumeSteeringPrompt");
    expect(readFileSync(new URL("./outbound-lane.ts", import.meta.url), "utf8"))
      .not.toContain("resumeSteeringPrompt");
    expect(app).toContain("sendQueuedNow: (key, id) => { outbound.sendQueuedNow(key, id); },");
  });

  it("keeps a rejected queue ADD advisory so accepted prompts still drain", () => {
    // Finding 2: a full/invalid/provider-command add leaves the existing queue
    // untouched, so it goes to the advisory channel, never the blocking one that
    // feeds the auto-delivery hasError gate (outbound-lane.test.ts drives both).
    // The tile shows the blocking error first.
    expect(app).toContain("queueError={queueErrors[key] ?? queueNotices[key] ?? null}");
    expect(app).toContain(
      "outbound.autoDeliver(steeringQueues, queueErrors, key => sessionByKey.get(key)?.status);");
  });

  it("durably pauses the queue when a turn ends badly or the session dies", () => {
    // Findings 3 & 4: a failed/non-retryable turn or a provider-initiated
    // session.closed must arm the same durable pause a user's Stop does
    // (outbound-lane.test.ts). App's part is forwarding every event to the lane.
    const onEvent = app.slice(app.indexOf("const offEvent = window.iface.onEvent((key, evt) => {"),
      app.indexOf("const offWorktree = window.iface.onWorktree("));
    expect(onEvent).toContain("outbound.observe(key, evt);");
    expect(app).toContain("notifyTurnDone, outbound, refreshAll]);");
  });

  it("echoes the launch prompt into the new chat", () => {
    // Live engines never emit user.message, and the born-here marker skips history, so
    // the first prompt would otherwise exist only as the session title.
    // Anchored on the dependency array's brackets and not on what is inside
    // them: this test is about what the callback DOES, and pinning its deps
    // made every new one look like a broken echo.
    const launch = app.match(
      /const launch = useCallback\([\s\S]*?\n  }, \[[\s\S]*?\]\);/)?.[0] ?? "";
    expect(launch).toContain("historyPaging.current.markStartedHere(key)");
    expect(launch).toContain("appendConversationEcho(key, launchPendingEcho(");
    expect(launch).toContain("opts.prompt");
    expect(launch).toContain("promptTurnImages(validatedContext)");
  });

  it("starts history at the shared page size and expands only on demand", () => {
    expect(app).toContain("limit = HISTORY_LIMIT");
    expect(app).toContain("window.iface.readHistory(key, { limit })");
    expect(app).toContain("const loadOlderHistoryFor");
    expect(readFileSync(new URL("./history-paging.ts", import.meta.url), "utf8"))
      .toContain("Math.min(HISTORY_MAX_LIMIT, current + HISTORY_LIMIT)");
  });

  it("keeps provider usage as an always-visible toolbar menu, not a workspace page", () => {
    expect(app).toContain("<UsageMenu");
    expect(app).toContain("engines={usageEngines}");
    expect(app).toContain('engine.id !== "acp:qwen"');
    expect(app).not.toContain(
      'type CenterView = "sessions" | "usage"');
    expect(app).toContain(
      'type CenterView = "sessions" | "canvas" | "terminal"');
    expect(css).toContain(".usage-menu-trigger");
    expect(css).toContain(".usage-menu-label");
    expect(app).toMatch(
      /<header className="workspace-toolbar"[\s\S]*<UsageMenu[\s\S]*<footer className="statusbar"/);
    expect(app).not.toMatch(/<footer className="statusbar"[\s\S]*<UsageMenu/);
  });

  it("shows the update pill beside the terminal controls only while an update exists", () => {
    // Mounted after the terminal slot, before Voice, and gated on the label
    // helper, which is null for every phase except available/downloading/ready.
    // Usage opens from the ··· menu now (owner, 2026-09-26): its hanger follows ···.
    expect(app).toMatch(
      /workspace-toolbar-terminal[\s\S]*\{updateLabel !== null && updateState !== null &&[\s\S]*<UpdateButton[\s\S]*aria-label="Voice assistant"/);
    expect(app).toMatch(/<WorkspaceMoreMenu[\s\S]*<UsageMenu\s+anchorRef=\{commandPaletteButtonRef\}/);
    expect(app).toMatch(/<AttentionTray\s+anchorRef=\{commandPaletteButtonRef\}/);
    // Ready restarts through the existing confirm; anything earlier opens Settings.
    expect(app).toContain(
      'onClick={updateState.phase === "ready" ? requestRestartToUpdate : openSettings} />');
    expect(app).toContain('disabled={updateState.phase === "ready" && updateRestartPending}');
    const idle: UpdateState = {
      phase: "idle", source: "local", currentVersion: "0.3.51", availableVersion: null,
      progress: null, lastCheckedAt: null, error: null, canCheck: true, canRestart: false,
    };
    expect(updateButtonLabel(null)).toBeNull();
    expect(updateButtonLabel(idle)).toBeNull();
    expect(updateButtonLabel({ ...idle, phase: "up-to-date" })).toBeNull();
    expect(updateButtonLabel({ ...idle, phase: "available", availableVersion: "0.3.52" })).toBe("Update available");
    expect(updateButtonLabel({ ...idle, phase: "downloading", progress: 41.6 })).toBe("Downloading 42%");
    expect(updateButtonLabel({ ...idle, phase: "ready", canRestart: true })).toBe("Restart to update");
  });

  it("uses integrated macOS chrome with a centered surface switch and Code destinations", () => {
    expect(main).toContain('titleBarStyle: "hiddenInset"');
    expect(main).toContain("trafficLightPosition");
    expect(css).toContain("--pane-left: 240px");
    expect(css).toMatch(/\.workspace-toolbar\s*\{[\s\S]*?-webkit-app-region: drag/);
    expect(css).toMatch(/\.workspace-composer-panel\s*\{[\s\S]*?inset: 0/);
    expect(app).toContain('className="workspace-toolbar-traffic"');
    expect(app).toContain('className="workspace-brand"');
    expect(app).toContain('<SurfaceSwitch surface={surface} onChange={showSurface} />');
    const navigation = renderToStaticMarkup(createElement(SurfaceSwitch, { surface: "code", onChange: () => {} }));
    // Threads keeps the internal "chat" surface id (owner, 2026-10-06).
    expect([...navigation.matchAll(/aria-label="(Agent|Code|Threads)"/g)].map(match => match[1])).toEqual(["Agent", "Code", "Threads"]);
    expect(navigation).toContain('aria-pressed="true" aria-label="Code"');
    expect(navigation).not.toContain("codicon");
    expect(css).toMatch(/\.surface-switch-tab\s*\{[\s\S]*?font-weight: var\(--wght-strong\);/);
    // Porcelain (owner, 2026-09-26): a flat recessed track, one raised tab.
    expect(css).toMatch(/\.surface-switch\s*\{[^}]*background: color-mix\(in srgb, var\(--fg\) 6%, transparent\);/);
    expect(css).not.toMatch(/\.surface-switch\s*\{[^}]*box-shadow/);
    expect(css).not.toMatch(/\.surface-switch-tab\s*\{[^}]*text-shadow/);
    expect(app).not.toContain('className="workspace-toolbar-destinations"');
    expect(app).not.toContain('className="workspace-toolbar-label">Chats</span>');
    // Code is the terminal surface. The canvas stays on the command palette;
    // the toolbar does not repeat it as chrome.
    expect(app).toContain('className="workspace-toolbar-actions"');
    expect(app).not.toContain('aria-label="Open Project Plans"');
    expect(app).not.toContain('aria-label="Open integrated terminals"');
    expect(app).toContain('className={surface === "code" && centerView === "canvas"');
    expect(app).toContain('className={surface === "code" && centerView === "terminal"');
    expect(app).not.toContain('className="workspace-toolbar-label">Terminal</span>');
    expect(app).toContain('className="workspace-toolbar-leading"');
    expect(app).toContain("<WorkspaceMoreMenu");
    expect(css).toContain("grid-template-columns: minmax(0, 1fr) auto minmax(0, 1fr)");
    expect(app).toContain("window.dispatchEvent(new Event(APP_SELECT_DISMISS_EVENT))");
    expect(app).toContain("<WorkspaceMoreMenu buttonRef={commandPaletteButtonRef}");
    expect(app).not.toContain("workspace-toolbar-files");
    expect(app).not.toContain("workspace-toolbar-new");
    expect(app).not.toContain("workspace-toolbar-secondary");
    // The agent overview has its own toggle at the rail's right edge again
    // (owner, 2026-10-01, closer to the reference); ··· keeps its row too.
    expect(app).toContain('className="workspace-icon-btn workspace-overview-toggle"');
    expect(app).toContain('aria-label="Live panes" aria-pressed={terminalMenu.overview.open}');
    // ONE attention indicator, in the toolbar. The statusbar's robot-glyph
    // twin counted a different population for the same destination and
    // appeared only when the pill vanished, so the indicator seemed to move.
    expect(app).not.toContain("backgroundTaskCount");
    expect(app).not.toContain("statusbar-count");
    expect(app).not.toContain("workspace-toolbar-ide");
    expect(app).not.toContain("Agent Workspace</span>");
    expect(store).toContain("engineHealth: [], composerOpen: true");
    expect(app).toContain("const composerOpenerRef = useRef<HTMLElement | null>(null)");
    expect(app).toContain("dismissComposer(true)");
    expect(app).toContain("dismissComposer(false)");
    expect(app).toContain("inert={state.composerOpen || settingsOpen ? true : undefined}");
    expect(app).toContain("aria-hidden={state.composerOpen || settingsOpen || undefined}");
    expect(app).toContain("if (!paletteCommandKeepsComposer(command.id)) dismissComposer(false);");
    expect(palette).toContain('"workspace.new", "workspace.sidebar", "workspace.refresh", "workspace.settings",');
  });

  // The assistant is toolbar chrome, not a surface: one toggle beside the
  // utility controls, one statusbar strip, and nothing drawn while it is off.
  it("puts the voice assistant in the toolbar and its state in the statusbar", () => {
    expect(app).toMatch(
      /workspace-toolbar-actions[\s\S]*?aria-label="Voice assistant"[\s\S]*?<AttentionTray/);
    expect(app).toContain('aria-pressed={voiceState !== "off"}');
    // Live stays a microphone (filled); the speaker marks only speech.
    expect(app).toContain('codicon codicon-mic" : "codicon codicon-mic-filled"');
    expect(app).toContain('voiceState === "speaking" ? "codicon codicon-unmute"');
    expect(app).toContain("cwd: voiceCwd");
    expect(voice).toContain("api.voiceSetEnabled(enable, cwd)");
    expect(voice).toContain("runUiCall(document, call.tool, call.input)");
    expect(app).toMatch(
      /<footer className="statusbar"[\s\S]*?className=\{`statusbar-item statusbar-text voice-indicator is-\$\{voiceState\}`\}/);
    expect(app).toContain('voice !== null && voiceState !== "off"');
    expect(css).toContain(".voice-indicator");
    expect(css).toContain(".voice-bar.is-lit");
    expect(codicons).toContain(".codicon-mic::before");
    expect(codicons).toContain(".codicon-unmute::before");
    expect(codicons).toContain(".codicon-mic-filled::before");
  });

  // The push arrives up to ten times a second while it listens. Selecting on
  // every frame would yank the user back from wherever they went, close the
  // launch sheet and clear the action error, so the KEY CHANGING is the event.
  it("selects the voice session once per session, not once per pushed frame", () => {
    expect(voice).toMatch(
      /const selectVoiceSession = useCallback\(\(key: string \| null\) => \{[\s\S]*?if \(voiceSelectedRef\.current === key\) return;/);
    expect(voice).toContain("const voiceSelectedRef = useRef<string | null>(null)");
    expect(voice).toContain(
      'selectVoiceSession(event.state === "off" ? null : event.session_key)');
    expect(voice).toContain("selectVoiceSession(enable ? result.session_key : null)");
    // The old unconditional select, in both places, is gone.
    expect(voice).not.toContain(
      "if (event.session_key !== null) selectSession(event.session_key as SessionKey)");
    expect(voice).not.toContain(
      "if (result.session_key !== null) selectSession(result.session_key as SessionKey)");
  });

  // One frame carries the level, the partial and any `notify` sentence, and a
  // null field in it means "unchanged", not "blank this".
  it("merges each voice frame into the last and shows a message on any state", () => {
    expect(voice).toContain("setVoice(current => nextVoiceState(current, event))");
    expect(app).toContain('voice.message !== null && voice.message !== ""');
    expect(app).not.toContain('voiceState === "error" && voice.message !== null');
    expect(css).toContain(".voice-indicator.is-error .voice-message");
  });

  // ui_snapshot is pre-approved and ui_act clicks whatever it is pointed at, so
  // the surfaces that carry a verdict are marked for voice-ui.ts to refuse.
  it("marks every approval surface so the voice assistant cannot answer one", () => {
    const collaboration = readFileSync(
      new URL("./components/CollaborationPanel.tsx", import.meta.url), "utf8");
    expect(conversation).toMatch(
      /className=\{sent \|\| !active \? "conv-approval"[\s\S]*?data-voice-guard="approval"/);
    expect(collaboration).toContain('data-voice-guard="approval"');
  });

  it("keeps each surface's sessions, tiles, and Code-only destinations to itself", () => {
    expect(app).toContain("sessionSurfaceOf(summary) === surface");
    expect(app).toContain("const [workspaces, setWorkspaces] = useState(loadInitialWorkspaces)");
    expect(app).toContain("const workspace = layoutFor(workspaces, surface)");
    expect(app).toContain('if (kind !== "code") setWorkspaceFor(kind, update);');
    expect(app).toContain("for (const kind of CONVERSATION_SURFACES)");
    expect(app).toContain("setWorkspaces(all => reconcileConversationLayouts(all, state.sessions))");
    expect(app).not.toContain("workspaces.code");
    expect(app).toContain(
      'const gridKeys = surface !== "agent"');
    expect(app).toContain('{surface === "agent" && centerView === "sessions"');
    expect(app).toContain("{agentTaskPane}");
    expect(app).toContain('visible={surface === "chat" && centerView === "sessions"}');

    // Every external session-selection path changes kind before writing the
    // active slot. Focusing an already-mounted tile stays within its surface.
    expect(app).toMatch(/const selectSession = useCallback[\s\S]*?ensureSurfaceFor\(key\);/);
    expect(app).toMatch(
      /const openSessionInNewPane = useCallback[\s\S]*?ensureSurfaceFor\(key\);/);
    const notificationClick = app.slice(app.indexOf("postNotification(title, body, () =>"),
      app.indexOf("window.focus();", app.indexOf("note.onclick = () =>")));
    expect(notificationClick).toContain("if (launchingRef.current) return;");
    expect(notificationClick.indexOf("if (launchingRef.current) return;"))
      .toBeLessThan(notificationClick.indexOf("ensureSurfaceForRef.current(key);"));
    expect(notificationClick).toContain("showSessionWorkspace();");
    const forkCompletion = app.slice(app.indexOf(".then(async ({ key: forked }) =>"),
      app.indexOf("showSessionWorkspace();", app.indexOf(".then(async ({ key: forked }) =>")));
    expect(forkCompletion).toContain("if (launchingRef.current) return;");
    expect(forkCompletion.indexOf("if (launchingRef.current) return;"))
      .toBeLessThan(forkCompletion.indexOf("ensureSurfaceForRef.current(forked);"));
    const focusTile = app.slice(app.indexOf("const focusWorkspaceTile"),
      app.indexOf("const closeWorkspaceTile"));
    expect(focusTile).not.toContain("ensureSurfaceFor");
    expect(focusTile).toContain("showSessionWorkspace();");

    // Choosing Code alone is terminal-first. Every route that has an exact
    // session key uses the explicit-session helper and must not collapse the
    // revealed tile back to the terminal destination when it receives focus.
    const showSessionStart = app.indexOf("const showSessionWorkspace");
    const showSession = app.slice(showSessionStart,
      app.indexOf("const [registryReady", showSessionStart));
    expect(showSession).toContain('setCenterView("sessions")');
    expect(showSession).not.toContain('surfaceRef.current === "code"');
    expect(app).toContain('setCenterView(kind === "code" ? codeDestinationRef.current : "sessions")');
    expect(app).toContain('selectSession(command.id.slice("agent:".length))');
    const libraryOpen = library.slice(library.indexOf("const openSessionFromLibrary"),
      library.indexOf("const openWorktreeFromLibrary"));
    expect(libraryOpen).toContain("selectSession(key);");
    expect(app).toContain("onOpenSession={openSessionFromLibrary}");
    const worktreeOpen = library.slice(library.indexOf("const openWorktreeFromLibrary"),
      library.indexOf("const openTerminalForWorktree"));
    expect(worktreeOpen).toContain("selectSession(key);");
    expect(worktreeOpen).not.toContain("showSessions();");

    // The adversarial-review pair is two Chat sessions (the host starts them
    // with no kind), so the reservation and the panes are Chat's whichever
    // surface launched them; Agent neither spends nor inherits it.
    expect(app).toContain("hasWorkspacePaneCapacity(workspacesRef.current.chat, total)");
    const openReview = app.slice(app.indexOf("const openAdversarialReviewSessions"),
      app.indexOf("const focusWorkspaceTile"));
    expect(openReview).toContain("if (launchingRef.current) return false;");
    expect(openReview).toContain('showSurface("chat");');
    expect(openReview).not.toContain('showSurface("code");');
    expect(app).toContain("setWorkspaceFor(\"chat\", state => openWorkspaceKey(");
    expect(app).toContain(
      'surfaceRef.current === "chat" ? workspacePaneReservation.current : 0');
    expect(app).toContain('const reservation = kind === "chat" ? workspacePaneReservation.current : 0;');
    expect(app).toContain('surface === "code" && centerView === "terminal"');
    expect(app).toContain('surface !== "code" || terminalSwitcher === null');
    for (const destination of ["showCanvas", "showTerminals"]) {
      const destinationStart = app.indexOf(`const ${destination} = useCallback`);
      const destinationBody = app.slice(destinationStart,
        app.indexOf("}, [", destinationStart));
      expect(destinationBody).toContain("if (launchingRef.current) return;");
      expect(destinationBody.indexOf("if (launchingRef.current) return;"))
        .toBeLessThan(destinationBody.indexOf('showSurface("code");'));
    }
  });

  it("wires kind-aware Chat launch and the Agent task pane without weakening Code", () => {
    expect(app).toContain('const SURFACE_KEY = "interface.surface.v1"');
    expect(app).toContain('chat: "No threads yet."');
    expect(app).toContain('code: "No workspaces yet."');
    expect(app).toContain('agent: "No tasks yet."');
    expect(app).toContain('chat: "New thread"');
    expect(app).toContain('code: "New workspace"');
    expect(app).toContain('agent: "New task"');
    expect(app).toContain("newLabel={surfaceNewLabel}");
    // Agent mode with no agent to hand a task to makes the agent first, from
    // every New: the sidebar, ⌘N, the palette and the empty pane.
    expect(app).toContain('const agentFirst = surface === "agent" && activeAgent === undefined;');
    expect(app).toContain('const surfaceNewLabel = agentFirst ? "New agent" : SURFACE_NEW_LABEL[surface];');
    expect(app.match(/else openComposerOrAgent\(\);/gu)).toHaveLength(3);
    expect(app).toContain("createDisabled={!agentReady}");
    expect(app).toContain("zeroStateText={SURFACE_ZERO_TEXT[surface]}");
    expect(app).toContain('document.querySelector<HTMLButtonElement>(".sidebar-nav-button")');
    const showSurfaceStart = app.indexOf("const showSurface");
    const showSurface = app.slice(showSurfaceStart,
      app.indexOf("const ensureSurfaceFor =", showSurfaceStart));
    expect(showSurface).toContain("if (launchingRef.current) return;");
    expect(app).toContain('const composerReady = composerDirectory.ready || surface === "chat"');
    expect(app).toContain("{composerReady && <Composer");
    expect(app).toContain("kind={surface}");
    expect(app).toContain('const workingDirectoryReady: Promise<unknown> = opts.kind === "chat"');
    expect(app).toContain('if (opts.kind !== "chat") {');
    expect(app).toContain('surface !== "chat" && surfaceSession?.cwd !== undefined');
    expect(app).toContain('className="empty workspace-hero agent-empty"');
    expect(app).toContain("<AgentTask key={agentTaskKey}");
    // Files touched reveals the task's session on its own surface, not Code.
    expect(app).toMatch(/onOpenDiff=\{\(\) => \{[\s\S]*?selectSession\(agentTaskKey\);\n {8}\}\}>/u);
    expect(app).toContain("engineVersion={state.engineHealth");
    expect(app).toContain('planName={usage.status === "ready"');
  });

  it("keeps the warm light and code-native dark palettes synchronized through native chrome", () => {
    for (const token of [
      "--chrome-bg: #eeece7",
      "--editor-bg: #f7f5f0",
      "--surface: #fffdfa",
      "--row-selected: #dfe2f0",
      "--selection-accent: #4057b2",
      "--review-accent: var(--mode-auto)",
      "--badge-bg: #4057b2",
      "--accent: #5a4634",
      "color-scheme: light",
    ]) expect(css).toContain(token);
    for (const token of [
      // Porcelain Night, repainted for dense tiles (owner, 2026-10-01).
      ':root[data-theme="dark"]',
      "--chrome-bg: #0e0e0d",
      "--editor-bg: #0e0e0d",
      "--surface: #1a1918",
      "--workbench-canvas: #0e0e0d",
      "--terminal-content-bg: #0a0a09",
      "--fg: #e4e2de",
      "--fg-strong: #f5f3ef",
      "--accent: #edebe8",
      "--selection-accent: #8da8f0",
      "--badge-bg: #8da8f0",
      "color-scheme: dark",
      // Porcelain (light): dark screens inside pale chrome.
      ':root[data-theme="light"]',
      "--terminal-content-bg: #18191b",
      "--terminal-color-scheme: dark",
    ]) expect(css).toContain(token);
    expect(css).toContain("@media (prefers-color-scheme: dark)");
    expect(css).toContain(':root:not([data-theme])');

    expect(main).toContain("backgroundColor: background");
    expect(main).toContain("const win = createWindow(currentWindowBackground(), location, buildChannel)");
    expect(main).toContain("nativeTheme.themeSource = appearanceThemeSource(appearance)");
    expect(main).toContain("applyWindowMaterial(win, currentWindowBackground(), surfaceTranslucency)");
    expect(appearanceContract).toContain('light: "#f0f0ee"');
    expect(appearanceContract).toContain('dark: "#0e0e0d"');
    expect(css).toContain(':root[data-theme="anthropic-light"]');
    expect(css).not.toContain(':root[data-theme="sauteed-paper-light"]');
    expect(css).toContain(':root[data-theme="catppuccin-mocha"]');
    expect(css).toContain(':root[data-font="serif"] .conv-text');
    expect(css).not.toContain("--font-ui: var(--font-ui-serif)");
  });

  it("retains System as a preference while resolving it live for CSS", () => {
    expect(appearance).toContain("root.dataset.theme = state.resolved");
    // A theme file paints over the custom palette of its kind.
    expect(appearance).toContain("root.dataset.theme = `custom-${kind}`");
    expect(appearance).toContain('window.matchMedia("(prefers-color-scheme: dark)")');
    expect(appearance).toContain("const systemChanged = (): void => current.refresh()");
  });

  it("switches ordinary navigation in place while preserving chat-owned state", () => {
    expect(app).toContain('"workspace-surface is-hidden"');
    expect(app).toMatch(/const selectSession = useCallback[\s\S]*?switchWorkspaceKey/);
    expect(app).toMatch(/const openSessionInNewPane = useCallback[\s\S]*?openWorkspaceKey/);
    expect(app).toContain("drafts={conversationDrafts}");
    expect(app).toContain("pendingEchoes={conversationEchoes[key]");
    expect(app).toContain("onOpenInNewPane={openSessionInNewPane}");
  });

  it("returns to the selected Code workspace and parks health under Settings", () => {
    expect(app).toContain('setCenterView(kind === "code" ? codeDestinationRef.current : "sessions")');
    expect(app).toContain(
      'state.engineHealth.filter(health => !health.ok || health.info?.auth === "signedOut")');
    expect(app).toContain('<footer className="statusbar"');
    // A provider fault sits in the sidebar footer under Settings (owner, 2026-09-28).
    expect(app).toContain("footerAlert={providerAlert}");
    // Review A4 (2026-10-14): the alert opens Models & providers, not Appearance,
    // and its tooltip says what to do rather than quoting the probe.
    expect(app).toContain("onFooterAlert={openProviderSettings}");
    expect(app).toContain('const openProviderSettings = useCallback(() => { openSettingsAt("providers"); }, [openSettingsAt]);');
    expect(app).toContain("return providerFaultSentence(name(health.engine), raw) ?? `${name(health.engine)}: ${raw}`;");
    expect(app).toContain("initialSection={settingsSection}");
    expect(readFileSync(new URL("./components/Sidebar.tsx", import.meta.url), "utf8"))
      .toContain("disabled={settingsDisabled} onClick={onFooterAlert ?? onSettings}>");
    expect(app).not.toContain('className="statusbar-item statusbar-alert"');
    expect(app).toContain("statusbar-warning");
    // The update affordance moved to the toolbar pill next to usage (2026-09-04).
    expect(app).not.toContain("statusbar-update");
    expect(app).not.toContain("workspace-toolbar-alert");
    expect(app).toContain("<UpdateButton ");
    expect(app).not.toContain("statusbar-session");
    expect(app).not.toContain("selectedSessionStatus");
  });

  it("lets restored workspace sessions change access before the first send", () => {
    const config = sessionConfigFor("claude#restored", { ...session, engine: "claude", kind: "code" },
      [], [], "Claude", {}, {}, {}, [], ["auto"], [], "auto", false, false);
    expect(config.canSetPermission).toBe(true);
    const restore = app.slice(app.indexOf("restoreAccessMode: key => {"),
      app.indexOf("checkpoint: key => { checkpointTurn(key, null); },"));
    expect(restore).toContain("return window.iface.setPermissionMode(key, mode).then(() => {");
    expect(restore).toContain("closedKeys.current.delete(key)");

    expect(conversation).toContain("newer model or command scope");
    expect(conversation).toContain("prelaunchCommandCache.delete(key)");
  });

  it("closes panes without a discard prompt because drafts remain session-owned", () => {
    expect(app).toContain("closeWorkspaceTile");
    expect(palette).toContain('label: "Close pane"');
    expect(app).toContain("closeWorkspaceTile(workspace.focusedKey)");
    expect(app).toContain("onClose={() => { closeWorkspaceTile(key); }}");
    expect(app).not.toContain("confirmCloseWorkspaceTile");
    expect(app).not.toContain("window.confirm");
  });

  it("keeps command-palette actions concise and explains every known disabled state", () => {
    for (const label of [
      "Go to Threads", "Go to Code", "Go to Agent",
      "Search transcripts", "Archived sessions", "Worktrees",
      "Open in external terminal",
      "Next pane", "Previous pane", "Close pane",
      "Settings", "Provider usage", "Refresh providers & sessions",
    ]) {
      expect(palette).toContain(`label: "${label}"`);
    }
    expect(palette).toContain("label: state.newLabel");
    expect(app).toContain("newLabel: surfaceNewLabel,");
    expect(palette).toContain('? "Restore layout" : "Maximize pane"');
    // Open in external terminal is enabled whenever Code's terminals show.
    expect(app).toContain('const terminalsShown = surface === "code" && centerView === "terminal";');
    expect(palette).toMatch(
      /id: "workspace\.externalTerminal",[^}]*?disabled: !state\.terminalsShown,/u);
    for (const reason of [
      "Show Code's terminals first.",
      "Open at least two panes.", "Open or focus a pane first.",
      "Focus an agent pane first.", "Wait for the new chat to start.",
      "A refresh is already running.",
    ]) {
      expect(palette).toContain(`disabledReason: "${reason}"`);
    }
  });

  it("provides recoverable session and worktree lifecycle surfaces", () => {
    expect(app).toContain("<SessionLibrary");
    expect(library).toContain("api.listArchivedSessions()");
    expect(library).toContain("api.searchSessions(query)");
    expect(library).toContain("api.listManagedWorktrees()");
    expect(library).toContain("api.cleanupManagedWorktree(key)");
    expect(library).toContain("api.openTerminal(worktree.path)");
    // The external terminal opens the project of the terminal in front.
    expect(app).toContain("const project = terminalCenter.current?.controlProject() ?? null;");
    expect(app).toContain("window.iface.openTerminal(project)");
    expect(app).toMatch(/const closeLiveSession = useCallback[\s\S]*?window\.iface\.closeSession\(key\)/);
    expect(app).toMatch(/const hideSessionLocally = useCallback[\s\S]*?closeLiveSession\(key\)[\s\S]*?archive\(key, true\)/);
  });

  it("makes the narrow sessions overlay dismissible without changing desktop", () => {
    expect(app).toContain('sidebarOpen || drawerLeaving ? "app-grid sidebar-overlay" : "app-grid no-sidebar"');
    // The drawer keeps its overlay class only while it glides out on narrow windows.
    expect(app).toContain('const drawerLeaving = mobileLayout && drawer.phase === "leaving";');
    expect(app).toContain('className="sidebar-backdrop"');
    // The file picker IS the palette now, so its own flag is gone from here:
    // one overlay, one predicate.
    expect(app).toContain('const overlayDialogOpen = commandPaletteOpen || sessionLibraryOpen');
    expect(app).not.toContain("quickOpen.open ||");
    expect(app).toContain(
      'const shellInert = overlayDialogOpen || (!settingsOpen && sidebarOpen && mobileLayout);');
    // Settings covers the sidebar, the work area and the status bar, so they
    // leave the tree under it too (owner, 2026-11-15).
    expect(app).toContain("const behindSettings = shellInert || settingsOpen;");
    expect(app).toMatch(/<footer className="statusbar"[\s\S]*?inert=\{behindSettings/);
    expect(app).toMatch(/<main className="workspace-shell"\s+inert=\{behindSettings/);
    expect(app).toContain('backgroundInert={overlayDialogOpen || settingsOpen}');
    expect(app).toContain('focusOnMobileOpen={!settingsOpen && sidebarOpen && mobileLayout}');
    expect(app).toContain('className="sidebar-backdrop" aria-hidden="true"');
    expect(app).toContain('const MOBILE_SIDEBAR_QUERY = "(max-width: 840px)"');
    expect(app).toContain('event.key !== "Escape"');
    expect(app).toMatch(/const selectSession = useCallback[\s\S]*?closeMobileSidebar\(\);/);

    // The named close button is absent from layout and tab order on desktop;
    // only the explicit open-overlay class activates it at the shared breakpoint.
    expect(css).toContain(".sidebar-backdrop { display: none; }");
    expect(css).toContain(".app-grid.sidebar-overlay .sidebar-backdrop");
    expect(css).toMatch(/@media \(max-width: 840px\)[\s\S]*?\.app-grid\.sidebar-overlay \.sidebar/);
    expect(css).toContain("grid-template-columns: minmax(0, 1fr) auto minmax(0, 1fr)");
    expect(css).not.toContain("@container workspace-shell");
  });

  it("creates native Code workspaces without the retired Clean setting", () => {
    expect(app).toContain('codeWorkspaces={surface === "code"}');
    expect(app).not.toContain('<WorkspaceTypePicker');
    expect(app).not.toContain('onPanePresentationChange=');
    const create = app.slice(app.indexOf('const newCodeWorkspace ='), app.indexOf('const openComposerFromSidebar'));
    expect(create).toContain('startCodeWorkspace(projectPath)');
    expect(create).not.toContain('openFreshComposer(');
    expect(app).toContain("projectSessions: state.sessions");
    expect(app).toContain("registryTerminals: terminalSummaries");
    expect(app).toContain("const startCodeWorkspace = useCallback");
    expect(app).toContain("terminalSwitcher?.onCreate(projectPath)");
    expect(app).toContain('if (surfaceRef.current === "code")');
    const showSurfaceStart = app.indexOf("const showSurface");
    const showSurface = app.slice(showSurfaceStart,
      app.indexOf("const ensureSurfaceFor =", showSurfaceStart));
    expect(showSurface).toContain('kind === "code" ? codeDestinationRef.current : "sessions"');
    expect(app).toContain("showTerminals();");
    expect(app).not.toContain(
      'if (centerView === "terminal") showSessions(); else showTerminals();');
  });

  it("lets a focused shell keep readline's chords before the app claims them", () => {
    // The guard runs after Ctrl+` (terminals) and before the ⌘P / ⌘N / Ctrl+Tab
    // branches, on the same `.terminal-center` test the terminal center uses.
    const guard = app.indexOf("if (shellOwnsChord(event, event.target instanceof Element");
    expect(guard).toBeGreaterThan(app.indexOf('event.code === "Backquote"'));
    expect(guard).toBeLessThan(app.indexOf("// ⌘P is go-to-file"));
    expect(app).toContain('&& event.target.closest(".terminal-center") !== null)) return;');
  });

  it("opens a fresh composer seeded to the project selected in the sidebar", () => {
    expect(app).toContain(
      'const openFreshComposer = useCallback((projectPath: string | null, prompt = "")');
    expect(app).toContain('cwd: chat ? "" : composerSeed.cwd ?? readStored(LAST_CWD)');
    expect(app).toContain("revision: current.revision + 1, cwd: projectPath, prompt,");
    expect(app).toContain("setComposerContextRoot(cwd)");
    expect(app).toContain("onNewInProject={openComposerInProjectFromSidebar}");
    expect(app).toContain("key={composerSeed.revision}");
    expect(app).toMatch(/const openComposerInProjectFromSidebar = useCallback[\s\S]*?setSidebarOpen\(false\)/);
  });

  it("uses each surface's focused session and a host-approved empty Code fallback", () => {
    expect(app).toContain("const [approvedDefaultDirectory, setApprovedDefaultDirectory]");
    expect(app).toContain("window.iface.getDefaultDirectory().then(path => {");
    expect(app).toContain("setApprovedDefaultDirectory(path)");
    expect(app).toContain("}, [defaultDirectoryAttempt]);");
    expect(app).toContain('cwd: chat ? "" : composerSeed.cwd ?? readStored(LAST_CWD)');
    expect(app).toContain("const surfaceWorkspaceKey = workspace.focusedKey");
    expect(app).toContain('sessionSurfaceOf(summary) === surface ? summary : null');
    // Code holds no conversation, so its terminals start in the approved folder.
    expect(app).toContain("activeCwd={approvedDefaultDirectory}");
    expect(app).not.toContain("codeSession");
    expect(app).not.toContain("activeSessionKey=");
    expect(app).not.toContain("activeCwd={selected?.cwd");
    expect(app).not.toContain("window.iface.openTerminal(selected.cwd)");
  });

  it("shows a named New chat loading and recovery state while resolving the default folder", () => {
    // The loading copy follows the button that opened the sheet: "Preparing
    // new task" on the Agent surface, not a hard-coded "New Chat".
    expect(app).toContain("`Preparing ${SURFACE_NEW_LABEL[kind].toLowerCase()}`");
    expect(app).toContain("aria-label={preparingLabel(surface)}");
    expect(app).toContain("<h2>{preparingLabel(surface)}</h2>");
    expect(app).toContain("Finding your default Projects folder and available models");
    expect(app).toContain("composerDirectory.error");
    expect(app).toContain("retryComposerDirectory");
    expect(app).toContain("You can choose a folder below or try again.");
  });

  it("owns durable per-model launch presets and passes them into the composer", () => {
    expect(app).toContain("readModelPresets(localStorage)");
    expect(app).toContain("rememberModelPreset(current, preset)");
    expect(app).toContain("writeModelPresets(localStorage, modelPresets)");
    // Selected agent profiles own their launch defaults; global MRU effort
    // must not override a different agent's saved model configuration.
    expect(app).toContain('modelPresets={surface === "agent" && activeAgent !== undefined ? undefined : modelPresets}');
    expect(app).toContain('model: agent !== undefined ? agent.model : (last?.model ?? "")');
    expect(app).toContain('effort: agent !== undefined ? agent.effort');
    expect(app).toContain("onModelPresetChange={rememberLaunchModelPreset}");
    // Every setEffort resolution is checked against the attempt that made it
    // before it writes a preset: a superseded pick resolving last would
    // otherwise remember an effort the session is not running.
    const changeEffort = app.match(
      /const changeEffortFor = useCallback\([\s\S]*?const changeFastModeFor/u)?.[0] ?? "";
    expect(changeEffort).toContain("if (!write.isCurrent()) return;");
    expect(changeEffort.indexOf("if (!write.isCurrent()) return;"))
      .toBeLessThan(changeEffort.indexOf("rememberConfirmedPreset(key, { effort })"));
    // The continuations read the latest registry and log through refs, so
    // neither callback re-mints per streamed frame.
    for (const name of ["changeEffortFor", "changePermissionMode", "rememberConfirmedPreset"]) {
      const deps = app.match(new RegExp(
        `const ${name} = useCallback\\([\\s\\S]*?\\n  \\}, \\[([^\\]]*)\\]\\);`, "u"))?.[1];
      expect(deps, name).toBeDefined();
      expect(deps, name).not.toContain("state.");
    }
  });

  it("opens a newly created agent without another agent's task under its name", () => {
    const submit = app.match(/const submitAgentProfile = useCallback\([\s\S]*?const openSessionInNewPane/u)?.[0] ?? "";
    expect(submit).toContain('if (agentDialog?.mode === "new") setWorkspace(() => EMPTY_WORKSPACE_STATE);');
  });

  it("opens app settings without unmounting or dismissing the active workspace", () => {
    expect(app).toContain('const [settingsOpen, setSettingsOpen] = useState(false)');
    expect(palette).toContain('id: "workspace.settings"');
    expect(app).toContain('key === ","');
    expect(app).toContain('className="workspace-settings-overlay"');
    expect(app).toContain('<SettingsPage');
    expect(app).toContain('worktree: false');
    expect(app).not.toContain('worktree: appPreferences.defaultWorktree');
    const openSettings = app.match(
      /const openSettingsAt = useCallback\([\s\S]*?\n  }, \[[^\]]*\]\);/)?.[0] ?? "";
    expect(openSettings).toContain("setSettingsOpen(true)");
    expect(openSettings).toContain("applySettingsChromeLock(true)");
    expect(openSettings).not.toContain("dismissComposer");
    expect(app).toContain("const openSettings = useCallback(() => { openSettingsAt(); }, [openSettingsAt]);");
    // It fills the window under the toolbar, beside the shell rather than a
    // card inside it, so it covers the sidebar (owner, 2026-11-15).
    expect(css).not.toContain(".workspace-settings-card");
    expect(app).not.toContain("workspace-settings-card");
    expect(css).toMatch(/\.workspace-settings-overlay \{[^}]*position:fixed;[^}]*inset:var\(--titlebar-h\) 0 0 0;/);
    expect(app.indexOf('className="workspace-settings-overlay"'))
      .toBeGreaterThan(app.indexOf("</main>"));
    expect(css).not.toContain(".app-grid.settings-mode .workspace-toolbar { display: none; }");
    expect(app).not.toContain("settings-mode");
  });

  it("leaves Code one destination bar, drawn by the terminal surface itself", () => {
    // Plans and the sessions board are gone, so Code's second axis has one
    // destination the terminal centre already owns: no view-bar component, no
    // props threading a view through the terminal surface.
    expect(app).not.toContain("CodeViewBar");
    expect(app).not.toContain("codeView=");
    expect(app).not.toContain("onShowCodeView=");
    expect(app).not.toContain("code-view-stack");
    // Nor a bar of the terminal surface's own: its controls are portaled into
    // the toolbar slot App draws while the terminals are showing.
    const terminalWorkspace = readFileSync(
      new URL("./components/TerminalWorkspace.tsx", import.meta.url), "utf8");
    expect(terminalWorkspace).not.toContain("terminal-center-bar");
    expect(terminalWorkspace).toContain("createPortal(");
    expect(app).toContain('<span className="workspace-toolbar-terminal" ref={setTerminalToolbarSlot} />');
    expect(app).toContain("toolbarSlot={terminalToolbarSlot}");
  });

  it("binds ⌘P to the palette in files mode instead of swallowing it", () => {
    // ⌘P — the go-to-file chord in every editor this app mirrors — produced no
    // response at all: no picker, no beep, no message. It was captured solely
    // to keep Electron's Print dialog away. Meanwhile the app HAD a fuzzy file
    // picker in a second 573-line dialog with no chord of its own.
    expect(app).toContain("openFilePalette();");
    expect(app).not.toContain('import { QuickOpen }');
    expect(app).toContain("<CommandPalette");
    expect(app).not.toContain("      <QuickOpen\n");
    // The armed file store is the mode: there is no second mode state to drift.
    expect(app).toContain('mode={quickOpen.open ? "files" : "all"}');
    expect(app).toContain("onOpenFile={openQuickOpenFile}");
    const files = app.match(
      /const openFilePalette = useCallback\([\s\S]*?\n  \}, \[[^\]]*\]\);/u)?.[0] ?? "";
    expect(files).toContain("openContextQuickOpen(cwd, target)");
    // With nothing focused there is no authorized index, so the chord still
    // answers with the ordinary palette rather than doing nothing.
    expect(files).toContain("setCommandPaletteOpen(true)");
    // ⌘⇧P is still the command list.
    expect(app).toMatch(
      /if \(modifier && event\.shiftKey && key === "p"\) \{[\s\S]*?setCommandPaletteOpen\(true\)/u);
    // One reset closes the picker and the palette together, so no dismissal can
    // leave a files-mode palette over a store that has gone.
    expect(quickOpenHook).toMatch(
      /const closeQuickOpen = useCallback\(\(\) => \{[\s\S]*?setQuickOpen\(QUICK_OPEN_CLOSED\);[\s\S]*?setCommandPaletteOpen\(false\);/u);
    expect(app).toMatch(/const openSettings = useCallback\([\s\S]*?closeQuickOpen\(\)/u);
  });

  it("treats Settings as a destination: navigating away from it leaves it", () => {
    // The shell behind the card stays interactive on purpose (the sidebar is
    // meant to stay readable), which made every click there do something
    // invisible: the surface switch moved under the card, a session row
    // changed the selection silently, New chat opened a composer nobody saw.
    // The three navigation entry points close Settings first.
    expect(app).toContain("const leaveSettings = useCallback(() => { setSettingsOpen(false); }");
    for (const entry of [
      /const showSurface = useCallback\([\s\S]*?\n  \}, \[[^\]]*\]\);/u,
      /const selectSession = useCallback\([\s\S]*?\n  \}, \[[^\]]*\]\);/u,
      /const openComposerFromSidebar = useCallback\([\s\S]*?\n  \}, \[[^\]]*\]\);/u,
    ]) {
      const body = app.match(entry)?.[0] ?? "";
      expect(body).toContain("leaveSettings();");
      // All three leave Settings AFTER refusing a click made during a launch,
      // so a refused click never half-navigates.
      expect(body.indexOf("if (launchingRef.current) return;"))
        .toBeLessThan(body.indexOf("leaveSettings();"));
    }
    // NOT closeSettings: that restores focus to whatever opened Settings, and
    // the click that navigated has already moved focus somewhere deliberate.
    // And no click-away on the scrim — a page, not a dialog.
    expect(app).not.toContain("if (event.target === event.currentTarget) closeSettings()");
    // Escape and the toolbar's ✕ are the other two ways out; the ✕ only exists
    // while the page does.
    expect(app).toContain('aria-label="Close settings" title="Close settings (Esc)"');
  });

  it("splits into a new pane through the seeded New Chat sheet, not a bare create", () => {
    // There is no "create a session in project X" call: buildLaunchOptions
    // refuses a launch with no prompt (launch.ts). So the split opens the
    // existing seeded sheet and marks the NEXT successful launch as a new pane.
    expect(app).toContain("const pendingNewPane = useRef(false);");
    expect(app).toContain("const splitWorkspaceTile = useCallback(");
    expect(app).toContain("openFreshComposer(projectPath);");
    // The pane grid is the Threads surface's: a split opens the New thread
    // card on the pane's thread project (threadProjectRoot: the thread's own
    // workspace, else a non-empty repository root, else null for No project).
    // splitWorkspaceTile takes `string | null`, so "nowhere in particular" has
    // a value that says so, and never "".
    expect(app).toContain("onSplit={() => { splitThreadTile(summary); }}");
    expect(app).toContain("const splitWorkspaceTile = useCallback((projectPath: string | null)");
    // Rust reservations: a split is an append, so it honors the same helper
    // every other new-pane path uses rather than a bare MAX_WORKSPACE_TILES
    // count that would race an in-flight builder/reviewer pair.
    expect(app).toMatch(
      /const splitWorkspaceTile = useCallback\([\s\S]*?mayOpenWorkspaceKeyWithReservation/);
    expect(app).toContain("splitDisabled={!mayOpenWorkspaceKeyWithReservation(");
    expect(app).not.toContain("splitDisabled={workspace.openKeys.length >= MAX_WORKSPACE_TILES}");
    // The success arm now chooses; the two named handlers are untouched.
    expect(app).toContain("setWorkspace(current => asNewPane");
    expect(app).toMatch(/const selectSession = useCallback[\s\S]*?switchWorkspaceKey/);
    expect(app).toMatch(/const openSessionInNewPane = useCallback[\s\S]*?openWorkspaceKey/);
  });

  it("mints choicesFor on the catalogue revision, not on every event frame", () => {
    // Composer memoizes commandChoices/modelRows on choicesFor's identity, so
    // the identity may only move when a log's newest models.available did.

    expect(app).toContain("const next = nextCatalogueRevision(catalogueRef.current, state.events);");
    expect(app).toContain("  const choicesCache = useMemo(() => new Map<string, EngineChoices>(),\n    [state.engineHealth, catalogueRevision]);");
    expect(app).toContain("    const known = choicesCache.get(engine);\n    if (known !== undefined) return known;\n    const events = eventsRef.current;");
    expect(app).toContain("  }, [state.engineHealth, choicesCache]);");
    expect(app).not.toContain("  }, [state.engineHealth, state.events]);");
  });

  describe("nextCatalogueRevision", () => {
    const catalogue: EngineEvent = { kind: "models.available", models: [] };
    const start = { byKey: new Map(), revision: 0 };

    it("bumps for a same-size swap: a log holding a catalogue leaves as one without arrives", () => {
      // The store deletes a closed session's log in the same frame a new one
      // appears; a size check reads that as unchanged and the removed
      // session's catalogue would stay in the composer's picker.
      const first = nextCatalogueRevision(start, { "claude#a": [catalogue] });
      expect(first.revision).toBe(1);
      const swapped = nextCatalogueRevision(first, { "claude#b": [] });
      expect(swapped.revision).toBe(2);
    });

    it("holds across a frame that only streamed text or only added a log without a catalogue", () => {
      const first = nextCatalogueRevision(start, { "claude#a": [catalogue] });
      const streamed = nextCatalogueRevision(first,
        { "claude#a": [catalogue, { kind: "message.delta", text: "hi" }] });
      expect(streamed.revision).toBe(first.revision);
      const added = nextCatalogueRevision(streamed,
        { "claude#a": [catalogue, { kind: "message.delta", text: "hi" }], "codex#b": [] });
      expect(added.revision).toBe(first.revision);
    });

    it("bumps when a session gains a catalogue or a session holding one leaves", () => {
      const first = nextCatalogueRevision(start, { "claude#a": [] });
      expect(first.revision).toBe(0);
      const gained = nextCatalogueRevision(first, { "claude#a": [catalogue] });
      expect(gained.revision).toBe(1);
      const left = nextCatalogueRevision(gained, {});
      expect(left.revision).toBe(2);
    });

    it("holds when a session that never reported a catalogue leaves", () => {
      // Departure is not the trigger; losing a catalogue is. A shell-only or
      // still-cold session going away changes nothing choicesFor can read, and
      // bumping there re-mints the callback and every composer memo on it.
      const first = nextCatalogueRevision(start,
        { "claude#a": [catalogue], "codex#b": [] });
      expect(first.revision).toBe(1);
      const left = nextCatalogueRevision(first, { "claude#a": [catalogue] });
      expect(left.revision).toBe(1);
      // And the one that did hold a catalogue still bumps on its way out.
      expect(nextCatalogueRevision(left, {}).revision).toBe(2);
    });
  });

  it("keeps field-equal diff stats and peek facts from last frame so Sidebar's memo hits", () => {
    // perLog caches per log identity, but the streaming session's log is new
    // every frame; a value-equal object must not defeat reuseRecord.
    expect(app).toContain("next[key] = prior !== undefined && prior.added === stat.added\n"
      + "        && prior.removed === stat.removed ? prior : stat;");
    expect(app).toContain("next[key] = prior !== undefined && prior.activity === facts.activity\n"
      + "        && prior.files === facts.files && prior.approval === facts.approval ? prior : facts;");
  });

  it("reuses unchanged config views and their arrays across unrelated updates", () => {
    const args: Parameters<typeof sessionConfigFor> = ["codex#memo", session, [], [], "Codex",
      {}, {}, {}, [], ["never"], [], "never", false, false];
    const first = sessionConfigFor(...args);
    expect(sessionConfigFor(...args)).toBe(first);
    // A registry push replaces every summary object; an equal view keeps its
    // identity so memo(SessionTile) skips the push.
    const pushed: Parameters<typeof sessionConfigFor> = [...args];
    pushed[1] = { ...session };
    expect(sessionConfigFor(...pushed)).toBe(first);
    args[12] = true;
    const pending = sessionConfigFor(...args);
    expect(pending).not.toBe(first);
    expect(pending.pendingPermission).toBe(true);
    expect(pending.models).toBe(first.models);
    expect(pending.effortLevels).toBe(first.effortLevels);
    expect(pending.permissionModes).toBe(first.permissionModes);
    const log: EngineEvent[] = [{ kind: "message.delta", text: "stream" }];
    expect(liveTurnProgress(log, "working")).toBe(liveTurnProgress(log, "working"));
    expect(liveTurnProgress(log, "idle")).not.toBe(liveTurnProgress(log, "working"));
  });

  it("feeds the composer's access chip the pane's own config, not the raw list", () => {
    // ONE SessionConfigView per pane. The access mode moved from the tile head
    // to the composer's access chip (Threads redesign, 2026-10-06), which reads
    // the same `config` the status line does, so the two cannot disagree on a
    // restored session. The head no longer computes a second picker list.
    expect(app).toContain("const tileConfig = sessionConfigFor(key, summary, log,");
    expect(app).toContain("config={tileConfig}");
    expect(app).not.toContain("permissionMode={tileConfig.permissionMode}");
    expect(app).not.toContain("NO_PERMISSION_MODES");
    expect(app).not.toContain("permissionModes={choicesFor(summary.engine).permissionModes}");
    expect(app).toContain("changePermissionModeFor: (key, mode) => { changePermissionMode(key, mode); },");
  });

  it("reads the split flag before queueing the update, and arms it after the sheet", () => {
    // The flag is a ref read inside a state updater, and React may run that
    // updater after the statements that follow it. Read it into a local FIRST or
    // every split sees `false` and replaces the focused pane instead of adding.
    expect(app).toContain("const asNewPane = pendingNewPane.current;");
    expect(app).toMatch(
      /const asNewPane = pendingNewPane\.current;\s*\n\s*pendingNewPane\.current = false;\s*\n\s*setWorkspace\(current => asNewPane/);
    expect(app).not.toContain("setWorkspace(current => pendingNewPane.current");
    // dismissComposer is not the only exit from the sheet: selectSession,
    // openSessionInNewPane and focusWorkspaceTile all close it with a bare
    // `composer: false`. So the flag is cleared on the way IN, which every route
    // must pass, and splitWorkspaceTile arms it AFTER that clear runs.
    expect(app).toMatch(
      /const openFreshComposer = useCallback\([\s\S]{0,400}?if \(launchingRef\.current\) return;\s*\n(?:\s*\/\/[^\n]*\n)*\s*pendingNewPane\.current = false;/);
    expect(app).toMatch(
      /openFreshComposer\(projectPath\);\s*\n\s*pendingNewPane\.current = true;\s*\n\s*return true;\s*\n\s*\}, \[openFreshComposer\]\);/);
  });

  it("shares tile geometry tokens between agent panes and terminal panes", () => {
    const terminalCss = readFileSync(new URL("./terminal.css", import.meta.url), "utf8");
    for (const token of ["--tile-radius", "--tile-gap", "--tile-head-h"] as const) {
      expect(css).toContain(`${token}:`);
      expect(terminalCss).toContain(`var(${token})`);
    }
    expect(css).toMatch(/\.workspace-tile \{[\s\S]*?border-radius: var\(--tile-radius\)/);
    expect(terminalCss).toMatch(/\.terminal-pane \{[\s\S]*?border-radius: var\(--tile-radius\)/);
    expect(css).toMatch(/\.workspace-grid \{[\s\S]*?gap: var\(--tile-gap\)/);
    expect(terminalCss).toMatch(
      /\.terminal-workspace\.is-equal-grid \{[\s\S]*?gap: var\(--tile-gap\)/);
    expect(css).toMatch(/\.workspace-tile-head \{[\s\S]*?height: var\(--tile-head-h\)/);
    expect(terminalCss).toMatch(/\.terminal-pane-head \{[\s\S]*?height: var\(--tile-head-h\)/);
  });
});


/* ============================================================================
   THE SUBAGENT DETAIL COLUMN — reachability, not intention.

   This app has shipped three separate dormant "fixes": a prop declared and
   never passed, presentation with no data behind it, a skeleton nothing
   rendered. Each one looked complete in the diff. So this suite asserts the
   two halves that a user actually walks through — the CALL SITE that hands the
   cell its callback, and the DATA PATH that turns a click into a panel with
   the agent's own prompt in it — and every assertion below fails if the
   corresponding wire is cut.
   ========================================================================= */
describe("the subagent detail column is reachable", () => {
  // The column's state and routes live in use-subagent-detail.ts; App draws it.
  const app = `${readFileSync(new URL("./App.tsx", import.meta.url), "utf8")}\n${
    readFileSync(new URL("./use-subagent-detail.ts", import.meta.url), "utf8")}`;
  const css = readFileSync(new URL("./theme.css", import.meta.url), "utf8");

  it("passes BOTH cell props at the per-tile call site, keyed per session", () => {
    // Cut either line and clicking a subagent cell does nothing: with no
    // handler the cell falls back to unfolding in place, which is a working
    // build that never opens the column — the exact shape of a dormant fix.
    const tile = app.match(
      /function tileConversation\([\s\S]*?<SessionTile[\s\S]*?actions=\{sessionTileActions\}\n {6}\/>/,
    )?.[0] ?? "";
    expect(tile).not.toBe("");
    expect(tile).toContain("selectedSubagentKey={subagentDetail?.sessionKey === key");
    // PER TILE. A bare agentKey handed to every pane would paint a selected
    // cell in panes the user never clicked, because a tool_use id is unique
    // only inside one session.
    expect(tile).toContain("? subagentDetail.agentKey : undefined}");
    const pane = readFileSync(new URL("./components/SessionTile.tsx", import.meta.url), "utf8").match(
      /const SessionTile = memo\(function SessionTile\([\s\S]*?<ProjectedConversation[\s\S]*?\n {4}\/>/,
    )?.[0] ?? "";
    expect(pane).not.toBe("");
    expect(pane).toContain("selectedSubagentKey={selectedSubagentKey}");
    expect(pane).toContain("onSelectSubagent={(agentKey, viaKeyboard) => {");
    expect(pane).toContain("openSubagentDetail(key, agentKey, viaKeyboard);");
    expect(app).toContain(
      "const [subagentDetail, setSubagentDetail] = useState<SubagentSelection | null>(null)");
    expect(app).toMatch(/type SubagentSelection = \{[\s\S]*?sessionKey: SessionKey;[\s\S]*?agentKey: string;/u);
  });

  it("renders the panel as the third grid child, with data behind every prop", () => {
    // Rendered AFTER the workspace shell and before fixed overlay components:
    // the third grid track, outside .workspace-grid, so it is chrome, not a tile.
    const shell = app.indexOf("</main>");
    const palette = app.indexOf("<CommandPalette", shell);
    const panel = app.indexOf("<SubagentPanel");
    expect(shell).toBeGreaterThan(0);
    expect(panel).toBeGreaterThan(shell);
    expect(panel).toBeLessThan(palette);
    // The track only exists while something is in it (or sliding out of it),
    // and the class and the render follow ONE presence, so they cannot disagree.
    expect(app).toMatch(/const detailOpen = subagentDetail !== null[\s\S]*?centerView === "sessions"/u);
    expect(app).toContain("const detail = usePresence(detailOpen);");
    expect(app).toContain('+ (detail.present ? " has-detail" : "")}');
    expect(app).toContain("<RetainedPresence open={detailOpen && subagentDetail !== null} phase={detail.phase}");
    // Data, not decoration: the agent, its prompt, its parent and the path root
    // all come from the reduced log rather than from a placeholder.
    expect(app).toContain("agent={detailView?.agent}");
    expect(app).toContain("prompt={detailView?.prompt}");
    expect(app).toContain("parentType={detailParent?.subagentType}");
    expect(app).toContain("pathRoot={detailSession?.projectRoot ?? detailSession?.cwd}");
    expect(app).toContain("focusOnOpen={subagentDetail.viaKeyboard}");
    expect(app).toContain("focusRequest={subagentDetail.focusRequest}");
    expect(app).toMatch(/const detailView = useMemo\(\(\) => \{[\s\S]*?conversationProjectionFor\(subagentDetail\.sessionKey\)\(detailEvents\)[\s\S]*?findSubagentItem\(liveItems, subagentDetail\.agentKey\)[\s\S]*?findSubagentItem\(historyItems, subagentDetail\.agentKey\)/u);
    expect(app).toContain("settleDetachedSubagents(currentItems)");
    expect(app).toContain("settleDetachedSubagents(detailHistoryItems)");
    expect(app).toContain("subagentPrompt(detailHistoryEvents, subagentDetail.agentKey)");
  });

  it("closes to the cell that opened it, and lets the panel keep Escape", () => {
    expect(app).toContain("onClose={closeSubagentDetail}");
    const close = app.match(
      /const closeSubagentDetail = useCallback\([\s\S]*?\n  \}, \[\]\);/u)?.[0] ?? "";
    expect(close).toContain("setSubagentDetail(null)");
    expect(close).toContain("focusSubagentRow(current, ancestor)");
    // Resolved BEFORE the state clears. A nested cell is drawn only because the
    // selection force-opens the agent above it, so querying after the clear
    // finds nothing and dumps a keyboard user at the top of the tile.
    expect(close.indexOf("const ancestor")).toBeLessThan(close.indexOf("setSubagentDetail(null)"));
    expect(close).toContain('.closest<HTMLElement>(".sub[data-agent-key]")');
    expect(app).toContain("ancestor?.isConnected === true");
    // Focus goes to a real node on every close route. The row is QUERIED, not
    // remembered, because the panel can be re-targeted while it is open; if it
    // has left the document, focus falls back to the owning tile's title and
    // never to document.body.
    expect(app).toContain('.querySelector<HTMLButtonElement>(".sub-row")');
    expect(app).toContain('.querySelector<HTMLButtonElement>(".workspace-tile-title")?.focus()');
    // The panel owns Escape inside itself; App also offers a guarded workspace
    // Escape for the narrow non-modal side sheet, without stealing it from a
    // foreground dialog, composer, settings, or the mobile sidebar.
    expect(app).toContain('event.key === "Escape" && subagentDetailRef.current !== null');
    expect(app).toContain("!foregroundDialog && !state.composerOpen && !settingsOpen");
    expect(app).toContain("!(mobileLayout && sidebarOpen)");
    // AND to the transcript's find bar, which is not a dialog but does own the
    // key. Both listeners are on window, so App must DECLINE rather than
    // preventDefault: Conversation's handler is the one guarded on
    // defaultPrevented, and which of the two registered last is not controlled.
    // Without this, one Escape closed find and the panel together — or, in the
    // commoner order, stopped closing find at all.
    expect(app).toContain('const TRANSCRIPT_FIND_SELECTOR = ".conv-findbar"');
    expect(app).toContain("document.querySelector(TRANSCRIPT_FIND_SELECTOR) === null");
    // The bar's presence IS its state: it renders only while open.
    const conv = readFileSync(
      new URL("./components/ConversationTranscript.tsx", import.meta.url), "utf8");
    expect(conv).toContain('{find.open && (');
    expect(conv).toContain('<div className="conv-findbar" role="search"');
  });

  it("keeps the panel's own controls wired to the shell", () => {
    // Both survived a mutation pass: the component tests cover the button and
    // the nested rows, and nothing asserted App hands them anything. Cut either
    // and Reveal-in-transcript disappears and every nested spawn row in the
    // panel becomes dead text — the two controls that make it navigable.
    expect(app).toContain("onReveal={revealSubagentDetail}");
    expect(app).toContain("onSelectChild={retargetSubagentDetail}");
    const retarget = app.match(
      /const retargetSubagentDetail = useCallback\([\s\S]*?\n  \}, \[\]\);/u)?.[0] ?? "";
    // Re-target, not re-open: the panel keeps its session and takes a new agent.
    expect(retarget).toContain("sessionKey: current.sessionKey");
    expect(retarget).toContain("agentKey, viaKeyboard: true");
    const reveal = app.match(
      /const revealSubagentDetail = useCallback\([\s\S]*?\n  \}, \[focusWorkspaceTile\]\);/u)?.[0] ?? "";
    expect(reveal).toContain("focusWorkspaceTile(current.sessionKey)");
    expect(reveal).toContain("scrollIntoView");
  });

  it("scopes reveal and focus to the owning session tile", () => {
    const tile = app.match(
      /function workspaceTile\(key: SessionKey\)[\s\S]*?\n\}/u)?.[0] ?? "";
    expect(tile).toContain('".workspace-agent[data-agent-key]"');
    expect(tile).toContain("node.dataset.agentKey === key");
    const lookup = app.match(
      /function subagentCell\(selection: SubagentSelection\)[\s\S]*?\n\}/u)?.[0] ?? "";
    expect(lookup).toContain("workspaceTile(selection.sessionKey)");
    expect(lookup).toContain('querySelectorAll<HTMLElement>(".sub[data-agent-key]")');
    expect(lookup).toContain("selection.agentKey");
    expect(app).toContain("subagentCell(current)?.scrollIntoView");
  });

  it("closes the inspector when its transcript destination is hidden", () => {
    expect(app).toContain('const sourceHidden = state.composerOpen || settingsOpen || centerView !== "sessions"');
    expect(app).toContain("workspace.maximizedKey !== subagentDetail.sessionKey");
    expect(app).toContain("if (sourceHidden) setSubagentDetail(null)");
  });

  it("clears a selection that outlives its pane", () => {
    // Closing the owning tile and pointing that tile at another session both
    // arrive as the key leaving openKeys. A column describing a pane that is
    // no longer open is presentation with no data behind it.
    const effect = app.match(
      /if \(subagentDetail === null\) return;\n {4}if \(workspace\.openKeys\.includes\(subagentDetail\.sessionKey\)\) return;\n {4}setSubagentDetail\(null\);/u);
    expect(effect).not.toBeNull();
    // Still resolving but no longer in the transcript is a DIFFERENT case: the
    // panel says so rather than vanishing.
    expect(app).toContain("missing={detailView?.agent === undefined}");
    expect(app).toContain("missingLabel={subagentNameRef.current}");
  });

  it("keeps the two-column shell for every window that opens no subagent", () => {
    // The base rule is untouched — asserted verbatim by the shell test above —
    // and the third track arrives as a modifier, above the width where three
    // columns fit. Below it, the column floats over the work area only.
    expect(css).toContain("grid-template-columns: var(--pane-left) minmax(0, 1fr);");
    expect(css).not.toContain("1fr var(--pane-right)");
    expect(css).toContain("--pane-detail: 340px;");
    expect(css).toContain(".app-grid.no-sidebar.has-detail {");
    expect(css).toMatch(/@media \(min-width: 1081px\)[\s\S]*?var\(--pane-left\) minmax\(0, 1fr\) var\(--pane-detail\)/u);
    // SCOPED TO THE BLOCK. The old assertion was a lazy [\s\S]*? across the whole
    // file, so `position: fixed` mutated to `static` here still matched the next
    // fixed overlay further down and the suite stayed green. Statically placed,
    // the pane becomes a third in-flow child of a TWO-column grid, lands in an
    // implicit row 2, and .app-grid's height:100vh; overflow:hidden clips it
    // away entirely between 841px and 1080px.
    const narrow = css.match(/@media \(max-width: 1080px\) \{\n([\s\S]*?)\n\}/u)?.[1] ?? "";
    expect(narrow).not.toBe("");
    expect(narrow).toContain(".app-grid.has-detail .detail-pane {");
    expect(narrow).toContain("position: fixed;");
    expect(narrow).toContain("width: min(var(--pane-detail), 86vw);");
  });

  it("sets the narrow toolbar's spacing once, in the block that wins", () => {
    // Every width at or under 840px also matches max-width 1080px, so a second
    // .workspace-toolbar spacing rule at 840px, placed earlier, never rendered.
    const spacing = [...css.matchAll(/\n {2}\.workspace-toolbar \{ gap: [^}]*\}/gu)]
      .map(([rule]) => rule.trim());
    expect(spacing).toEqual([".workspace-toolbar { gap: 2px; padding-inline: 4px; }"]);
  });

  it("gives the row's pixels to the agent's identity, not to its summary", () => {
    // Flex shrink is weighted by CONTENT basis, so three equal-priority slots
    // hand the most room to the LONGEST string — the summary — and clip the
    // name. Measured at the 420px pane, that left `rend…` and `render…` for two
    // different agents while the summary held 187px. This asserts the ORDER
    // rather than the numbers, so the weights stay tunable.
    const blockOf = (selector: string): string => {
      const at = css.indexOf(`\n${selector} {`);
      return at < 0 ? "" : css.slice(at, css.indexOf("}", at));
    };
    const shrinkOf = (selector: string): number => {
      const block = blockOf(selector);
      expect(block).not.toBe("");
      const shorthand = block.match(/flex:\s*[\d.]+\s+([\d.]+)/u);
      if (shorthand !== null) return Number(shorthand[1]);
      const single = block.match(/flex-shrink:\s*([\d.]+)/u);
      return single === null ? 1 : Number(single[1]);
    };
    const name = shrinkOf(".sub-type");
    const description = shrinkOf(".sub-desc");
    const summary = shrinkOf(".sub-row .conv-sum");
    expect(name).toBeLessThan(description);
    expect(description).toBeLessThan(summary);
    expect(name).toBeLessThan(1);
    // Scoped: .conv-sum is shared with the tool rows, which have two text slots
    // and already distribute correctly. theme.css must not reach the bare class.
    expect(css).not.toMatch(/\n\.conv-sum[ ,{]/u);
  });

  it("turns a real event log into a panel holding that agent's own prompt", () => {
    // THE DATA PATH, END TO END, with no source-string assertion in it: the
    // exact chain App runs — raw events → conversationItems → findSubagentItem
    // → SubagentPanel — over a log shaped like the one the engines emit.
    const events: EngineEvent[] = [
      { kind: "tool.call", callId: "agent-1", tool: "Task", summary: "Task",
        input: { subagent_type: "ui_audit", description: "Audit the cell",
          prompt: "Measure every subagent row at 420px and report what repeats." } },
      { kind: "subagent.started", agentKey: "agent-1", subagentType: "ui_audit",
        description: "Audit the cell" },
      { kind: "tool.call", callId: "c1", tool: "Read", summary: "theme.css",
        input: {}, agentKey: "agent-1" },
      { kind: "tool.result", callId: "c1", ok: true, summary: "1694 lines",
        agentKey: "agent-1" },
      { kind: "subagent.finished", agentKey: "agent-1", ok: true,
        summary: "The cell prints its own name twice." },
    ];
    const items = conversationItems(events);
    const agent = findSubagentItem(items, "agent-1");
    expect(agent).toBeDefined();
    if (agent === undefined) return;

    const html = renderToStaticMarkup(createElement(SubagentPanel, {
      agent,
      prompt: (events[0] as { input: { prompt: string } }).input.prompt,
      pathRoot: "/work/interface",
      onClose: () => undefined,
    }));
    // The heading names the agent, and the panel holds the three things the
    // one-row cell cannot: the task in full, the prompt the parent sent, and
    // the untruncated result.
    expect(html).toContain("ui_audit");
    expect(html).toContain("Audit the cell");
    expect(html).toContain("Measure every subagent row at 420px and report what repeats.");
    expect(html).toContain("The cell prints its own name twice.");
    // And the model row states its absence rather than printing a blank or the
    // string "undefined": neither live engine sets one.
    expect(html).not.toContain("undefined");
    expect(html).toContain("Inherits the parent");
  });
});

describe("a launch into a directory that is not there", () => {
  const app = readFileSync(new URL("./App.tsx", import.meta.url), "utf8");

  it("names the directory instead of letting the SDK blame the binary", () => {
    // Node reports ENOENT against the BINARY when the cwd is what is missing,
    // and the Claude SDK then reports a libc mismatch for a binary that is
    // present and runs. Observed 2026-08-27 against a folder whose name ended
    // in a space, which this field trims off.
    const launch = app.match(
      /const launch = useCallback\([\s\S]*?\n {2}}, \[[\s\S]*?\]\);/)?.[0] ?? "";
    expect(launch).toContain("window.iface.listDir(opts.cwd)");
    expect(launch).toContain("Working directory does not exist: ${opts.cwd}");
    expect(launch).toContain("including a trailing space in a folder name");
    // It has to run before the session is created, or the message arrives after
    // the failure it is explaining.
    expect(launch.indexOf("window.iface.listDir(opts.cwd)"))
      .toBeLessThan(launch.indexOf("window.iface.createSession(request)"));
  });
});

describe("errors the user used to X out of", () => {
  const app = readFileSync(new URL("./App.tsx", import.meta.url), "utf8");
  const sidebar = readFileSync(new URL("./components/Sidebar.tsx", import.meta.url), "utf8");
  const css = readFileSync(new URL("./theme.css", import.meta.url), "utf8");

  it("fade out on their own, pausing while read, in every place they appear", () => {
    // One countdown, declared once in CSS and finished by the element itself:
    // no JS timer to keep in step with the fade.
    expect(css).toMatch(/@keyframes error-linger\s*\{[\s\S]{0,120}?100% \{ opacity: 0; \}/u);
    expect(css).toMatch(/\.corner-notice\.is-error,\s*\.sidebar-say-error\s*\{[^}]*animation: error-linger 8s/u);
    // A message must not vanish under the reader's pointer or focus: hovering
    // or focusing the corner stack holds every notice in it.
    expect(css).toMatch(
      /\.corner-notices:hover \.corner-notice,\s*\.corner-notices:focus-within \.corner-notice,\s*\.sidebar-say-error:hover,\s*\.sidebar-say-error:focus-within\s*\{\s*animation-play-state: paused;/u);
    // The global reduced-motion reset shrinks every animation to 0.01ms; the
    // linger keeps its length there and ends with a cut instead of a fade.
    expect(css).toMatch(
      /\.corner-notice\.is-error, \.sidebar-say-error \{\s*animation-duration: 8s !important;\s*animation-timing-function: steps\(1, end\) !important;/u);
    // The full-width bar is gone (owner, 2026-11-15): the action error, a
    // rollback's result and each refresh failure are corner notices that
    // clear their own state when the linger ends (errors only).
    expect(css).not.toContain(".pane-error");
    expect(app).not.toContain('className="pane-error"');
    expect(app).toContain("onExpire: () => { setActionError(null); },");
    expect(app).toContain("onExpire: () => { setTurnRestoreNotice(null); },");
    expect(app).toContain("onExpire: drop,");
    expect(app).toContain("<CornerNotices items={cornerNotices}");
    expect(sidebar).toContain(
      '<div key={lifecycle.error} className="sidebar-say sidebar-say-error" role="alert"\n          onAnimationEnd={lifecycle.onDismissError}>');
    // Status notices keep their X: only errors go away by themselves.
    expect(css).not.toMatch(/\.notice\s*\{[^}]*animation/u);
  });
});
