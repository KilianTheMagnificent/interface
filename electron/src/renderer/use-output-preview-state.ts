import { useCallback, useEffect, useMemo, useRef, useState, type Dispatch, type RefObject, type SetStateAction } from "react";
import type { SessionKey, SessionSummary } from "../shared/engine";
import type { IfaceApi } from "../shared/ipc";
import type { OutputPreviewOpenRequest } from "./components/OutputPreviewPanel";
import type { WorkspaceTool } from "./components/TerminalPane";
import { attachmentPreviewTarget, outputPreviewTarget } from "./output-preview";
import type { PromptContextItem, PromptTurnAttachment } from "./prompt-context";
import type { AppState } from "./store";
import type { useThreadDiff } from "./use-thread-pane";

type ThreadDiff = ReturnType<typeof useThreadDiff>;

/** What the output preview reads from App: its state, its refs and its own verbs. */
export interface OutputPreviewStateOptions {
  state: Pick<AppState, "composerOpen">;
  threadDiff: Pick<ThreadDiff, "target">;
  /** The subagent detail column's selection: any selection hides the preview. */
  subagentDetail: object | null;
  sessionByKeyRef: RefObject<ReadonlyMap<SessionKey, SessionSummary>>;
  threadDiffRef: RefObject<Pick<ThreadDiff, "close">>;
  setActionError: Dispatch<SetStateAction<string | null>>;
  setSubagentDetail: (selection: null) => void;
  openWorkspaceTool: (tool: WorkspaceTool, cwd: string) => void;
}

/**
 * The thread output preview (OutputPreviewPanel) as App drives it: which
 * file or attachment it shows and for which thread, the openers, Close, and
 * the effects that hide it when the diff panel, a subagent detail or the
 * composer's close takes its place. Moved out of App as it was; whether the
 * panel is visible (`threadPreviewOpen`) is still App's, because it reads
 * the layout. Every returned callback keeps App's dependency array, so the
 * memo(OutputPreviewPanel) props stay as stable as they were.
 */
export function useOutputPreviewState(api: Pick<IfaceApi, "previewOutput" | "visualFrame">, {
  state, threadDiff, subagentDetail, sessionByKeyRef, threadDiffRef,
  setActionError, setSubagentDetail, openWorkspaceTool,
}: OutputPreviewStateOptions) {
  const [threadPreview, setThreadPreview] = useState<{
    key: SessionKey | null; root: string; shown: boolean; request: OutputPreviewOpenRequest | null;
  } | null>(null);
  const previewNonce = useRef(0);
  const previewThreadFile = useCallback((key: SessionKey, path: string | null) => {
    const summary = sessionByKeyRef.current.get(key);
    if (!summary) return;
    const target = path === null ? { root: summary.cwd, path: null } : outputPreviewTarget(path, summary.cwd);
    if (!target) { setActionError("Choose a PDF, image or Markdown file inside this thread's folder."); return; }
    setSubagentDetail(null);
    threadDiffRef.current.close();
    previewNonce.current += 1;
    setThreadPreview({ key, root: target.root, shown: true,
      request: target.path === null ? null : { path: target.path, nonce: previewNonce.current } });
  }, []);
  const previewAttachment = useCallback((
      key: SessionKey | null,
      attachment: PromptTurnAttachment | PromptContextItem,
      fallbackRoot = "",
  ) => {
    const summary = key === null ? undefined : sessionByKeyRef.current.get(key);
    const baseRoot = summary?.projectRoot ?? summary?.cwd ?? fallbackRoot;
    if ("value" in attachment) {
      if (attachment.kind !== "file" && attachment.kind !== "folder" && attachment.kind !== "diff") return;
      if (attachment.kind === "diff") {
        if (attachment.safeContent === undefined) return;
        setSubagentDetail(null);
        threadDiffRef.current.close();
        previewNonce.current += 1;
        const root = baseRoot.startsWith("/") ? baseRoot : "/repository";
        setThreadPreview({
          key, root, shown: true,
          request: {
            root, path: `${attachment.label}.diff`, nonce: previewNonce.current,
            inlineText: { text: attachment.safeContent, format: "diff" },
          },
        });
        return;
      }
    }
    const rawPath = "path" in attachment ? attachment.path : attachment.value;
    if (attachment.kind === "folder") {
      const trimmed = rawPath.replace(/\/+$/u, "");
      if (!trimmed.startsWith("/")) return;
      setSubagentDetail(null);
      threadDiffRef.current.close();
      previewNonce.current += 1;
      setThreadPreview({
        key, root: baseRoot.startsWith("/") ? baseRoot : trimmed, shown: true,
        request: { root: trimmed, path: "", directory: true, nonce: previewNonce.current },
      });
      return;
    }
    const target = attachmentPreviewTarget(rawPath, baseRoot);
    if (target === null) {
      if (attachment.previewUrl !== undefined) {
        const root = baseRoot.startsWith("/") ? baseRoot : "/attachments";
        setSubagentDetail(null);
        threadDiffRef.current.close();
        previewNonce.current += 1;
        setThreadPreview({
          key, root, shown: true,
          request: {
            root, path: attachment.label || "image.png", nonce: previewNonce.current,
            inlineImage: attachment.previewUrl,
          },
        });
        return;
      }
      setActionError("Choose a local file attachment to preview.");
      return;
    }
    setSubagentDetail(null);
    threadDiffRef.current.close();
    previewNonce.current += 1;
    setThreadPreview({
      key, root: baseRoot.startsWith("/") ? baseRoot : target.root, shown: true,
      request: {
        root: target.root, path: target.path, nonce: previewNonce.current,
        ...(attachment.previewUrl === undefined ? {} : { inlineImage: attachment.previewUrl }),
      },
    });
  }, []);
  const closeThreadPreview = useCallback(() => { setThreadPreview(value => value ? { ...value, shown: false } : null); }, []);
  const threadPreviewApi = useMemo(() => ({ previewOutput: api.previewOutput,
    visualFrame: api.visualFrame }), [api]);
  useEffect(() => { if (threadDiff.target !== null) setThreadPreview(value => value ? { ...value, shown: false } : null); }, [threadDiff.target]);
  useEffect(() => { if (subagentDetail !== null) setThreadPreview(value => value ? { ...value, shown: false } : null); }, [subagentDetail]);
  useEffect(() => { if (!state.composerOpen) setThreadPreview(value => value?.key === null ? { ...value, shown: false } : value); }, [state.composerOpen]);
  const openPreviewFolder = useCallback((root: string): void => {
    openWorkspaceTool("files", root);
  }, [openWorkspaceTool]);

  return {
    threadPreview, previewThreadFile, previewAttachment, closeThreadPreview, threadPreviewApi,
    openPreviewFolder,
  };
}
