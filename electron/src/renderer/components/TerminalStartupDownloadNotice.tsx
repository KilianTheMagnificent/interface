import type { ReactNode } from "react";
import type { TerminalStartupDownloadV1 } from "../../shared/ipc";
import { STARTUP_DOWNLOAD_HELP, startupDownloadNotice } from "../terminal-startup-download";

/** Why a pane is still blank: its program is waiting on cloud-only files. */
export function TerminalStartupDownloadNotice(
  { download }: { readonly download: TerminalStartupDownloadV1 },
): ReactNode {
  return (
    <div className="terminal-startup-download" role="status" title={STARTUP_DOWNLOAD_HELP}>
      <span className="codicon codicon-cloud-download" aria-hidden="true" />
      <span className="terminal-startup-download-text">{startupDownloadNotice(download)}</span>
    </div>
  );
}
