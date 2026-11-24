import { useCallback, useEffect, useState } from "react";
import type { ImportProgress } from "../main/bridge";
import type { IfaceApi } from "../shared/ipc";
import { ipcMessage } from "./ipc-message";
import { importSummary } from "./sidebar-view";

type ImportApi = Pick<IfaceApi, "adoptAll" | "onImportProgress">;

/** The fewest milliseconds between two progress renders while an import runs. */
export const IMPORT_PROGRESS_INTERVAL_MS = 120;

/**
 * The main process pushes progress once per imported session, and each push
 * rendered App and the whole Sidebar: about 1,100 renders for one Import All.
 * This hands on the first push at once (it carries the total), then at most
 * one per interval (the newest), and the last (done of total) at once.
 */
export class ImportProgressThrottle {
  private pending: ImportProgress | null = null;
  private timer: ReturnType<typeof setTimeout> | null = null;

  constructor(private readonly publish: (progress: ImportProgress) => void,
    private readonly intervalMs = IMPORT_PROGRESS_INTERVAL_MS) {}

  push(progress: ImportProgress): void {
    const last = progress.total > 0 && progress.done >= progress.total;
    if (this.timer !== null && !last) {
      this.pending = progress;
      return;
    }
    this.cancel();
    this.publish(progress);
    if (!last) this.timer = setTimeout(() => { this.flush(); }, this.intervalMs);
  }

  /** Drops anything not yet handed on: the import ended and cleared its row. */
  cancel(): void {
    if (this.timer !== null) clearTimeout(this.timer);
    this.timer = null;
    this.pending = null;
  }

  private flush(): void {
    this.timer = null;
    const progress = this.pending;
    this.pending = null;
    if (progress !== null) this.push(progress);
  }
}

/**
 * The bulk import of everything already on this machine. `importProgress` is
 * the live counter, `importNote` the sentence left behind afterwards.
 */
export function useSessionImport(api: ImportApi, reportError: (message: string) => void) {
  const [importProgress, setImportProgress] = useState<ImportProgress | null>(null);
  const [importNote, setImportNote] = useState<string | null>(null);
  const [importing, setImporting] = useState(false);

  // Import progress: one push before any work with the real total, then one
  // per session. The batch runs in the main process, so this never blocks.
  const [progressThrottle] = useState(() => new ImportProgressThrottle(setImportProgress));
  useEffect(() => {
    const stop = api.onImportProgress(progress => { progressThrottle.push(progress); });
    return () => { stop(); progressThrottle.cancel(); };
  }, [api, progressThrottle]);

  // ~170 sessions across two stores. The batch runs in the main process, four
  // at a time, and pushes progress; nothing here blocks, and the button that
  // started it says so while it runs.
  const importAll = useCallback(() => {
    setImporting(true);
    setImportNote(null);
    // Seeded so the progress row appears on the click rather than on the first
    // push, which arrives only once both stores have been re-listed.
    setImportProgress({ adopt: true, done: 0, total: 0, ok: 0, failed: 0 });
    api.adoptAll(true)
      .then(report => { setImportNote(importSummary(report)); })
      .catch((err: unknown) => { reportError(ipcMessage(err)); })
      .finally(() => {
        setImporting(false);
        progressThrottle.cancel();
        setImportProgress(null);
      });
  }, [api, progressThrottle, reportError]);

  const dismissImportNote = useCallback(() => { setImportNote(null); }, []);

  return { importProgress, importNote, importing, importAll, dismissImportNote };
}
