import { flushSync } from "react-dom";
import type { Root } from "react-dom/client";
import { isResolvedTheme, themeKind } from "../../shared/appearance";
import { APP_SELECT_CSS } from "../components/AppSelect";
import { TerminalPane } from "../components/TerminalPane";
import { TerminalView, type TerminalViewHandle } from "../components/TerminalView";
import "../terminal.css";
import { frame, noop } from "./fixture-support";

/** Compare rendered component structure, including decorations, not only root tokens. */
export function themeStructure(selectors: string): unknown {
  const properties = ["display", "position", "flex-direction", "align-items", "justify-content",
    "gap", "padding", "margin", "border-radius", "border-width", "border-style",
    "font-size", "font-weight", "line-height", "letter-spacing", "text-transform",
    "box-shadow", "backdrop-filter", "background-image", "animation-name"];
  return [...document.querySelectorAll<HTMLElement>(selectors)].map(element => ({
    className: element.className,
    styles: Object.fromEntries(properties.map(property => [property,
      // Shadows may change color, but their offsets, spread and blur stay fixed.
      getComputedStyle(element).getPropertyValue(property).replace(/(?:rgba?|color)\([^)]*\)/gu, "COLOR"),
    ])),
    before: getComputedStyle(element, "::before").content,
    after: getComputedStyle(element, "::after").content,
  }));
}

/** Keep one actual xterm alive while every palette is applied to both pane states. */
export function createTerminalThemeFixture(root: Root): (theme: string) => Promise<unknown> {
  let current: TerminalViewHandle | null = null;
  let original: TerminalViewHandle | null = null;
  let originalNode: Element | null = null;
  let mounts = 0;
  let disposals = 0;
  let disableStdin = false;
  const render = (): void => {
    flushSync(() => root.render(<div className="terminal-center" style={{ height: "100vh" }}>
      <style>{APP_SELECT_CSS}</style>
      <div className="terminal-workspace is-equal-grid terminal-count-2">
        {[true, false].map(focused => <TerminalPane key={String(focused)}
            pane={{ id: String(focused), tabs: [{ terminalId: "theme-fixture" }], activeTerminalId: "theme-fixture" }}
            terminals={new Map([["theme-fixture", { id: "theme-fixture", title: focused ? "OpenCode · Focused" : "OpenCode · Idle", cwd: "/fixture", status: "running", profileId: "opencode" }]])}
            focused={focused} maximized={false} onFocus={noop} onSelectTab={noop} onCloseTab={noop}
            onMoveTerminal={noop} onRequestSplit={noop} onToggleMaximize={noop}
            renderTerminal={() => focused
              ? <TerminalView terminalId="theme-fixture" active options={{ disableStdin }} onData={noop} onReady={handle => {
                current = handle;
                if (handle) { mounts++; handle.completeSetup(); } else disposals++;
              }} />
              : <div style={{ padding: 18 }}>The same pane structure, with an idle header.</div>} />)}
      </div>
    </div>));
  };
  return async theme => {
    if (!isResolvedTheme(theme)) throw new Error(`Unknown fixture theme ${theme}`);
    document.documentElement.dataset.theme = theme;
    document.documentElement.dataset.chrome = "refined";
    document.documentElement.style.colorScheme = themeKind(theme);
    if (original === null) {
      render();
      await frame(); await frame();
      original = current;
      originalNode = document.querySelector(".xterm");
      if (original === null || originalNode === null) throw new Error("Theme fixture terminal did not mount");
      await new Promise<void>(resolve => original!.write(new TextEncoder().encode("Theme switch keeps this terminal and its scrollback.\r\n"), resolve));
    }
    document.documentElement.dispatchEvent(new CustomEvent("interface-theme-change", { detail: theme }));
    // A status/options update before the pending frame must not lose the theme.
    disableStdin = !disableStdin;
    render();
    await frame(); await frame();
    const viewport = document.querySelector<HTMLElement>(".xterm-scrollable-element");
    const probe = document.createElement("span");
    probe.style.backgroundColor = "var(--terminal-content-bg)";
    document.body.append(probe);
    const expected = getComputedStyle(probe).backgroundColor;
    probe.remove();
    // Hidden Chromium can retain computed styles past rAF even after xterm's
    // inline style updates. Require the actual painted color, with a deadline.
    const deadline = performance.now() + 3000;
    while (!viewport || getComputedStyle(viewport).backgroundColor !== expected) {
      if (performance.now() >= deadline) throw new Error(`${theme}: live xterm background ${viewport && getComputedStyle(viewport).backgroundColor}, expected ${expected}`);
      await new Promise(resolve => setTimeout(resolve, 20));
    }
    if (current !== original || document.querySelector(".xterm") !== originalNode || mounts !== 1 || disposals !== 0) {
      throw new Error(`${theme}: theme switch recreated the terminal`);
    }
    if (!original.readLines().some(line => line.includes("Theme switch keeps"))) throw new Error(`${theme}: theme switch lost scrollback`);
    return { background: expected, mounts, disposals,
      structure: themeStructure(".terminal-pane,.terminal-pane-head,.terminal-tab,.terminal-pane-body,.terminal-view") };
  };
}

