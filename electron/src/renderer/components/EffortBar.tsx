import {
  useCallback, useEffect, useId, useLayoutEffect, useRef, useState,
} from "react";
import type { CSSProperties, Ref } from "react";
import { createPortal } from "react-dom";
import {
  APP_SELECT_DISMISS_EVENT,
  positionAppSelect,
  type AppSelectPosition,
  type AppSelectRect,
  type AppSelectSize,
  type AppSelectViewport,
} from "./AppSelect";
import { focusPastTrigger } from "./ModalBehavior";
import {
  effortChangeCommitsImmediately, effortDisplayName, effortLevelCelebrates, orderEffortLevels,
  shouldCelebrateEffortChange, topTierLevel, type EffortInputInteraction,
} from "../effort-levels";
import { scrollMovesAnchor } from "./scroll-anchor";

export interface EffortBarProps {
  levels: readonly string[];
  value: string;
  onChange: (value: string) => void;
  disabled?: boolean;
  pending?: boolean;
  className?: string;
  /** Extra copy for a specific level, e.g. session-only max. */
  notes?: Readonly<Record<string, string>>;
}

const EFFORT_POPOVER_MIN_WIDTH = 224;
const VIEWPORT_GUTTER = 8;

const clampIndex = (levels: readonly string[], index: number): number =>
  Math.max(0, Math.min(Math.max(0, levels.length - 1), index));

/** Reuse the app picker geometry, but align this compact control by its right edge. */
export function positionEffortPopover(
  anchor: AppSelectRect,
  menu: AppSelectSize,
  viewport: AppSelectViewport,
): AppSelectPosition {
  const positioned = positionAppSelect(anchor, {
    width: Math.max(menu.width, EFFORT_POPOVER_MIN_WIDTH),
    height: menu.height,
  }, viewport);
  const rightAligned = anchor.right - positioned.width;
  const maximumLeft = Math.max(
    VIEWPORT_GUTTER,
    viewport.width - VIEWPORT_GUTTER - positioned.width,
  );
  return {
    ...positioned,
    left: Math.min(Math.max(rightAligned, VIEWPORT_GUTTER), maximumLeft),
  };
}

/** Matches the longest CLI celebrate animation (hue 2.4s). */
export const EFFORT_CELEBRATE_MS = 2400;

const effortPercent = (index: number, count: number): number =>
  count <= 1 ? 50 : (index / (count - 1)) * 100;

const EFFORT_STEP_KEYS = [
  "ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End", "PageUp", "PageDown",
] as const;

/* The top-tier celebration: a token-tinted starfield inside the track fill.
   Positions are fixed, not random — the surface must render identically on
   every open (and in the no-DOM suite). Sizes in px, offsets in track %. */
const EFFORT_SPARKS = [
  { left: "7%", top: "26%", size: 3, delay: "0s" },
  { left: "16%", top: "62%", size: 2, delay: "0.9s" },
  { left: "27%", top: "38%", size: 2, delay: "0.4s" },
  { left: "38%", top: "70%", size: 3, delay: "1.5s" },
  { left: "49%", top: "24%", size: 2, delay: "0.7s" },
  { left: "58%", top: "55%", size: 4, delay: "1.9s" },
  { left: "69%", top: "33%", size: 2, delay: "0.2s" },
  { left: "78%", top: "64%", size: 3, delay: "1.2s" },
  { left: "88%", top: "40%", size: 2, delay: "1.7s" },
] as const;

