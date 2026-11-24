import { useEffect, useId, useState } from "react";
import type { ChecksState, CommitChoice, MergeMethod } from "../../shared/threads";
import {
  aheadBehindText, checksText, commitCountText, leftOutText, pushOffered, pushStateText, shipNext,
  type ShipNext,
} from "../thread-ship";
import type { ThreadShipState } from "../use-thread-ship";
import { pullRequestHref } from "./ThreadPaneHead";
import { AppSelect } from "./AppSelect";
import { ConfirmDialog } from "./ConfirmDialog";
import { Icon } from "./glyphs";
import { ThreadGlyph } from "./ThreadGlyph";
import { CommitSelectionForm } from "./CommitSelectionForm";
import { PullRequestTextForm, type WriteWithAiPort } from "./WriteWithAi";

/* ============================================================================
   SHIP — the Changes panel's foot: commit, branch, push and pull request,
   checks, review comments and merge for one thread (protocol 37)

   Rust reads the checkout and decides every rule (`shipStatus`): which files
   Commit stages and which it leaves out, why a push is refused, whether `gh`
   works and how to fix it, and how each effect ended. This section only draws
   that answer and sends the owner's choices. A refused effect's message stays
   until dismissed; an outcome nobody could confirm holds every control until
   Check again has read where things stand. Nothing here retries.

   Threads look (owner, 2026-10-23, canvas board "Review"): the branch and
   where it stands in one dim line, the other actions on hairline edges, and
   the one next step (`shipNext`, the changes card's own rule) filled in ink.

   PRESENTATIONAL. Props in, callbacks out: no `window.iface`, no store.
   ========================================================================= */

export interface ThreadShipBarProps {
  state: ThreadShipState;
  /** Opens the commit form; each new value opens it again (the card's Commit…). */
  commitRequest?: number;
  onRefresh: () => void;
  onReviewCommit: () => void;
  onCommit: (message: string, revision: string, choices: readonly CommitChoice[]) => void;
  onCreateBranch: (branch: string) => void;
  onOpenPullRequest: () => void;
  onFixChecks: () => void;
  onBringComments: () => void;
  onMerge: (number: number, method: MergeMethod) => void;
  onDismiss: () => void;
  /**
   * Protocol 62: the pull-request watch's toggle (absent without an open,
   * tracked pull request). While on, a failing check or a new review comment
   * sends the thread a prompt; it stops when the pull request merges or closes.
   */
  watch?: { on: boolean; title: string; onToggle: (on: boolean) => void };
  /**
   * Protocol 67: Push & open PR first opens a form with the pull request's
   * title and body (`pullRequestDraft`'s words) and Write with AI beside
   * them; absent, it pushes and opens the pull request at once.
   */
  pullRequest?: {
    draft: () => { title: string; body: string };
    onOpen: (title: string, body: string) => void;
  };
  /** Protocol 67: Write with AI beside the commit message and the pull request's text. */
  writeWithAi?: WriteWithAiPort;
}

/* Corners derive from the radius tokens, so the CLI chrome squares them.
   `--tdiff-x` is the Changes panel's side inset; alone, the foot keeps 24px. */
