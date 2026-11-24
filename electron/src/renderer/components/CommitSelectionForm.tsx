import { useId, useState } from "react";
import type { CommitChoice, CommitReview } from "../../shared/threads";
import { useWriteWithAi, WriteWithAiButton, type WriteWithAiPort } from "./WriteWithAi";

/** Only selection and input state. Rust owns the reviewed bytes and stale check. */
export function CommitSelectionForm({ review, message, held, committing, onCommit, onCancel, onReview, writeWithAi }: {
  review: CommitReview; message: string; held: boolean; committing: boolean;
  onCommit: (message: string, revision: string, choices: readonly CommitChoice[]) => void;
  onCancel: () => void; onReview: () => void;
  /** Protocol 67: Write with AI beside the message (absent: not offered). */
  writeWithAi?: WriteWithAiPort;
}) {
  const id = useId();
  const [text, setText] = useState(message);
  const [choices, setChoices] = useState<CommitChoice[]>([]);
  const ai = useWriteWithAi(writeWithAi, setText);
  function whole(path: string, checked: boolean) {
    setChoices(previous => [...previous.filter(choice => choice.path !== path), ...(checked ? [{ path }] : [])]);
  }
  function hunk(path: string, all: number[], selected: number, checked: boolean) {
    setChoices(previous => {
      const file = previous.find(choice => choice.path === path);
      const selectedIds = file === undefined ? [] : file.hunks ?? all;
      const hunks = checked ? [...selectedIds, selected] : selectedIds.filter(value => value !== selected);
      return [...previous.filter(choice => choice.path !== path), ...(hunks.length === 0 ? [] : [{ path, hunks }])];
    });
  }
  return <form className="tship-form" aria-label="Commit selected changes" onSubmit={event => {
    event.preventDefault();
    if (!held && text.trim() !== "" && choices.length > 0) onCommit(text, review.revision, choices);
  }}>
    <div className="tship-row">
      <label className="tship-field-label" htmlFor={id}>Commit message</label>
      {/* The files chosen so far, else every file the review lists. */}
      {writeWithAi !== undefined && <WriteWithAiButton port={writeWithAi} writing={ai.writing} disabled={held}
        what="commit" onClick={() => {
          ai.run("commit", choices.length > 0 ? choices.map(choice => choice.path) : review.files.map(file => file.path));
        }} />}
    </div>
    {ai.error !== null && <p className="tship-note" role="alert">{ai.error}</p>}
    <textarea id={id} className="tship-input" rows={3} value={text} disabled={held || ai.writing}
      onChange={event => { setText(event.target.value); }} />
    <p className="tship-note">Choose exactly what to include. A file attributed to this thread may also contain someone else's edits.</p>
    <ul className="tship-files" aria-label="Choose files and hunks">
      {review.files.map(file => {
        const choice = choices.find(value => value.path === file.path);
        const all = file.hunks.map(value => value.id);
        const partial = choice?.hunks !== undefined && choice.hunks.length > 0 && choice.hunks.length < all.length;
        return <li key={file.path} className="tship-choice">
          <label className="tship-file">
            <input type="checkbox"
              ref={el => { if (el !== null) el.indeterminate = partial; }}
              checked={choice !== undefined && (choice.hunks === undefined || choice.hunks.length === all.length)}
              disabled={held || file.blocked !== undefined} aria-label={`Include ${file.path}`}
              onChange={event => { whole(file.path, event.target.checked); }} />
            <span className="tship-path" title={file.path}>{file.path}</span>
          </label>
          <p className="tship-note">{file.blocked ?? (file.attributed ? "Named by this thread's edits" : "Other or unattributed change — includes shell edits")}</p>
          {file.blocked === undefined && file.hunks.length > 0 && <details>
            <summary>{file.hunks.length} {file.hunks.length === 1 ? "hunk" : "hunks"} · choose individual changes</summary>
            {file.hunks.map((part, index) => <div className="tship-hunk" key={part.id}>
              <label><input type="checkbox" disabled={held}
                checked={choice !== undefined && (choice.hunks === undefined || choice.hunks.includes(part.id))}
                aria-label={`Include hunk ${index + 1} of ${file.path}`}
                onChange={event => { hunk(file.path, all, part.id, event.target.checked); }} /> Hunk {index + 1}</label>
              <pre>{part.patch}</pre>
            </div>)}
          </details>}
        </li>;
      })}
    </ul>
    <div className="tship-row">
      <button className="tship-btn" type="button" disabled={held} onClick={onReview}>Review latest changes</button>
      <span className="tship-gap" />
      <button className="tship-btn" type="button" disabled={committing} onClick={onCancel}>Cancel</button>
      <button className="tship-btn is-primary" type="submit" disabled={held || choices.length === 0 || text.trim() === ""}>
        {committing ? "Committing…" : `Commit selection (${choices.length})`}
      </button>
    </div>
  </form>;
}
