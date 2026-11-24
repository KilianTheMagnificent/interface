import { useCallback, useEffect, useRef, useState } from 'react';
import type { SharedDraft } from '../shared/harness';
import { startSharedDraftPolling, type SharedDraftPoller } from './shared-draft-polling';
function checked(answer: SharedDraft): SharedDraft {
  if (!answer || typeof answer.text !== 'string' || !Number.isSafeInteger(answer.revision) || answer.revision < 0 || !answer.target) throw new Error('Invalid saved draft response');
  return answer;
}
export interface DraftPort { read: () => Promise<SharedDraft>; save: (revision: number, text: string) => Promise<SharedDraft> }
/** Local edits survive conflicts. Only the host owns revisions; this hook never sends input. */
export function useSharedDraft(port: DraftPort | undefined): { draft: string; setDraft: (text: string) => void; draftStatus: string; draftNeedsAttention: boolean; reloadDraft: () => void } {
  const [draft, updateDraft] = useState('');
  const [draftStatus, setStatus] = useState('');
  const local = useRef('');
  const revision = useRef<number | null>(null);
  const edited = useRef(false);
  const halted = useRef(false);
  const active = useRef(true);
  const lane = useRef<Promise<void>>(Promise.resolve());
  const currentPort = useRef(port); currentPort.current = port;
  const poller = useRef<SharedDraftPoller | null>(null);
  /** Answers with the revision it read, for the poll's pace; undefined when it read none. */
  const load = useCallback((replace: boolean, quiet = false): Promise<number | undefined> => {
    const api = currentPort.current; if (!api || (quiet && (edited.current || halted.current))) return Promise.resolve(undefined);
    let read: number | undefined;
    lane.current = lane.current.then(async () => {
      if (!active.current || (quiet && (edited.current || halted.current || document.hidden))) return;
      const answer = checked(await api.read());
      read = answer.revision;
      if (!active.current) return;
      if (edited.current && !replace) {
        if (!quiet) { halted.current = true; setStatus('A saved draft is available. Your local edit is preserved; reload to use the saved version.'); }
        return;
      }
      revision.current = answer.revision; local.current = answer.text; updateDraft(answer.text);
      edited.current = false; halted.current = false; setStatus('Draft saved on your Mac.');
    }).catch(() => { if (!quiet && active.current) { halted.current = true; setStatus('Saved draft unavailable. Local text is preserved.'); } });
    return lane.current.then(() => read);
  }, []);
  const save = useCallback((): void => {
    const api = currentPort.current;
    if (!api || !edited.current || halted.current || revision.current === null) return;
    lane.current = lane.current.then(async () => {
      if (!edited.current || halted.current || revision.current === null) return;
      const captured = local.current;
      const answer = checked(await api.save(revision.current, captured));
      revision.current = answer.revision; poller.current?.saved();
      if (local.current === captured) { edited.current = false; if (active.current) setStatus('Draft saved on your Mac.'); }
    }).catch(() => { halted.current = true; if (active.current) setStatus('Draft changed elsewhere or could not be saved. Your local text is preserved; reload before sharing more edits.'); });
  }, []);
  useEffect(() => {
    active.current = true;
    const stop = startSharedDraftPolling(quiet => load(false, quiet));
    poller.current = stop;
    return () => { active.current = false; poller.current = null; stop(); save(); };
  }, [load, save]);
  const setDraft = useCallback((text: string): void => {
    local.current = text; edited.current = true; updateDraft(text);
    if (currentPort.current && !halted.current) setStatus('Saving draft on your Mac…');
  }, []);
  useEffect(() => { const timer = setTimeout(save, 350); return () => clearTimeout(timer); }, [draft, save]);
  return { draft, setDraft, draftStatus, draftNeedsAttention: halted.current, reloadDraft: () => { void load(true); } };
}
