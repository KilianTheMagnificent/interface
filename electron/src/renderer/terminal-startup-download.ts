import type { TerminalStartupDownloadV1 } from "../shared/ipc";

/** One line over the pane naming the startup files the cloud still owes it. */
export function startupDownloadNotice(download: TerminalStartupDownloadV1): string {
  if (download.files.length === 0) {
    return `Waiting for cloud download: ${download.remaining.toString()} files`;
  }
  const named = download.files.join(", ");
  const more = download.remaining - download.files.length;
  return more > 0
    ? `Waiting for cloud download: ${named} and ${more.toString()} more`
    : `Waiting for cloud download: ${named}`;
}

/** Why a pane can sit blank here, and the one setting that avoids it. */
export const STARTUP_DOWNLOAD_HELP = "These project files are stored only in the cloud "
  + "(iCloud Drive with Optimize Mac Storage). Claude, Codex and git-aware prompts read "
  + "them before they draw anything, so this pane waits until they download. Turning on "
  + "Keep Downloaded for the project folder in Finder avoids the wait.";
