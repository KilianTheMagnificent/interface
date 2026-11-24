import { describe, expect, it } from "vitest";
import {
  paletteCommandKeepsComposer, paletteThemes, shellPaletteCommands, type ShellPaletteState,
} from "./palette-commands";

const state = (over: Partial<ShellPaletteState> = {}): ShellPaletteState => ({
  newLabel: "New workspace", newDescription: "Run your CLI agents side by side.",
  attentionItems: 0, terminalsShown: false, openPanes: 0, focusedPane: false,
  maximized: false, sidebarOpen: true, launching: false, refreshing: false, ...over,
});
const command = (commands: ReturnType<typeof shellPaletteCommands>, id: string) => {
  const found = commands.find(entry => entry.id === id);
  expect(found, id).toBeDefined();
  return found!;
};

describe("the shell's palette commands", () => {
  it("opens an external terminal whenever Code's terminals show, and says why not elsewhere", () => {
    const shown = command(shellPaletteCommands(state({ terminalsShown: true })), "workspace.externalTerminal");
    expect(shown.disabled).toBe(false);
    const hidden = command(shellPaletteCommands(state()), "workspace.externalTerminal");
    expect(hidden).toMatchObject({ disabled: true, disabledReason: "Show Code's terminals first." });
  });

  it("explains every disabled command", () => {
    const commands = shellPaletteCommands(state({ launching: true, refreshing: true }));
    const disabled = commands.filter(entry => entry.disabled === true);
    expect(disabled.map(entry => entry.id)).toEqual([
      "harness.next", "workspace.externalTerminal", "workspace.next", "workspace.previous",
      "workspace.maximize", "workspace.close", "workspace.settings", "workspace.refresh",
    ]);
    for (const entry of disabled) expect(entry.disabledReason, entry.id).toMatch(/\.$/u);
  });

  it("follows the layout it would act on", () => {
    const one = shellPaletteCommands(state({ openPanes: 1, focusedPane: true }));
    expect(command(one, "workspace.next").disabled).toBe(true);
    expect(command(one, "workspace.maximize")).toMatchObject({ label: "Maximize pane", disabled: false });
    const two = shellPaletteCommands(state({ openPanes: 2, focusedPane: true, maximized: true }));
    expect(command(two, "workspace.previous").disabled).toBe(false);
    expect(command(two, "workspace.maximize").label).toBe("Restore layout");
    expect(command(shellPaletteCommands(state({ sidebarOpen: false })), "workspace.sidebar").label)
      .toBe("Show sidebar");
  });

  it("names New for the surface on screen and keeps the ids unique", () => {
    const commands = shellPaletteCommands(state({ newLabel: "New agent", newDescription: "Name it." }));
    expect(command(commands, "workspace.new")).toMatchObject({ label: "New agent", description: "Name it." });
    expect(new Set(commands.map(entry => entry.id)).size).toBe(commands.length);
  });

  it("offers one Task details row instead of four rows that opened the same dialog", () => {
    const commands = shellPaletteCommands(state());
    const task = commands.filter(entry => entry.group === "Task" && entry.id !== "harness.next");
    expect(task.map(entry => entry.label)).toEqual(["Task details…"]);
    expect(command(commands, "harness.results").keywords)
      .toEqual(expect.arrayContaining(["context", "inventory", "presets"]));
    expect(commands.some(entry => /slash command/iu.test(entry.label))).toBe(false);
  });

  it("reaches Away, voice, iPhone access, Files, Browser, overview, New project and themes (review C3)", () => {
    const commands = shellPaletteCommands(state({
      voiceOn: false, overviewOpen: false,
      themes: paletteThemes("hermes", [{ id: "custom:mine", name: "mine", kind: "dark",
        revision: "1", preview: {} }, { id: "custom:broken", name: "broken", kind: "light",
        revision: "1", preview: {}, problem: "No colours." }]),
    }));
    for (const [id, label] of [["away.open", "Away loop…"], ["voice.toggle", "Start voice assistant"],
      ["settings.remote", "iPhone remote access"], ["tool.files", "Files"], ["tool.browser", "Browser"],
      ["workspace.overview", "Live panes"], ["workspace.newProject", "New project…"]] as const) {
      expect(command(commands, id).label).toBe(label);
    }
    expect(command(shellPaletteCommands(state({ voiceOn: true })), "voice.toggle").label)
      .toBe("Stop voice assistant");
    expect(command(shellPaletteCommands(state({ voiceBusy: true })), "voice.toggle"))
      .toMatchObject({ disabled: true, disabledReason: "The voice assistant is starting or stopping." });
    const themes = commands.filter(entry => entry.group === "Theme");
    expect(themes[0]).toMatchObject({ id: "theme:system", label: "Theme: Follow macOS" });
    expect(themes.map(entry => entry.id)).toContain("theme:custom:mine");
    expect(themes.map(entry => entry.id)).not.toContain("theme:custom:broken");
    expect(command(commands, "theme:hermes").description).toMatch(/^Current theme\./u);
    expect(themes.filter(entry => entry.description?.startsWith("Current theme"))).toHaveLength(1);
    expect(new Set(commands.map(entry => entry.id)).size).toBe(commands.length);
  });

  it("drops Integrated terminals, which only repeated Go to Code", () => {
    const commands = shellPaletteCommands(state());
    expect(commands.some(entry => entry.id === "workspace.terminals")).toBe(false);
    expect(commands.some(entry => entry.label === "Integrated terminals")).toBe(false);
    expect(command(commands, "surface.code").label).toBe("Go to Code");
  });

  it("keeps the composer for commands that act in place", () => {
    for (const id of ["theme:dark", "voice.toggle", "away.open", "settings.remote", "workspace.settings"]) {
      expect(paletteCommandKeepsComposer(id), id).toBe(true);
    }
    for (const id of ["tool.files", "workspace.canvas", "surface.code"]) {
      expect(paletteCommandKeepsComposer(id), id).toBe(false);
    }
  });
});