export const THREAD_SHIP_CSS = `
.tship { display:flex; flex:none; flex-direction:column; gap:10px; max-height:60%; overflow-y:auto;
  padding:12px var(--tdiff-x, 24px); border-top:var(--border-w) solid var(--border); background:var(--surface);
  font-family:var(--font-ui); }
.tship-head { display:flex; flex-wrap:wrap; align-items:center; justify-content:space-between; gap:8px;
  min-height:30px; }
/* The line takes the row's width before the buttons may share it; each part
   carries its own dot, so a wrap never leaves one hanging at a line's end. */
.tship-state { display:flex; flex:1 1 240px; flex-wrap:wrap; align-items:center; gap:4px 6px; min-width:0;
  margin:0; color:var(--fg-dim); font-size:var(--text-body); line-height:var(--lh-tight); }
.tship-glyph { flex:none; color:var(--fg-dim); }
.tship-branch { min-width:0; max-width:100%; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
.tship-sep { color:var(--fg-faint); }
.tship-meta { display:inline-flex; flex:none; gap:6px; color:var(--fg-dim); font-variant-numeric:tabular-nums;
  white-space:nowrap; }
.tship-merge { display:inline-flex; flex:none; align-items:center; gap:8px; margin-left:auto; }
.tship-actions { display:flex; flex:none; flex-wrap:wrap; align-items:center; justify-content:flex-end; gap:8px;
  margin-left:auto; }
.tship-gap { flex:1 1 auto; }
.tship-row { display:flex; flex-wrap:wrap; align-items:center; gap:8px; }
.tship-label { flex:1 1 auto; min-width:0; color:var(--fg); font-size:var(--text-body); }
.tship-note { margin:0; color:var(--fg-dim); font-size:var(--text-ui); line-height:var(--lh-ui); }
.tship-notice { display:flex; align-items:flex-start; gap:8px; margin:0; padding:8px 10px;
  color:var(--fg); background:var(--row-hover); border-radius:var(--radius-container);
  font-size:var(--text-ui); line-height:var(--lh-ui); }
.tship-notice.is-error { color:var(--danger-fg); background:var(--danger-bg); }
.tship-notice.is-unknown { color:var(--fg-strong); box-shadow:inset 3px 0 0 var(--focus); }
.tship-notice-text { flex:1 1 auto; min-width:0; overflow-wrap:anywhere; }
/* Buttons: the others on a hairline edge; the one next step filled in ink. */
.tship-btn { display:inline-flex; flex:none; align-items:center; gap:8px; box-sizing:border-box; height:30px;
  padding:0 12px; color:var(--fg); background:transparent; border:0;
  box-shadow:inset 0 0 0 var(--border-w) var(--border-strong); border-radius:var(--radius-container);
  font:inherit; font-size:var(--text-body); font-weight:var(--wght-medium); white-space:nowrap; cursor:pointer; }
.tship-btn:hover:not(:disabled) { color:var(--fg-strong); background:var(--row-hover); }
.tship-btn:disabled { color:var(--fg-faint); box-shadow:inset 0 0 0 var(--border-w) var(--border); cursor:default; }
.tship-btn.is-primary { color:var(--surface); background:var(--fg-strong); box-shadow:none; }
.tship-btn.is-primary:hover:not(:disabled) { color:var(--surface);
  background:color-mix(in srgb, var(--fg-strong) 86%, var(--surface)); }
.tship-btn.is-primary:disabled { color:var(--fg-faint); background:transparent;
  box-shadow:inset 0 0 0 var(--border-w) var(--border); }
.tship-btn:focus-visible { outline:var(--focus-ring-w) solid var(--focus); outline-offset:var(--focus-ring-offset); }
.tship-iconbtn { display:inline-flex; flex:none; align-items:center; justify-content:center; box-sizing:border-box;
  width:28px; height:28px; padding:0; color:var(--fg-dim); background:transparent; border:0;
  border-radius:calc(var(--radius-control) * 1.75); cursor:pointer; }
.tship-iconbtn:hover:not(:disabled) { color:var(--fg-strong); background:var(--row-hover); }
.tship-iconbtn:disabled { color:var(--fg-faint); cursor:default; }
.tship-iconbtn:focus-visible { outline:var(--focus-ring-w) solid var(--focus); outline-offset:var(--focus-ring-offset); }
.tship-x { display:inline-flex; flex:none; align-items:center; justify-content:center; width:20px; height:20px;
  padding:0; color:inherit; background:transparent; border:0; border-radius:calc(var(--radius-control) * 1.25);
  cursor:pointer; }
.tship-x:hover { background:var(--row-hover); }
.tship-x:focus-visible { outline:var(--focus-ring-w) solid var(--focus); outline-offset:var(--focus-ring-offset); }
.tship-form { display:flex; flex-direction:column; gap:8px; }
.tship-form > label, .tship-field-label { color:var(--fg-dim); font-size:var(--text-ui); }
.tship-field-label { flex:1 1 auto; min-width:0; }
.tship-input { display:block; box-sizing:border-box; width:100%; min-height:30px; margin:0; padding:6px 10px;
  color:var(--input-fg); background:var(--surface); border:0;
  box-shadow:inset 0 0 0 var(--border-w) var(--border-strong); border-radius:var(--radius-container);
  font:inherit; font-size:var(--text-body); line-height:var(--lh-ui); }
.tship-input:focus { outline:none; box-shadow:inset 0 0 0 var(--border-w) var(--focus), 0 0 0 var(--border-w) var(--focus); }
textarea.tship-input { min-height:64px; resize:vertical; font-family:var(--font-mono); font-size:var(--text-ui); }
.tship-files { display:flex; flex-direction:column; gap:2px; margin:0; padding:0; list-style:none; }
.tship-file { display:flex; gap:8px; min-width:0; font-family:var(--font-mono); font-size:var(--text-meta);
  line-height:var(--lh-ui); }
.tship-path { min-width:0; overflow:hidden; color:var(--fg); text-overflow:ellipsis; white-space:nowrap; }
.tship-choice { padding:8px 0; border-bottom:var(--border-w) solid var(--border); }
.tship-choice summary { color:var(--fg-dim); font-size:var(--text-meta); cursor:pointer; }
.tship-hunk { margin:6px 0; font-size:var(--text-meta); }
.tship-hunk pre { overflow:auto; max-height:240px; margin:4px 0; padding:8px; background:var(--editor-bg);
  border-radius:calc(var(--radius-control) * 1.5); font-family:var(--font-mono); }
.tship-pr { display:flex; flex-direction:column; gap:8px; padding-top:10px; border-top:var(--border-w) solid var(--border); }
.tship-pr-title { min-width:0; overflow:hidden; color:var(--fg-strong); text-overflow:ellipsis; white-space:nowrap;
  font-size:var(--text-body); text-decoration:none; }
.tship-pr-title:hover { text-decoration:underline; }
.tship-pr-title:focus-visible { outline:var(--focus-ring-w) solid var(--focus); outline-offset:var(--focus-ring-offset); }
.tship-checks { display:inline-flex; align-items:center; gap:6px; color:var(--fg-dim); font-size:var(--text-ui); }
.tship-checks.is-failing { color:var(--danger-fg); }
.tship-checks.is-passing { color:var(--diff-added); }
.tship .app-select-trigger { height:30px; border-radius:var(--radius-container); font-size:var(--text-body); }
.tship-sr { position:absolute; width:1px; height:1px; margin:-1px; padding:0; overflow:hidden;
  clip-path:inset(50%); white-space:nowrap; border:0; }
`;

