import { flushSync } from "react-dom";
import type { Root } from "react-dom/client";
import { isResolvedTheme, themeKind, type SurfaceTranslucency } from "../../shared/appearance";
import { applySurfaceTranslucency } from "../appearance";
import { TerminalPane } from "../components/TerminalPane";
import { TerminalView, type TerminalViewHandle } from "../components/TerminalView";
import { TranslucencySlider } from "../components/TranslucencySlider";
import { Sidebar } from "../components/Sidebar";
import type { SidebarGroup } from "../sidebar-view";
import { frame, noop } from "./fixture-support";

const groups: SidebarGroup[] = ["Materials", "Renderer", "Workspaces"].map(projectName => ({
  projectName, projectPath: `/fixture/${projectName}`, counts: { total: 8, waiting: 0, working: 0 },
  rows: Array.from({ length: 8 }, (_, i) => ({ key: `codex#${projectName}-${i}`, engine: "codex",
    title: `Fixture ${i + 1}: crisp scrolling text`, status: "idle", badge: "idle",
    cloud: false, selected: false, external: false, lastActivity: "2026-09-08T00:00:00Z" })),
}));
export function createTranslucencyFixture(root: Root) {
  let value: SurfaceTranslucency = { version: 1, terminal: 0, sidebar: 0 };
  let terminal: TerminalViewHandle | null = null;
  let original: TerminalViewHandle | null = null;
  let mounts = 0;
  let disposals = 0;
  const ready = (handle: TerminalViewHandle | null): void => {
    terminal = handle;
    if (handle) { mounts++; handle.completeSetup(); } else disposals++;
  };
  const paint = (): void => {
    applySurfaceTranslucency(value);
    document.documentElement.dispatchEvent(new CustomEvent("interface-theme-change"));
    flushSync(() => root.render(<div className="app-grid">
      <header className="workspace-toolbar" style={{ paddingInline: 18 }}>Interface · Material preview</header>
      <Sidebar groups={groups} rateLimits={{}} hidden={{ external: 0, trivial: 0 }} hideable={0}
        glass={value.sidebar > 0}
        onSelect={noop} onNew={noop} onNewInProject={noop} onArchive={noop} onToggleFilters={noop}
        agentMenu={<div className="translucency-controls" style={{ padding: 12 }}>
          {(["terminal", "sidebar"] as const).map(surface => <TranslucencySlider key={surface}
            id={`fixture-${surface}`} label={surface === "terminal" ? "Terminal panes" : "Sidebar"}
            value={value[surface]} onChange={percent => { value = { ...value, [surface]: percent }; paint(); }} />)}
        </div>} />
      <main className="workspace-shell"><div className="workspace-content">
        <div className="workspace-surface"><div className="terminal-center">
          <div className="terminal-workspace is-equal-grid terminal-count-2">
            {[true, false].map(focused => <TerminalPane key={String(focused)}
              pane={{ id: String(focused), tabs: [{ terminalId: "glass-fixture" }], activeTerminalId: "glass-fixture" }}
              terminals={new Map([["glass-fixture", { id: "glass-fixture", title: focused ? "Codex · Focused" : "Claude · Idle", cwd: "/fixture", status: "running", profileId: "codex" }]])}
              focused={focused} maximized={false} onFocus={noop} onSelectTab={noop} onCloseTab={noop}
              onMoveTerminal={noop} onRequestSplit={noop} onToggleMaximize={noop}
              renderTerminal={() => focused
                ? <TerminalView terminalId="glass-fixture" active onData={noop} onReady={ready} />
                : <div style={{ padding: 18 }}>The idle pane keeps its separate header.</div>} />)}
          </div>
        </div></div>
      </div></main>
    </div>));
  };
  return async (theme: string, terminalPercent: number, sidebarPercent: number, chrome = "refined", settleMaterials = true) => {
    if (!isResolvedTheme(theme)) throw new Error("Invalid theme");
    document.documentElement.dataset.theme = theme;
    document.documentElement.dataset.chrome = chrome;
    document.documentElement.style.colorScheme = themeKind(theme);
    value = { version: 1, terminal: terminalPercent, sidebar: sidebarPercent };
    paint(); await frame(); await frame();
    // A hidden Chromium document can leave a 0.01ms reduced-motion transition
    // parked at time zero until capture. Finish that visual interpolation before
    // measuring final materials, while keeping the real terminal alive.
    getComputedStyle(document.querySelector(".sidebar")!).backgroundColor;
    for (const animation of settleMaterials ? document.getAnimations() : []) {
      if (animation instanceof CSSTransition) animation.finish();
    }
    await frame();
    if (!original) {
      original = terminal;
      if (!original) throw new Error("Terminal did not mount");
      await new Promise<void>(resolve => original!.write("Scrollback survives material changes.\r\n\x1b[32mTests passed.\x1b[0m\r\n\x1b[44mExplicit ANSI backgrounds remain intact.\x1b[0m\r\n", resolve));
    }
    if (terminal !== original || mounts !== 1 || disposals !== 0) throw new Error("Material change recreated xterm");
    if (!original.readLines().some(line => line.includes("Scrollback survives"))) throw new Error("Material change lost scrollback");
    const inspect = (selector: string) => {
      const node = document.querySelector<HTMLElement>(selector)!;
      const style = getComputedStyle(node);
      return { background: style.backgroundColor, color: style.color, opacity: style.opacity };
    };
    return { value, mounts, disposals,
      active: getComputedStyle(document.documentElement).getPropertyValue("--terminal-glass-active").trim(),
      pane: inspect(".terminal-pane"), sidebar: inspect(".sidebar"),
      projectHeader: inspect(".group-header-row"),
      head: inspect(".terminal-pane-head"), idleHead: inspect(".terminal-pane:not(.is-focused) .terminal-pane-head"),
      focusedPane: inspect(".terminal-pane.is-focused"), idlePane: inspect(".terminal-pane:not(.is-focused)"),
      xterm: inspect(".xterm-scrollable-element"), viewport: inspect(".xterm-viewport"),
      body: inspect(".terminal-pane-body"), view: inspect(".terminal-view"),
      canvas: inspect(".terminal-center"), workspace: inspect(".workspace-content") };
  };
}

