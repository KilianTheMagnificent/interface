import { flushSync } from "react-dom";
import type { Root } from "react-dom/client";
import { CommandPalette, type CommandPaletteCommand } from "../components/CommandPalette";
import { assert, frames } from "./fixture-support";

const RECENTS_KEY = "interface.commandPalette.recents";

/**
 * The palette's highlight follows the command the user chose. A registry push
 * while it is open (a new agent, a renamed one) rebuilds the command list; the
 * highlight stays on the chosen command and Enter runs that command.
 */
export async function commandPaletteFixture(root: Root): Promise<Record<string, unknown>> {
  document.documentElement.dataset.theme = "dark";
  document.documentElement.dataset.chrome = "refined";
  try { localStorage.removeItem(RECENTS_KEY); } catch { /* storage-less context */ }
  const command = (id: string, label: string, group = "Workspace"): CommandPaletteCommand =>
    ({ id, label, group });
  const base = [command("go.chat", "Go to Chat"), command("go.code", "Go to Code"),
    command("go.agent", "Go to Agent"), command("open.library", "Search transcripts"),
    command("agent:claude#one", "Agent one", "Agents")];
  let query = "";
  let commands = base;
  const ran: string[] = [];
  const render = (): void => {
    flushSync(() => root.render(<CommandPalette open query={query} commands={commands}
      onQuery={next => { query = next; render(); }} onRun={chosen => { ran.push(chosen.id); }}
      onClose={() => undefined} />));
  };
  const highlighted = (): string => document.querySelector(
    '.command-palette-option[aria-selected="true"] .command-palette-label')?.textContent ?? "";
  const input = (): HTMLInputElement => {
    const field = document.querySelector<HTMLInputElement>(".command-palette-input");
    assert(field, "The palette has no search field"); return field;
  };
  const press = (key: string): void => {
    flushSync(() => input().dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true })));
  };
  try {
    render();
    await frames();
    assert(highlighted() === "Go to Chat", `The palette opened on ${highlighted()}`);
    press("ArrowDown");
    press("ArrowDown");
    await frames();
    assert(highlighted() === "Go to Agent", `Arrows landed on ${highlighted()}`);
    // A push: a command sorts in above, and every object is new.
    commands = [command("go.new", "New workspace"), ...base.map(entry => ({ ...entry })),
      command("agent:claude#two", "Agent two", "Agents")];
    render();
    await frames();
    const afterPush = highlighted();
    assert(afterPush === "Go to Agent", `A command push moved the highlight to ${afterPush}`);
    press("Enter");
    assert(ran.at(-1) === "go.agent", `Enter after a push ran ${ran.at(-1)}`);
    // The pointer chooses the same way.
    const library = [...document.querySelectorAll<HTMLButtonElement>(".command-palette-option")]
      .find(option => option.textContent?.includes("Search transcripts"));
    assert(library, "Missing the library command");
    flushSync(() => library.dispatchEvent(new MouseEvent("mouseover", { bubbles: true, relatedTarget: document.body })));
    await frames();
    assert(highlighted() === "Search transcripts", `Hover highlighted ${highlighted()}`);
    commands = commands.filter(entry => entry.id !== "go.new");
    render();
    await frames();
    assert(highlighted() === "Search transcripts", `A shrinking list moved the hovered highlight to ${highlighted()}`);
    // A new query starts again from the first match.
    query = "go";
    render();
    await frames();
    assert(highlighted() === "Go to Chat", `A new query kept the highlight on ${highlighted()}`);
    // A highlighted command that leaves the list falls back to the first row.
    press("ArrowDown");
    await frames();
    assert(highlighted() === "Go to Code", `Arrow after a query landed on ${highlighted()}`);
    commands = commands.filter(entry => entry.id !== "go.code");
    render();
    await frames();
    assert(highlighted() === "Go to Chat", `A removed command left the highlight on ${highlighted()}`);
    return { highlightAfterPush: afterPush, ran };
  } finally {
    flushSync(() => root.render(null));
    try { localStorage.removeItem(RECENTS_KEY); } catch { /* storage-less context */ }
  }
}
