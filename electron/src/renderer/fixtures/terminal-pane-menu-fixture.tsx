import { flushSync } from "react-dom";
import type { Root } from "react-dom/client";
import { useState } from "react";
import { TerminalPaneMoreMenu } from "../components/TerminalPane";
import { TerminalPaneNewMenu } from "../components/TerminalPaneNewMenu";
import type { TerminalLaunchProfile } from "../../shared/ipc";
import { assert, frame } from "./fixture-support";

export async function terminalPaneMenuFixture(root: Root): Promise<unknown> {
  document.documentElement.dataset.theme = "dark";
  document.documentElement.dataset.chrome = "refined";
  const events: string[] = [];
  const profiles: TerminalLaunchProfile[] = [
    { id: "shell", label: "Shell", available: true },
    { id: "claude", label: "Claude Code", available: true },
    { id: "codex", label: "Codex", available: true },
    { id: "opencode", label: "OpenCode", available: false, unavailableReason: "Not installed" },
  ];
  function Fixture() {
    const [open, setOpen] = useState(false);
    const [moreOpen, setMoreOpen] = useState(false);
    return <div style={{ height: "100vh", padding: 24, boxSizing: "border-box", background: "var(--terminal-canvas)" }}>
      <div style={{ display: "flex", justifyContent: "flex-end" }}>
        <TerminalPaneMoreMenu open={moreOpen} onOpenChange={setMoreOpen}
          onSplit={direction => { events.push(`split:${direction}`); }} />
        <TerminalPaneNewMenu open={open} onOpenChange={setOpen} cwd="/fixture/project"
          profiles={profiles} onChoose={id => { events.push(`pane:${id}`); }}
          onNewTab={() => { events.push("tab"); }} onNewSession={() => { events.push("session"); }}
          onOpenTool={tool => { events.push(tool); }}
          onSplitDown={() => { events.push("split:vertical"); }}
          onLoad={() => { events.push("refresh"); }} />
      </div>
    </div>;
  }
  flushSync(() => root.render(<Fixture />));
  // A printed chord is aria-hidden beside the label, not part of it.
  const rowLabel = (row: Element): string => [...row.childNodes]
    .filter(node => !(node instanceof Element && node.classList.contains("terminal-menu-shortcut")))
    .map(node => node.textContent).join("").trim();
  const open = async (trigger = "New terminal in this pane",
      name = "Open in a new pane"): Promise<HTMLElement> => {
    const button = document.querySelector<HTMLButtonElement>(`[aria-label="${trigger}"]`);
    assert(button, `Missing ${trigger}`);
    flushSync(() => button.click()); await frame(); await frame(); await frame();
    const menu = document.querySelector<HTMLElement>(`[role="menu"][aria-label="${name}"]`);
    assert(menu, `${name} did not open`);
    const bounds = menu.getBoundingClientRect();
    assert(bounds.left >= 0 && bounds.right <= innerWidth + 1 && bounds.bottom <= innerHeight + 1,
      "Pane menu escaped the viewport");
    const deadline = performance.now() + 1500;
    while (!menu.contains(document.activeElement)) {
      assert(performance.now() < deadline, `Menu focus failed after ${events.at(-1)}: ${document.activeElement?.outerHTML.slice(0, 300)}`);
      await frame();
    }
    return menu;
  };
  const choose = async (label: string, expected: string,
      trigger?: string, name = "Open in a new pane"): Promise<void> => {
    const menu = await open(trigger, name);
    const row = [...menu.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')]
      .find(button => rowLabel(button) === label);
    assert(row && !row.disabled, `Unavailable menu item: ${label}`);
    flushSync(() => row.click()); await frame();
    assert(events.at(-1) === expected, `${label} routed to ${events.at(-1)}`);
    assert(!document.querySelector(`[role="menu"][aria-label="${name}"]`), `${label} did not dismiss menu`);
  };
  for (const [label, expected] of [
    ["Terminal", "pane:shell"], ["Files", "files"],
    ["iOS Simulator", "ios-simulator"], ["Claude Code", "pane:claude"], ["Codex", "pane:codex"],
    ["Split down", "split:vertical"], ["New tab", "tab"], ["New session…", "session"],
  ]) await choose(label, expected);
  await choose("Split right", "split:horizontal", "More pane actions", "Pane actions");
  // Browser opens a tab in this pane, not a dialog (the tab itself is the
  // terminal interaction fixture's: it needs the whole terminal centre).
  await choose("Browser", "browser"); await frame();
  assert(!document.querySelector('[role="dialog"]'), "Browser opened a dialog instead of a tab");
  const menu = await open();
  const unavailable = menu.querySelector<HTMLButtonElement>('[data-terminal-launch-profile="opencode"]');
  assert(unavailable?.disabled && unavailable.title === "Not installed", "Unavailable harness was launchable");
  assert(!menu.textContent?.includes("Cursor"), "Menu invented an unsupported harness");
  const heading = menu.querySelector(".terminal-pane-menu-heading");
  assert(heading?.textContent === "New pane/fixture/project", `Wrong menu heading: ${heading?.textContent}`);
  const shortcut = (label: string): string => [...menu.querySelectorAll('[role="menuitem"]')]
    .find(row => rowLabel(row) === label)?.querySelector(".terminal-menu-shortcut")?.textContent ?? "";
  assert(shortcut("Terminal") === "⌘D" && shortcut("Split down") === "⌘⇧D" && shortcut("Files") === "",
    "Menu printed a chord that does not exist");
  const first = menu.querySelector<HTMLButtonElement>('[role="menuitem"]'); first?.focus();
  first?.dispatchEvent(new KeyboardEvent("keydown", { key: "End", bubbles: true }));
  assert(document.activeElement?.textContent === "New session…", "End did not reach last action");
  document.activeElement?.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  await frame(); await frame();
  assert(document.activeElement?.getAttribute("aria-label") === "New terminal in this pane", "Escape did not restore + focus");
  const finalMenu = await open();
  await document.fonts.ready; await frame(); await frame();
  assert(getComputedStyle(finalMenu.querySelector('button')!).visibility === "visible", "Menu labels are hidden");
  // A menu with room to spare shows every row without a scroll gutter. Its
  // fractional heading once left the measured height half a pixel short.
  const spare = innerHeight - finalMenu.getBoundingClientRect().bottom;
  finalMenu.scrollTop = 50;
  assert(spare < 16 || finalMenu.scrollTop === 0, `A menu that fits still scrolls by ${finalMenu.scrollTop}px`);
  finalMenu.scrollTop = 0;
  return { actions: 11, existingHarnessesOnly: true, browserOpensTab: "passed", keyboard: "passed", viewport: [innerWidth, innerHeight] };
}
