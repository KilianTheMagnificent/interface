import { useCallback, useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import type { CSSProperties, KeyboardEvent } from "react";
import { createPortal } from "react-dom";
import {
  DEFAULT_PROMPT_CONTEXT_LIMIT,
  normalizePromptContextItems,
  promptContextKindLabel,
  promptContextLimit,
  promptContextOptions,
  promptContextTokenEstimate,
  type PromptContextCapabilities,
  type PromptContextItem,
  type PromptContextKind,
  type PromptContextOption,
} from "../prompt-context";
import {
  providerCommandMenuKeyAction, type ProviderCommandKeyEvent,
} from "../provider-commands";
import { clamp } from "../helpers";
import { focusPastTrigger } from "./ModalBehavior";
import "../codicons.css";
import { scrollMovesAnchor } from "./scroll-anchor";

/** One codicon per kind, shared with the Threads composer's "+" menu. */
export const KIND_ICON: Record<PromptContextKind, string> = {
  file: "file",
  folder: "folder",
  diff: "git-compare",
  chat: "comment-discussion",
  url: "link-external",
  // One glyph, one meaning. `gear` is the app's settings mark and `robot` is a
  // subagent everywhere else in the transcript; an MCP server is something the
  // session plugs into and a skill is a capability it can reach for.
  mcp: "plug",
  skill: "lightbulb",
};

export const PROMPT_CONTEXT_PICKER_CSS = `
.prompt-context { position:relative; display:flex; align-items:center; gap:4px; min-width:0; }
/* One of the icon buttons on its row and drawn as one: 24 square, borderless,
   --radius-control, with the same hover wash .conv-iconbtn and
   .composer-icon-btn take. It shipped as a 30px round OUTLINED control between
   a 24px square one and a 24px pill, which is what made that row read as
   assembled — and it left the composer wearing two circles, one to add context
   and one to send. Send keeps the only round fill on the row. */
.prompt-context-trigger {
  display:inline-flex; flex:none; align-items:center; justify-content:center;
  width:24px; height:24px; padding:0;
  color:var(--fg-muted); background:transparent;
  border:0; border-radius:var(--radius-control);
  font:inherit; cursor:pointer;
}
.prompt-context-trigger:not(:disabled):hover,
.prompt-context-trigger[aria-expanded="true"] { color:var(--fg); background:var(--row-hover); }
.prompt-context-trigger:disabled { color:var(--fg-faint); cursor:default; background:transparent; }
.prompt-context-menu {
  position:fixed; z-index:1050; box-sizing:border-box;
  width:min(340px, calc(100vw - 24px)); max-height:min(420px, calc(100vh - 24px));
  padding:4px; overflow-y:auto; color:var(--fg); background:var(--dropdown-bg);
  border:var(--border-w) solid var(--border-strong); border-radius:var(--radius-overlay);
  box-shadow:var(--overlay-shadow); font-family:var(--font-ui);
}
.prompt-context-option {
  display:grid; grid-template-columns:var(--icon-size) minmax(0, 1fr);
  column-gap:8px; width:100%; min-height:48px; padding:6px 8px;
  color:var(--fg); text-align:left; background:transparent; border:0;
  border-radius:var(--radius-control); font:inherit; cursor:pointer;
}
.prompt-context-option:not([aria-disabled="true"]):hover,
.prompt-context-option:focus-visible { background:var(--row-hover); }
.prompt-context-option[aria-disabled="true"] { color:var(--fg-muted); cursor:default; }
.prompt-context-option-icon { display:inline-flex; align-items:center; justify-content:center;
  width:var(--icon-size); height:var(--icon-size); margin-top:1px; color:var(--fg-dim); }
.prompt-context-option-copy { display:flex; flex-direction:column; gap:2px; min-width:0; }
.prompt-context-option-label { color:inherit; font-size:var(--text-ui); line-height:var(--lh-ui); }
.prompt-context-option-description,
.prompt-context-option-reason { color:var(--fg-dim); font-size:var(--text-meta); line-height:var(--lh-ui); }
.prompt-context-option[aria-disabled="true"] .prompt-context-option-reason { color:var(--fg-muted); }
.prompt-context-chips { display:flex; flex:1 1 auto; align-items:center; gap:4px;
  min-width:0; margin:0; padding:0; overflow-x:auto; list-style:none; scrollbar-width:none; }
.prompt-context-chips::-webkit-scrollbar { display:none; }
.prompt-context-chip { display:inline-flex; flex:none; align-items:center; gap:4px;
  max-width:220px; height:var(--control-h); padding:0 3px 0 6px;
  color:var(--fg-muted); background:var(--surface-raised);
  border:var(--border-w) solid var(--border-strong); border-radius:var(--radius-control); }
.prompt-context-chip-icon { flex:none; color:var(--fg-dim); }
/* An image attachment shows the image. A 16px codicon of a picture frame tells
   you a picture is attached; it does not tell you WHICH screenshot, which is
   the entire question when three of them are on the clipboard in a row. The
   chip grows to fit rather than the thumbnail shrinking to the control height,
   because a 20px screenshot is a grey smudge. */
.prompt-context-chip.has-thumb { height:auto; padding-left:3px; }
.prompt-context-chip-thumb { flex:none; box-sizing:border-box; width:34px; height:34px;
  margin:3px 0; object-fit:cover; background:var(--surface);
  border:var(--border-w) solid var(--border); border-radius:calc(var(--radius-control) - 1px); }
.prompt-context-chip-label { min-width:0; overflow:hidden; text-overflow:ellipsis;
  white-space:nowrap; font-size:var(--text-meta); }
.prompt-context-chip-open { display:inline-flex; flex:1 1 auto; align-items:center; gap:4px;
  min-width:0; padding:0; color:inherit; background:transparent; border:0;
  border-radius:var(--radius-control); font:inherit; cursor:pointer; }
.prompt-context-chip-open:hover .prompt-context-chip-label { color:var(--fg); }
.prompt-context-chip-open:focus-visible { outline:2px solid var(--focus); outline-offset:1px; }
.prompt-context-chip-remove { display:inline-flex; flex:none; align-items:center; justify-content:center;
  width:24px; height:24px; padding:0; color:var(--fg-dim); background:transparent;
  border:0; border-radius:var(--radius-control); cursor:pointer; }
.prompt-context-chip-remove:not(:disabled):hover { color:var(--fg); background:var(--row-hover); }
.prompt-context-chip-remove:disabled { color:var(--fg-faint); cursor:default; }
.prompt-context-summary { flex:none; color:var(--fg-dim); font-size:var(--text-meta); white-space:nowrap; }

/* Typed "@" completion. It shares the option row with the button menu above so
   the two ways of attaching context stay one visual language, and it takes the
   inline geometry of the provider-command menu that opens under the same
   caret, so a prompt surface never has two differently-anchored completions. */
.prompt-context-mention {
  position:relative; z-index:7; flex:none; box-sizing:border-box;
  margin:0 7px 4px; padding:4px; max-height:min(248px, 36vh); overflow-y:auto;
  color:var(--fg); background:var(--dropdown-bg);
  border:var(--border-w) solid var(--border-strong); border-radius:var(--radius-overlay);
  box-shadow:var(--overlay-shadow); font-family:var(--font-ui);
}
.prompt-context-mention-head { display:flex; align-items:center; justify-content:space-between;
  gap:10px; min-height:24px; padding:1px 7px 4px; color:var(--fg-dim);
  font-size:var(--text-meta); font-weight:var(--wght-strong); letter-spacing:var(--track-caps); text-transform:uppercase; }
.prompt-context-mention-hint { font-weight:var(--wght-regular); letter-spacing:0; text-transform:none; }
.prompt-context-mention .prompt-context-option { min-height:40px; }
.prompt-context-mention .prompt-context-option.is-active {
  color:var(--fg-strong); background:var(--row-selected); }
.prompt-context-mention .prompt-context-option.is-active .prompt-context-option-icon,
.prompt-context-mention .prompt-context-option.is-active .prompt-context-option-description {
  color:var(--fg-selected-secondary); }
`;

const MENU_GUTTER = 12;
const MENU_GAP = 6;
const MENU_WIDTH = 340;
const MENU_MAX_HEIGHT = 420;

export interface PromptContextMenuRect {
  top: number;
  right: number;
  bottom: number;
  left: number;
  width: number;
  height: number;
}

export interface PromptContextMenuPosition {
  top: number;
  left: number;
  width: number;
  maxHeight: number;
  side: "above" | "below";
}

/** Fixed, viewport-clamped geometry that honors the requested side until it collides. */
export function positionPromptContextMenu(
  anchor: PromptContextMenuRect,
  menu: { width: number; height: number },
  viewport: { width: number; height: number },
  preferred: "above" | "below" = "above",
): PromptContextMenuPosition {
  const availableWidth = Math.max(0, viewport.width - MENU_GUTTER * 2);
  const width = Math.min(Math.max(menu.width, MENU_WIDTH), availableWidth);
  const left = clamp(anchor.left, MENU_GUTTER, viewport.width - MENU_GUTTER - width);
  const desiredHeight = Math.min(menu.height, MENU_MAX_HEIGHT);
  const available = {
    above: Math.max(0, anchor.top - MENU_GAP - MENU_GUTTER),
    below: Math.max(0, viewport.height - anchor.bottom - MENU_GAP - MENU_GUTTER),
  };
  const alternate = preferred === "above" ? "below" : "above";
  const side = desiredHeight <= available[preferred]
    || available[preferred] >= available[alternate] ? preferred : alternate;
  const maxHeight = Math.min(desiredHeight, available[side]);
  const top = side === "below"
    ? anchor.bottom + MENU_GAP
    : Math.max(MENU_GUTTER, anchor.top - MENU_GAP - maxHeight);
  return { top, left, width, maxHeight, side };
}

export type PromptContextPickerKeyAction =
  | { type: "move"; index: number }
  | { type: "choose"; index: number }
  | { type: "close" };

/** Menu navigation keeps disabled placeholders focusable so their reason is reachable. */
export function promptContextPickerKeyAction(options: readonly PromptContextOption[],
    activeIndex: number, key: string): PromptContextPickerKeyAction | null {
  if (key === "Escape") return { type: "close" };
  if (options.length === 0) return null;
  const at = activeIndex >= 0 && activeIndex < options.length ? activeIndex : 0;
  if (key === "ArrowDown") return { type: "move", index: (at + 1) % options.length };
  if (key === "ArrowUp") {
    return { type: "move", index: (at - 1 + options.length) % options.length };
  }
  if (key === "Home") return { type: "move", index: 0 };
  if (key === "End") return { type: "move", index: options.length - 1 };
  if ((key === "Enter" || key === " ") && !options[at].disabled) {
    return { type: "choose", index: at };
  }
  return null;
}

/* ---- typed "@" completion ------------------------------------------------
   Cursor's core prompting ergonomic is typing "@" mid-draft instead of leaving
   the keyboard for the button menu. Everything below is only the TRIGGER: the
   options, their capability gating and the surface that resolves a chosen kind
   are the same ones the button menu already uses, so an engine can never be
   offered a kind through "@" that its picker row refuses. */

/** Past this many characters the draft is prose, not a reference being typed. */
export const PROMPT_CONTEXT_MENTION_MAX_QUERY = 24;

export interface PromptContextMention {
  /** Offset of the "@" that opened the menu. */
  start: number;
  /** Offset just past the typed query — the caret. */
  end: number;
  /** Text typed after the "@", verbatim. */
  query: string;
}

/**
 * The mention under the caret, or null when this draft is not one. "@" only
 * opens a reference at a word boundary, so `git@host`, an email address and a
 * decorator stay ordinary text; the query itself may not contain whitespace or
 * a second "@", and the caret may not sit inside an adjacent word — a trigger
 * that fires there would delete text the user did not type as a trigger.
 */
export function promptContextMention(draft: string, caret: number): PromptContextMention | null {
  if (!Number.isInteger(caret) || caret < 0 || caret > draft.length) return null;
  const start = draft.slice(0, caret).lastIndexOf("@");
  if (start < 0) return null;
  const query = draft.slice(start + 1, caret);
  if (query.length > PROMPT_CONTEXT_MENTION_MAX_QUERY || /[\s@]/.test(query)) return null;
  const before = start === 0 ? "" : draft[start - 1];
  if (before !== "" && !/[\s([{<"'`]/.test(before)) return null;
  if (/[A-Za-z0-9_]/.test(draft[caret] ?? "")) return null;
  return { start, end: caret, query };
}

const matchesMention = (option: PromptContextOption, query: string): boolean =>
  query === "" || option.kind.startsWith(query)
    || option.label.toLowerCase().split(/\s+/).some(word => word.startsWith(query));

/**
 * The kinds "@" may offer for this query. Only kinds the caller reports as
 * available appear at all — an inline completion listing seven rows the engine
 * cannot supply would be noise on every "@" typed in prose. A kind that is
 * available but currently blocked by the item cap stays visible and disabled,
 * carrying the same reason the button menu gives.
 */
export function promptContextMentionOptions(capabilities: PromptContextCapabilities,
    selectedCount: number, query: string,
    requestedLimit = DEFAULT_PROMPT_CONTEXT_LIMIT): PromptContextOption[] {
  const wanted = query.trim().toLowerCase();
  return promptContextOptions(capabilities, selectedCount, requestedLimit)
    .filter(option => capabilities[option.kind]?.available === true
      && matchesMention(option, wanted));
}

/**
 * A dismissal belongs to the token it dismissed. Escape must keep the menu shut
 * while the user finishes typing that word, and must be forgotten the moment
 * the caret leaves it — otherwise a later "@" landing on the same offset opens
 * nothing and reads as broken.
 */
export function retainedPromptContextDismissal(draft: string, caret: number | null,
    dismissed: number | null): number | null {
  if (dismissed === null || caret === null) return null;
  const mention = promptContextMention(draft, caret);
  return mention !== null && mention.start === dismissed ? dismissed : null;
}

export interface PromptContextMentionEdit {
  prompt: string;
  caret: number;
}

/** Remove exactly the typed trigger — never a character on either side of it. */
export function applyPromptContextMention(draft: string,
    mention: PromptContextMention): PromptContextMentionEdit {
  return {
    prompt: draft.slice(0, mention.start) + draft.slice(mention.end),
    caret: mention.start,
  };
}

export function promptContextMentionOptionId(menuId: string, index: number): string {
  return `${menuId}-option-${index}`;
}

/* ---- the controller every prompt surface wires ---------------------------
   THIS IS THE WHOLE TRIGGER, and it is deliberately not a component: the
   launch card and the per-session composer are different components with
   different layouts, and the only way they can be guaranteed to agree about
   what "@" does is to share the decisions rather than the markup. Everything
   below is pure — state in, state or an action out — so each rule is a fact a
   test can pin, and a surface that wires it cannot quietly get a different
   answer from the one this file was tested for.

   A surface owns exactly one `PromptContextMentionState` and calls these; see
   the wiring notes on `promptContextMentionKeyAction`. */

export interface PromptContextMentionState {
  /** Caret offset the trigger is following, or null when it follows none. */
  caret: number | null;
  /** Offset of the "@" Escape dismissed, or null. Belongs to that token only. */
  dismissed: number | null;
  /** Highlighted row, clamped to the live option list when it is read. */
  index: number;
}

export const IDLE_PROMPT_CONTEXT_MENTION_STATE: PromptContextMentionState =
  { caret: null, dismissed: null, index: 0 };

/** The parts of a text field the trigger reads. `HTMLTextAreaElement` is one. */
export interface PromptContextSelection {
  value: string;
  selectionStart: number | null;
  selectionEnd: number | null;
}

/**
 * The caret the trigger should follow, or null when there is not one.
 *
 * A RANGE selection is not a caret. `selectionStart` is the selection ANCHOR,
 * so treating a range as a caret opened the menu against text the user had just
 * selected — drag-select " and more" in `use @f and more` and the anchor lands
 * one character past an "@" — and the Enter that was meant to replace the
 * selection was then swallowed by the menu, which deleted the token instead.
 *
 * Following the caret only while the draft holds an "@" at all is what keeps
 * ordinary prose from re-rendering the surface on every arrow key.
 */
export function promptContextCaret(field: PromptContextSelection): number | null {
  const { selectionStart, selectionEnd } = field;
  if (selectionStart === null || selectionEnd === null) return null;
  if (selectionStart !== selectionEnd) return null;
  return field.value.includes("@") ? selectionStart : null;
}

/**
 * Next state after the field's selection moved. `retarget` is the typing case:
 * editing the query re-points at the top row. The previous state object is
 * returned unchanged when nothing moved, so a `setState(fn)` caller does not
 * re-render on every keystroke in prose.
 */
export function trackedPromptContextMention(state: PromptContextMentionState,
    field: PromptContextSelection, retarget = false): PromptContextMentionState {
  const caret = promptContextCaret(field);
  const dismissed = retainedPromptContextDismissal(field.value, caret, state.dismissed);
  const index = retarget ? 0 : state.index;
  return state.caret === caret && state.dismissed === dismissed && state.index === index
    ? state : { caret, dismissed, index };
}

/**
 * The field lost the caret. The menu closes without remembering a dismissal, so
 * returning to the draft and typing on picks it back up.
 */
export function releasedPromptContextMention(
    state: PromptContextMentionState): PromptContextMentionState {
  return state.caret === null && state.dismissed === null
    ? state : { ...state, caret: null, dismissed: null };
}

export function dismissedPromptContextMention(state: PromptContextMentionState,
    start: number): PromptContextMentionState {
  return state.dismissed === start ? state : { ...state, dismissed: start };
}

export function movedPromptContextMention(state: PromptContextMentionState,
    index: number): PromptContextMentionState {
  return state.index === index ? state : { ...state, index };
}

export interface PromptContextMentionInput {
  draft: string;
  state: PromptContextMentionState;
  capabilities: PromptContextCapabilities;
  selectedCount: number;
  /** False where a chosen kind has no surface that could resolve it. */
  enabled: boolean;
  /** The slash menu owns this caret: only one completion is ever open. */
  suppressed?: boolean;
  limit?: number;
}

export interface PromptContextMentionView {
  /** The trigger can finish here at all: this engine offers at least one kind. */
  capable: boolean;
  mention: PromptContextMention | null;
  options: readonly PromptContextOption[];
  open: boolean;
  activeIndex: number;
}

const NO_MENTION_OPTIONS: readonly PromptContextOption[] = [];

/** Everything the surface renders, derived from its state and the live draft. */
export function promptContextMentionView(
    input: PromptContextMentionInput): PromptContextMentionView {
  const capable = input.enabled
    && promptContextMentionOptions(input.capabilities, 0, "", input.limit).length > 0;
  const mention = capable && input.state.caret !== null
    ? promptContextMention(input.draft, input.state.caret) : null;
  const options = mention === null ? NO_MENTION_OPTIONS
    : promptContextMentionOptions(input.capabilities, input.selectedCount,
      mention.query, input.limit);
  const open = mention !== null && options.length > 0
    && mention.start !== input.state.dismissed && input.suppressed !== true;
  const activeIndex = options.length === 0
    ? 0 : Math.min(Math.max(input.state.index, 0), options.length - 1);
  return { capable, mention, options, open, activeIndex };
}

export interface PromptContextMentionChoice {
  option: PromptContextOption;
  edit: PromptContextMentionEdit;
}

/**
 * The edit a chosen row produces, or null when that row cannot be taken. One
 * decision for the pointer and the keyboard, so a click on a capped row and an
 * Enter on it do the same nothing — and neither removes the typed "@".
 */
export function promptContextMentionChoice(view: PromptContextMentionView,
    option: PromptContextOption | undefined, draft: string): PromptContextMentionChoice | null {
  if (view.mention === null || option === undefined || option.disabled === true) return null;
  return { option, edit: applyPromptContextMention(draft, view.mention) };
}

export type PromptContextMentionKeyAction =
  | { type: "move"; index: number }
  | { type: "dismiss"; start: number }
  | { type: "choose"; option: PromptContextOption; edit: PromptContextMentionEdit };

/**
 * What this key does to the menu, or NULL when the key is not the menu's and
 * the surface must handle it exactly as it would with no menu open.
 *
 * The null is as load-bearing as the actions. At the item cap every offered row
 * is disabled, and swallowing Enter and Tab there left the prompt with no way
 * to submit and no way to tab out, while the menu's own hint still said "Enter
 * attach". A literal "@" is valid prompt text, so the submit it was blocking is
 * a legitimate action. Escape is never swallowed in the other direction: it
 * always closes, capped or not.
 *
 * WIRING (both prompt surfaces do exactly this):
 *   const [mention, setMention] = useState(IDLE_PROMPT_CONTEXT_MENTION_STATE);
 *   const view = promptContextMentionView({ draft, state: mention, capabilities,
 *     selectedCount, enabled, suppressed: commandMenuOpen });
 *   onChange  → setMention(s => trackedPromptContextMention(s, field, true))
 *   onSelect  → setMention(s => trackedPromptContextMention(s, field))
 *   onBlur    → setMention(releasedPromptContextMention)
 *   onKeyDown → const key = promptContextMentionKeyAction(view, draft, event);
 *               if (key !== null) { preventDefault(); stopPropagation(); … }
 *               …and when it IS null, fall through untouched.
 */
export function promptContextMentionKeyAction(view: PromptContextMentionView,
    draft: string, event: ProviderCommandKeyEvent): PromptContextMentionKeyAction | null {
  if (!view.open || view.mention === null) return null;
  // The provider-command menu's mapper: arrows move, Enter/Tab take the row,
  // Escape closes, modified keys and IME composition stay text editing. Both
  // completions under this caret answer to one key contract.
  const action = providerCommandMenuKeyAction(event, view.options.length, view.activeIndex);
  if (action === null) return null;
  if (action.type === "close") return { type: "dismiss", start: view.mention.start };
  if (action.type === "move") return { type: "move", index: action.index };
  const choice = promptContextMentionChoice(view, view.options[action.index], draft);
  return choice === null ? null : { type: "choose", ...choice };
}

/**
 * The head's hint, which has to be true in the state it is shown in. With every
 * row capped, "Enter attach" was a falsehood printed directly above four rows
 * that each said they could not be attached.
 */
export function promptContextMentionHint(
    options: readonly PromptContextOption[]): string {
  return options.some(option => option.disabled !== true)
    ? "↑↓ choose · Enter attach" : "Esc dismiss";
}

export interface PromptContextMentionMenuProps {
  id: string;
  options: readonly PromptContextOption[];
  activeIndex: number;
  onChoose: (option: PromptContextOption) => void;
  onActiveIndexChange: (index: number) => void;
}

/**
 * Presentational listbox for the "@" trigger. Focus stays in the textarea that
 * owns it — that is the whole point of typing the reference — so this is a
 * `listbox` addressed by aria-activedescendant, not the button menu's `menu`.
 */
export function PromptContextMentionMenu(props: PromptContextMentionMenuProps) {
  const options = useRef(new Map<number, HTMLButtonElement>());
  useEffect(() => {
    options.current.get(props.activeIndex)?.scrollIntoView({ block: "nearest" });
  }, [props.activeIndex, props.options]);
  return (
    <div id={props.id} className="prompt-context-mention is-floating" role="listbox"
      aria-label="Add prompt context">
      <style>{PROMPT_CONTEXT_PICKER_CSS}</style>
      <div className="prompt-context-mention-head" role="presentation" aria-hidden="true">
        <span>Add context</span>
        <span className="prompt-context-mention-hint"
        >{promptContextMentionHint(props.options)}</span>
      </div>
      {props.options.map((option, index) => (
        <button key={option.kind} id={promptContextMentionOptionId(props.id, index)}
          type="button" tabIndex={-1}
          ref={node => {
            if (node === null) options.current.delete(index);
            else options.current.set(index, node);
          }}
          role="option" aria-selected={index === props.activeIndex}
          aria-disabled={option.disabled || undefined}
          className={index === props.activeIndex
            ? "prompt-context-option is-active" : "prompt-context-option"}
          onMouseEnter={() => { props.onActiveIndexChange(index); }}
          // The draft keeps focus and the caret keeps its offset: a mouse pick
          // must land in the same place a keyboard pick does.
          onMouseDown={event => { event.preventDefault(); }}
          onClick={() => { props.onChoose(option); }}>
          <span className={`codicon codicon-${KIND_ICON[option.kind]} prompt-context-option-icon`}
            aria-hidden="true" />
          <span className="prompt-context-option-copy">
            <span className="prompt-context-option-label">{option.label}</span>
            <span className="prompt-context-option-description">
              {option.disabledReason ?? option.description}
            </span>
          </span>
        </button>
      ))}
    </div>
  );
}

export interface PromptContextChipsProps {
  items: readonly PromptContextItem[];
  onRemove: (id: string) => void;
  onPreview?: (item: PromptContextItem) => void;
  maxItems?: number;
  /** False when PromptContextPicker already injected the shared component CSS. */
  injectStyles?: boolean;
  disabled?: boolean;
}

const isPreviewableContextItem = (item: PromptContextItem): boolean =>
  item.kind === "file" || item.kind === "folder"
    || (item.kind === "diff" && item.safeContent !== undefined);

export function PromptContextChips({ items, onRemove, onPreview,
    maxItems = DEFAULT_PROMPT_CONTEXT_LIMIT,
    injectStyles = true, disabled = false }: PromptContextChipsProps) {
  const normalized = normalizePromptContextItems(items, maxItems);
  if (normalized.length === 0) return null;
  return (
    <>
    {injectStyles && <style>{PROMPT_CONTEXT_PICKER_CSS}</style>}
    <ul className="prompt-context-chips" aria-label="Attached prompt context">
      {normalized.map(item => {
        const kind = promptContextKindLabel(item.kind);
        const detail = item.detail ?? item.value;
        const estimate = item.estimatedTokens === undefined
          ? "" : `, approximately ${item.estimatedTokens} tokens`;
        const canPreview = onPreview !== undefined && isPreviewableContextItem(item);
        const body = <>
          {item.previewUrl === undefined
            ? <span className={`codicon codicon-sm codicon-${KIND_ICON[item.kind]} prompt-context-chip-icon`}
                aria-hidden="true" />
            // Named, not decorative. "image" would be true of every one of
            // them; the filename is the only thing that says which.
            : <img className="prompt-context-chip-thumb" src={item.previewUrl}
                alt={`Preview of ${item.label}`} />}
          <span className="prompt-context-chip-label">{item.label}</span>
        </>;
        return (
          <li key={item.id}
            className={item.previewUrl === undefined
              ? "prompt-context-chip" : "prompt-context-chip has-thumb"}
            title={`${kind}: ${detail}${estimate}`}>
            {canPreview
              ? <button type="button" className="prompt-context-chip-open"
                  aria-label={`Preview ${kind} context ${item.label}`}
                  onClick={() => onPreview(item)}>
                  {body}
                </button>
              : body}
            <button type="button" className="prompt-context-chip-remove"
              disabled={disabled}
              aria-label={`Remove ${kind} context ${item.label}`}
              onClick={() => onRemove(item.id)}>
              <span className="codicon codicon-sm codicon-close" aria-hidden="true" />
            </button>
          </li>
        );
      })}
    </ul>
    </>
  );
}

export interface PromptContextPickerProps {
  open: boolean;
  items: readonly PromptContextItem[];
  capabilities: PromptContextCapabilities;
  onOpenChange: (open: boolean) => void;
  /** Request the caller-owned picker/editor for this context kind. */
  onRequest: (kind: PromptContextKind) => void;
  onRemove: (id: string) => void;
  onPreview?: (item: PromptContextItem) => void;
  maxItems?: number;
  placement?: "above" | "below";
  showChips?: boolean;
  disabled?: boolean;
  /** Tooltip for the trigger. Callers that also accept a typed "@" say so here. */
  triggerHint?: string;
}

export function PromptContextPicker({ open, items, capabilities, onOpenChange, onRequest,
    onRemove, onPreview, maxItems = DEFAULT_PROMPT_CONTEXT_LIMIT, placement = "above",
    showChips = true, disabled = false, triggerHint }: PromptContextPickerProps) {
  const uid = useId();
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const optionRefs = useRef<Array<HTMLButtonElement | null>>([]);
  const [activeIndex, setActiveIndex] = useState(0);
  const [position, setPosition] = useState<PromptContextMenuPosition | null>(null);
  const limit = promptContextLimit(maxItems);
  const selected = normalizePromptContextItems(items, limit);
  const options = promptContextOptions(capabilities, selected.length, limit);
  const optionsLayoutKey = options.map(option =>
    `${option.kind}:${option.disabledReason ?? ""}:${option.description}`).join("\u0000");
  const menuId = `${uid}-menu`;
  const tokens = promptContextTokenEstimate(selected, limit);

  useEffect(() => {
    if (!open || position === null) return;
    setActiveIndex(0);
    optionRefs.current[0]?.focus();
  }, [open, position]);

  useEffect(() => {
    if (!open) setPosition(null);
  }, [open]);

  useLayoutEffect(() => {
    if (!open) return;
    const trigger = triggerRef.current;
    const menu = menuRef.current;
    if (trigger === null || menu === null) return;
    setPosition(positionPromptContextMenu(
      trigger.getBoundingClientRect(),
      { width: menu.offsetWidth, height: menu.scrollHeight },
      { width: window.innerWidth, height: window.innerHeight },
      placement,
    ));
  }, [open, optionsLayoutKey, placement]);

  const close = useCallback((restoreFocus: boolean): void => {
    onOpenChange(false);
    if (restoreFocus) queueMicrotask(() => triggerRef.current?.focus());
  }, [onOpenChange]);

  const choose = (option: PromptContextOption): void => {
    if (option.disabled) return;
    // The requested surface may be another modal (Quick Open or the URL
    // dialog). Hand focus back synchronously before mounting it; a deferred
    // trigger restore after onRequest would steal focus out of that aria-modal.
    triggerRef.current?.focus();
    close(false);
    onRequest(option.kind);
  };

  const moveFocusPastTrigger = (backwards: boolean): void => {
    const trigger = triggerRef.current;
    const target = focusPastTrigger(trigger, backwards, menuRef.current);
    close(false);
    if (trigger !== null) queueMicrotask(() => { (target ?? trigger).focus(); });
  };

  const handleOptionKey = (event: KeyboardEvent<HTMLButtonElement>, index: number): void => {
    if (event.key === "Tab") {
      event.preventDefault();
      event.stopPropagation();
      moveFocusPastTrigger(event.shiftKey);
      return;
    }
    const action = promptContextPickerKeyAction(options, index, event.key);
    if (action === null) return;
    event.preventDefault();
    event.stopPropagation();
    if (action.type === "close") close(true);
    else if (action.type === "choose") choose(options[action.index]);
    else {
      setActiveIndex(action.index);
      optionRefs.current[action.index]?.focus();
    }
  };

  useEffect(() => {
    if (!open) return;
    const outside = (event: PointerEvent): void => {
      const path = event.composedPath();
      if (rootRef.current !== null && path.includes(rootRef.current)) return;
      if (menuRef.current !== null && path.includes(menuRef.current)) return;
      close(false);
    };
    const viewportChanged = (event: Event): void => {
      const target = event.target;
      if (event.type === "scroll" && target instanceof Node
          && menuRef.current?.contains(target) === true) return;
      if (event.type === "scroll" && !scrollMovesAnchor(target, rootRef.current)) return;
      close(true);
    };
    document.addEventListener("pointerdown", outside, true);
    window.addEventListener("resize", viewportChanged);
    window.addEventListener("scroll", viewportChanged, true);
    window.visualViewport?.addEventListener("resize", viewportChanged);
    window.visualViewport?.addEventListener("scroll", viewportChanged);
    return () => {
      document.removeEventListener("pointerdown", outside, true);
      window.removeEventListener("resize", viewportChanged);
      window.removeEventListener("scroll", viewportChanged, true);
      window.visualViewport?.removeEventListener("resize", viewportChanged);
      window.visualViewport?.removeEventListener("scroll", viewportChanged);
    };
  }, [close, open]);

  useEffect(() => {
    if (open && disabled) close(true);
  }, [close, disabled, open]);

  const menuStyle: CSSProperties = position === null
    ? { top: 0, left: 0, width: MENU_WIDTH, maxHeight: MENU_MAX_HEIGHT,
        visibility: "hidden" }
    : { top: position.top, left: position.left, width: position.width,
        maxHeight: position.maxHeight };

  const menu = open ? (
    <div ref={menuRef} id={menuId} className="prompt-context-menu is-floating"
      style={menuStyle} data-side={position?.side}
      role="menu" aria-label="Add prompt context">
      {options.map((option, index) => {
        const descriptionId = `${uid}-${option.kind}-description`;
        const reasonId = `${uid}-${option.kind}-reason`;
        return (
          <button key={option.kind} ref={node => { optionRefs.current[index] = node; }}
            type="button" role="menuitem"
            className="prompt-context-option"
            tabIndex={index === activeIndex ? 0 : -1}
            aria-disabled={option.disabled || undefined}
            aria-describedby={option.disabled ? `${descriptionId} ${reasonId}` : descriptionId}
            onFocus={() => setActiveIndex(index)}
            onKeyDown={event => handleOptionKey(event, index)}
            onClick={() => choose(option)}>
            <span className={`codicon codicon-${KIND_ICON[option.kind]} prompt-context-option-icon`}
              aria-hidden="true" />
            <span className="prompt-context-option-copy">
              <span className="prompt-context-option-label">{option.label}</span>
              <span id={descriptionId} className="prompt-context-option-description">
                {option.description}
              </span>
              {option.disabledReason !== undefined && (
                <span id={reasonId} className="prompt-context-option-reason">
                  {option.disabledReason}
                </span>
              )}
            </span>
          </button>
        );
      })}
    </div>
  ) : null;

  const popup = menu === null ? null : typeof document === "undefined"
    ? menu : createPortal(menu, document.body);

  return (
    <div ref={rootRef} className="prompt-context"
      onBlur={event => {
        const next = event.relatedTarget;
        if (!open) return;
        const inside = next instanceof Node
          && (rootRef.current?.contains(next) === true || menuRef.current?.contains(next) === true);
        if (!inside) close(false);
      }}>
      <style>{PROMPT_CONTEXT_PICKER_CSS}</style>
      <button ref={triggerRef} type="button" className="prompt-context-trigger"
        disabled={disabled}
        title={triggerHint}
        aria-label={`Add prompt context, ${selected.length} of ${limit} selected`}
        aria-haspopup="menu" aria-expanded={open} aria-controls={open ? menuId : undefined}
        onClick={() => onOpenChange(!open)}>
        <span className="codicon codicon-add" aria-hidden="true" />
      </button>
      {showChips && <PromptContextChips items={selected} onRemove={onRemove}
        onPreview={onPreview} maxItems={limit} injectStyles={false} disabled={disabled} />}
      {selected.length > 0 && (
        <span className="prompt-context-summary" aria-label={tokens > 0
          ? `${selected.length} context items, approximately ${tokens} tokens`
          : `${selected.length} context items, token estimate unavailable`}>
          {selected.length}/{limit}{tokens > 0 ? ` · ~${tokens.toLocaleString()} tok` : ""}
        </span>
      )}
      {popup}
    </div>
  );
}
