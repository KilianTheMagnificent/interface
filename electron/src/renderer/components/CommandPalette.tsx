import { useEffect, useId, useMemo, useRef, useState } from "react";
import type { ChangeEvent, KeyboardEvent, MouseEvent, ReactNode } from "react";
import type { ProjectFileEntry } from "../../shared/ipc";
import { useModalFocus } from "./ModalBehavior";
import { lastSegment } from "../display-paths";
import { isPreviewableOutput } from "../output-preview";
import {
  fuzzyPathMatchIndices, indexPaletteFiles, pathDirectory,
  PALETTE_QUERY_MAX_CHARS, rankPaletteIndex, type RankedPaletteFile,
} from "./palette-match";

/** One action shown in the command palette. Command ids must be stable. */
export interface CommandPaletteCommand {
  id: string;
  label: string;
  group?: string;
  description?: string;
  keywords?: readonly string[];
  shortcut?: string | readonly string[];
  disabled?: boolean;
  /** Plain-language explanation shown beside a disabled command. */
  disabledReason?: string;
  /**
   * A project-relative path this row was derived from. Set only by the palette
   * itself, for the Files group: it switches the row to the file layout and
   * marks the characters the query matched. Callers never pass it.
   */
  matchPath?: string;
}

/**
 * "all" is ⌘⇧P: commands, plus files once something is typed. "files" is ⌘P —
 * files only, so a project with thousands of them is not competing with the
 * command registry for the same hundred rows. Picking one ADDS it to the
 * prompt's context; this renderer has no editor to navigate to.
 */
export type CommandPaletteMode = "all" | "files";

export interface CommandPaletteProps {
  open: boolean;
  query: string;
  commands: readonly CommandPaletteCommand[];
  mode?: CommandPaletteMode;
  /** The authorized index for the picker's root. Empty outside files work. */
  files?: readonly ProjectFileEntry[];
  filesLoading?: boolean;
  filesError?: string | null;
  /** The backend hit a walk ceiling, so absent matches are not authoritative. */
  filesTruncated?: boolean;
  onQuery: (query: string) => void;
  onRun: (command: CommandPaletteCommand) => void;
  /** Required for the Files group to be actionable. */
  onOpenFile?: (file: ProjectFileEntry) => void;
  /**
   * Open a PDF, image or Markdown file in the thread's preview instead of
   * attaching it (review B5): ⌘↵, or ⌘-click, on its row. Absent, files only
   * attach.
   */
  onPreviewFile?: (file: ProjectFileEntry) => void;
  onClose: () => void;
}

export interface CommandPaletteGroup {
  name: string;
  commands: readonly CommandPaletteCommand[];
}

export type CommandPaletteKeyAction =
  | { type: "move"; index: number }
  | { type: "run"; command: CommandPaletteCommand }
  | { type: "close" };

/** The palette never mounts an unbounded number of option nodes. */
export const COMMAND_PALETTE_MAX_RESULTS = 100;
export const COMMAND_PALETTE_AGENT_PREVIEW = 8;
/**
 * Files belong to "files" mode alone (⌘P), where they are the whole list. A
 * hundred is Quick Open's own cap, kept.
 */
export const COMMAND_PALETTE_FILE_RESULTS = 100;
const COMMAND_PALETTE_FILE_PREFIX = "file:";
const COMMAND_PALETTE_RECENT_KEY = "interface.commandPalette.recents";

const NO_FILES: readonly ProjectFileEntry[] = [];
const NO_RANKED_FILES: readonly RankedPaletteFile[] = [];
const NO_COMMANDS: readonly CommandPaletteCommand[] = [];

const DEFAULT_GROUP = "Commands";

// Commands are immutable once built, so each one's haystack is built once,
// not once per command per keystroke (about 1,100 sessions on every key).
const SEARCH_TEXT = new WeakMap<CommandPaletteCommand, string>();

