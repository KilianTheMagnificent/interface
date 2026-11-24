import type { MessageBoxSyncOptions } from "electron";

/**
 * The two "something stopped" alerts: the Rust host exited (`backend`), or the
 * renderer process died (`window`). Each used to offer only OK, so recovering
 * meant quitting and reopening by hand. Relaunch is the default now, and it
 * goes through the ordinary quit — the renderer's last writes land and the
 * host is stopped — before macOS starts the app again; Claude and Codex panes
 * resume where they were.
 *
 * A harness that answers every dialog for itself (hands-free, the packaged
 * smoke) answers 0. It must never relaunch an app nobody is watching, so it
 * gets the alert without the Relaunch button.
 */
export type CrashKind = "backend" | "window";

export interface CrashDialog {
  readonly options: MessageBoxSyncOptions;
  /** The response index that means Relaunch; absent when it is not offered. */
  readonly relaunch?: number;
}

export function crashDialog(kind: CrashKind, product: string, detail: string,
    offerRelaunch: boolean): CrashDialog {
  const message = kind === "backend" ? `${product} backend stopped` : `${product} window stopped`;
  const what = kind === "backend"
    ? `${detail}\n\n`
    : "The desktop window stopped unexpectedly. ";
  if (!offerRelaunch) {
    return { options: { type: "error", title: product, message, buttons: ["OK"],
      detail: kind === "backend"
        ? `${what}Quit and reopen ${product} to recover.`
        : `${what}${product} will close cleanly so it can be reopened.` } };
  }
  return {
    options: {
      type: "error", title: product, message, noLink: true,
      buttons: kind === "backend" ? ["Relaunch", "OK"] : ["Relaunch", "Quit"],
      defaultId: 0, cancelId: 1,
      detail: kind === "backend"
        ? `${what}Relaunch ${product} to recover. Agent panes resume where they were.`
        : `${what}Relaunch ${product}, or quit and reopen it later.`,
    },
    relaunch: 0,
  };
}

/** Whether the owner chose Relaunch. */
export function chooseRelaunch(dialog: CrashDialog, response: number): boolean {
  return dialog.relaunch !== undefined && response === dialog.relaunch;
}
