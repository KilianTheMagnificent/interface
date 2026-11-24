import {
  Fragment, useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState,
} from "react";
import type {
  CSSProperties, KeyboardEvent as ReactKeyboardEvent, ReactNode,
} from "react";
import { createPortal } from "react-dom";
import { clamp } from "../helpers";
import { focusPastTrigger } from "./ModalBehavior";
import { PROVIDER_LOGO_CSS } from "./ProviderLogo";
import { scrollMovesAnchor } from "./scroll-anchor";

/** One provider-owned value. Values should be unique within a picker. */
export interface AppSelectOption {
  value: string;
  label: string;
  /** Optional non-interactive heading; consecutive equal groups share one heading. */
  group?: string;
  /** Copy used only by the closed trigger when a menu row needs a shorter label. */
  triggerLabel?: string;
  description?: string;
  meta?: string;
  disabled?: boolean;
  /** Opens this picker's text-entry surface instead of selecting `value`. */
  custom?: boolean;
  customLabel?: string;
  customPlaceholder?: string;
}

export interface AppSelectProps {
  /** Accessible name for both the trigger and its listbox. */
  label: string;
  value: string;
  options: readonly AppSelectOption[];
  onChange: (value: string) => void;
  /** Small visible key before the selected value, such as "model". */
  prefix?: string;
  /** Decorative visual before the selected value; the text remains the accessible name. */
  leading?: ReactNode;
  /** Decorative visual shown once beside each provider/group heading. */
  renderGroupLeading?: (option: AppSelectOption) => ReactNode;
  placeholder?: string;
  disabled?: boolean;
  pending?: boolean;
  compact?: boolean;
  className?: string;
  /** Added to the portalled root so a surface can scope its menu density safely. */
  popoverClassName?: string;
  /** Opt-in visible filtering for long catalogues. Compact pickers remain unchanged. */
  searchable?: boolean;
  searchPlaceholder?: string;
  /** Adds an in-app text-entry surface after the advertised options. */
  allowCustom?: boolean;
  customLabel?: string;
  customPlaceholder?: string;
  /** Distinct from advertised selection so arbitrary text can never impersonate an option key. */
  onCustom?: (value: string, option?: AppSelectOption) => void;
  onOpenChange?: (open: boolean) => void;
  /**
   * Opt-in extra selection. With it, Shift-click (or Shift+Enter) on a row
   * other than `value` calls this instead of choosing, and the menu stays
   * open; `checkedValues` are drawn checked beside the chosen row. Absent, a
   * Shift-click chooses exactly as a plain click does.
   */
  onShiftChoose?: (value: string) => void;
  checkedValues?: ReadonlySet<string>;
  /**
   * A quiet note after the selected value, inside the trigger (a provider's
   * plan headroom, "38% left"). Said in the accessible name too; `trailingTitle`
   * is its longer form for the tooltip.
   */
  trailing?: string;
  trailingTitle?: string;
}

export type AppSelectMove = "next" | "previous" | "first" | "last";

export type AppSelectKeyAction =
  | { type: "open"; index: number }
  | { type: "move"; index: number }
  | { type: "choose"; index: number }
  | { type: "close" };

const VIEWPORT_GUTTER = 8;
const POPOVER_GAP = 4;
const MIN_MENU_WIDTH = 220;
const MAX_MENU_HEIGHT = 320;
const TYPEAHEAD_WINDOW_MS = 650;

/**
 * App-owned navigation surfaces use this to dismiss every portalled picker
 * before the surface underneath becomes inert. A custom event keeps the
 * reusable control controlled internally without teaching App about each
 * individual model, effort, access, or appearance menu.
 */
export const APP_SELECT_DISMISS_EVENT = "interface:app-select-dismiss";

function enabledIndices(options: readonly AppSelectOption[], allowCustom: boolean): number[] {
  const out: number[] = [];
  for (let index = 0; index < options.length; index += 1) {
    if (options[index].disabled !== true) out.push(index);
  }
  if (allowCustom) out.push(options.length);
  return out;
}

/** The selected value is the initial row when selectable; otherwise use the first choice. */
export function initialAppSelectIndex(
  options: readonly AppSelectOption[],
  value: string,
  allowCustom = false,
): number {
  const selected = options.findIndex(option => option.value === value && option.disabled !== true);
  if (selected >= 0) return selected;
  return enabledIndices(options, allowCustom)[0] ?? -1;
}

/** Roving listbox navigation. Disabled rows are never active and movement wraps. */
export function moveAppSelectIndex(
  options: readonly AppSelectOption[],
  current: number,
  move: AppSelectMove,
  allowCustom = false,
): number {
  const enabled = enabledIndices(options, allowCustom);
  if (enabled.length === 0) return -1;
  if (move === "first") return enabled[0];
  if (move === "last") return enabled[enabled.length - 1];
  const at = enabled.indexOf(current);
  if (at < 0) return move === "next" ? enabled[0] : enabled[enabled.length - 1];
  const delta = move === "next" ? 1 : -1;
  return enabled[(at + delta + enabled.length) % enabled.length];
}