export const EFFORT_PICKER_CSS = `
.effort-picker { min-width:0; }
.effort-picker .app-select-trigger { min-width:0; }
.effort-picker .app-select-value { max-width:88px; }
/* The trigger reserves the widest label of ITS OWN ladder: every level is
   rendered into the same grid cell, invisible except the current one, so
   choosing Ultracode over High cannot widen the pill and shove the row's
   other controls sideways. */
.effort-value-stack { display:inline-grid; }
.effort-value-stack > span { grid-area:1/1; min-width:0; overflow:hidden;
  text-overflow:ellipsis; white-space:nowrap; }
.effort-value-ghost { visibility:hidden; }
.effort-picker-maximum .app-select-value,
.effort-picker-xhigh .app-select-value { color:var(--accent-text); }
/* The top rung is the one colour that does not follow the theme accent: the
   ceiling reads as the same purple in light and dark so it stays recognisable
   across a theme switch. Scoped token overrides, so every rule below — pill,
   dot, ramp, sparks, thumb glow — keeps using --accent as written. */
.effort-picker-maximum,
.effort-popover.is-maximum { --accent:var(--effort-max);
  --accent-text:var(--effort-max); --accent-fg:var(--effort-max-fg); }
.effort-picker.app-select-pending .app-select-value { color:var(--fg-muted); }
.effort-picker.app-select-pending .app-select-trigger:hover { color:var(--fg-muted);
  background:transparent; }
.effort-picker .app-select-trigger:disabled .app-select-value { color:var(--fg-faint); }
.effort-picker-single .app-select-trigger { cursor:default; }
.effort-picker-single .app-select-trigger:hover { color:var(--fg-muted);
  background:transparent; }
/* Named ladder is markup-only until CLI chrome reveals it. Refined keeps
   the track; a second visible list would be two controls for one axis. */
.effort-cli-ladder { display:none; }

.effort-popover { --effort-color:var(--fg-strong); position:fixed; z-index:102;
  box-sizing:border-box; min-width:${EFFORT_POPOVER_MIN_WIDTH}px; max-width:320px;
  padding:10px; overflow:hidden; color:var(--fg); background:var(--dropdown-bg);
  border:var(--border-w) solid var(--border-strong); border-radius:var(--radius-overlay);
  box-shadow:var(--overlay-shadow); font-family:var(--font-ui);
  transform-origin:top right; animation:effort-pop-in 140ms ease-out; }
.effort-popover[data-side="above"] { transform-origin:bottom right; }
.effort-popover.is-maximum { --effort-color:var(--accent-text); }
.effort-popover-head { display:flex; align-items:baseline; justify-content:space-between;
  gap:12px; min-width:0; }
.effort-popover-title { color:var(--fg-strong); font-size:var(--text-ui); font-weight:var(--wght-strong); }
.effort-popover-value { min-width:0; overflow:hidden; color:var(--effort-color);
  font-size:var(--text-ui); text-overflow:ellipsis; white-space:nowrap; }
/* Claude Code's shape: an airy dot track with a draggable thumb — no
   per-level labels to compact, the header narrates the value. The value
   applies live on every change and choosing never dismisses the surface;
   only Esc, Tab, the trigger, or a genuine outside click do. */
.effort-popover-ends { display:flex; justify-content:space-between; margin-top:8px;
  padding:0 2px; color:var(--fg-dim); font-size:var(--text-meta); line-height:var(--lh-tight); }
.effort-track-shell { position:relative; height:28px; margin-top:4px; }
.effort-range { position:absolute; z-index:3; inset:0; width:100%; height:28px;
  margin:0; opacity:0; appearance:none; cursor:pointer; }
.effort-range::-webkit-slider-runnable-track { height:28px; background:transparent; }
.effort-range::-webkit-slider-thumb { width:28px; height:28px; margin:0;
  appearance:none; background:transparent; border:0; }
.effort-track { position:absolute; z-index:1; inset:0; border-radius:var(--radius-pill);
  background:var(--surface-raised); pointer-events:none; }
.effort-range:focus-visible + .effort-track {
  outline:var(--focus-ring-w) solid var(--focus);
  outline-offset:var(--focus-ring-offset); box-shadow:0 0 0 1px var(--focus-contrast); }
.effort-track-scale { position:absolute; inset:0 14px; }
.effort-dot { position:absolute; top:50%; width:5px; height:5px;
  transform:translate(-50%, -50%); border-radius:50%; background:var(--fg-faint); }
.effort-dot.is-maximum { width:6px; height:6px;
  background:var(--accent-text); }
/* border-box, or the 2px ring renders OUTSIDE the box and the disc sits
   +2px right and down of the geometric point — invisible mid-track, obvious
   at the ends where the pill cap frames it. 24px in the 28px track nests
   the thumb concentrically inside the cap at either extreme. */
.effort-thumb { position:absolute; top:50%; width:24px; height:24px;
  margin:-12px 0 0 -12px; box-sizing:border-box; background:var(--dropdown-bg);
  border:2px solid var(--effort-color); border-radius:50%;
  box-shadow:var(--overlay-shadow); }
/* Top-tier levels light the track: an accent ramp sweeps in and a fixed
   starfield twinkles inside it. Tokens only — the ramp mixes the theme's
   own accent, the sparks use its on-accent colour — so every theme keeps
   its identity. The plain dots yield to the fill while it is up. */
.effort-track-fill { position:absolute; top:0; bottom:0; left:0; overflow:hidden;
  border-radius:var(--radius-pill);
  background:linear-gradient(90deg,
    color-mix(in srgb, var(--accent) 28%, var(--surface-raised)),
    color-mix(in srgb, var(--accent) 72%, var(--surface-raised)),
    var(--accent));
  animation:effort-fill-sweep 380ms ease-out; }
.effort-spark { position:absolute; border-radius:50%; background:var(--accent-fg);
  opacity:0; animation:effort-twinkle 2.6s ease-in-out infinite;
  animation-play-state:paused; }
.effort-popover.is-maximum .effort-dot { opacity:0; }
/* A slow luster crosses the ramp and then dwells. --accent-fg is the theme's
   on-accent colour — the same token the sparks use — so the band stays
   legible on every palette instead of washing out on the pale ones. */
.effort-track-fill::after { content:""; position:absolute; inset:0;
  background:linear-gradient(105deg, transparent 42%,
    color-mix(in srgb, var(--accent-fg) 32%, transparent) 50%, transparent 58%);
  background-size:250% 100%; animation:effort-sheen 4s ease-in-out infinite;
  animation-play-state:paused; }
/* The thumb breathes on the twinkle's own 2.6s cadence, one pulse for the
   whole surface. Every frame keeps the elevation shadow first, so the disc
   never flattens onto the track mid-breath. */
.effort-popover.is-maximum .effort-thumb {
  animation:effort-thumb-glow 2.6s ease-in-out infinite;
  animation-play-state:paused; }
/* PAUSED UNLESS THE READER IS HERE. Three loops ran at once on a popover that
   was merely open — a continuous animation budget spent on a surface nobody
   was looking at, in an app whose rule is that loops are reserved for real
   provider activity. Pointing at the popover or tabbing into it starts them;
   the one-shot fill sweep is unaffected, and reduced motion still wins below. */
.effort-popover:hover .effort-spark,
.effort-popover:focus-within .effort-spark,
.effort-popover:hover .effort-track-fill::after,
.effort-popover:focus-within .effort-track-fill::after,
.effort-popover.is-maximum:hover .effort-thumb,
.effort-popover.is-maximum:focus-within .effort-thumb {
  animation-play-state:running; }
@keyframes effort-fill-sweep {
  from { clip-path:inset(0 100% 0 0); }
  to { clip-path:inset(0 0 0 0); }
}
@keyframes effort-twinkle {
  0%, 100% { opacity:0; transform:scale(.6); }
  50% { opacity:.95; transform:scale(1); }
}
@keyframes effort-sheen {
  0% { background-position:100% 0; }
  55%, 100% { background-position:0 0; }
}
@keyframes effort-thumb-glow {
  0%, 100% { box-shadow:var(--overlay-shadow),
    0 0 6px 1px color-mix(in srgb, var(--accent) 30%, transparent); }
  50% { box-shadow:var(--overlay-shadow),
    0 0 12px 3px color-mix(in srgb, var(--accent) 60%, transparent); }
}
.effort-popover-level-note { margin:6px 0 0; color:var(--fg-muted);
  font-size:var(--text-meta); line-height:var(--lh-tight); }
@keyframes effort-pop-in {
  from { opacity:0; transform:scale(.98); }
  to { opacity:1; transform:scale(1); }
}
@media (prefers-reduced-motion: reduce) {
  .effort-popover { animation:none; }
  .effort-track-fill { animation:none; }
  .effort-track-fill::after { content:none; animation:none; }
  .effort-spark { animation:none; opacity:.55; transform:none; }
  .effort-popover.is-maximum .effort-thumb { animation:none;
    box-shadow:var(--overlay-shadow),
      0 0 8px 1px color-mix(in srgb, var(--accent) 40%, transparent); }
}
`;