export const commandSearchText = (command: CommandPaletteCommand): string => {
  const known = SEARCH_TEXT.get(command);
  if (known !== undefined) return known;
  const shortcut = typeof command.shortcut === "string"
    ? command.shortcut
    : command.shortcut?.join(" ");
  const text = [command.label, command.description, command.group, shortcut,
    ...(command.keywords ?? [])]
    .filter((part): part is string => typeof part === "string" && part !== "")
    .join(" ")
    .toLowerCase();
  SEARCH_TEXT.set(command, text);
  return text;
};

/**
 * Small, deterministic search for a small command registry. Each whitespace-
 * separated query term is a literal, case-insensitive substring; every term
 * must occur somewhere in the command's label, group, description, keywords,
 * or shortcut. Registry order is retained and rendered results are capped.
 */
export function searchCommandPalette(
  commands: readonly CommandPaletteCommand[],
  query: string,
  limit = COMMAND_PALETTE_MAX_RESULTS,
): CommandPaletteCommand[] {
  const boundedLimit = Math.max(0, Math.min(
    COMMAND_PALETTE_MAX_RESULTS,
    Number.isFinite(limit) ? Math.floor(limit) : COMMAND_PALETTE_MAX_RESULTS,
  ));
  if (boundedLimit === 0) return [];

  const terms = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
  const matches: CommandPaletteCommand[] = [];
  for (const command of commands) {
    const haystack = commandSearchText(command);
    if (terms.every(term => haystack.includes(term))) {
      matches.push(command);
      if (matches.length === boundedLimit) break;
    }
  }
  return matches;
}

/** Keep the empty palette useful even when the registry contains hundreds of sessions. */
export function displayCommandPalette(
  commands: readonly CommandPaletteCommand[],
  query: string,
  recentIds: readonly string[] = [],
): CommandPaletteCommand[] {
  if (query.trim() !== "") return searchCommandPalette(commands, query);
  const matches = [...commands];

  const byId = new Map(matches.map(command => [command.id, command]));
  const recent = recentIds.flatMap(id => {
    const command = byId.get(id);
    return command === undefined ? [] : [{ ...command, group: "Recent" }];
  }).slice(0, 5);
  const recentSet = new Set(recent.map(command => command.id));
  const remaining = matches.filter(command => !recentSet.has(command.id));
  const agents = remaining.filter(command => command.id.startsWith("agent:"));
  const core = remaining.filter(command => !command.id.startsWith("agent:"));
  const coreLimit = Math.max(0,
    COMMAND_PALETTE_MAX_RESULTS - recent.length - COMMAND_PALETTE_AGENT_PREVIEW);
  return [...recent, ...core.slice(0, coreLimit),
    ...agents.slice(0, COMMAND_PALETTE_AGENT_PREVIEW)];
}

function readRecentCommandIds(): string[] {
  if (typeof window === "undefined") return [];
  try {
    const parsed: unknown = JSON.parse(
      window.localStorage.getItem(COMMAND_PALETTE_RECENT_KEY) ?? "[]",
    );
    return Array.isArray(parsed)
      ? parsed.filter((value): value is string => typeof value === "string").slice(0, 5)
      : [];
  } catch {
    return [];
  }
}

function writeRecentCommandIds(ids: readonly string[]): void {
  if (typeof window === "undefined") return;
  try { window.localStorage.setItem(COMMAND_PALETTE_RECENT_KEY, JSON.stringify(ids.slice(0, 5))); }
  catch { /* Recent commands are an enhancement, never a reason to block an action. */ }
}

/** Consolidate groups in first-seen order, retaining order within each group. */
export function groupCommandPalette(
  commands: readonly CommandPaletteCommand[],
): CommandPaletteGroup[] {
  const groups: Array<{ name: string; commands: CommandPaletteCommand[] }> = [];
  const byName = new Map<string, CommandPaletteCommand[]>();
  for (const command of commands) {
    const name = command.group?.trim() || DEFAULT_GROUP;
    let entries = byName.get(name);
    if (entries === undefined) {
      entries = [];
      byName.set(name, entries);
      groups.push({ name, commands: entries });
    }
    entries.push(command);
  }
  return groups;
}

function firstEnabledIndex(commands: readonly CommandPaletteCommand[]): number {
  return commands.findIndex(command => !command.disabled);
}

