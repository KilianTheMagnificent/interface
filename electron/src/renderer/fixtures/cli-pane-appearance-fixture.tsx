import { flushSync } from "react-dom";
import type { Root } from "react-dom/client";
import type { PanePresentation } from "../../shared/appearance";
import { TerminalPane } from "../components/TerminalPane";
import { TerminalView, type TerminalViewHandle } from "../components/TerminalView";
import { APP_SELECT_CSS } from "../components/AppSelect";
import { assert, frame, noop } from "./fixture-support";

/** Four real emulators stay mounted through the same appearance path used by Settings. */
export function createCliPaneAppearanceFixture(root: Root) {
  const handles = new Map<string, TerminalViewHandle>();
  const originals = new Map<string, TerminalViewHandle>();
  let nodes: Element[] = [];
  const inputs: string[] = [];
  let mounts = 0;
  let disposals = 0;
  const ids = ["cli-1", "cli-2", "cli-3", "cli-4"];
  const titles = ["Refine the conversation panes", "Review keyboard navigation", "Check narrow layouts", "Inspect changed files"];
  let claudeGeometry = '';
  const redrawClaude = async (): Promise<void> => {
    const claude = handles.get('cli-2')!;
    claude.fit();
    const size = claude.size()!;
    const geometry = `${size.columns}:${size.rows}`;
    if (geometry === claudeGeometry) return;
    claudeGeometry = geometry;
    // Full-screen programs redraw on PTY resize; unlike the normal buffer,
    // xterm cannot reflow their old cells. Model that real producer behavior.
    const transcript = size.rows < 12
      ? '⏺ Ready.\r\nKeyboard controls stay native.\r\n'
      : '⏺ I am checking the panes.\r\n\r\n' +
        '⏺ Bash(npm test)\r\n  ⎿  All tests passed\r\n\r\n' +
        '⏺ The CLI stays connected while its output becomes easier to read.\r\n\r\n' +
        'Keyboard controls stay native.\r\n';
    await new Promise<void>(resolve => claude.write(
      '\x1b[?1049h\x1b[H\x1b[2J' + transcript +
      `\x1b[${Math.max(1, size.rows - 1)};1H❯ Draft for cli-2`, resolve));
  };
  let initialized = false;
  return async (style: PanePresentation["paneStyle"], theme = "dark", chrome = "refined") => {
    document.documentElement.dataset.theme = theme;
    document.documentElement.dataset.chrome = chrome;
    if (!initialized) {
      flushSync(() => root.render(<div className="terminal-center" style={{ height: "100vh" }}>
        <style>{APP_SELECT_CSS}</style>
        <div className="terminal-workspace is-equal-grid terminal-count-4">
          {ids.map((id, index) => <TerminalPane key={id}
            pane={{ id, tabs: [{ terminalId: id }], activeTerminalId: id }}
            terminals={new Map([[id, { id, title: titles[index], cwd: "/fixture/project",
              status: "running", profileId: index % 2 ? "claude" : "codex" }]])}
            focused={index === 0} maximized={false} onFocus={noop} onSelectTab={noop}
            onCloseTab={noop} onMoveTerminal={noop} onRequestSplit={noop} onToggleMaximize={noop}
            renderTerminal={() => <TerminalView terminalId={id} active onData={data => inputs.push(`${id}:${data}`)}
              onResize={() => {
                // Font changes can finish fitting after the appearance frames.
                // A real fullscreen CLI redraws on every PTY resize, including those late fits.
                if (id === 'cli-2' && initialized) queueMicrotask(() => { void redrawClaude(); });
              }}
              onReady={handle => {
                if (handle) { handles.set(id, handle); mounts++; handle.completeSetup(); }
                else { handles.delete(id); disposals++; }
              }} />} />)}
        </div>
      </div>));
      await frame(); await frame();
      nodes = [...document.querySelectorAll(".xterm")];
      assert(nodes.length === 4 && handles.size === 4, "Four CLI panes did not mount");
      for (const [id, handle] of handles) {
        originals.set(id, handle);
        const lines = Array.from({ length: 45 }, (_, i) => `${id} retained line ${i}`).join("\r\n");
        const toolResult = JSON.stringify({ files: Array.from({ length: 16 }, (_, n) =>
          ({ path: `components/pane-${n}.tsx`, checked: true, details: 'Fixture command output retained in full' })) });
        await new Promise<void>(resolve => handle.write(`${lines}\r\n\r\n› Make the panes easier to read.\r\n\r\n`+
          `• I am checking how the existing panes render output.\r\n\r\n`+
          `• Called\r\n  read_files({"path":"components"})\r\n${toolResult}\r\n\r\n`+
          `• The reading layout is ready.\r\n  Each pane keeps the same live CLI, keyboard controls and history.\r\n\r\n`+
          `• Ran cargo check\r\n  └ All checks passed.\r\n\r\n`+
          `• You can expand the command details above and continue typing below.\r\n\r\n› Draft for ${id}`, resolve));
      }
      // A fullscreen CLI has a second buffer; changing appearance must keep both.
      await redrawClaude();
      await new Promise<void>(resolve => handles.get("cli-4")!.write(
        "\x1b[?1049h\x1b[H\x1b[2JCLI menu\r\n> Continue\r\n  Cancel", resolve));
      initialized = true;
    }
    // Settings' appearance path ends in this event; a legacy Clean value adds
    // nothing to it, so every style must leave the same native panes.
    document.documentElement.dispatchEvent(new CustomEvent("interface-theme-change"));
    await document.fonts.ready;
    await frame(); await frame();
    for (const handle of handles.values()) handle.fit();
    await frame(); await frame();
    const head = document.querySelector<HTMLElement>(".terminal-pane-head")!;
    const body = document.querySelector<HTMLElement>(".terminal-xterm")!;
    // Roomier tiles (owner, 2026-10-02): text 12px from the tile edge, 10px under the head.
    const expectedPadding = "10px 0px 2px 12px";
    const deadline = performance.now() + 3000;
    // Hidden Chromium can expose the previous frame's inherited layout even
    // after rAF. Verify the actual boxes before recording a successful switch.
    while (getComputedStyle(body).padding !== expectedPadding
      || getComputedStyle(head).height !== "32px") {
      assert(performance.now() < deadline, `CLI ${style}/${theme}/${chrome}/${innerWidth} layout did not settle: padding=${getComputedStyle(body).padding}, head=${head.offsetHeight}`);
      await new Promise(resolve => setTimeout(resolve, 20));
    }
    await redrawClaude();
    assert(!document.querySelector('.terminal-clean-reader, .terminal-clean-composer, .terminal-conversation'),
      'A retired Clean overlay remains mounted');
    for (const native of document.querySelectorAll<HTMLElement>('.terminal-xterm')) {
      assert(!native.inert && native.style.clipPath === '', 'Native input or output is hidden');
    }
    // The cell grid ends at the bottom inset at every fitted size.
    for (const pane of document.querySelectorAll<HTMLElement>('.terminal-pane')) {
      const edge = getComputedStyle(pane, '::after');
      const content = pane.querySelector<HTMLElement>('.terminal-view')!;
      const screen = pane.querySelector<HTMLElement>('.xterm-screen')!;
      assert(Math.abs(screen.getBoundingClientRect().left - content.getBoundingClientRect().left - 12) <= 1,
        'Native terminal text lost its 12px left inset');
      while (Math.abs(content.getBoundingClientRect().bottom - screen.getBoundingClientRect().bottom - 2) > 1) {
        assert(performance.now() < deadline, 'Terminal footer is not aligned to its 2px bottom inset');
        await new Promise(resolve => setTimeout(resolve, 20));
      }
      assert(edge.content !== 'none' && edge.borderStyle === 'solid' && parseFloat(edge.borderWidth) >= 1,
        'An unselected pane lost its visible edge');
      assert(edge.pointerEvents === 'none' && parseInt(edge.zIndex) > 2,
        'Pane edges must remain visible without intercepting input');
      assert(screen.getBoundingClientRect().top >= content.getBoundingClientRect().top,
        `Bottom alignment clipped the first terminal row: ${theme}/${chrome}/${innerWidth}, screen=${screen.getBoundingClientRect().top}, content=${content.getBoundingClientRect().top}`);
    }
    const currentNodes = [...document.querySelectorAll(".xterm")];
    assert(mounts === 4 && disposals === 0, "Appearance recreated an emulator");
    assert(nodes.every((node, i) => node === currentNodes[i]), "Appearance replaced terminal DOM");
    for (const [id, original] of originals) {
      assert(handles.get(id) === original, "Appearance changed terminal identity");
      const text = original.readLines().join("\n");
      assert(text.includes(`${id} retained line 0`), `Appearance lost ${id} scrollback`);
      assert(text.includes(`Draft for ${id}`), `Appearance lost ${id} draft display`);
      const count = inputs.length;
      original.paste(`input-${style}-${id}`);
      assert(inputs.length === count + 1 && inputs.at(-1)?.includes(`${id}:input-${style}-${id}`),
        "Input did not stay routed to its original CLI");
    }
    assert(handles.get("cli-4")!.readLines().includes("CLI menu"), "Appearance lost fullscreen CLI state");
    await document.fonts.ready;
    for (const handle of handles.values()) handle.fit();
    await frame(); await frame();
    const screen = body.querySelector<HTMLElement>(".xterm-screen")!;
    const cellHeight = screen.getBoundingClientRect().height / handles.get("cli-1")!.size()!.rows;
    return { style, panes: 4, mounts, disposals, inputRouting: "preserved", scrollback: "preserved",
      alternateScreen: "preserved", cellHeight, headHeight: head.getBoundingClientRect().height,
      readingSize: getComputedStyle(body).getPropertyValue("--terminal-reading-size").trim() || "11.5",
      bodyPadding: getComputedStyle(body).padding };
  };
}