/** Exercise actual project headers at rest and while pinned. elementFromPoint
 * must never hit a scrolling row in the sticky header's transparent gutter. */
export async function sidebarGlassScrollFixture() {
  const scroller = document.querySelector<HTMLElement>(".sidebar .session-list")!;
  const reports = [];
  for (const top of [0, 12, 40, 100, 180, 0]) {
    scroller.scrollTop = top;
    await frame(); await frame();
    const head = scroller.querySelector<HTMLElement>(".group-header-row")!;
    const rows = scroller.querySelector<HTMLElement>(".group > .session-items")!;
    const bounds = head.getBoundingClientRect();
    const clip = Number.parseFloat(rows.style.getPropertyValue("--sticky-header-clip"));
    const expected = Math.max(0, bounds.bottom - rows.getBoundingClientRect().top);
    if (Math.abs(clip - expected) > 0.5) throw new Error(`Sticky header clipping lagged at ${top}: ${clip} / ${expected}`);
    const paintedClip = getComputedStyle(rows).clipPath;
    const paintedTop = Number.parseFloat(paintedClip.replace("inset(", ""));
    if (!paintedClip.startsWith("inset(") || Math.abs(paintedTop - expected) > 0.5) {
      throw new Error(`Sticky header mask is not painted: ${paintedClip} / ${expected}`);
    }
    const hit = document.elementFromPoint(bounds.left + 1, bounds.bottom - 1);
    if (hit?.closest(".session-items")) throw new Error("Scrolling text paints through the glass project header");
    reports.push({ top: scroller.scrollTop, clip, expected, paintedClip });
  }
  if (!reports.some(report => report.clip > 0)) throw new Error("Sidebar never scrolled beneath its sticky header");
  return reports;
}

/** Sample the first frame as well as subsequent frames, with normal motion.
 * Never finish/cancel animations on behalf of the product in this probe. */
export async function readableSurfaceFrames() {
  const surface = document.createElement("div");
  surface.innerHTML = '<div class="workspace-surface conversation-pane-surface">Readable conversation</div><div class="workspace-settings-overlay">Readable settings</div>';
  document.body.append(surface);
  const samples = [];
  try {
    for (let i = 0; i < 5; i++) {
      samples.push([...surface.children].map(node => {
        const style = getComputedStyle(node);
        if (style.opacity !== "1" || style.transform !== "none" || style.animationName !== "none") {
          throw new Error(`Reading surface fades or moves: ${node.className} ${style.opacity} ${style.transform} ${style.animationName}`);
        }
        return { opacity: style.opacity, transform: style.transform, animation: style.animationName };
      }));
      await frame();
    }
  } finally { surface.remove(); }
  return samples;
}

export async function materialOverlayProbe(show: boolean): Promise<void> {
  document.getElementById("material-overlay-probe")?.remove();
  if (show) {
    const overlay = document.createElement("div");
    overlay.id = "material-overlay-probe";
    overlay.className = "workspace-settings-overlay";
    overlay.innerHTML = '<div class="workspace-settings-card" style="background:#ff00ff">Dismissed content must disappear</div>';
    document.querySelector(".workspace-content")!.append(overlay);
  }
  await frame(); await frame();
}