/** Keyboard semantics kept pure so the no-DOM test suite can pin them. */
export function appSelectKeyAction(
  options: readonly AppSelectOption[],
  activeIndex: number,
  key: string,
  open: boolean,
  allowCustom = false,
): AppSelectKeyAction | null {
  if (!open) {
    if (key === "Escape") return null;
    if (key === "Home") {
      return { type: "open", index: moveAppSelectIndex(options, activeIndex, "first", allowCustom) };
    }
    if (key === "End") {
      return { type: "open", index: moveAppSelectIndex(options, activeIndex, "last", allowCustom) };
    }
    if (key === "Enter" || key === " " || key === "ArrowDown" || key === "ArrowUp") {
      const fallback = key === "ArrowUp" ? "last" : "first";
      const enabled = enabledIndices(options, allowCustom);
      const index = enabled.includes(activeIndex)
        ? activeIndex
        : moveAppSelectIndex(options, activeIndex, fallback, allowCustom);
      return { type: "open", index };
    }
    return null;
  }

  if (key === "Escape") return { type: "close" };
  if (key === "ArrowDown") {
    return { type: "move", index: moveAppSelectIndex(options, activeIndex, "next", allowCustom) };
  }
  if (key === "ArrowUp") {
    return { type: "move", index: moveAppSelectIndex(options, activeIndex, "previous", allowCustom) };
  }
  if (key === "Home") {
    return { type: "move", index: moveAppSelectIndex(options, activeIndex, "first", allowCustom) };
  }
  if (key === "End") {
    return { type: "move", index: moveAppSelectIndex(options, activeIndex, "last", allowCustom) };
  }
  if (key === "Enter" || key === " ") {
    return enabledIndices(options, allowCustom).includes(activeIndex)
      ? { type: "choose", index: activeIndex }
      : null;
  }
  return null;
}

/** Literal prefix typeahead over every useful piece of option copy. */
export function appSelectTypeaheadIndex(
  options: readonly AppSelectOption[],
  query: string,
  activeIndex: number,
): number {
  const needle = query.trim().toLocaleLowerCase();
  if (needle === "") return -1;
  const candidates = options
    .map((option, index) => ({ option, index }))
    .filter(entry => entry.option.disabled !== true);
  if (candidates.length === 0) return -1;
  const after = candidates.findIndex(entry => entry.index === activeIndex);
  for (let offset = 1; offset <= candidates.length; offset += 1) {
    const entry = candidates[(Math.max(after, -1) + offset) % candidates.length];
    const { option } = entry;
    const fields = [option.label, option.triggerLabel, option.value, option.group,
      option.description, option.meta]
      .filter((field): field is string => typeof field === "string")
      .map(field => field.toLocaleLowerCase());
    if (fields.some(field => field.startsWith(needle))) return entry.index;
  }
  return -1;
}

/** Keep the same logical row active when a live provider catalogue inserts or
 * removes rows. A numeric index alone can silently move to another provider. */
export function reconcileAppSelectIndex(
  previous: readonly AppSelectOption[],
  next: readonly AppSelectOption[],
  previousIndex: number,
  selectedValue: string,
  allowCustom = false,
): number {
  if (allowCustom && previousIndex === previous.length) return next.length;
  const active = previous[previousIndex];
  if (active !== undefined) {
    const same = next.findIndex(option => option.value === active.value
      && option.disabled !== true);
    if (same >= 0) return same;
  }
  return initialAppSelectIndex(next, selectedValue, allowCustom);
}

export function appSelectOptionAccessibleName(
  option: AppSelectOption,
): string {
  return [option.label, option.group, option.meta]
    .filter((part): part is string => part !== undefined && part !== "")
    .join(", ");
}

/**
 * Provider model IDs are useful routing/search keys, but displaying one beside
 * a friendly, versioned model name repeats the same decision in protocol form.
 * Keep semantic badges such as "Current" and "Confirm" visible while leaving
 * machine IDs in `value`/`meta` for exact selection and search. Reveal the ID
 * only when two friendly names collide inside the same provider group.
 */
export function appSelectOptionVisibleMeta(option: AppSelectOption,
    options: readonly AppSelectOption[] = []): string | undefined {
  return visibleMetaAmong(option, appSelectNameValues(options));
}

/** `appSelectOptionVisibleMeta` for every option, indexing the names once. */
export function appSelectOptionVisibleMetas(
  options: readonly AppSelectOption[],
): (string | undefined)[] {
  const names = appSelectNameValues(options);
  return options.map(option => visibleMetaAmong(option, names));
}

/** The friendly name two options collide on: trimmed, case-folded label and group. */
function appSelectNameKey(option: AppSelectOption): string {
  return JSON.stringify([option.label.trim().toLocaleLowerCase(),
    option.group?.trim().toLocaleLowerCase() ?? ""]);
}

/** Each friendly name, with the distinct values listed under it. */
function appSelectNameValues(
  options: readonly AppSelectOption[],
): ReadonlyMap<string, ReadonlySet<string>> {
  const names = new Map<string, Set<string>>();
  for (const option of options) {
    const key = appSelectNameKey(option);
    const values = names.get(key);
    if (values === undefined) names.set(key, new Set([option.value]));
    else values.add(option.value);
  }
  return names;
}