/** The highlighted command, and the row it was on when the user chose it. */
export interface PaletteHighlight {
  readonly id: string;
  readonly index: number;
}

/**
 * The row the highlight is on, which is the row Enter runs. It follows the
 * command the user chose, not a row number: a registry push that reorders or
 * renames agents while the palette is open leaves the same command
 * highlighted. The chosen row is kept while it still holds that command; a
 * command that moved is found by id; one that is gone or disabled falls back
 * to the first enabled row, as does no choice at all.
 */
export function paletteHighlightIndex(
  visible: readonly CommandPaletteCommand[],
  highlight: PaletteHighlight | null,
): number {
  if (highlight !== null) {
    const at = visible[highlight.index];
    if (at !== undefined && at.id === highlight.id && !at.disabled) return highlight.index;
    const moved = visible.findIndex(command => command.id === highlight.id && !command.disabled);
    if (moved >= 0) return moved;
  }
  return firstEnabledIndex(visible);
}

/**
 * Keyboard behaviour shared by the component and node-only tests. Navigation
 * wraps and skips disabled commands; Enter cannot dispatch a disabled action.
 */
export function commandPaletteKeyAction(
  commands: readonly CommandPaletteCommand[],
  activeIndex: number,
  key: string,
): CommandPaletteKeyAction | null {
  if (key === "Escape") return { type: "close" };

  const enabled = commands
    .map((command, index) => command.disabled ? -1 : index)
    .filter(index => index >= 0);

  if (key === "Enter") {
    const command = commands[activeIndex];
    return command !== undefined && !command.disabled
      ? { type: "run", command }
      : null;
  }
  if (enabled.length === 0) return null;
  if (key === "Home") return { type: "move", index: enabled[0] };
  if (key === "End") return { type: "move", index: enabled[enabled.length - 1] };
  if (key !== "ArrowDown" && key !== "ArrowUp") return null;

  const position = enabled.indexOf(activeIndex);
  if (position === -1) {
    return {
      type: "move",
      index: key === "ArrowDown" ? enabled[0] : enabled[enabled.length - 1],
    };
  }
  const direction = key === "ArrowDown" ? 1 : -1;
  const next = (position + direction + enabled.length) % enabled.length;
  return { type: "move", index: enabled[next] };
}

const shortcutParts = (shortcut: CommandPaletteCommand["shortcut"]): readonly string[] =>
  typeof shortcut === "string" ? [shortcut] : (shortcut ?? []);

