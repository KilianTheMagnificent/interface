import type { MenuItemConstructorOptions } from "electron";

/**
 * The macOS application menu. Before this the app shipped Electron's stock
 * menu; this one keeps every stock accelerator the window relied on (see
 * docs/APP_MENU.md for the audit) and adds Settings… and the Help items a
 * user can reach when something goes wrong.
 *
 * ⌘, stays the renderer's: the page toggles Settings itself (and keeps the
 * chord while a dialog is up), so the menu item shows ⌘, without registering
 * it. A click opens Settings through the menu command channel. ⌘W is Close
 * Pane and ⇧⌘W Close Window (see the Window menu below).
 */
export interface AppMenuActions {
  openSettings(): void;
  closePane(): void;
  revealLogs(): void;
  copyDiagnostics(): void;
  background?: { enabled: boolean; available: boolean; setEnabled(enabled: boolean): void };
}

export function appMenuTemplate(productName: string, actions: AppMenuActions): MenuItemConstructorOptions[] {
  return [
    {
      label: productName,
      submenu: [
        { role: "about" },
        { type: "separator" },
        {
          label: "Settings…",
          accelerator: "CommandOrControl+,",
          registerAccelerator: false,
          click: () => { actions.openSettings(); },
        },
        { type: "separator" },
        ...(actions.background === undefined ? [] : [{
          label: "Run with window closed",
          type: "checkbox" as const,
          checked: actions.background.enabled,
          enabled: actions.background.available,
          toolTip: "Keeps this local runner alive with the window closed. Quit stops it; your Mac must remain awake.",
          click: () => { actions.background?.setEnabled(!actions.background.enabled); },
        }, {
          label: !actions.background.available ? "Local runner: preference unavailable"
            : actions.background.enabled ? "Local runner: ready while Mac is awake" : "Local runner: stops when window closes",
          enabled: false,
        }, { type: "separator" as const }]),
        { role: "services" },
        { type: "separator" },
        { role: "hide" },
        { role: "hideOthers" },
        { role: "unhide" },
        { type: "separator" },
        { role: "quit" },
      ],
    },
    // The stock Edit menu, item for item: undo/redo/cut/copy/paste/select
    // all reach inputs, the composer and xterm panes through these roles.
    { role: "editMenu" },
    // The stock View menu: reload, developer tools, zoom and full screen
    // keep their chords.
    { role: "viewMenu" },
    {
      role: "window",
      submenu: [
        { role: "minimize" },
        { role: "zoom" },
        // ⌘W closes the selected pane (owner, 2026-11-15), not the window.
        // The page takes the chord itself and keeps it from this item, which
        // acts on a click and when the keys never reached the page (a Browser
        // page has the keyboard). Command, never Control: ⌃W deletes a word
        // in every shell.
        { label: "Close Pane", accelerator: "Command+W", click: () => { actions.closePane(); } },
        // Close Window hides the window, as ⌘W did; ⇧⌘W is its macOS chord
        // wherever ⌘W closes a tab.
        { role: "close", accelerator: "Shift+Command+W" },
        { type: "separator" },
        { role: "front" },
      ],
    },
    {
      role: "help",
      submenu: [
        { label: "Reveal Logs", click: () => { actions.revealLogs(); } },
        { label: "Copy Diagnostics", click: () => { actions.copyDiagnostics(); } },
      ],
    },
  ];
}

/** Every accelerator the template registers with macOS, spelled as written. */
export function registeredAccelerators(template: readonly MenuItemConstructorOptions[]): string[] {
  const found: string[] = [];
  const walk = (items: readonly MenuItemConstructorOptions[]): void => {
    for (const item of items) {
      if (item.accelerator !== undefined && item.registerAccelerator !== false) {
        found.push(String(item.accelerator));
      }
      if (Array.isArray(item.submenu)) walk(item.submenu);
    }
  };
  walk(template);
  return found;
}
