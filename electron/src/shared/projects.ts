/**
 * The Projects folder New project creates in (protocol 40). Mirrors
 * `interface-contracts/src/projects.rs`; Rust resolves the folder and keeps
 * the owner's choice. `iface:createProject` still takes `{name}` for a create.
 */

/** Electron's main process fills `selectedPath` from the native picker for `choose`. */
export type ProjectsFolderRequest =
  | { projectsFolder: "read" }
  | { projectsFolder: "choose"; selectedPath?: string | null };

/** Why New project uses this folder. */
export type ProjectsFolderSource = "chosen" | "default" | "lastPicked";

export interface ProjectsFolderView {
  version: 1;
  /** The absolute folder New project creates in (created when missing). */
  path: string;
  /** `path` with the home folder written `~`. */
  display: string;
  source: ProjectsFolderSource;
}
