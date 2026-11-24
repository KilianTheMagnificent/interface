import type { SessionKey } from "../shared/engine.js";

// These four channels remain separate from shared/ipc.ts because that is the
// retained source contract. The Rust host validates the combined versioned
// namespace; this file contains presentation types only.
export const EXTRA_CHANNELS = {
  adoptAll: "iface:adoptAll",
  sendLive: "iface:sendLive",
  importProgress: "iface:importProgress",
  worktree: "iface:worktree",
} as const satisfies Record<string, string>;

// The same naming rule as CHANNELS, which the preload relies on.
EXTRA_CHANNELS satisfies { readonly [K in keyof typeof EXTRA_CHANNELS]: `iface:${K}` };

export interface ImportProgress {
  adopt: boolean;
  done: number;
  total: number;
  ok: number;
  failed: number;
  key?: SessionKey;
}

export interface ImportReport {
  adopt: boolean;
  total: number;
  ok: number;
  local: number;
  failed: number;
  errors: string[];
}

export interface WorktreeNotice {
  key: SessionKey;
  path: string;
  branch: string;
  state: "created" | "removed" | "kept";
  detail?: string;
}

export interface IfaceImportApi {
  adoptAll(adopt: boolean): Promise<ImportReport>;
  onImportProgress(cb: (progress: ImportProgress) => void): () => void;
  onWorktree(cb: (notice: WorktreeNotice) => void): () => void;
}

declare module "../shared/ipc.js" {
  interface IfaceApi extends IfaceImportApi {}
}