const MERGE_OPTIONS = [
  { value: "squash", label: "Squash and merge" },
  { value: "merge", label: "Merge commit" },
  { value: "rebase", label: "Rebase and merge" },
];

/** A pull request's checks at a glance: passed, failing, running, or none yet. */
function ChecksMark({ state }: { state: ChecksState }) {
  if (state === "passing") return <ThreadGlyph name="pass" size={14} />;
  if (state === "failing") return <ThreadGlyph name="error" size={14} />;
  return <Icon name={state === "pending" ? "loading" : "circle-outline"} sm
    className={state === "pending" ? "codicon-modifier-spin" : undefined} />;
}

/** The panel's foot for shipping one thread's work. */
export function ThreadShipBar({
  state, commitRequest, onRefresh, onReviewCommit, onCommit, onCreateBranch, onOpenPullRequest, onFixChecks,
  onBringComments, onMerge, onDismiss, watch, pullRequest, writeWithAi,
}: ThreadShipBarProps) {
  const branchId = useId();
  const { ship, busy, notice, needsCheck } = state;
  const [commitOpen, setCommitOpen] = useState(false);
  const [branch, setBranch] = useState<string | null>(null);
  const [method, setMethod] = useState<MergeMethod>("squash");
  const [confirmingMerge, setConfirmingMerge] = useState(false);
  // Protocol 67: the pull request form, with the words it opened with.
  const [prForm, setPrForm] = useState<{ title: string; body: string } | null>(null);
  // The card's Commit… opens the form here.
  useEffect(() => {
    if (commitRequest !== undefined && commitRequest > 0) { setCommitOpen(true); onReviewCommit(); }
  }, [commitRequest]);
  // A commit that went through (nothing left to stage) closes the form.
  const files = ship?.files ?? [];
  useEffect(() => {
    if (files.length === 0 && (ship?.leftOut.length ?? 0) === 0) {
      setCommitOpen(false);
    }
  }, [files.length, ship?.leftOut.length]);

  const held = (busy !== null && busy !== "read") || needsCheck;
  const reading = busy === "read";
  if (ship === null) {
    return (
      <section className="tship" aria-label="Ship this thread's work" aria-busy={reading || undefined}>
        <style>{THREAD_SHIP_CSS}</style>
        <p className="tship-note" role="status">{reading ? "Reading the checkout…" : "Not read yet."}</p>
        {notice !== null && <Notice tone={notice.tone} message={notice.message} onDismiss={onDismiss} />}
      </section>
    );
  }
  const pr = ship.pullRequest;
  const open = pr?.state === "open" ? pr : undefined;
  const ghReady = ship.gh === "ready";
  const shownBranch = branch ?? ship.suggestedBranch ?? "";
  const leftOut = leftOutText(ship.leftOut);
  const pushLabel = open === undefined ? "Push & open PR" : "Push";
  const offerPush = pushOffered(ship);
  const canPush = offerPush && (ghReady || !ship.remoteRead);
  const href = pr === undefined ? undefined : pullRequestHref(pr.url);
  // The one filled button: the step the changes card offers next.
  const next = shipNext(ship, needsCheck);
  const fill = (step: ShipNext): string => (next === step ? " is-primary" : "");
  const otherChanges = files.length === 0 && ship.leftOut.some(file => file.reason === "notThisThread");
  const offerCommit = ship.commitBlocked === undefined && (files.length > 0 || otherChanges) && !commitOpen;
  // Where the branch stands, in one dim line after its name.
  const standing = [aheadBehindText(ship), pushStateText(ship),
    ship.commitBlocked === undefined ? commitCountText(files.length) : ""].filter(part => part !== "");
  const ghBroken = ship.remoteRead && !ghReady;

  return (
    <section className="tship" aria-label="Ship this thread's work" aria-busy={busy !== null || undefined}>
      <style>{THREAD_SHIP_CSS}</style>
      <div className="tship-head">
        <p className="tship-state">
          <ThreadGlyph name="branch" size={13} className="tship-glyph" />
          <span className="tship-branch" title={ship.upstream ?? ship.branch}>{ship.branch ?? "No branch"}</span>
          {standing.map(part => (
            <span key={part} className="tship-meta"><span className="tship-sep" aria-hidden="true">·</span>{part}</span>
          ))}
        </p>
        <span className="tship-actions">
          {!needsCheck && (
            <button type="button" className="tship-iconbtn" disabled={reading || busy !== null}
              aria-label="Refresh" title="Read the checkout and the pull request again" onClick={onRefresh}>
              <ThreadGlyph name="retry" size={15} className={reading ? "codicon-modifier-spin" : undefined} />
            </button>
          )}
          {offerPush && (
            <button type="button" className={`tship-btn${fill("push")}`}
              disabled={held || !canPush || prForm !== null}
              onClick={pullRequest !== undefined && open === undefined
                ? () => { setPrForm(pullRequest.draft()); } : onOpenPullRequest}
              title={ship.pushBlocked ?? (ghReady ? undefined : ship.ghFix)}>
              {busy === "push" ? "Pushing…" : pushLabel}
            </button>
          )}
          {offerCommit && (
            <button type="button" className={`tship-btn${fill("commit")}`} disabled={held}
              title={otherChanges ? "Review other checkout changes" : undefined}
              onClick={() => { setCommitOpen(true); onReviewCommit(); }}>
              Commit…
            </button>
          )}
          {needsCheck && (
            <button type="button" className="tship-btn is-primary" disabled={reading}
              title="Read the checkout and the pull request again" onClick={onRefresh}>
              Check again
            </button>
          )}
        </span>
      </div>

      {notice !== null && <Notice tone={notice.tone} message={notice.message} onDismiss={onDismiss} />}
      {needsCheck && (
        <p className="tship-note">Interface couldn't confirm the last step. Check again before trying it again.</p>
      )}

      {ship.suggestedBranch !== undefined && (
        <form className="tship-form" aria-label="Create a branch" onSubmit={event => {
          event.preventDefault();
          if (shownBranch.trim() !== "") onCreateBranch(shownBranch.trim());
        }}>
          <p className="tship-note">{ship.pushBlocked}</p>
          <div className="tship-row">
            <label className="tship-sr" htmlFor={branchId}>New branch name</label>
            <input id={branchId} className="tship-input" value={shownBranch} style={{ flex: "1 1 160px", width: "auto" }}
              onChange={event => { setBranch(event.target.value); }} />
            <button type="submit" className={`tship-btn${fill("branch")}`} disabled={held || shownBranch.trim() === ""}>
              {busy === "branch" ? "Creating…" : "Create branch"}
            </button>
          </div>
        </form>
      )}
      {ship.commitBlocked !== undefined && <p className="tship-note">{ship.commitBlocked}</p>}
      {ship.commitBlocked === undefined && commitOpen && (state.commitReview === null
        ? <div className="tship-form"><p className="tship-note">{busy === "reviewCommit" ? "Reading exact changes…" : "Read the changes before selecting a commit."}</p>
            <div className="tship-row"><button className="tship-btn" type="button" disabled={held} onClick={onReviewCommit}>Review changes</button></div></div>
        : <CommitSelectionForm key={state.commitReview.revision} review={state.commitReview} message={ship.message}
            held={held} committing={busy === "commit"} onCommit={onCommit} writeWithAi={writeWithAi}
            onCancel={() => { setCommitOpen(false); }} onReview={onReviewCommit} />)}
      {prForm !== null && pullRequest !== undefined && open === undefined && offerPush && (
        <PullRequestTextForm draft={prForm} held={held || !canPush} pushing={busy === "push"} port={writeWithAi}
          onSubmit={(title, body) => { setPrForm(null); pullRequest.onOpen(title, body); }}
          onCancel={() => { setPrForm(null); }} />
      )}
      {files.length === 0 && leftOut !== "" && !commitOpen && <p className="tship-note">{leftOut} Choose Commit… to review other changes explicitly.</p>}

      {ship.suggestedBranch === undefined && ship.pushBlocked !== undefined && (
        <p className="tship-note">{ship.pushBlocked}</p>
      )}

      {(ghBroken || pr !== undefined) && (
        <div className="tship-pr" role="group" aria-label="Pull request">
          {ghBroken && (
            <div className="tship-row">
              <ThreadGlyph name="warning" size={14} className="tship-glyph" />
              <span className="tship-label tship-note">{ship.ghFix ?? "The GitHub CLI isn't working here."}</span>
            </div>
          )}
          {pr !== undefined && (
            <div className="tship-row">
              <ThreadGlyph name="pr" size={14} className="tship-glyph" />
              {href === undefined
                ? <span className="tship-pr-title">#{pr.number} {pr.title}</span>
                : <a className="tship-pr-title" href={href} target="_blank" rel="noreferrer noopener"
                  title={`Open #${pr.number} on GitHub`}>#{pr.number} {pr.title}</a>}
              <span className="tship-meta">{pr.state === "open" ? (pr.draft ? "Draft" : "Open")
                : pr.state === "merged" ? "Merged" : "Closed"}</span>
            </div>
          )}
          {open !== undefined && (
            <div className="tship-row">
              <span className={`tship-checks is-${ship.checks?.state ?? "none"}`} role="status">
                <ChecksMark state={ship.checks?.state ?? "none"} />
                {checksText(ship.checks)}
              </span>
              <span className="tship-gap" />
              {ship.checks?.state === "failing" && (
                <button type="button" className={`tship-btn${fill("fixChecks")}`} disabled={held || !ghReady}
                  onClick={onFixChecks} title="Send this thread the failing checks and the end of their logs">
                  {busy === "fixChecks" ? "Sending…" : "Fix failing checks"}
                </button>
              )}
            </div>
          )}
          {open !== undefined && (
            <div className="tship-row">
              <button type="button" className="tship-btn" disabled={held || !ghReady} onClick={onBringComments}
                title="Put the pull request's unresolved review comments into this thread's prompt">
                {busy === "comments" ? "Reading…" : "Bring in review comments"}
              </button>
              {watch !== undefined && (
                <button type="button" className="tship-btn" aria-pressed={watch.on} data-thread-watch=""
                  title={watch.title} onClick={() => { watch.onToggle(!watch.on); }}>
                  {watch.on ? "Watching" : "Watch"}
                </button>
              )}
              {/* The method and Merge… wrap as one, so the choice stays beside its button. */}
              <span className="tship-merge">
                <AppSelect label="Merge method" value={method} options={MERGE_OPTIONS}
                  onChange={value => { setMethod(value as MergeMethod); }} />
                <button type="button" className={`tship-btn${fill("merge")}`} disabled={held || !ghReady}
                  onClick={() => { setConfirmingMerge(true); }}>
                  {busy === "merge" ? "Merging…" : "Merge…"}
                </button>
              </span>
            </div>
          )}
        </div>
      )}

      {open !== undefined && (
        <ConfirmDialog
          open={confirmingMerge}
          title={`Merge #${open.number} into ${open.base}?`}
          description={`${MERGE_OPTIONS.find(option => option.value === method)?.label ?? "Merge"} on GitHub${
            ship.checks?.state === "failing" ? ", although some checks are failing" : ""
          }. Once it's merged, this thread settles.`}
          confirmLabel="Merge"
          tone="default"
          onConfirm={() => {
            setConfirmingMerge(false);
            onMerge(open.number, method);
          }}
          onCancel={() => { setConfirmingMerge(false); }}
        />
      )}
    </section>
  );
}

function Notice({ tone, message, onDismiss }: { tone: "done" | "unknown" | "error"; message: string;
    onDismiss: () => void }) {
  return (
    <p className={`tship-notice is-${tone}`} role={tone === "error" ? "alert" : "status"}>
      {tone === "unknown" ? <Icon name="info" sm /> : <ThreadGlyph name={tone === "done" ? "pass" : "error"} size={14} />}
      <span className="tship-notice-text">{message}</span>
      <button type="button" className="tship-x" aria-label="Dismiss" onClick={onDismiss}>
        <ThreadGlyph name="close" size={12} />
      </button>
    </p>
  );
}