export interface EffortPopoverProps {
  id: string;
  levels: readonly string[];
  index: number;
  notes?: Readonly<Record<string, string>>;
  position: AppSelectPosition | null;
  panelRef?: Ref<HTMLDivElement>;
  /** The native range input, focused by the owner when the surface opens. */
  rangeRef?: Ref<HTMLInputElement>;
  /** Thumb movement while dragging or stepping — visual only. */
  onPreview: (index: number) => void;
  /** Commit the previewed level. The surface STAYS OPEN. */
  onCommit: (index: number) => void;
  onClose: () => void;
  onExit: (backwards: boolean) => void;
}

/** Exported so the no-DOM suite can verify the real portalled surface. */
export function EffortPopover({
  id, levels, index, notes, position, panelRef, rangeRef,
  onPreview, onCommit, onClose, onExit,
}: EffortPopoverProps) {
  const interaction = useRef<EffortInputInteraction>(null);
  const safeIndex = clampIndex(levels, index);
  const level = levels[safeIndex] ?? "";
  const label = effortDisplayName(level);
  const levelNote = notes?.[level];
  const accent = level !== "" && level === topTierLevel(levels);
  const xhigh = level.trim().toLocaleLowerCase() === "xhigh";
  const style: CSSProperties = position === null
    ? { top: 0, left: 0, visibility: "hidden" }
    : { top: position.top, left: position.left, width: position.width,
        maxHeight: position.maxHeight };

  return (
    <div ref={panelRef} id={id}
      className={`effort-popover is-floating${accent ? " is-maximum" : ""}${xhigh ? " is-xhigh" : ""}`}
      style={style} data-side={position?.side}
      role="dialog" aria-labelledby={`${id}-title`}>
      <div className="effort-popover-head">
        <span id={`${id}-title`} className="effort-popover-title">Effort</span>
        <span className="effort-popover-value" role="status" aria-live="polite">
          {label}
        </span>
      </div>
      <div className="effort-popover-ends" aria-hidden="true">
        <span>Faster</span><span>Smarter</span>
      </div>
      <div className="effort-track-shell">
        <input ref={rangeRef} className="effort-range" type="range"
          min={0} max={Math.max(0, levels.length - 1)} step={1} value={safeIndex}
          aria-label="Effort level" aria-valuetext={label}
          aria-describedby={levelNote === undefined ? undefined : `${id}-level-note`}
          onChange={event => {
            const next = clampIndex(levels, event.currentTarget.valueAsNumber);
            onPreview(next);
            if (effortChangeCommitsImmediately(interaction.current)) onCommit(next);
          }}
          onPointerDown={() => { interaction.current = "pointer"; }}
          onPointerUp={event => {
            interaction.current = null;
            onCommit(clampIndex(levels, event.currentTarget.valueAsNumber));
          }}
          onPointerCancel={() => { interaction.current = null; }}
          onKeyUp={event => {
            if ((EFFORT_STEP_KEYS as readonly string[]).includes(event.key)) {
              interaction.current = null;
              onCommit(clampIndex(levels, event.currentTarget.valueAsNumber));
            }
          }}
          onKeyDown={event => {
            if ((EFFORT_STEP_KEYS as readonly string[]).includes(event.key)) {
              interaction.current = "keyboard";
            }
            if (event.key === "Tab") {
              event.preventDefault();
              event.stopPropagation();
              onExit(event.shiftKey);
              return;
            }
            if (event.key === "Enter") {
              event.preventDefault();
              event.stopPropagation();
              onCommit(safeIndex);
              onClose();
              return;
            }
            if (event.key === "Escape") {
              event.preventDefault();
              event.stopPropagation();
              onClose();
            }
          }}
          onBlur={() => { interaction.current = null; }} />
        <div className="effort-track" aria-hidden="true">
          {accent && (
            <span className="effort-track-fill" data-effort-fill={level}
              style={{ width: `${effortPercent(safeIndex, levels.length)}%` }}>
              {EFFORT_SPARKS.map((spark, sparkIndex) => (
                <span key={sparkIndex} className="effort-spark"
                  style={{ left: spark.left, top: spark.top, width: spark.size,
                    height: spark.size, animationDelay: spark.delay }} />
              ))}
            </span>
          )}
          <span className="effort-track-scale">
            {levels.map((dot, dotIndex) => (
              <span key={`${dot}-${dotIndex}`}
                className={`effort-dot${dotIndex === levels.length - 1
                  && topTierLevel(levels) !== null ? " is-maximum" : ""}`}
                data-effort-dot={dot}
                style={{ left: `${effortPercent(dotIndex, levels.length)}%` }} />
            ))}
            <span className="effort-thumb"
              style={{ left: `${effortPercent(safeIndex, levels.length)}%` }} />
          </span>
        </div>
      </div>
      <div className="effort-cli-ladder" role="listbox" aria-label="Effort">
        {levels.map((option, optionIndex) => {
          const current = optionIndex === safeIndex;
          return (
            <button key={`${option}-${optionIndex}`} type="button"
              className={current ? "effort-cli-opt is-current" : "effort-cli-opt"}
              role="option" aria-selected={current}
              data-effort-opt={option}
              onClick={() => { onPreview(optionIndex); onCommit(optionIndex); }}
              onKeyDown={event => {
                if (event.key === "ArrowDown" || event.key === "ArrowUp") {
                  event.preventDefault();
                  const next = clampIndex(levels, optionIndex
                    + (event.key === "ArrowDown" ? 1 : -1));
                  onPreview(next);
                  onCommit(next);
                  return;
                }
                if (event.key === "Enter") {
                  event.preventDefault();
                  onCommit(optionIndex);
                  onClose();
                  return;
                }
                if (event.key === "Escape") {
                  event.preventDefault();
                  onClose();
                  return;
                }
                if (event.key === "Tab") {
                  event.preventDefault();
                  onExit(event.shiftKey);
                }
              }}>
              {effortDisplayName(option)}
            </button>
          );
        })}
      </div>
      {levelNote !== undefined && (
        <p id={`${id}-level-note`} className="effort-popover-level-note">{levelNote}</p>
      )}
    </div>
  );
}

