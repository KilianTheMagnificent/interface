import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { app, BrowserWindow, clipboard, dialog, Menu, shell } from "electron";
import { BROWSER_EVENT_CHANNEL } from "../shared/browser-events.js";
import type { BrowserPaneHooks } from "./browser-pane.js";

/**
 * The Browser guards' hooks for the running app: the owner's Downloads
 * folder, a native sheet on the app window, a native menu, the system's
 * handler for another app's link. A quiet run (hands-free, the packaged
 * smoke) asks nobody, shows no popup, opens nothing outside and saves into
 * a temp folder, never the owner's Downloads.
 */
export function browserPaneNativeHooks(window: () => BrowserWindow | null, quiet: boolean): BrowserPaneHooks {
  let quietDownloads: string | null = null;
  return {
    downloads: () => {
      if (!quiet) return app.getPath("downloads");
      quietDownloads ??= mkdtempSync(join(tmpdir(), "interface-browser-downloads-"));
      return quietDownloads;
    },
    ask: async question => {
      const win = window();
      if (quiet || win === null || win.isDestroyed()) return false;
      const { response } = await dialog.showMessageBox(win, {
        type: "question", message: question.message, detail: question.detail,
        // Block is the default: a page picks when the sheet appears, so Return
        // meant for a terminal or the composer must never grant anything.
        buttons: ["Block", "Allow"], defaultId: 0, cancelId: 0, noLink: true,
      });
      return response === 1;
    },
    showMenu: (template, contents) => {
      const owner = BrowserWindow.fromWebContents(contents) ?? window();
      Menu.buildFromTemplate(template).popup(owner === null || owner.isDestroyed() ? {} : { window: owner });
    },
    openExternal: url => quiet ? Promise.resolve() : shell.openExternal(url),
    copyText: text => { clipboard.writeText(text); },
    send: event => {
      const win = window();
      if (win === null || win.isDestroyed() || win.webContents.isDestroyed()) return;
      win.webContents.send(BROWSER_EVENT_CHANNEL, event);
    },
    window,
    showPopups: !quiet,
    chooseSavePath: (defaultPath, name) => {
      const win = window();
      if (quiet || win === null || win.isDestroyed()) return null;
      return dialog.showSaveDialogSync(win, {
        defaultPath,
        message: `Choose where to save “${name}” if you trust this download.`,
      }) ?? null;
    },
    downloadFinished: path => { if (!quiet) app.dock?.downloadFinished(path); },
  };
}