function visibleMetaAmong(option: AppSelectOption,
    names: ReadonlyMap<string, ReadonlySet<string>>): string | undefined {
  const meta = option.meta?.trim();
  if (meta === undefined || meta === "") return undefined;
  const quotedMeta = JSON.stringify(meta);
  const technical = meta === option.value || option.value.includes(quotedMeta);
  const labelKey = option.label.trim().toLocaleLowerCase();
  // Another option with a different value shares this friendly name.
  const values = names.get(appSelectNameKey(option));
  const needsDisambiguation = values !== undefined
    && values.size > (values.has(option.value) ? 1 : 0);
  if (technical && !needsDisambiguation) return undefined;
  if (meta.toLocaleLowerCase() === labelKey) return undefined;
  return meta;
}

/**
 * Do not spend a second row repeating copy the friendly label already says.
 * `visibleMeta` is `appSelectOptionVisibleMeta`'s answer for this option.
 */
export function appSelectOptionSupportingText(option: AppSelectOption,
    visibleMeta: string | undefined): string | undefined {
  const description = option.description?.trim();
  if (description === undefined || description === "") return undefined;
  const normalized = (value: string): string => value.toLocaleLowerCase()
    .replace(/[^\p{L}\p{N}]+/gu, " ").trim();
  const descriptionKey = normalized(description);
  const duplicates = [option.label, option.triggerLabel, visibleMeta]
    .filter((part): part is string => part !== undefined && part !== "")
    .some(part => normalized(part) === descriptionKey);
  return duplicates ? undefined : description;
}

/** Search every piece of user-facing provider/model copy without fuzzy surprises. */
export function appSelectOptionMatches(option: AppSelectOption, query: string): boolean {
  const needle = query.trim().toLocaleLowerCase();
  if (needle === "") return true;
  return [option.label, option.triggerLabel, option.value, option.group,
    option.description, option.meta]
    .filter((field): field is string => typeof field === "string")
    .some(field => field.toLocaleLowerCase().includes(needle));
}

export function appSelectPopoverClasses(className?: string): string {
  return ["app-select-popover is-floating", className ?? ""].filter(Boolean).join(" ");
}

export interface AppSelectRect {
  top: number;
  right: number;
  bottom: number;
  left: number;
  width: number;
  height: number;
}

export interface AppSelectSize { width: number; height: number }
export interface AppSelectViewport { width: number; height: number }
export interface AppSelectPosition {
  top: number;
  left: number;
  width: number;
  maxHeight: number;
  side: "above" | "below";
}

/**
 * Fixed-position geometry: prefer below, flip when needed, then clamp to the viewport.
 * `end` lines the menu's right edge up with the trigger's, for toolbar menus.
 */
export function positionAppSelect(
  anchor: AppSelectRect,
  menu: AppSelectSize,
  viewport: AppSelectViewport,
  heightLimit = MAX_MENU_HEIGHT,
  align: "start" | "end" = "start",
): AppSelectPosition {
  const availableWidth = Math.max(0, viewport.width - VIEWPORT_GUTTER * 2);
  const width = Math.min(
    Math.max(anchor.width, menu.width, MIN_MENU_WIDTH),
    availableWidth,
  );
  const left = clamp(align === "end" ? anchor.right - width : anchor.left,
    VIEWPORT_GUTTER, viewport.width - VIEWPORT_GUTTER - width);
  const desiredHeight = Math.min(menu.height, heightLimit);
  const below = Math.max(0, viewport.height - anchor.bottom - POPOVER_GAP - VIEWPORT_GUTTER);
  const above = Math.max(0, anchor.top - POPOVER_GAP - VIEWPORT_GUTTER);
  const side: AppSelectPosition["side"] = desiredHeight <= below || below >= above
    ? "below" : "above";
  const availableHeight = side === "below" ? below : above;
  const maxHeight = Math.min(desiredHeight, availableHeight);
  const top = side === "below"
    ? anchor.bottom + POPOVER_GAP
    : Math.max(VIEWPORT_GUTTER, anchor.top - POPOVER_GAP - maxHeight);
  return { top, left, width, maxHeight, side };
}