export const COMMAND_PALETTE_CSS = `
.command-palette-backdrop {
  position: fixed;
  inset: 0;
  z-index: 1000;
  display: flex;
  align-items: flex-start;
  justify-content: center;
  padding-top: max(64px, 10vh);
  background: color-mix(in srgb, var(--chrome-bg) 60%, transparent);
  animation: command-palette-backdrop-in var(--motion-base)
    var(--ease-standard) both;
}
.command-palette-dialog {
  display: flex;
  flex-direction: column;
  width: min(680px, calc(100vw - 32px));
  max-height: min(640px, calc(100vh - 96px));
  overflow: hidden;
  color: var(--fg);
  background: var(--widget-bg);
  border: var(--border-w) solid var(--border-strong);
  border-radius: var(--radius-overlay);
  box-shadow: var(--overlay-shadow);
  font-family: var(--font-ui);
  transform-origin: 50% 0;
  animation: command-palette-dialog-in var(--motion-slow)
    var(--ease-out) both;
}
.command-palette-search {
  flex: none;
  padding: 8px;
  border-bottom: var(--border-w) solid var(--border);
}
.command-palette-input {
  box-sizing: border-box;
  width: 100%;
  height: 34px;
  padding: 0 10px;
  color: var(--input-fg);
  background: var(--input-bg);
  border: var(--border-w) solid var(--input-border);
  border-radius: var(--radius-control);
  font: inherit;
}
.command-palette-input::placeholder { color: var(--input-placeholder); }
.command-palette-list {
  min-height: 0;
  margin: 0;
  padding: 4px;
  overflow-y: auto;
}
.command-palette-group + .command-palette-group { margin-top: 4px; }
.command-palette-group-label {
  padding: 6px 8px 4px;
  color: var(--fg-dim);
  font-size: var(--text-meta);
  font-weight: var(--wght-strong);
}
.command-palette-option {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  gap: 12px;
  align-items: center;
  box-sizing: border-box;
  width: 100%;
  min-height: 38px;
  padding: 5px 8px;
  color: var(--fg);
  background: transparent;
  border: 0;
  border-radius: var(--radius-control);
  font: inherit;
  text-align: left;
  transition: background-color var(--motion-fast) linear,
    color var(--motion-fast) linear,
    transform var(--motion-fast) var(--ease-out);
}
.command-palette-option:not([aria-disabled="true"]):active { transform:scale(.995); }
.command-palette-option[aria-selected="true"] { background: var(--row-selected); }
.command-palette-option:not([aria-disabled="true"]):hover { background: var(--row-hover); }
.command-palette-option[aria-disabled="true"] {
  color: var(--fg-faint);
  cursor: default;
}
/* The Files group's row: an icon column ahead of the copy. Same 38px, same
   padding, same gap as every other option — one overlay recipe, two datasets. */
.command-palette-option-file { grid-template-columns: 18px minmax(0, 1fr) auto; }
.command-palette-file-icon { color: var(--fg-faint); }
.command-palette-match {
  color: var(--fg-strong); background: var(--selection); border-radius: var(--radius-control);
}
.command-palette-copy { min-width: 0; }
.command-palette-label {
  display: block;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.command-palette-description {
  display: block;
  margin-top: 2px;
  overflow: hidden;
  color: var(--fg-dim);
  font-size: var(--text-meta);
  text-overflow: ellipsis;
  white-space: nowrap;
}
.command-palette-option[aria-disabled="true"] .command-palette-description {
  color: var(--fg-faint);
}
.command-palette-disabled-reason {
  display:block; margin-top:2px; color:var(--fg-muted); font-size:var(--text-meta);
}
.command-palette-shortcut {
  display: inline-flex;
  gap: 4px;
  align-items: center;
  color: var(--fg-muted);
}
/* Plain text, as in every menu (owner, 2026-09-28), not a boxed keycap. */
.command-palette-shortcut kbd {
  min-width: 0;
  padding: 0;
  background: transparent;
  border: 0;
  font-family: var(--font-ui);
  font-size: var(--text-meta);
  line-height: var(--lh-prose);
  text-align: center;
}
.command-palette-empty {
  padding: 24px 12px;
  color: var(--fg-dim);
  text-align: center;
}
.command-palette-error { color: var(--danger-fg); }
.command-palette-more {
  min-height:30px; padding:7px 10px; color:var(--fg-dim);
  border-top:var(--border-w) solid var(--border); font-size:var(--text-meta);
}
.command-palette-sr-only {
  position: absolute;
  width: 1px;
  height: 1px;
  padding: 0;
  margin: -1px;
  overflow: hidden;
  clip: rect(0, 0, 0, 0);
  white-space: nowrap;
  border: 0;
}
@keyframes command-palette-backdrop-in { from { opacity:0; } to { opacity:1; } }
@keyframes command-palette-dialog-in {
  from { opacity:0; transform:translateY(-8px) scale(.985); }
  to { opacity:1; transform:translateY(0) scale(1); }
}
@media (prefers-reduced-motion: reduce) {
  .command-palette-backdrop, .command-palette-dialog { animation:none; }
  .command-palette-option { transition:none; }
}
`;

/**
 * The per-character match marks, ported from Quick Open. They are the reason a
 * fuzzy result is trustworthy: "qopen" landing on QuickOpen.tsx reads as magic
 * until the five characters it matched are drawn.
 */
