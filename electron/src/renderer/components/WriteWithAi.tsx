import { useId, useState } from "react";
import type { ShipTextKind } from "../../shared/threads";
import { WRITE_WITH_AI_TITLE } from "../thread-ship";
import type { ShipTextResult } from "../use-thread-ship";

/* ============================================================================
   WRITE WITH AI (protocol 67, T3 section 12) — a commit message or a pull
   request's text from one tool-less Claude call

   Rust assembles what Claude reads (the changes being shipped, credential-
   filtered, and the thread's prompts) and makes the call; this only asks and
   puts the answer in the field for the owner to edit. Clicking it is the
   owner's consent to spend a little Claude usage (the tooltip says so).
   Nothing is committed or pushed by it.

   PRESENTATIONAL. Props in, callbacks out: no `window.iface`, no store.
   ========================================================================= */

/** What the ship forms need to offer Write with AI. */
export interface WriteWithAiPort {
  /** Why it is off (Claude not installed, or signed out), or null. */
  blocked: string | null;
  write: (what: ShipTextKind, paths?: readonly string[]) => Promise<ShipTextResult>;
}

/** One Write with AI ask: whether it runs, what went wrong, and the click. */
export function useWriteWithAi(port: WriteWithAiPort | undefined, onText: (text: string) => void) {
  const [writing, setWriting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const run = (what: ShipTextKind, paths?: readonly string[]): void => {
    if (port === undefined || writing) return;
    setWriting(true);
    setError(null);
    void port.write(what, paths).then(result => {
      setWriting(false);
      if ("text" in result) onText(result.text);
      else setError(result.error);
    });
  };
  return { writing, error, run };
}

/** The button: its words say when it is writing, its tooltip why it is off or what it costs. */
export function WriteWithAiButton({ port, writing, disabled, what, onClick }: {
  port: WriteWithAiPort; writing: boolean; disabled: boolean; what: ShipTextKind; onClick: () => void;
}) {
  return (
    <button type="button" className="tship-btn" data-write-with-ai={what}
      disabled={disabled || writing || port.blocked !== null}
      title={port.blocked ?? WRITE_WITH_AI_TITLE} onClick={onClick}>
      {writing ? "Writing…" : "Write with AI"}
    </button>
  );
}

/**
 * Push & open PR's form: the pull request's title and body, first filled
 * from the commit message, with Write with AI beside them.
 */
export function PullRequestTextForm({ draft, held, pushing, port, onSubmit, onCancel }: {
  draft: { title: string; body: string }; held: boolean; pushing: boolean; port?: WriteWithAiPort;
  onSubmit: (title: string, body: string) => void; onCancel: () => void;
}) {
  const titleId = useId();
  const bodyId = useId();
  const [title, setTitle] = useState(draft.title);
  const [body, setBody] = useState(draft.body);
  const ai = useWriteWithAi(port, text => {
    const [first = "", ...rest] = text.split("\n");
    setTitle(first.trim());
    setBody(rest.join("\n").trim());
  });
  return (
    <form className="tship-form" aria-label="Pull request" onSubmit={event => {
      event.preventDefault();
      if (!held) onSubmit(title, body);
    }}>
      <div className="tship-row">
        <label className="tship-field-label" htmlFor={titleId}>Pull request title</label>
        {port !== undefined && (
          <WriteWithAiButton port={port} writing={ai.writing} disabled={held} what="pullRequest"
            onClick={() => { ai.run("pullRequest"); }} />
        )}
      </div>
      {ai.error !== null && <p className="tship-note" role="alert">{ai.error}</p>}
      <input id={titleId} className="tship-input" value={title} disabled={held || ai.writing}
        onChange={event => { setTitle(event.target.value); }} />
      <label htmlFor={bodyId}>Description</label>
      <textarea id={bodyId} className="tship-input" rows={5} value={body} disabled={held || ai.writing}
        onChange={event => { setBody(event.target.value); }} />
      <div className="tship-row">
        <span className="tship-gap" />
        <button className="tship-btn" type="button" disabled={pushing} onClick={onCancel}>Cancel</button>
        <button className="tship-btn is-primary" type="submit" disabled={held || ai.writing}>
          {pushing ? "Pushing…" : "Push & open PR"}
        </button>
      </div>
    </form>
  );
}