export const APP_SELECT_CSS = `
${PROVIDER_LOGO_CSS}
.app-select { display:inline-flex; min-width:0; }
.app-select-trigger { display:inline-flex; align-items:center; gap:4px; box-sizing:border-box;
  max-width:100%; height:var(--control-h); padding:0 7px; color:var(--fg-muted);
  background:transparent; border:var(--border-w) solid transparent;
  border-radius:var(--radius-control); font:inherit; font-size:var(--text-ui); cursor:pointer; }
.app-select-compact .app-select-trigger { height:24px; padding:0 6px; font-size:var(--text-meta); }
.app-select-trigger:hover:not(:disabled), .app-select-trigger[aria-expanded="true"] {
  color:var(--fg); background:var(--row-hover); }
.app-select-trigger:disabled { color:var(--fg-faint); cursor:default; }
.app-select-prefix { flex:none; color:var(--fg-muted); }
.app-select-leading { display:inline-flex; flex:none; align-items:center; justify-content:center;
  width:16px; height:16px; }
.app-select-value { min-width:0; overflow:hidden; color:var(--fg); text-overflow:ellipsis;
  white-space:nowrap; }
.app-select-placeholder { color:var(--fg-dim); }
.app-select-chevron { flex:none; color:var(--fg-dim); }
.app-select-pending .app-select-value { color:var(--fg-muted); font-style:italic; }
.app-select-trailing { flex:none; color:var(--fg-dim); font-size:var(--text-meta);
  font-variant-numeric:tabular-nums; white-space:nowrap; }
.app-select-popover { position:fixed; z-index:100; display:flex; flex-direction:column;
  width:max-content; min-width:${MIN_MENU_WIDTH}px; max-width:min(460px, calc(100vw - 16px));
  box-sizing:border-box; overflow:hidden; color:var(--fg); background:var(--dropdown-bg);
  border:var(--border-w) solid var(--border-strong); border-radius:var(--radius-overlay);
  box-shadow:var(--overlay-shadow); font-family:var(--font-ui); }
.app-select-search-wrap { flex:none; padding:7px 8px; border-bottom:var(--border-w) solid var(--border); }
.app-select-search { box-sizing:border-box; width:100%; height:28px; padding:0 8px;
  color:var(--input-fg); background:var(--input-bg); border:var(--border-w) solid var(--input-border);
  border-radius:var(--radius-control); font-family:var(--font-ui); font-size:var(--text-ui); }
.app-select-search::placeholder { color:var(--input-placeholder); }
.app-select-results-status { position:absolute; width:1px; height:1px; padding:0; overflow:hidden;
  clip:rect(0, 0, 0, 0); white-space:nowrap; border:0; }
.app-select-no-results { margin:0; padding:12px 8px; color:var(--fg-dim); font-size:var(--text-ui); }
.app-select-options { min-height:0; margin:0; padding:4px; overflow-y:auto; outline:none; }
.app-select-group { display:grid; grid-template-columns:var(--icon-size-sm) minmax(0, 1fr);
  column-gap:8px; align-items:center; padding:7px 8px 3px; color:var(--fg-dim);
  font-size:var(--text-meta); font-weight:var(--wght-strong); line-height:var(--lh-tight); letter-spacing:var(--track-caps);
  text-transform:uppercase; }
.app-select-group:not(:first-child) { margin-top:3px; border-top:var(--border-w) solid var(--border); }
.app-select-group-leading { display:inline-flex; align-items:center; justify-content:center;
  width:var(--icon-size-sm); height:var(--icon-size-sm); }
.app-select-group-leading .provider-logo { width:12px; height:12px; }
.app-select-group-label { min-width:0; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
.app-select-option { display:grid; grid-template-columns:var(--icon-size-sm) minmax(0, 1fr);
  column-gap:8px; align-items:start; box-sizing:border-box; width:100%; min-height:32px;
  padding:5px 6px; color:var(--fg); background:transparent; border:0;
  border-radius:var(--radius-control); font:inherit; font-size:var(--text-ui); text-align:left;
  cursor:pointer; }
.app-select-option.is-active { background:var(--row-selected); }
.app-select-option:not(.is-disabled):hover { background:var(--row-hover); }
.app-select-option.is-selected { color:var(--fg-strong); }
.app-select-option.is-disabled { color:var(--fg-faint); cursor:default; }
.app-select-check { align-self:start; width:var(--icon-size-sm); height:var(--icon-size-sm);
  margin-top:1px; color:var(--accent-text); }
.app-select-copy { display:flex; flex-direction:column; gap:2px; min-width:0; }
.app-select-option-heading { display:flex; align-items:baseline; gap:7px; min-width:0; }
.app-select-option-label { flex:1; min-width:0; overflow:hidden; line-height:var(--lh-ui);
  text-overflow:ellipsis; white-space:nowrap; }
.app-select-option-description { overflow:hidden; margin:0; color:var(--fg-dim);
  font-size:var(--text-meta); line-height:var(--lh-tight); text-overflow:ellipsis; white-space:nowrap; }
.app-select-option.is-disabled .app-select-option-description { color:var(--fg-faint); }
.app-select-option-meta { flex:none; max-width:120px; overflow:hidden; color:var(--fg-dim);
  font-size:var(--text-meta); line-height:var(--lh-tight); text-overflow:ellipsis; white-space:nowrap; }
.app-select-option:is(.is-active, .is-selected) :is(.app-select-option-description, .app-select-option-meta) {
  color:var(--fg-selected-secondary); }
.app-select-option.is-disabled :is(.app-select-option-description, .app-select-option-meta) {
  color:var(--fg-faint); }
.app-select-has-custom-value .app-select-value { font-family:var(--font-mono); }
.app-select-divider { height:var(--border-w); margin:4px 6px; background:var(--border); }
.app-select-custom { display:flex; flex-direction:column; gap:6px; padding:8px; }
.app-select-custom-label { color:var(--fg-muted); font-size:var(--text-meta); }
.app-select-custom-input { box-sizing:border-box; width:100%; height:var(--control-h); padding:0 7px;
  color:var(--input-fg); background:var(--input-bg); border:var(--border-w) solid var(--input-border);
  border-radius:var(--radius-control); font:inherit; font-family:var(--font-mono); font-size:var(--text-ui); }
.app-select-custom-input::placeholder { color:var(--input-placeholder); }
.app-select-custom-actions { display:flex; justify-content:flex-end; gap:6px; }
.app-select-custom-button { height:var(--control-h); padding:0 8px; color:var(--fg);
  background:transparent; border:var(--border-w) solid var(--border-strong);
  border-radius:var(--radius-control); font:inherit; font-size:var(--text-ui); cursor:pointer; }
.app-select-custom-button:hover:not(:disabled) { background:var(--row-hover); }
.app-select-custom-button:disabled { color:var(--fg-faint); cursor:default; }
.app-select-custom-button.is-primary { color:var(--accent-fg); background:var(--accent);
  border-color:var(--accent); }
.app-select-custom-button.is-primary:hover:not(:disabled) { background:var(--accent-hover);
  border-color:var(--accent-hover); }
`;

