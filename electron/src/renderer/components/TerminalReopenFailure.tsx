import type { MovedTerminalFolder } from "../../shared/ipc";
import { homeRelativePath } from "../display-paths";

/**
 * What a saved pane shows when the host refused to put it back. Without it the
 * pane was an empty box whose only sign of trouble was a red dot. When the
 * pane's folder has moved and left a link behind, the host keeps refusing the
 * link, so the pane offers to follow the move to the folder it now leads to.
 */
export function TerminalReopenFailure({ message, moved, onReopenMoved }: {
  readonly message: string;
  readonly moved?: MovedTerminalFolder;
  readonly onReopenMoved?: () => void;
}) {
  return (
    <div className="terminal-reopen-failure">
      <p className="terminal-reopen-failure-title">This terminal did not reopen</p>
      {moved === undefined || onReopenMoved === undefined
        ? <p className="terminal-reopen-failure-reason">{message}</p>
        : <>
            <p className="terminal-reopen-failure-reason">
              It was saved in <span title={moved.from}>{homeRelativePath(moved.from)}</span>,
              which now leads to <span title={moved.to}>{homeRelativePath(moved.to)}</span>.
            </p>
            <button type="button" className="btn btn-primary" title={moved.to}
              onClick={onReopenMoved}>Reopen in {homeRelativePath(moved.to)}</button>
          </>}
    </div>
  );
}
