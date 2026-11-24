import { useEffect, useId, useRef, useState } from "react";
import type { DraftStashSummary } from "../../shared/draft-stash";
import { stashRowWords } from "../draft-stash-view";
import { ThreadGlyph } from "./ThreadGlyph";

/**
 * "Stashed (n)" by a thread's composer (T3 section 9): the drafts parked in
 * Rust, newest first, each its first line and age. Choosing one puts it back
 * (the pane stashes a non-empty composer first, in the same Rust commit);
 * each row has a remove button. Plain: the + menu's own rows and surface.
 */
export function StashedDrafts({ entries, busy, onRestore, onRemove }: {
  entries: readonly DraftStashSummary[];
  busy: boolean;
  onRestore: (id: string) => void;
  onRemove: (id: string) => void;
}) {
  const panelId = `${useId()}-stashed`;
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  useEffect(() => {
    if (!open) return undefined;
    const outside = (event: PointerEvent): void => {
      if (root.current !== null && event.composedPath().includes(root.current)) return;
      setOpen(false);
    };
    document.addEventListener("pointerdown", outside, true);
    return () => { document.removeEventListener("pointerdown", outside, true); };
  }, [open]);
  useEffect(() => { if (entries.length === 0) setOpen(false); }, [entries.length]);
  if (entries.length === 0) return null;
  const now = Date.now();
  const close = (): void => { setOpen(false); trigger.current?.focus(); };
  return (
    <div ref={root} className="thread-composer-stash"
      onKeyDown={event => {
        if (event.key !== "Escape" || !open) return;
        event.preventDefault();
        event.stopPropagation();
        close();
      }}>
      <button ref={trigger} type="button" className="conv-iconbtn thread-composer-verb is-stash"
        aria-expanded={open} aria-controls={open ? panelId : undefined}
        title="Drafts you stashed in this thread"
        onClick={() => { setOpen(value => !value); }}>
        <span>{`Stashed (${entries.length})`}</span>
      </button>
      {open && (
        <div id={panelId} role="group" aria-label="Stashed drafts"
          className="thread-composer-menu is-floating" aria-busy={busy || undefined}>
          {entries.map(entry => {
            const words = stashRowWords(entry, now);
            return (
              <div key={entry.id} className="thread-composer-stash-row">
                <button type="button" className="thread-composer-menu-row" disabled={busy}
                  aria-label={`Restore stashed draft: ${words.label}, ${words.detail}`}
                  onClick={() => { setOpen(false); onRestore(entry.id); }}>
                  <span className="thread-composer-menu-glyph" aria-hidden="true">
                    <ThreadGlyph name="comment" size={14} />
                  </span>
                  <span className="thread-composer-menu-copy">
                    <span className="thread-composer-menu-label">{words.label}</span>
                    <span className="thread-composer-menu-detail">{words.detail}</span>
                  </span>
                </button>
                <button type="button" className="conv-iconbtn thread-composer-icon" disabled={busy}
                  aria-label={`Remove stashed draft: ${words.label}`} title="Remove"
                  onClick={() => { onRemove(entry.id); }}>
                  <ThreadGlyph name="close" size={14} />
                </button>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