const CLOSED_VISIBLE: readonly boolean[] = [];
const CLOSED_METAS: readonly (string | undefined)[] = [];

function isPrintableKey(event: ReactKeyboardEvent): boolean {
  return event.key.length === 1 && event.key !== " "
    && !event.metaKey && !event.ctrlKey && !event.altKey;
}

export function AppSelect({
  label, value, options, onChange, prefix, leading, renderGroupLeading,
  placeholder = "Select…", disabled = false,
  pending = false, compact = false, className, popoverClassName,
  searchable = false, searchPlaceholder = "Search options",
  allowCustom = false,
  customLabel = "Enter a custom value", customPlaceholder = "Type a value",
  onCustom, onOpenChange, onShiftChoose, checkedValues, trailing, trailingTitle,
}: AppSelectProps) {
  const baseId = useId();
  const popupId = `${baseId}-popup`;
  const triggerRef = useRef<HTMLButtonElement>(null);
  const popoverRef = useRef<HTMLDivElement>(null);
  const listboxRef = useRef<HTMLDivElement>(null);
  const searchInputRef = useRef<HTMLInputElement>(null);
  const customInputRef = useRef<HTMLInputElement>(null);
  const optionRefs = useRef<Array<HTMLDivElement | null>>([]);
  const previousOptionsRef = useRef<readonly AppSelectOption[]>(options);
  const typeahead = useRef({ text: "", at: 0 });
  const [open, setOpen] = useState(false);
  const [activeIndex, setActiveIndex] = useState(() =>
    initialAppSelectIndex(options, value, allowCustom));
  const [position, setPosition] = useState<AppSelectPosition | null>(null);
  const [customEditing, setCustomEditing] = useState(false);
  const [customOption, setCustomOption] = useState<AppSelectOption | null>(null);
  const [customDraft, setCustomDraft] = useState("");
  const [searchQuery, setSearchQuery] = useState("");

  const selected = options.find(option => option.value === value);
  const shown = selected?.triggerLabel ?? selected?.label ?? (value || placeholder);
  const isPlaceholder = selected === undefined && value === "";
  // Only the open popover reads these. A closed picker has no search (it is
  // cleared on every open and close), so its keyboard navigates `options`
  // as they are, and nothing below is derived until it opens.
  const visible = useMemo(() => open ? options.map(option =>
    !searchable || appSelectOptionMatches(option, searchQuery)) : CLOSED_VISIBLE,
  [open, options, searchQuery, searchable]);
  const navigationOptions = useMemo(() => open ? options.map((option, index) => visible[index]
    ? option : { ...option, disabled: true }) : options, [open, options, visible]);
  const visibleIndices = useMemo(() => visible.flatMap((shownOption, index) =>
    shownOption ? [index] : []), [visible]);
  const optionVisibleMetas = useMemo(
    () => open ? appSelectOptionVisibleMetas(options) : CLOSED_METAS,
    [open, options],
  );
  const visibleRankByIndex = useMemo(() => {
    const ranks = new Map<number, number>();
    for (let rank = 0; rank < visibleIndices.length; rank++) {
      ranks.set(visibleIndices[rank]!, rank);
    }
    return ranks;
  }, [visibleIndices]);

  const closeMenu = useCallback((restoreFocus: boolean) => {
    setOpen(false);
    setCustomEditing(false);
    setCustomOption(null);
    setSearchQuery("");
    setPosition(null);
    onOpenChange?.(false);
    if (restoreFocus) queueMicrotask(() => { triggerRef.current?.focus(); });
  }, [onOpenChange]);

  const openMenu = useCallback((index?: number) => {
    const initial = initialAppSelectIndex(options, value, allowCustom);
    setActiveIndex(index !== undefined && index >= 0 ? index : initial);
    setCustomEditing(false);
    setCustomOption(null);
    setCustomDraft(selected === undefined && value !== "" ? value : "");
    setSearchQuery("");
    setPosition(null);
    setOpen(true);
    onOpenChange?.(true);
  }, [allowCustom, onOpenChange, options, selected, value]);

  const choose = useCallback((next: string) => {
    closeMenu(true);
    if (next !== value) onChange(next);
  }, [closeMenu, onChange, value]);

  const beginCustom = useCallback((option?: AppSelectOption) => {
    setCustomOption(option ?? null);
    if (searchable && searchQuery.trim() !== "") setCustomDraft(searchQuery.trim());
    setCustomEditing(true);
  }, [searchQuery, searchable]);

  const chooseIndex = useCallback((index: number, shift = false) => {
    if (allowCustom && index === options.length) {
      beginCustom();
      return;
    }
    const option = options[index];
    if (option !== undefined && option.disabled !== true) {
      if (option.custom === true) beginCustom(option);
      else if (shift && onShiftChoose !== undefined && option.value !== value) {
        onShiftChoose(option.value);
      } else choose(option.value);
    }
  }, [allowCustom, beginCustom, choose, onShiftChoose, options, value]);

  const applyTypeahead = useCallback((character: string) => {
    const now = Date.now();
    const prior = now - typeahead.current.at <= TYPEAHEAD_WINDOW_MS
      ? typeahead.current.text : "";
    const combined = `${prior}${character}`;
    // Repeating one key cycles through rows beginning with it rather than
    // searching for an unlikely string such as "ggg".
    const search = [...combined].every(char => char.toLocaleLowerCase()
      === combined[0]?.toLocaleLowerCase()) ? character : combined;
    typeahead.current = { text: combined, at: now };
    const match = appSelectTypeaheadIndex(options, search, activeIndex);
    if (match >= 0) setActiveIndex(match);
    return match;
  }, [activeIndex, options]);

  const runKeyAction = useCallback((event: ReactKeyboardEvent, isOpen: boolean) => {
    const action = appSelectKeyAction(navigationOptions,
      activeIndex, event.key, isOpen, allowCustom);
    if (action === null) return false;
    event.preventDefault();
    event.stopPropagation();
    if (action.type === "open") openMenu(action.index);
    else if (action.type === "move") setActiveIndex(action.index);
    else if (action.type === "choose") chooseIndex(action.index, event.shiftKey);
    else closeMenu(true);
    return true;
  }, [activeIndex, allowCustom, chooseIndex, closeMenu, navigationOptions, openMenu]);

  useEffect(() => {
    if (!open) return;
    if (customEditing) customInputRef.current?.focus();
    else if (searchable) searchInputRef.current?.focus();
    else listboxRef.current?.focus();
  }, [customEditing, open, searchable]);

  useEffect(() => {
    if (!open || customEditing || !searchable) return;
    setActiveIndex(initialAppSelectIndex(navigationOptions, value, allowCustom));
  }, [allowCustom, customEditing, navigationOptions, open, searchable, value]);

  useEffect(() => {
    if (!open || customEditing || activeIndex < 0) return;
    optionRefs.current[activeIndex]?.scrollIntoView({ block: "nearest" });
  }, [activeIndex, customEditing, open]);

  useEffect(() => {
    if (open && disabled) closeMenu(true);
  }, [closeMenu, disabled, open]);

  useEffect(() => {
    if (!open) return;
    const dismiss = (): void => { closeMenu(false); };
    window.addEventListener(APP_SELECT_DISMISS_EVENT, dismiss);
    return () => { window.removeEventListener(APP_SELECT_DISMISS_EVENT, dismiss); };
  }, [closeMenu, open]);

  useLayoutEffect(() => {
    const previous = previousOptionsRef.current;
    // Keep the pre-editor catalogue while the text-entry surface is open. If a
    // provider inserts models there, Back must reconcile the same custom row by
    // value rather than reusing its now-stale numeric position.
    if (customEditing) return;
    previousOptionsRef.current = options;
    if (!open) return;
    const reconciled = reconcileAppSelectIndex(
      previous, options, activeIndex, value, allowCustom);
    if (reconciled !== activeIndex) setActiveIndex(reconciled);
  }, [activeIndex, allowCustom, customEditing, open, options, value]);

  useLayoutEffect(() => {
    if (!open) return;
    const trigger = triggerRef.current;
    const popover = popoverRef.current;
    if (trigger === null || popover === null) return;
    const rect = trigger.getBoundingClientRect();
    setPosition(positionAppSelect(rect, {
      width: popover.offsetWidth,
      height: popover.scrollHeight,
    }, {
      width: window.innerWidth,
      height: window.innerHeight,
    }));
  }, [customEditing, open, options.length, searchQuery]);

  useEffect(() => {
    if (!open) return;
    const outside = (event: PointerEvent): void => {
      const path = event.composedPath();
      if (triggerRef.current !== null && path.includes(triggerRef.current)) return;
      if (popoverRef.current !== null && path.includes(popoverRef.current)) return;
      closeMenu(false);
    };
    const viewportChanged = (event: Event): void => {
      const target = event.target;
      if (event.type === "scroll" && target instanceof Node
          && popoverRef.current?.contains(target) === true) return;
      // A streaming transcript scrolls itself every frame without moving
      // the trigger; only a scroll that can move it dismisses.
      if (event.type === "scroll" && !scrollMovesAnchor(target, triggerRef.current)) return;
      closeMenu(true);
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
  }, [closeMenu, open]);

  const submitCustom = (): void => {
    const next = customDraft.trim();
    if (next === "") return;
    closeMenu(true);
    if (onCustom !== undefined) onCustom(next, customOption ?? undefined);
    else if (next !== value) onChange(next);
  };

  const moveFocusPastTrigger = (backwards: boolean): void => {
    const trigger = triggerRef.current;
    const target = focusPastTrigger(trigger, backwards, popoverRef.current);
    closeMenu(false);
    if (trigger !== null) queueMicrotask(() => { (target ?? trigger).focus(); });
  };

  const rootClass = ["app-select", compact ? "app-select-compact" : "",
    pending ? "app-select-pending" : "",
    selected === undefined && value !== "" ? "app-select-has-custom-value" : "",
    className ?? ""]
    .filter(Boolean).join(" ");
  const activeOptionId = activeIndex >= 0 ? `${baseId}-option-${activeIndex}` : undefined;
  const popoverStyle: CSSProperties = position === null
    ? { top: 0, left: 0, maxHeight: MAX_MENU_HEIGHT, visibility: "hidden" }
    : { top: position.top, left: position.left, width: position.width,
        maxHeight: position.maxHeight };

  const popup = open && typeof document !== "undefined" ? createPortal(
    <div
      ref={popoverRef}
      className={appSelectPopoverClasses(popoverClassName)}
      style={popoverStyle}
      data-side={position?.side}
    >
      {customEditing ? (
        <form className="app-select-custom" role="group"
          aria-label={customOption?.customLabel ?? customLabel}
          onSubmit={event => { event.preventDefault(); submitCustom(); }}
          onKeyDown={event => {
            if (event.key === "Escape") {
              event.preventDefault();
              event.stopPropagation();
              closeMenu(true);
            } else if (event.key === "Tab") {
              const customControls = [customInputRef.current,
                ...popoverRef.current?.querySelectorAll<HTMLButtonElement>("button") ?? []]
                .filter((node): node is HTMLButtonElement | HTMLInputElement =>
                  node !== null && !node.hasAttribute("disabled"));
              const at = customControls.findIndex(node => node === document.activeElement);
              if ((!event.shiftKey && at === customControls.length - 1)
                  || (event.shiftKey && at <= 0)) {
                event.preventDefault();
                event.stopPropagation();
                moveFocusPastTrigger(event.shiftKey);
              }
            }
          }}>
          <label className="app-select-custom-label" htmlFor={`${baseId}-custom`}>
            {customOption?.customLabel ?? customLabel}
          </label>
          <input ref={customInputRef} id={`${baseId}-custom`} className="app-select-custom-input"
            value={customDraft} placeholder={customOption?.customPlaceholder ?? customPlaceholder}
            autoComplete="off" spellCheck={false}
            onChange={event => { setCustomDraft(event.currentTarget.value); }} />
          <div className="app-select-custom-actions">
            <button type="button" className="app-select-custom-button"
              onClick={() => { setCustomEditing(false); }}>Back</button>
            <button type="submit" className="app-select-custom-button is-primary"
              disabled={customDraft.trim() === ""}>Use value</button>
          </div>
        </form>
      ) : (
        <>
        {searchable && <div className="app-select-search-wrap">
          <input ref={searchInputRef} type="search" className="app-select-search"
            value={searchQuery} placeholder={searchPlaceholder} autoComplete="off"
            role="combobox" aria-label={`Search ${label.toLocaleLowerCase()}`}
            aria-expanded="true" aria-autocomplete="list" aria-controls={popupId}
            aria-activedescendant={activeOptionId}
            onChange={event => { setSearchQuery(event.currentTarget.value); }}
            onKeyDown={event => {
              if (["ArrowDown", "ArrowUp", "Enter", "Escape"].includes(event.key)
                  && runKeyAction(event, true)) return;
              if (event.key === "Tab") {
                event.preventDefault();
                event.stopPropagation();
                moveFocusPastTrigger(event.shiftKey);
              }
            }} />
          <span className="app-select-results-status" role="status" aria-live="polite">
            {visibleIndices.length} {visibleIndices.length === 1 ? "option" : "options"}
          </span>
        </div>}
        {visibleIndices.length === 0 && !allowCustom && (
          <p className="app-select-no-results" role="status">No matching options.</p>
        )}
        <div ref={listboxRef} id={popupId} className="app-select-options" role="listbox" tabIndex={-1}
          aria-label={label} aria-activedescendant={activeOptionId}
          aria-multiselectable={onShiftChoose !== undefined || undefined}
          onKeyDown={event => {
            if (runKeyAction(event, true)) return;
            if (isPrintableKey(event)) {
              event.preventDefault();
              event.stopPropagation();
              applyTypeahead(event.key);
            } else if (event.key === "Tab") {
              event.preventDefault();
              event.stopPropagation();
              moveFocusPastTrigger(event.shiftKey);
            }
          }}>
          {options.map((option, index) => {
            const selectedOption = option.value === value
              || checkedValues?.has(option.value) === true;
            const active = index === activeIndex;
            const visibleMeta = optionVisibleMetas[index];
            const supportingText = appSelectOptionSupportingText(option, visibleMeta);
            const optionClass = ["app-select-option", active ? "is-active" : "",
              selectedOption ? "is-selected" : "", option.disabled ? "is-disabled" : ""]
              .filter(Boolean).join(" ");
            const visibleAt = visibleRankByIndex.get(index) ?? -1;
            const previousVisible = visibleAt <= 0 ? undefined
              : options[visibleIndices[visibleAt - 1]];
            const beginsGroup = visible[index] && option.group !== undefined
              && previousVisible?.group !== option.group;
            return <Fragment key={`${option.value}-${index}`}>
              {beginsGroup && (
                <div className="app-select-group" role="presentation">
                  <span className="app-select-group-leading" aria-hidden="true">
                    {renderGroupLeading?.(option)}
                  </span>
                  <span className="app-select-group-label">{option.group}</span>
                </div>
              )}
              <div id={`${baseId}-option-${index}`}
                ref={node => { optionRefs.current[index] = node; }}
                className={optionClass} role="option"
                hidden={!visible[index]}
                aria-label={appSelectOptionAccessibleName(option)}
                aria-describedby={supportingText === undefined
                  ? undefined : `${baseId}-option-${index}-description`}
                aria-selected={option.custom === true ? false : selectedOption}
                aria-disabled={option.disabled || undefined}
                onMouseEnter={() => { if (option.disabled !== true) setActiveIndex(index); }}
                onMouseDown={event => { event.preventDefault(); }}
                onClick={event => {
                  if (option.disabled === true) return;
                  if (option.custom === true) beginCustom(option);
                  else if (event.shiftKey && onShiftChoose !== undefined
                      && option.value !== value) onShiftChoose(option.value);
                  else choose(option.value);
                }}>
                {option.custom === true
                  ? <span className="codicon codicon-sm codicon-add app-select-check"
                      aria-hidden="true" />
                  : selectedOption
                  ? <span className="codicon codicon-sm codicon-check app-select-check"
                      aria-hidden="true" />
                  : <span className="app-select-check" aria-hidden="true" />}
                <span className="app-select-copy">
                  <span className="app-select-option-heading">
                    <span className="app-select-option-label">{option.label}</span>
                    {visibleMeta !== undefined && (
                      <span className="app-select-option-meta">{visibleMeta}</span>
                    )}
                  </span>
                  {supportingText !== undefined && (
                    <span id={`${baseId}-option-${index}-description`}
                      className="app-select-option-description">{supportingText}</span>
                  )}
                </span>
              </div>
            </Fragment>;
          })}
          {allowCustom && <>
            {options.length > 0 && <div className="app-select-divider" aria-hidden="true" />}
            <div id={`${baseId}-option-${options.length}`}
              ref={node => { optionRefs.current[options.length] = node; }}
              className={`app-select-option${activeIndex === options.length ? " is-active" : ""}`}
              role="option" aria-selected="false" aria-label={customLabel}
              aria-describedby={`${baseId}-option-${options.length}-description`}
              onMouseEnter={() => { setActiveIndex(options.length); }}
              onMouseDown={event => { event.preventDefault(); }}
              onClick={() => { beginCustom(); }}>
              <span className="codicon codicon-sm codicon-add app-select-check" aria-hidden="true" />
              <span className="app-select-copy">
                <span className="app-select-option-label">{customLabel}</span>
                <span id={`${baseId}-option-${options.length}-description`}
                  className="app-select-option-description">Use a model or value not listed above</span>
              </span>
            </div>
          </>}
        </div>
        </>
      )}
    </div>,
    document.body,
  ) : null;

  return <>
    <span className={rootClass}>
      <button ref={triggerRef} type="button" className="app-select-trigger"
        disabled={disabled}
        aria-label={trailing === undefined ? `${label}: ${shown}` : `${label}: ${shown}, ${trailing}`}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={open && !customEditing ? popupId : undefined}
        aria-busy={pending || undefined}
        title={selected === undefined
          ? trailingTitle
          : [shown, selected.description, trailingTitle].filter(Boolean).join(" — ")}
        onClick={() => { if (open) closeMenu(true); else openMenu(); }}
        onKeyDown={event => {
          if (runKeyAction(event, open)) return;
          if (isPrintableKey(event)) {
            event.preventDefault();
            event.stopPropagation();
            const match = applyTypeahead(event.key);
            openMenu(match >= 0 ? match : undefined);
          }
        }}>
        {prefix !== undefined && <span className="app-select-prefix">{prefix}</span>}
        {leading !== undefined && leading !== null && (
          <span className="app-select-leading" aria-hidden="true">{leading}</span>
        )}
        <span className={isPlaceholder
          ? "app-select-value app-select-placeholder" : "app-select-value"}>{shown}</span>
        {trailing !== undefined && (
          <span className="app-select-trailing" aria-hidden="true">{trailing}</span>
        )}
        <span className="codicon codicon-sm codicon-chevron-down app-select-chevron"
          aria-hidden="true" />
      </button>
    </span>
    {popup}
  </>;
}
