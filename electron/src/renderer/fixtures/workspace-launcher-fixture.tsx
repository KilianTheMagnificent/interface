import { flushSync } from "react-dom";
import type { Root } from "react-dom/client";
import { isResolvedTheme, themeKind } from "../../shared/appearance";
import type { TerminalLaunchProfile } from "../../shared/ipc";
import { TerminalLauncher, type TerminalLauncherProps } from "../components/TerminalLauncher";
import { projectCreationFixture } from "./project-creation-fixture";
import { assert, frames as settle } from "./fixture-support";

const profiles: TerminalLaunchProfile[] = [
  { id: "claude", engine: "claude", label: "Claude Code", available: true },
  { id: "codex", engine: "codex", label: "Codex", available: true },
  { id: "grok", engine: "acp:grok", label: "Grok", available: true },
  { id: "gemini", engine: "acp:gemini", label: "Antigravity", available: false,
    unavailableReason: "Antigravity CLI was not found.",
    installHint: "Install Antigravity so `agy` runs in your login shell, then press Refresh." },
  { id: "omp", engine: "acp:omp", label: "OMP", available: true },
  { id: "opencode", engine: "acp:opencode", label: "OpenCode", available: true },
  { id: "shell", label: "Terminal", available: true },
];

export async function workspaceLauncherFixture(root: Root, theme: string, chrome: string): Promise<unknown> {
  assert(isResolvedTheme(theme), `Unknown theme ${theme}`);
  document.documentElement.dataset.theme = theme;
  document.documentElement.dataset.chrome = chrome;
  document.documentElement.style.colorScheme = themeKind(theme);
  await projectCreationFixture(root);
  const launches: unknown[] = [];
  const folders: string[] = [];
  let cancels = 0;
  let browses = 0;
  let refreshes = 0;
  let failBrowse = false;
  const picked = "/work/Shared projects/a very long project folder with spaces/Interface workspace";
  const base: TerminalLauncherProps = {
    profiles, profilesLoading: false, cwd: "/work/Interface", launching: false, notice: null,
    defaultProfileId: "codex", defaultCount: 1,
    // Each provider's plan headroom under its name (review E).
    headroom: { claude: { text: "38% left", title: "Claude Code plan: 62% of the 5-hour window used" },
      codex: { text: "~81% left", title: "Codex plan: 19% of the 5-hour window used (read 7m ago)" } },
    onLaunch: (...args) => { launches.push(args); },
    onBrowse: async () => { browses++; if (failBrowse) throw new Error("Fixture picker failed"); return picked; },
    onCwdChange: cwd => { folders.push(cwd); },
    onCancel: () => { cancels++; }, onRetryProfiles: () => { refreshes++; },
  };
  const render = (props: Partial<TerminalLauncherProps> = {}, key = "workspace") => {
    flushSync(() => root.render(<div style={{ container: "workspace / inline-size", height: "100vh" }}>
      <TerminalLauncher key={key} {...base} {...props} />
    </div>));
  };
  render(); await document.fonts.ready; await settle();
  const get = (selector: string): HTMLButtonElement => {
    const button = document.querySelector<HTMLButtonElement>(selector);
    assert(button, `Missing launcher control ${selector}`); return button;
  };
  const profile = (id: string) => get(`[data-terminal-launch-profile="${id}"]`);
  const count = (n: number) => get(`.terminal-launcher-count[aria-label="${n} terminal${n === 1 ? "" : "s"}"]`);
  const lineup = () => [...document.querySelectorAll('.terminal-launcher-lineup-label')].map(node => node.textContent);
  const seats = () => document.querySelector('.terminal-launcher-lineup-count')?.textContent;
  assert(document.activeElement === profile("codex"), "Launcher did not focus its remembered agent");
  assert(lineup().join() === "Codex", "Opening lineup differs from the selected agent");
  // A missing tool other than Claude Code, Codex and Terminal is no tile: it
  // folds under a closed More with its install hint and a Refresh (review C4).
  assert(!document.querySelector('[data-terminal-launch-profile="gemini"]'), "A missing agent kept its tile");
  const more = document.querySelector<HTMLDetailsElement>('.terminal-launcher-more');
  assert(more && !more.open && !more.closest('[role="radiogroup"]'), "Missing agents did not fold under a closed More");
  more.querySelector<HTMLElement>('summary')!.click(); await settle();
  const folded = more.querySelector<HTMLElement>('[data-terminal-launch-folded="gemini"]');
  assert(more.open && folded !== null && folded.getBoundingClientRect().height > 0, "More did not open on its folded agents");
  assert(folded.textContent?.includes("Install Antigravity so agy runs in your login shell, then press Refresh.")
    && folded.querySelector('code')?.textContent === "agy", "Folded agent hid its install hint");
  get('.terminal-launcher-more-refresh').click(); await settle();
  assert(refreshes === 1 && lineup().join() === "Codex", "Refresh did not rebuild the roster or changed the preview");
  count(8).click(); await settle();
  assert(lineup().join() === "Codex" && seats() === "× 8", "Eight seats were not previewed as one line");
  profile("opencode").click(); await settle();
  assert(lineup().every(label => label === "OpenCode"), "Provider selection left a stale preview");
  // Arrow navigation skips the unavailable provider and updates the same preview.
  profile("grok").focus(); profile("grok").click(); await settle();
  profile("grok").dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
  await settle();
  assert(document.activeElement === profile("omp") && lineup().every(label => label === "OMP"),
    "Keyboard navigation did not skip the unavailable agent");
  profile("codex").click(); count(8).click(); await settle();
  get('.terminal-launcher-cwd-browse').click(); await settle();
  assert(folders[0] === picked && document.querySelector('.terminal-launcher-cwd-path')?.textContent === picked,
    "Folder picker did not update the displayed launch destination");
  get('.terminal-launcher-actions .btn-primary').click(); await settle();
  assert(JSON.stringify(launches) === JSON.stringify([["codex", 8, picked]]), "Launch disagreed with the preview or folder");
  // ⌘↵ from anywhere in the launcher is the same launch; a bare Enter on a tile only selects it.
  profile("codex").dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
  await settle();
  assert(launches.length === 1, "A bare Enter launched");
  profile("codex").dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", metaKey: true, bubbles: true, cancelable: true }));
  await settle();
  assert(JSON.stringify(launches[1]) === JSON.stringify(["codex", 8, picked]), "⌘↵ did not launch the shown lineup");
  render({ launching: true }); await settle();
  assert([...document.querySelectorAll<HTMLButtonElement>('.terminal-launcher button')].every(b => b.disabled),
    "A launcher control remained enabled during launch");
  render(); await settle();
  failBrowse = true;
  get('.terminal-launcher-cwd-browse').click(); await settle();
  assert(document.querySelector('[role="alert"]')?.textContent?.includes("folder picker"), "Picker failure was hidden");
  assert(document.querySelector('.terminal-launcher-cwd-path')?.textContent === picked, "Picker failure discarded the folder");
  get('.terminal-launcher-close').dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  await settle(); assert(cancels === 1, "Escape did not dismiss the launcher");
  render({ profiles: [], profilesLoading: true }, "loading"); await settle();
  assert(!document.querySelector('.terminal-launcher-lineup') && get('.btn-primary').disabled,
    "An empty roster offered a launch or fabricated a lineup");
  // Finish with the reference's single selected seat for visual inspection.
  render({ cwd: picked }, "visual"); await settle();
  // Open, so the folded list is inside the bounds and size checks below.
  document.querySelector<HTMLDetailsElement>('.terminal-launcher-more')!.open = true; await settle();
  const launcher = document.querySelector<HTMLElement>('.terminal-launcher')!;
  const grid = document.querySelector<HTMLElement>('.terminal-launcher-agent-grid')!;
  const columns = getComputedStyle(grid).gridTemplateColumns.split(' ').length;
  // Four columns from 900px up (owner, 2026-09-12), so seven tiles never orphan one.
  assert(columns === (innerWidth <= 460 ? 1 : innerWidth <= 700 ? 2 : innerWidth < 900 ? 3 : 4), "Unexpected responsive provider columns");
  assert(launcher.scrollWidth <= launcher.clientWidth + 1, "Launcher has horizontal overflow");
  const bounds = launcher.getBoundingClientRect();
  for (const control of launcher.querySelectorAll<HTMLButtonElement>('button')) {
    const box = control.getBoundingClientRect();
    assert(box.left >= bounds.left && box.right <= bounds.right + 1, `Control leaves launcher: ${control.ariaLabel}`);
    assert(box.height >= 24 && box.width >= 24, `Control is too small: ${control.ariaLabel}`);
  }
  const counts = [...document.querySelectorAll('.terminal-launcher-count')].map(node => node.getBoundingClientRect());
  assert(counts.length === 8, "Launcher omitted one of the eight session choices");
  assert(new Set(counts.map(box => box.top)).size === (innerWidth <= 640 ? 2 : 1),
    "Session count choices did not use the expected responsive rows");
  get('.btn-primary').scrollIntoView({ block: "nearest" }); await settle();
  assert(get('.btn-primary').getBoundingClientRect().bottom <= innerHeight + 1, "Launch button cannot be reached by scrolling");
  launcher.scrollTop = 0;
  (document.activeElement as HTMLElement)?.blur(); await settle();
  return { theme, chrome, width: innerWidth, columns, launches, browses, cancels, refreshes, horizontalOverflow: false };
}