/** Inspect captured pixels: hidden Chromium can expose the preceding frame. */
export async function terminalCaptureMatches(dataUrl: string): Promise<{ matches: boolean; samples: unknown[] }> {
  const screenshot = new Image();
  screenshot.src = dataUrl;
  await screenshot.decode();
  const canvas = document.createElement("canvas");
  canvas.width = canvas.height = 1;
  const context = canvas.getContext("2d", { willReadFrequently: true });
  if (context === null) throw new Error("Screenshot pixel reader unavailable");
  const samples = [".terminal-center", ".terminal-pane.is-focused .terminal-pane-head", ".xterm-scrollable-element", ".xterm-viewport"].map(selector => {
    const element = document.querySelector<HTMLElement>(selector);
    if (element === null) throw new Error(`Missing screenshot sample ${selector}`);
    const bounds = element.getBoundingClientRect();
    const emulator = selector === '.xterm-viewport' || selector === '.xterm-scrollable-element';
    // Both terminal fixtures leave the final row's right-hand cells blank.
    // Sampling mid-screen can hit glyphs or the Clean reader after row fitting.
    // With no outer frame/inset, the old top-left sample lands on a pane
    // corner. Measure the exposed canvas between the first pair of panes.
    // Porcelain's focused frame lifts on a soft shadow that falls into the
    // gutter beside it, so prefer a gutter between two unfocused panes.
    const panes = [...document.querySelectorAll<HTMLElement>('.terminal-pane')]
      .map(pane => ({ rect: pane.getBoundingClientRect(), focused: pane.classList.contains('is-focused') }))
      .filter(pane => pane.rect.width > 0 && pane.rect.height > 0);
    const calmPair = panes.findIndex((pane, index) => index + 1 < panes.length
      && !pane.focused && !panes[index + 1]!.focused);
    const pairStart = calmPair >= 0 ? calmPair : 0;
    const first = panes[pairStart]!.rect;
    const second = panes[pairStart + 1]!.rect;
    const sideBySide = second.left >= first.right;
    const x = selector === '.terminal-center'
      ? sideBySide ? (first.right + second.left) / 2 : first.left + first.width / 2
      : emulator ? bounds.right - 20 : bounds.left + bounds.width / 2;
    const y = selector === '.terminal-center'
      ? sideBySide ? Math.max(first.top, second.top) + 3 : (first.bottom + second.top) / 2
      : emulator ? bounds.bottom - 3 : bounds.top + 3;
    context.clearRect(0, 0, 1, 1);
    context.drawImage(screenshot, Math.floor(x * screenshot.naturalWidth / innerWidth),
      Math.floor(y * screenshot.naturalHeight / innerHeight), 1, 1, 0, 0, 1, 1);
    const actual = context.getImageData(0, 0, 1, 1).data;
    // Translucent header tokens paint over their ancestors, never over the
    // captured pixel itself (which would apply their alpha twice).
    context.clearRect(0, 0, 1, 1);
    const layers: HTMLElement[] = [];
    for (let layer: HTMLElement | null = element; layer !== null; layer = layer.parentElement) layers.unshift(layer);
    for (const layer of layers) {
      context.fillStyle = getComputedStyle(layer).backgroundColor;
      context.fillRect(0, 0, 1, 1);
    }
    const expected = context.getImageData(0, 0, 1, 1).data;
    // The WebGL surface can round an RGB channel by one 8-bit unit.
    return { selector, actual: [...actual], expected: [...expected],
      matches: actual.every((channel, index) => Math.abs(channel - expected[index]!) <= (index === 3 ? 0 : 1)) };
  });
  const idlePane = document.querySelector<HTMLElement>('.terminal-pane:not(.is-focused)');
  if (idlePane === null) throw new Error('Missing idle pane border sample');
  const bounds = idlePane.getBoundingClientRect();
  context.clearRect(0, 0, 1, 1);
  context.drawImage(screenshot, Math.floor((bounds.left + .5) * screenshot.naturalWidth / innerWidth),
    Math.floor((bounds.top + bounds.height / 2) * screenshot.naturalHeight / innerHeight), 1, 1, 0, 0, 1, 1);
  const actual = context.getImageData(0, 0, 1, 1).data;
  context.clearRect(0, 0, 1, 1);
  context.fillStyle = getComputedStyle(idlePane, '::after').borderLeftColor;
  context.fillRect(0, 0, 1, 1);
  const expected = context.getImageData(0, 0, 1, 1).data;
  samples.push({ selector: '.terminal-pane:not(.is-focused)', actual: [...actual], expected: [...expected],
    matches: actual.every((channel, index) => Math.abs(channel - expected[index]!) <= (index === 3 ? 0 : 1)) });
  return { matches: samples.every(sample => sample.matches), samples };
}
