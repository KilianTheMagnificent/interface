import { readFileSync } from "node:fs";
import { describe, expect, it, vi } from "vitest";
import type { MenuItemConstructorOptions } from "electron";
import { appMenuTemplate, registeredAccelerators } from "./app-menu";
import { parseMenuCommand } from "../shared/app-menu";

function actions() {
  return { openSettings: vi.fn(), closePane: vi.fn(), revealLogs: vi.fn(), copyDiagnostics: vi.fn() };
}

const submenu = (item: MenuItemConstructorOptions): MenuItemConstructorOptions[] =>
  Array.isArray(item.submenu) ? item.submenu : [];
const roles = (items: readonly MenuItemConstructorOptions[]): string[] =>
  items.flatMap(item => item.role === undefined ? [] : [item.role]);
const click = (item: MenuItemConstructorOptions | undefined): void => {
  (item?.click as (() => void) | undefined)?.();
};

describe("application menu", () => {
  it("replaces the stock menu with App, Edit, View, Window and Help", () => {
    const template = appMenuTemplate("Interface", actions());
    expect(template.map(item => item.label ?? item.role)).toEqual([
      "Interface", "editMenu", "viewMenu", "window", "help",
    ]);
    const app = submenu(template[0]!);
    expect(roles(app)).toEqual(["about", "services", "hide", "hideOthers", "unhide", "quit"]);
    const settings = app.find(item => item.label === "Settings…");
    expect(settings?.accelerator).toBe("CommandOrControl+,");
    // The renderer keeps ⌘, (it toggles Settings and keeps the chord under a dialog).
    expect(settings?.registerAccelerator).toBe(false);
    expect(roles(submenu(template[3]!))).toEqual(["minimize", "zoom", "close", "front"]);
  });

  it("⌘W is Close Pane and Close Window moves to ⇧⌘W", () => {
    const menu = actions();
    const windowMenu = submenu(appMenuTemplate("Interface", menu)[3]!);
    const closePane = windowMenu.find(item => item.label === "Close Pane");
    // Command, never Control: ⌃W deletes a word in every shell.
    expect(closePane?.accelerator).toBe("Command+W");
    click(closePane);
    expect(menu.closePane).toHaveBeenCalledOnce();
    expect(windowMenu.find(item => item.role === "close")?.accelerator).toBe("Shift+Command+W");
    expect(windowMenu.indexOf(closePane!)).toBeLessThan(windowMenu.findIndex(item => item.role === "close"));
  });

  it("help offers Reveal Logs and Copy Diagnostics, and Settings… opens Settings", () => {
    const menu = actions();
    const template = appMenuTemplate("Interface Dev", menu);
    expect(template[0]?.label).toBe("Interface Dev");
    const help = submenu(template[4]!);
    expect(help.map(item => item.label)).toEqual(["Reveal Logs", "Copy Diagnostics"]);
    click(help[0]);
    click(help[1]);
    click(submenu(template[0]!).find(item => item.label === "Settings…"));
    expect(menu.revealLogs).toHaveBeenCalledOnce();
    expect(menu.copyDiagnostics).toHaveBeenCalledOnce();
    expect(menu.openSettings).toHaveBeenCalledOnce();
  });

  it("registers only Close Pane and Close Window beyond the stock roles (docs/APP_MENU.md audit)", () => {
    // ⌘, is display-only. ⌘W is the page's first (it closes the selected
    // pane and keeps the chord from the menu); the menu's item covers a
    // Browser page that has the keyboard. No other renderer chord the audit
    // lists is taken by the menu.
    expect(registeredAccelerators(appMenuTemplate("Interface", actions())))
      .toEqual(["Command+W", "Shift+Command+W"]);
  });

  it("main installs the menu at startup and runs Help through the Rust host", () => {
    const main = readFileSync(new URL("./main.ts", import.meta.url), "utf8");
    expect(main).toContain("appProductTitle = productTitle(buildChannel);\n  installAppMenu();");
    expect(main).toContain("Menu.setApplicationMenu(Menu.buildFromTemplate(appMenuTemplate(appProductTitle, {");
    expect(main).toContain('host.invoke(CHANNELS.diagnostics, { kind: "report" })');
    expect(main).toContain("clipboard.writeText(result.report);");
    expect(main).toContain('rustHost.invoke(CHANNELS.diagnostics, { kind: "reveal", target: "logs" })');
    expect(main).toContain('sendMenuCommand({ version: 1, command: "settings" });');
    // Close Pane reaches only the window in front.
    expect(main).toContain("if (win === null || win.isDestroyed() || !win.isVisible() || !win.isFocused()) return;\n      sendMenuCommand({ version: 1, command: \"close-pane\" });");
  });

  it("the renderer accepts only exact menu commands", () => {
    expect(parseMenuCommand({ version: 1, command: "settings" }))
      .toEqual({ version: 1, command: "settings" });
    expect(parseMenuCommand({ version: 1, command: "close-pane" }))
      .toEqual({ version: 1, command: "close-pane" });
    expect(parseMenuCommand({ version: 1, command: "error", text: "Finder could not show the logs." }))
      .toEqual({ version: 1, command: "error", text: "Finder could not show the logs." });
    for (const bad of [null, [], { version: 2, command: "settings" },
      { version: 1, command: "settings", extra: 1 }, { version: 1, command: "quit" },
      { version: 1, command: "close-pane", pane: "a" },
      { version: 1, command: "error", text: "" },
      { version: 1, command: "error", text: "x".repeat(301) },
      { version: 1, command: "error", text: "ok", error: true },
      { version: 1, command: "status", text: "ok" }]) {
      expect(parseMenuCommand(bad), JSON.stringify(bad)).toBeNull();
    }
  });
});

describe("local background runner menu", () => {
  it("shows the saved choice and asks for a change without stopping active work", () => {
    const setEnabled = vi.fn();
    const menu = appMenuTemplate("Interface", { ...actions(), background: { enabled: true, available: true, setEnabled } });
    const items = submenu(menu[0]!);
    const toggle = items.find(item => item.label === "Run with window closed");
    expect(toggle?.type).toBe("checkbox");
    expect(toggle?.checked).toBe(true);
    expect(toggle?.toolTip).toContain("Quit stops it");
    click(toggle);
    expect(setEnabled).toHaveBeenCalledWith(false);
    expect(items.some(item => item.label === "Local runner: ready while Mac is awake")).toBe(true);
  });
  it("makes an unavailable preference explicit and leaves its toggle disabled", () => {
    const menu = appMenuTemplate("Interface", { ...actions(), background: { enabled: false, available: false, setEnabled: vi.fn() } });
    expect(submenu(menu[0]!).find(item => item.label === "Run with window closed")?.enabled).toBe(false);
  });
});
