import { flushSync } from "react-dom";
import type { Root } from "react-dom/client";
import { themeKind, type ResolvedTheme } from "../../shared/appearance";
import { ProjectedConversation } from "../components/ProjectedConversation";
import type { ConversationProps } from "../components/Conversation";
import { Sidebar } from "../components/Sidebar";
import { TerminalPane } from "../components/TerminalPane";
import type { SidebarGroup } from "../sidebar-view";
import { assert, frame, noop } from "./fixture-support";

const states = ["working", "idle", "waiting", "failed"] as const;
const groups: SidebarGroup[] = [{ projectName: "Working indicators", projectPath: "/fixture/wave",
  counts: { total: 4, waiting: 1, working: 1 }, rows: states.map((status, i) => ({
    key: `codex#wave-${i}`, engine: "codex", title: ["Polish the interface", "Ready for a task", "Needs your approval", "Check failed"][i],
    status, badge: status, cloud: false, selected: i === 0, external: false,
    lastActivity: "2026-09-08T00:00:00Z",
  })) }, { projectName: "Active project", projectPath: "/fixture/active",
  counts: { total: 1, waiting: 0, working: 1 }, rows: [{
    key: "codex#active", engine: "codex", title: "Active workspace", status: "working", badge: "working",
    cloud: false, selected: false, external: false, lastActivity: "2026-09-08T00:00:00Z",
  }] }];

