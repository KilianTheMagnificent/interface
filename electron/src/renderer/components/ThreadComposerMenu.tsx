import { useCallback, useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import type { KeyboardEvent as ReactKeyboardEvent, ReactNode } from "react";
import { threadMenuKeyAction, threadMenuTriggerKeyAction } from "../thread-composer";
import { focusPastTrigger } from "./ModalBehavior";
import { ThreadGlyph } from "./ThreadGlyph";

/**
 * THE THREADS COMPOSER'S "+" MENU: every control that used to lead the
 * composer's control row — the slash-command launcher, prompt context, the
 * transcript-detail dial and the session-context meter — behind one quiet
 * button, so the row itself carries only the run settings and the verbs.
 *
 * A real ARIA menu with roving focus, the context picker's keyboard: arrows
 * move and wrap over every row (a disabled row stays focusable so its reason
 * can be read), Home/End jump, Enter/Space are the row's own click, Escape
 * closes and hands focus back to the button, Tab leaves for the control after
 * the button. "Add context" drills into the context kinds in place (→ in,
 * ← or Escape back out) rather than stacking a second popover on the first.
 */

export interface ThreadMenuRow {
  key: string;
  label: string;
  /** Overrides the name the visible label and meta make together. */
  ariaLabel?: string;
  /** Short right-hand text: a count, a key, a figure. */
  meta?: string;
  /** The meta in the warning tier (a session window at or past 80%). */
  metaHigh?: boolean;
  /** One quiet line under the label: what the row does, or why it cannot. */
  detail?: string;
  /** Figures drawn under the label, one per line (the session-context readout). */
  figures?: readonly { label: string; value: string }[];
  /** A decorative leading glyph. Radio rows draw their check here instead. */
  glyph?: ReactNode;
  disabled?: boolean;
  /** Present on a radio row: whether it is the chosen one. */
  checked?: boolean;
  /** The key of the view this row drills into. */
  opens?: string;
  onChoose?: () => void;
}

export interface ThreadMenuSection {
  key: string;
  /** Visible group heading; also the group's accessible name. */
  heading?: string;
  rows: readonly ThreadMenuRow[];
}

export interface ThreadMenuView {
  /** The drilled-in menu's name, and the label of the row that leads back. */
  title: string;
  /** The back row's meta, e.g. "2 of 8". */
  meta?: string;
  sections: readonly ThreadMenuSection[];
}

interface DrawnRow { row: ThreadMenuRow; back: boolean }

export function ThreadComposerMenu({ label, sections, views }: {
  /** The button's name and the top menu's. */
  label: string;
  sections: readonly ThreadMenuSection[];
  views?: Readonly<Record<string, ThreadMenuView>>;
}) {
  const uid = useId();
  const menuId = `${uid}-menu`;
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const rowRefs = useRef<(HTMLButtonElement | null)[]>([]);
  const [open, setOpen] = useState(false);
  const [viewKey, setViewKey] = useState<string | null>(null);
  const [active, setActive] = useState(0);
  // Bumped whenever a move should land DOM focus on the active row; a row
  // that took focus by itself (a click) needs no second focus() call.
  const [focusAsk, setFocusAsk] = useState(0);

  const view = viewKey === null ? undefined : views?.[viewKey];
  const drawnSections: readonly ThreadMenuSection[] = view === undefined ? sections : [
    { key: "back", rows: [{
      key: "back", label: view.title, meta: view.meta, ariaLabel: `Back from ${view.title}`,
      glyph: <ThreadGlyph name="back" size={14} />,
    }] },
    ...view.sections,
  ];
  const rows: DrawnRow[] = drawnSections.flatMap(section => section.rows.map(row => ({
    row, back: view !== undefined && section.key === "back",
  })));

  const firstEnabled = (list: readonly DrawnRow[], from = 0): number => {
    for (let index = from; index < list.length; index += 1) {
      if (list[index].row.disabled !== true) return index;
    }
    return 0;
  };

  const close = useCallback((restoreFocus: boolean): void => {
    setOpen(false);
    setViewKey(null);
    if (restoreFocus) triggerRef.current?.focus();
  }, []);

  const openAt = (where: "first" | "last"): void => {
    const list = sections.flatMap(section => section.rows.map(row => ({ row, back: false })));
    setViewKey(null);
    setActive(where === "last" ? Math.max(0, list.length - 1) : firstEnabled(list));
    setOpen(true);
    setFocusAsk(ask => ask + 1);
  };

  const enter = (key: string): void => {
    const next = views?.[key];
    if (next === undefined) return;
    setViewKey(key);
    // Land on the first thing to pick, past the row that leads back.
    const list = next.sections.flatMap(section => section.rows);
    let at = 0;
    while (at < list.length && list[at].disabled === true) at += 1;
    setActive(1 + (at < list.length ? at : 0));
    setFocusAsk(ask => ask + 1);
  };

  const leaveView = (): void => {
    const opener = sections.flatMap(section => section.rows)
      .findIndex(row => row.opens === viewKey);
    setViewKey(null);
    setActive(Math.max(0, opener));
    setFocusAsk(ask => ask + 1);
  };

  const choose = (drawn: DrawnRow): void => {
    if (drawn.back) { leaveView(); return; }
    const { row } = drawn;
    if (row.disabled === true) return;
    if (row.opens !== undefined) { enter(row.opens); return; }
    // Focus goes home BEFORE the row acts: a chosen context kind can open a
    // modal (Quick Open, the URL dialog), and a deferred restore would pull
    // focus back out of it.
    triggerRef.current?.focus();
    close(false);
    row.onChoose?.();
  };

  useLayoutEffect(() => {
    if (!open || focusAsk === 0) return;
    rowRefs.current[active]?.focus();
  // `active` is read at the ask, not followed: a row focused by the pointer
  // reports itself through onFocus and must not be focused a second time.
  }, [open, focusAsk, viewKey]);

  useEffect(() => {
    if (!open) return;
    const outside = (event: PointerEvent): void => {
      if (rootRef.current !== null && event.composedPath().includes(rootRef.current)) return;
      close(false);
    };
    document.addEventListener("pointerdown", outside, true);
    return () => { document.removeEventListener("pointerdown", outside, true); };
  }, [close, open]);

  const onMenuKey = (event: ReactKeyboardEvent<HTMLDivElement>): void => {
    const current = rows[active];
    const action = threadMenuKeyAction(rows.length, active, event.key, {
      nested: view !== undefined,
      opens: current !== undefined && current.row.opens !== undefined
        && current.row.disabled !== true,
    });
    if (action === null) return;
    event.stopPropagation();
    if (action.type === "leave") {
      event.preventDefault();
      const target = focusPastTrigger(triggerRef.current, event.shiftKey, menuRef.current);
      close(false);
      queueMicrotask(() => { (target ?? triggerRef.current)?.focus(); });
      return;
    }
    event.preventDefault();
    if (action.type === "close") close(true);
    else if (action.type === "back") leaveView();
    else if (action.type === "open") { if (current?.row.opens !== undefined) enter(current.row.opens); }
    else {
      setActive(action.index);
      setFocusAsk(ask => ask + 1);
    }
  };

  rowRefs.current.length = rows.length;
  let index = -1;
  const menu = open ? (
    <div ref={menuRef} id={menuId} role="menu"
      aria-label={view === undefined ? label : view.title}
      className="thread-composer-menu is-floating"
      onKeyDown={onMenuKey}>
      {drawnSections.map((section, sectionIndex) => {
        const headingId = `${uid}-${section.key}-heading`;
        const body = section.rows.map(row => {
          index += 1;
          const at = index;
          const drawn = rows[at];
          const radio = row.checked !== undefined;
          const labelId = `${uid}-row-${at}-label`;
          const metaId = `${uid}-row-${at}-meta`;
          const detailId = `${uid}-row-${at}-detail`;
          const describedBy = row.detail !== undefined || row.figures !== undefined
            ? detailId : undefined;
          return (
            <button key={row.key} ref={node => { rowRefs.current[at] = node; }}
              type="button" role={radio ? "menuitemradio" : "menuitem"}
              className={row.disabled === true
                ? "thread-composer-menu-row is-disabled" : "thread-composer-menu-row"}
              tabIndex={at === active ? 0 : -1}
              aria-label={row.ariaLabel}
              aria-labelledby={row.ariaLabel === undefined
                ? [labelId, row.meta !== undefined ? metaId : null].filter(Boolean).join(" ")
                : undefined}
              aria-describedby={describedBy}
              aria-disabled={row.disabled === true || undefined}
              aria-checked={radio ? row.checked : undefined}
              aria-haspopup={row.opens !== undefined ? "menu" : undefined}
              onFocus={() => { setActive(at); }}
              onClick={() => { choose(drawn); }}>
              <span className="thread-composer-menu-glyph" aria-hidden="true">
                {radio ? (row.checked === true ? <ThreadGlyph name="pass" size={14} /> : null) : row.glyph}
              </span>
              <span className="thread-composer-menu-copy">
                <span id={labelId} className="thread-composer-menu-label">{row.label}</span>
                {row.detail !== undefined && row.figures === undefined && (
                  <span id={detailId} className="thread-composer-menu-detail">{row.detail}</span>
                )}
                {row.figures !== undefined && (
                  <span id={detailId} className="thread-composer-menu-figures">
                    {row.figures.map(figure => (
                      <span key={figure.label} className="thread-composer-menu-figure">
                        <span>{figure.label}</span>
                        <span className="thread-composer-menu-figure-value">{figure.value}</span>
                      </span>
                    ))}
                  </span>
                )}
              </span>
              {row.meta !== undefined && (
                <span id={metaId} className={row.metaHigh === true
                  ? "thread-composer-menu-meta is-high" : "thread-composer-menu-meta"}>
                  {row.meta}
                </span>
              )}
              {row.opens !== undefined && (
                <span className="thread-composer-menu-more" aria-hidden="true">
                  <ThreadGlyph name="open" size={14} />
                </span>
              )}
            </button>
          );
        });
        return (
          <div key={section.key} role="none" className="thread-composer-menu-section">
            {sectionIndex > 0 && (
              <div role="separator" className="thread-composer-menu-sep" />
            )}
            {section.heading !== undefined ? (
              <div role="group" aria-labelledby={headingId}>
                <div id={headingId} role="none" className="thread-composer-menu-heading">
                  {section.heading}
                </div>
                {body}
              </div>
            ) : body}
          </div>
        );
      })}
    </div>
  ) : null;

  return (
    <div ref={rootRef} className="thread-composer-plus"
      onBlur={event => {
        if (!open) return;
        const next = event.relatedTarget;
        if (next instanceof Node && rootRef.current?.contains(next) === true) return;
        close(false);
      }}>
      <button ref={triggerRef} type="button"
        className="conv-iconbtn thread-composer-icon thread-composer-plus-button"
        aria-label={label} title={label}
        aria-haspopup="menu" aria-expanded={open}
        aria-controls={open ? menuId : undefined}
        onClick={() => { if (open) close(false); else openAt("first"); }}
        onKeyDown={event => {
          if (event.key === "Escape" && open) {
            event.preventDefault();
            event.stopPropagation();
            close(true);
            return;
          }
          const where = threadMenuTriggerKeyAction(event.key);
          if (where === null) return;
          event.preventDefault();
          openAt(where);
        }}>
        <ThreadGlyph name="add" />
      </button>
      {menu}
    </div>
  );
}
