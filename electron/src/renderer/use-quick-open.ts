import { useCallback, useRef, useState, type Dispatch, type SetStateAction } from "react";
import type { IfaceApi, ProjectFileEntry } from "../shared/ipc";
import { ipcMessage } from "./ipc-message";

/**
 * The file picker's STORE. The picker itself is the command palette in "files"
 * mode; this is the authorized index it reads and the target a pick attaches
 * to. `open` means "a picker session is armed", which is what lets a late
 * listProjectFiles response know whether it still belongs to anything.
 */
export interface QuickOpenState {
  open: boolean;
  /** Exact session cwd whose authorized index these files came from. */
  root: string | null;
  files: ProjectFileEntry[];
  loading: boolean;
  error: string | null;
  truncated: boolean;
  /** @composer or an exact SessionKey the picked file is attached to. */
  contextTarget: string | null;
}

const QUICK_OPEN_CLOSED: QuickOpenState = {
  open: false, root: null, files: [], loading: false, error: null, truncated: false,
  contextTarget: null,
};

/** The palette state the picker shares: arming it shows the palette, closing it closes both. */
export interface QuickOpenOptions {
  setCommandPaletteOpen: Dispatch<SetStateAction<boolean>>;
  setCommandQuery: Dispatch<SetStateAction<string>>;
}

/**
 * The file picker's store as App drives it: the armed state, the one reset
 * and the opener that reads a session's authorized index. Moved out of App
 * as it was. A pick (`openQuickOpenFile`), ⌘P (`openFilePalette`) and the
 * effect that closes the picker when its session's folder changes stay in
 * App: they read session state App builds after this store.
 */
export function useQuickOpen(api: Pick<IfaceApi, "listProjectFiles">, {
  setCommandPaletteOpen, setCommandQuery,
}: QuickOpenOptions) {
  // The file store IS the palette's mode: `quickOpen.open` means files mode.
  // A second mode state let the two drift, and a reset that lowered one and
  // not the other stranded an open palette over a dead index.
  const [quickOpen, setQuickOpen] = useState<QuickOpenState>(QUICK_OPEN_CLOSED);
  const quickOpenRequest = useRef(0);
  /** The one reset. Everything that dismisses the palette goes through it. */
  const closeQuickOpen = useCallback(() => {
    quickOpenRequest.current += 1;
    setQuickOpen(QUICK_OPEN_CLOSED);
    setCommandPaletteOpen(false);
    setCommandQuery("");
  }, []);

  /**
   * Arm the file picker over one session's authorized index and show it: the
   * command palette in "files" mode. There is no second dialog any more — the
   * two were the same 680px combobox with two datasets.
   */
  const openContextQuickOpen = useCallback((root: string, target: string) => {
    if (root.trim() === "") return;
    const request = quickOpenRequest.current + 1;
    quickOpenRequest.current = request;
    setQuickOpen({
      open: true, root, files: [], loading: true, error: null, truncated: false,
      contextTarget: target,
    });
    setCommandQuery("");
    setCommandPaletteOpen(true);
    api.listProjectFiles(root).then(
      response => {
        if (quickOpenRequest.current !== request) return;
        setQuickOpen(current => current.open && current.root === root
          && current.contextTarget === target
          ? { ...current, files: response.files, loading: false,
              truncated: response.truncated }
          : current);
      },
      (err: unknown) => {
        if (quickOpenRequest.current !== request) return;
        setQuickOpen(current => current.open && current.root === root
          && current.contextTarget === target
          ? { ...current, loading: false, error: ipcMessage(err) }
          : current);
      },
    );
  }, [api]);

  return { quickOpen, closeQuickOpen, openContextQuickOpen };
}