/** Actual callers and theme cascade, with no provider or canonical state writes. */
export async function workingIndicatorFixture(root: Root, base: ConversationProps,
    theme: ResolvedTheme, chrome: string, engine = "codex") {
  document.documentElement.dataset.theme = theme;
  document.documentElement.dataset.chrome = chrome;
  document.documentElement.style.colorScheme = themeKind(theme);
  const render = (working: boolean): void => flushSync(() => root.render(<div className="app-grid wave-fixture">
    <style>{`
      .wave-fixture .workspace-toolbar { padding:12px 18px; }
      .wave-fixture-grid { display:grid; grid-template-columns:minmax(0, 1fr) 220px; gap:12px; height:100%; padding:12px; box-sizing:border-box; }
      .wave-fixture-side { display:flex; flex-direction:column; gap:12px; min-width:0; }
      .wave-fixture-side > .terminal-pane { flex:none; height:50px; }
      @media(max-width:800px) {
        .wave-fixture .sidebar { display:none; }
        .wave-fixture .workspace-shell { grid-column:1 / -1; }
        .wave-fixture-grid { grid-template-columns:minmax(0, 1fr); grid-template-rows:minmax(400px, 1fr) auto; }
        .wave-fixture-side { display:grid; grid-template-columns:repeat(2, minmax(0, 1fr)); }
      }
    `}</style>
    <header className="workspace-toolbar">Working indicators</header>
    <Sidebar groups={groups} rateLimits={{}} hidden={{ external: 0, trivial: 0 }} hideable={0}
      onSelect={noop} onNew={noop} onNewInProject={noop} onArchive={noop} onToggleFilters={noop} />
    <main className="workspace-shell"><div className="terminal-center wave-fixture-grid">
      <div style={{ minHeight: 0, display: "flex", flexDirection: "column" }}>
          <ProjectedConversation {...base} sessionKey={`${engine}#wave-main`} live={false} thinking={working}
            config={{ ...base.config, engine, engineLabel: engine }} workingLabel="Working"
            events={[
              { kind: "user.message", text: "Give the working indicator a little more motion." },
              { kind: "message.delta", text: "I’m bringing the **same Wave** to the Chat and Agent activity indicators.\n\nThe label stays steady while the bars move." },
              { kind: "tool.call", callId: "wave-read", tool: "Read", summary: "Checking the interface components", input: {} },
              { kind: "subagent.started", agentKey: "wave-worker", subagentType: "Reviewer", description: "Check the compact indicator" },
            ]} />
      </div>
      <aside className="wave-fixture-side">
      <TerminalPane pane={{ id: "wave-main", tabs: [{ terminalId: "wave-main" }], activeTerminalId: "wave-main" }}
        terminals={new Map([["wave-main", { id: "wave-main", title: "Polish the interface", cwd: "/fixture/wave",
          status: "running", profileId: engine, activity: working ? "working" : "idle" }]])}
        focused maximized={false} onFocus={noop} onSelectTab={noop} onCloseTab={noop} onMoveTerminal={noop}
        onRequestSplit={noop} onToggleMaximize={noop} renderTerminal={() => null} />
        {states.slice(1).map(state => <TerminalPane key={state}
          pane={{ id: state, tabs: [{ terminalId: state }], activeTerminalId: state }}
          terminals={new Map([[state, { id: state, title: state === "waiting" ? "Needs approval" : state === "failed" ? "Check failed" : "Ready",
            cwd: "/fixture/wave", status: state === "failed" ? "failed" : "running",
            activity: state === "waiting" ? "waiting" : "idle" }]])}
          focused={false} maximized={false} onFocus={noop} onSelectTab={noop} onCloseTab={noop} onMoveTerminal={noop}
          onRequestSplit={noop} onToggleMaximize={noop} renderTerminal={() => null} />)}
      </aside>
    </div></main>
  </div>));
  render(true); await frame(); await frame();
  // The global Reduce Motion guard assigns every element a 0.01ms transition.
  // Hidden Chromium can park that transition at its initial value until capture.
  document.body.getBoundingClientRect();
  if (matchMedia('(prefers-reduced-motion: reduce)').matches) {
    for (const animation of document.getAnimations()) if (animation instanceof CSSTransition) animation.finish();
    await frame();
  }
  const visible = (node: Element): boolean => node.getBoundingClientRect().width > 0
    && getComputedStyle(node).visibility !== "hidden";
  const marks = [...document.querySelectorAll<HTMLElement>('.working-indicator')];
  const waves = marks.flatMap(mark => [...mark.querySelectorAll<HTMLElement>('.working-wave')]);
  const count = waves.filter(visible).length;
  const footer = document.querySelector<HTMLElement>('.conv-working')!;
  const composer = document.querySelector('.conv-ta');
  assert(footer && composer, "Working conversation or composer missing");
  if (chrome !== "cli") {
    for (const selector of ['.conv-working .working-wave']) {
      const node = document.querySelector(selector);
      assert(node && visible(node), `Chat did not display ${selector} (${theme}/${chrome}/${innerWidth})`);
    }
    assert(getComputedStyle(footer, '::before').content === "none", "Legacy CLI ticker duplicates Wave");
    for (const mark of marks.filter(mark => visible(mark.querySelector(".working-wave")!))) {
      const wave = mark.querySelector<HTMLElement>('.working-wave')!;
      assert(visible(wave), "Working caller still shows its legacy glyph");
      assert(mark.getBoundingClientRect().width === (mark.classList.contains('working-indicator-large') ? 24 : 16), `Incorrect wave size: ${theme}/${chrome}: ${mark.className}: ${mark.getBoundingClientRect().width}`);
      assert(getComputedStyle(mark).animationName === "none", "Old pulse moves the entire wave");
      const children = [...wave.children];
      assert(children.length === 5, "Wave lost a bar");
      assert(children.every(bar => getComputedStyle(bar).backgroundColor === getComputedStyle(mark).color), `Wave lost theme color: ${mark.className}: ${getComputedStyle(mark).color} / ${children.map(bar => getComputedStyle(bar).backgroundColor).join(", ")}`);
      assert(getComputedStyle(mark, '::before').display === "none", "Legacy glyph remains beside Wave");
    }
    const reduced = matchMedia('(prefers-reduced-motion: reduce)').matches;
    const bars = waves.filter(visible).flatMap(wave => [...wave.children]);
    if (reduced) assert(bars.every(bar => getComputedStyle(bar).animationName === "none"
      && getComputedStyle(bar).opacity === "1"), "Reduce Motion left animation or a dimmed glyph");
    else {
      const bar = bars[0]; const before = getComputedStyle(bar).transform;
      const label = document.querySelector('.conv-working-label')!.getBoundingClientRect();
      await new Promise(resolve => setTimeout(resolve, 240));
      assert(getComputedStyle(bar).transform !== before, "Wave did not animate over time");
      const after = document.querySelector('.conv-working-label')!.getBoundingClientRect();
      assert(label.x === after.x && label.y === after.y, "Animation moved the status text");
    }
  } else assert(count === 0, "Wave leaked into Terminal appearance");
  for (const state of states.slice(1)) {
    assert(!document.querySelector(`[data-terminal-pane-id="${state}"] .working-wave`), `${state} pane was marked working`);
  }
  assert(!document.querySelector('.sidebar .working-wave'), "Wave leaked into the sidebar");
  assert(document.querySelector('.sidebar .group-attention-working'), "Working project dot is missing");
  assert(!document.querySelector('.terminal-pane-head .working-wave'), "Wave leaked into a pane header");
  const headerDot = document.querySelector('.terminal-tab-status.is-working');
  assert(headerDot && getComputedStyle(headerDot).width === "6px", "Pane header lost its compact dot");
  const sidebarWorking = document.querySelector('.session-row [aria-label="Working"]');
  assert(sidebarWorking && sidebarWorking.getAttribute('aria-hidden') !== 'true', "Sidebar lost its accessible working label");
  assert(document.documentElement.scrollWidth <= innerWidth, "Working layout overflows horizontally");
  // A stowed desktop sidebar stays mounted behind visibility:hidden, which does
  // not stop animations: its working marks (CLI glyphs step `content` on the
  // main thread) pause until it returns, whatever each provider's glyph sets.
  if (innerWidth >= 841) {
    const grid = document.querySelector<HTMLElement>(".app-grid")!;
    const sidebarMarks = [...document.querySelectorAll<HTMLElement>(".sidebar .dot-pulse")];
    assert(sidebarMarks.some(mark => mark.dataset.provider === "codex"), "Sidebar has no Codex working mark");
    const playStates = (): string[] => sidebarMarks.flatMap(mark => [
      getComputedStyle(mark).animationPlayState, getComputedStyle(mark, "::before").animationPlayState]);
    grid.classList.add("no-sidebar");
    const stowed = playStates();
    grid.classList.remove("no-sidebar");
    const shown = playStates();
    assert(stowed.every(state => state === "paused"), `Stowed sidebar still animates (${theme}/${chrome}): ${stowed.join(", ")}`);
    assert(shown.every(state => state === "running"), `Shown sidebar paused its working marks (${theme}/${chrome}): ${shown.join(", ")}`);
  }
  // Existing state transition must remove both footer and header animation.
  render(false); await frame(); await frame();
  assert(!document.querySelector('.conv-working'), "Idle conversation still shows Working");
  assert(!document.querySelector('[data-terminal-pane-id="wave-main"] .terminal-tab-status.is-working'), "Idle header still reports working");
  assert(document.querySelector('.conv-ta') === composer, "Status change remounted the composer");
  render(true); await frame(); await frame();
  return { theme, chrome, engine, visibleWaves: count, reducedMotion: matchMedia('(prefers-reduced-motion: reduce)').matches };
}