function highlighted(value: string, indexes: ReadonlySet<number>, offset: number): ReactNode {
  if (indexes.size === 0) return value;
  const parts: ReactNode[] = [];
  let plain = "";
  let markStart = -1;
  let markText = "";
  const flushPlain = (): void => {
    if (plain === "") return;
    parts.push(plain);
    plain = "";
  };
  const flushMark = (): void => {
    if (markStart < 0) return;
    parts.push(<mark className="command-palette-match" key={`${offset}:${markStart}`}>
      {markText}
    </mark>);
    markStart = -1;
    markText = "";
  };
  for (let index = 0; index < value.length; index += 1) {
    if (!indexes.has(index + offset)) {
      flushMark();
      plain += value[index];
      continue;
    }
    flushPlain();
    if (markStart < 0) markStart = index;
    markText += value[index];
  }
  flushMark();
  flushPlain();
  return parts;
}

export function CommandPalette({
  open, query, commands, mode = "all", files = NO_FILES, filesLoading = false,
  filesError = null, filesTruncated = false, onQuery, onRun, onOpenFile, onPreviewFile, onClose,
}: CommandPaletteProps) {
  const baseId = useId();
  const inputId = `${baseId}-input`;
  const listboxId = `${baseId}-listbox`;
  const dialogRef = useRef<HTMLElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const optionRefs = useRef<Array<HTMLButtonElement | null>>([]);
  const [highlight, setHighlight] = useState<PaletteHighlight | null>(null);
  const [recentIds, setRecentIds] = useState<readonly string[]>(readRecentCommandIds);

  const fileMode = mode === "files";
  // Normalized and clutter-filtered once per index load, not per keystroke.
  const fileIndex = useMemo(() => indexPaletteFiles(files), [files]);
  // Files only ever arrive with the mode that armed them: App hands the palette
  // an empty list until ⌘P indexes a project, so an "all"-mode tail could never
  // have anything in it. A closed palette ranks nothing.
  const rankedFiles = useMemo(() => {
    if (!open || !fileMode || onOpenFile === undefined) return NO_RANKED_FILES;
    return rankPaletteIndex(fileIndex, query, COMMAND_PALETTE_FILE_RESULTS);
  }, [fileIndex, fileMode, onOpenFile, open, query]);

  const fileCommands = useMemo<CommandPaletteCommand[]>(() => rankedFiles.map(entry => ({
    id: `${COMMAND_PALETTE_FILE_PREFIX}${entry.file.path}`,
    label: lastSegment(entry.file.relativePath),
    description: pathDirectory(entry.file.relativePath),
    group: "Files",
    matchPath: entry.file.relativePath,
  })), [rankedFiles]);
  const fileByCommandId = useMemo(() => new Map(rankedFiles.map(entry =>
    [`${COMMAND_PALETTE_FILE_PREFIX}${entry.file.path}`, entry.file])), [rankedFiles]);

  // Closed, nothing is drawn: a registry push that rebuilds `commands` ranks
  // and groups nothing until the palette opens.
  const matches = useMemo(
    () => !open ? NO_COMMANDS
      : fileMode ? fileCommands : displayCommandPalette(commands, query, recentIds),
    [commands, fileCommands, fileMode, open, query, recentIds],
  );
  const groups = useMemo(() => groupCommandPalette(matches), [matches]);
  const visible = useMemo(
    () => groups.flatMap(group => group.commands),
    [groups],
  );
  const resolvedActiveIndex = paletteHighlightIndex(visible, highlight);

  // A new query, mode or opening starts again from the first enabled row. A
  // new command list alone does not: the highlight follows its command.
  useEffect(() => {
    if (open) setHighlight(null);
  }, [fileMode, open, query]);

  // Capture, restore and the Tab trap come from the shared modal hook, the way
  // SessionLibrary's already did, rather than from a third hand-rolled copy.
  const handleModalKey = useModalFocus<HTMLElement>({
    open,
    containerRef: dialogRef,
    initialFocus: () => inputRef.current,
    onEscape: onClose,
  });

  useEffect(() => {
    if (!open || resolvedActiveIndex < 0) return;
    optionRefs.current[resolvedActiveIndex]?.scrollIntoView({ block: "nearest" });
  }, [open, resolvedActiveIndex]);

  if (!open) return null;

  const activeOptionId = resolvedActiveIndex >= 0
    ? `${baseId}-option-${resolvedActiveIndex}`
    : undefined;

  const handleInput = (event: ChangeEvent<HTMLInputElement>) => {
    onQuery(event.currentTarget.value);
  };

  const runCommand = (command: CommandPaletteCommand) => {
    // A file is not a command: it never enters the recents list, and it goes
    // to the picker's own handler rather than the command dispatcher.
    const file = fileByCommandId.get(command.id);
    if (file !== undefined) {
      onOpenFile?.(file);
      return;
    }
    const next = [command.id, ...recentIds.filter(id => id !== command.id)].slice(0, 5);
    setRecentIds(next);
    writeRecentCommandIds(next);
    onRun(command);
  };

  // ⌘↵ on a previewable file opens it in the thread's preview.
  const previewable = (command: CommandPaletteCommand | undefined): ProjectFileEntry | undefined => {
    const file = command === undefined ? undefined : fileByCommandId.get(command.id);
    return onPreviewFile !== undefined && file !== undefined && isPreviewableOutput(file.relativePath)
      ? file : undefined;
  };
  const handleInputKey = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
      const file = previewable(visible[resolvedActiveIndex]);
      if (file !== undefined) {
        event.preventDefault();
        event.stopPropagation();
        onPreviewFile?.(file);
        return;
      }
    }
    const action = commandPaletteKeyAction(visible, resolvedActiveIndex, event.key);
    if (action === null) return;
    event.preventDefault();
    event.stopPropagation();
    if (action.type === "move") {
      setHighlight({ id: visible[action.index].id, index: action.index });
    } else if (action.type === "run") runCommand(action.command);
    else onClose();
  };

  const handleBackdrop = (event: MouseEvent<HTMLDivElement>) => {
    if (event.target === event.currentTarget) onClose();
  };

  let optionIndex = 0;
  const matchTerms = query;
  const totalAgents = commands.filter(command => command.id.startsWith("agent:")).length;
  const shownAgents = visible.filter(command => command.id.startsWith("agent:")).length;
  const hiddenAgents = !fileMode && query.trim() === ""
    ? Math.max(0, totalAgents - shownAgents) : 0;
  // The files mode ADDS a file to the prompt's context; it does not navigate
  // to one. "Go to file" promised an editor this renderer does not have.
  const fieldLabel = fileMode ? "Search files by name" : "Search commands";
  const blocked = fileMode && (filesError !== null || filesLoading);
  return <>
    <style>{COMMAND_PALETTE_CSS}</style>
    <div className="command-palette-backdrop" onMouseDown={handleBackdrop}>
      <section
        ref={dialogRef}
        className="command-palette-dialog is-floating"
        role="dialog"
        aria-modal="true"
        aria-label={fileMode ? "Add a file to the prompt" : "Command palette"}
        onKeyDown={handleModalKey}
      >
        <div className="command-palette-search">
          <label className="command-palette-sr-only" htmlFor={inputId}>
            {fieldLabel}
          </label>
          <input
            ref={inputRef}
            id={inputId}
            className="command-palette-input"
            type="text"
            role="combobox"
            aria-autocomplete="list"
            aria-expanded="true"
            aria-controls={listboxId}
            aria-activedescendant={activeOptionId}
            autoComplete="off"
            spellCheck={false}
            maxLength={fileMode ? PALETTE_QUERY_MAX_CHARS : undefined}
            placeholder={fileMode ? "Search files by name…" : "Search commands…"}
            value={query}
            onChange={handleInput}
            onKeyDown={handleInputKey}
          />
        </div>

        <div
          id={listboxId}
          className="command-palette-list"
          role="listbox"
          aria-label={fileMode ? "Matching project files" : "Matching commands"}
        >
          {filesError !== null && fileMode
            ? <div className="command-palette-empty command-palette-error" role="alert">
                {filesError}
              </div>
            : filesLoading && fileMode
            ? <div className="command-palette-empty" role="status">Loading project files…</div>
            : visible.length === 0
            ? <div className="command-palette-empty" role="status">
                {fileMode
                  ? (query.trim() === "" ? "No project files available." : "No matching files.")
                  : (query.trim() === "" ? "No commands available." : "No matching commands.")}
              </div>
            : <>
                <div className="command-palette-sr-only" role="status" aria-live="polite">
                  {visible.length} {fileMode
                    ? (visible.length === 1 ? "file" : "files")
                    : (visible.length === 1 ? "command" : "commands")} available.
                </div>
                {groups.map((group, groupIndex) => {
                  const groupId = `${baseId}-group-${groupIndex}`;
                  return <div
                    key={`${group.name}-${groupIndex}`}
                    className="command-palette-group"
                    role="group"
                    aria-labelledby={groupId}
                  >
                    <div id={groupId} className="command-palette-group-label">
                      {group.name}
                    </div>
                    {group.commands.map(command => {
                      const index = optionIndex++;
                      const optionId = `${baseId}-option-${index}`;
                      const shortcuts = shortcutParts(command.shortcut);
                      const disabled = command.disabled === true;
                      const path = command.matchPath;
                      // The name is the tail of the path, so its match
                      // positions are offset by the directory it sits under.
                      const marks = path === undefined
                        ? null : new Set(fuzzyPathMatchIndices(path, matchTerms));
                      const nameOffset = command.description === undefined
                        || command.description === "" ? 0 : command.description.length + 1;
                      return <button
                        key={`${command.id}-${index}`}
                        id={optionId}
                        ref={node => { optionRefs.current[index] = node; }}
                        className={path === undefined
                          ? "command-palette-option"
                          : "command-palette-option command-palette-option-file"}
                        type="button"
                        role="option"
                        tabIndex={-1}
                        title={path}
                        aria-selected={index === resolvedActiveIndex}
                        aria-disabled={disabled || undefined}
                        onMouseDown={event => event.preventDefault()}
                        onMouseEnter={() => {
                          if (!disabled) setHighlight({ id: command.id, index });
                        }}
                        onClick={event => {
                          if (disabled) return;
                          const file = event.metaKey || event.ctrlKey ? previewable(command) : undefined;
                          if (file !== undefined) onPreviewFile?.(file);
                          else runCommand(command);
                        }}
                      >
                        {marks !== null && <span
                          className="codicon codicon-file command-palette-file-icon"
                          aria-hidden="true" />}
                        <span className="command-palette-copy">
                          <span className="command-palette-label">
                            {marks === null
                              ? command.label
                              : highlighted(command.label, marks, nameOffset)}
                          </span>
                          {command.description && <span className="command-palette-description">
                            {marks === null
                              ? command.description
                              : highlighted(command.description, marks, 0)}
                          </span>}
                          {disabled && <span className="command-palette-disabled-reason">
                            {command.disabledReason ?? "Unavailable right now"}
                          </span>}
                        </span>
                        {index === resolvedActiveIndex && previewable(command) !== undefined && <span
                          className="command-palette-shortcut command-palette-preview"
                          aria-label="Command Enter opens it in the preview">
                          <kbd aria-hidden="true">⌘↵</kbd>
                          <span aria-hidden="true">Preview</span>
                        </span>}
                        {shortcuts.length > 0 && <span
                          className="command-palette-shortcut" role="img"
                          aria-label={`Shortcut: ${shortcuts.join(" then ")}`}
                        >
                          {shortcuts.map((part, shortcutIndex) => <kbd
                            key={`${part}-${shortcutIndex}`}
                            aria-hidden="true"
                          >{part}</kbd>)}
                        </span>}
                      </button>;
                    })}
                  </div>;
                })}
              </>}
        </div>
        {hiddenAgents > 0 && <div className="command-palette-more" role="status">
          Search to find {hiddenAgents} more {hiddenAgents === 1 ? "agent" : "agents"}.
        </div>}
        {fileMode && !blocked && <div className="command-palette-more" role="status">
          {files.length} {files.length === 1 ? "file" : "files"} indexed
          {filesTruncated ? " · index incomplete" : ""}
          {onPreviewFile !== undefined ? " · ↵ attaches · ⌘↵ opens a PDF, image or Markdown in the preview" : ""}
        </div>}
      </section>
    </div>
  </>;
}