/**
 * Compact resting value with the provider's effort ladder in a pop-out.
 * The public name stays EffortBar to avoid making callers care how it is drawn.
 */
export function EffortBar({
  levels: advertised, value, onChange, disabled = false, pending = false,
  className, notes,
}: EffortBarProps) {
  const uid = useId();
  const popupId = `${uid}-effort`;
  const triggerRef = useRef<HTMLButtonElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const rangeRef = useRef<HTMLInputElement>(null);
  const levels = orderEffortLevels(advertised);
  const selectedIndex = levels.indexOf(value);
  const displayedIndex = selectedIndex >= 0 ? selectedIndex : levels.length === 1 ? 0 : -1;
  const currentLevel = displayedIndex >= 0 ? levels[displayedIndex] ?? "" : "";
  const currentLabel = effortDisplayName(currentLevel);
  const hasChoices = levels.length > 1;
  const blocked = disabled || pending;
  const hardDisabled = disabled && !pending;
  const catalogueKey = levels.join("\0");
  const [open, setOpen] = useState(false);
  const [draftIndex, setDraftIndex] = useState(() => clampIndex(levels, selectedIndex));
  const [position, setPosition] = useState<AppSelectPosition | null>(null);
  // Bumped when the viewport moves under the open surface; the layout effect
  // below re-anchors to the trigger instead of dismissing. A live session's
  // transcript scrolls itself on every config round-trip (resuming a session
  // auto-scrolls the log), and closing on that killed the pop-out at the
  // exact moment a commit landed — the third "disappears on release" report.
  const [reflow, setReflow] = useState(0);
  const [celebrate, setCelebrate] = useState(false);
  const previousLevelRef = useRef<string | null>(null);

  useEffect(() => {
    const previous = previousLevelRef.current;
    previousLevelRef.current = currentLevel;
    if (!effortLevelCelebrates(currentLevel, levels)) {
      setCelebrate(false);
      return;
    }
    if (!shouldCelebrateEffortChange(previous, currentLevel, levels)) return;
    if (typeof window !== "undefined"
        && typeof window.matchMedia === "function"
        && window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
      return;
    }
    setCelebrate(true);
    const timer = window.setTimeout(() => { setCelebrate(false); }, EFFORT_CELEBRATE_MS);
    return () => { window.clearTimeout(timer); };
    // catalogueKey, not `levels`: orderEffortLevels returns a new array every
    // render, and a dep change would cancel the one-shot mid-play.
  }, [catalogueKey, currentLevel]);

  const close = useCallback((restoreFocus: boolean): void => {
    setOpen(false);
    setPosition(null);
    if (restoreFocus) queueMicrotask(() => { triggerRef.current?.focus(); });
  }, []);

  const openPicker = useCallback((): void => {
    setDraftIndex(clampIndex(levels, selectedIndex));
    setPosition(null);
    setOpen(true);
  }, [levels, selectedIndex]);

  // Committing a level never dismisses the surface — that was the original
  // sin, twice: apply-on-release closed it, then click-to-choose closed it.
  // The pop-out lingers for further adjustment until Esc, Enter, Tab, the
  // trigger, or a genuine outside click. While a change is pending the
  // commit is ignored; the reconcile effect snaps the thumb back if the
  // provider refuses the value.
  const commit = useCallback((index: number): void => {
    const next = levels[clampIndex(levels, index)];
    if (pending || next === undefined || next === value) return;
    onChange(next);
  }, [levels, onChange, pending, value]);

  const moveFocusPastTrigger = useCallback((backwards: boolean): void => {
    const trigger = triggerRef.current;
    const target = focusPastTrigger(trigger, backwards, panelRef.current);
    close(false);
    if (trigger !== null) queueMicrotask(() => { (target ?? trigger).focus(); });
  }, [close]);

  // Provider catalogues can refresh while the pop-out is open. Reconcile by
  // value instead of keeping a numeric slot that may now name another level.
  useLayoutEffect(() => {
    if (open) setDraftIndex(clampIndex(levels, selectedIndex));
  }, [catalogueKey, open, selectedIndex]);

  useLayoutEffect(() => {
    if (!open) return;
    const trigger = triggerRef.current;
    const panel = panelRef.current;
    if (trigger === null || panel === null) return;
    setPosition(positionEffortPopover(trigger.getBoundingClientRect(), {
      width: panel.offsetWidth,
      height: panel.scrollHeight,
    }, {
      width: window.innerWidth,
      height: window.innerHeight,
    }));
  }, [catalogueKey, draftIndex, open, reflow]);

  useEffect(() => {
    if (!open) return;
    const cli = document.documentElement.dataset.chrome === "cli";
    if (cli) {
      panelRef.current?.querySelector<HTMLElement>(".effort-cli-opt.is-current")?.focus();
      return;
    }
    rangeRef.current?.focus();
  }, [draftIndex, open, position]);

  // Hard-disabled loses the surface; a merely pending change does NOT — the
  // pop-out stays open across the round-trip it itself just started.
  useEffect(() => {
    if (open && (hardDisabled || !hasChoices)) close(true);
  }, [close, hardDisabled, hasChoices, open]);

  useEffect(() => {
    if (!open) return;
    const outside = (event: PointerEvent): void => {
      const path = event.composedPath();
      if (triggerRef.current !== null && path.includes(triggerRef.current)) return;
      if (panelRef.current !== null && path.includes(panelRef.current)) return;
      close(false);
    };
    const focusMoved = (event: FocusEvent): void => {
      const target = event.target;
      if (!(target instanceof Node)) return;
      if (triggerRef.current?.contains(target) === true) return;
      if (panelRef.current?.contains(target) === true) return;
      close(false);
    };
    // Scrolls and resizes RE-ANCHOR the open surface, never dismiss it. The
    // trigger lives in the pinned composer row, so a transcript scroll (which
    // every config commit causes on a live session) moves nothing this
    // pop-out is anchored to.
    const viewportChanged = (event: Event): void => {
      const target = event.target;
      if (event.type === "scroll" && target instanceof Node
          && panelRef.current?.contains(target) === true) return;
      if (event.type === "scroll" && !scrollMovesAnchor(target, triggerRef.current)) return;
      setReflow(count => count + 1);
    };
    const dismiss = (): void => { close(false); };
    document.addEventListener("pointerdown", outside, true);
    document.addEventListener("focusin", focusMoved, true);
    window.addEventListener("resize", viewportChanged);
    window.addEventListener("scroll", viewportChanged, true);
    window.visualViewport?.addEventListener("resize", viewportChanged);
    window.visualViewport?.addEventListener("scroll", viewportChanged);
    window.addEventListener(APP_SELECT_DISMISS_EVENT, dismiss);
    return () => {
      document.removeEventListener("pointerdown", outside, true);
      document.removeEventListener("focusin", focusMoved, true);
      window.removeEventListener("resize", viewportChanged);
      window.removeEventListener("scroll", viewportChanged, true);
      window.visualViewport?.removeEventListener("resize", viewportChanged);
      window.visualViewport?.removeEventListener("scroll", viewportChanged);
      window.removeEventListener(APP_SELECT_DISMISS_EVENT, dismiss);
    };
  }, [close, open]);

  if (levels.length === 0) return null;

  const rootClass = [
    "app-select", "app-select-compact", "effort-picker",
    pending ? "app-select-pending" : "",
    currentLevel !== "" && currentLevel === topTierLevel(levels)
      ? "effort-picker-maximum" : "",
    currentLevel.trim().toLocaleLowerCase() === "xhigh" ? "effort-picker-xhigh" : "",
    celebrate ? "effort-picker-celebrate" : "",
    hasChoices ? "" : "effort-picker-single",
    className ?? "",
  ].filter(Boolean).join(" ");
  const title = currentLevel === ""
    ? "Choose effort"
    : [currentLabel, notes?.[currentLevel]].filter(Boolean).join(" — ");
  const popup = open ? (
    <EffortPopover id={popupId} levels={levels} index={draftIndex} notes={notes}
      position={position} panelRef={panelRef} rangeRef={rangeRef}
      onPreview={setDraftIndex} onCommit={commit} onClose={() => { close(true); }}
      onExit={moveFocusPastTrigger} />
  ) : null;

  return <>
    <style>{EFFORT_PICKER_CSS}</style>
    <span className={rootClass}>
      <button ref={triggerRef} type="button" className="app-select-trigger effort-picker-trigger"
        disabled={hardDisabled}
        aria-label={currentLevel === "" ? "Effort" : `Effort: ${currentLabel}`}
        aria-disabled={blocked || !hasChoices || undefined}
        aria-haspopup={hasChoices ? "dialog" : undefined}
        aria-expanded={hasChoices ? open : undefined}
        aria-controls={open ? popupId : undefined}
        aria-busy={pending || undefined}
        title={title}
        onClick={() => {
          if (blocked || !hasChoices) return;
          if (open) close(true); else openPicker();
        }}>
        <span className={currentLevel === ""
          ? "app-select-value effort-value-stack app-select-placeholder"
          : "app-select-value effort-value-stack"}>
          {levels.map((levelOption, levelIndex) => (
            <span key={`${levelOption}-${levelIndex}`}
              aria-hidden={levelOption === currentLevel ? undefined : true}
              className={levelOption === currentLevel
                ? "effort-value-current" : "effort-value-ghost"}>
              {effortDisplayName(levelOption)}
            </span>
          ))}
          {currentLevel === "" && (
            <span className="effort-value-current">{currentLabel}</span>
          )}
        </span>
        {hasChoices && (
          <span className="codicon codicon-sm codicon-chevron-down app-select-chevron"
            aria-hidden="true" />
        )}
      </button>
    </span>
    {popup === null ? null : typeof document === "undefined"
      ? popup : createPortal(popup, document.body)}
  </>;
}
