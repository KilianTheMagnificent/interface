import { useLayoutEffect, useRef, useState, type CSSProperties } from "react";
import {
  CORNER_NOTICE_INSET,
  CORNER_NOTICE_LIMIT,
  cornerLift,
  foldCornerNotices,
  orderNewestFirst,
} from "../corner-notices";

/** One notice in the bottom-right stack. */
export interface CornerNoticeItem {
  /** Stable while the notice is the same message; a new id restarts its countdown. */
  id: string;
  /** A short bold lead before the message, for a notice that explains
   *  rather than reports (the first-run privacy summary). */
  title?: string;
  /** What happened, in plain words. */
  message: string;
  /** The raw text behind it, as the tooltip. */
  detail?: string;
  /** An error is an alert that leaves by itself (`onExpire`); a status waits
   *  for Dismiss. */
  tone: "error" | "status";
  onExpire?: () => void;
  /** Present only where a retry exists (a refresh re-runs). */
  onRetry?: () => void;
  /** One more action beside Dismiss, named by its own words. */
  action?: { label: string; onClick: () => void };
  /** The Dismiss button's accessible name; it reads `dismissText`. */
  dismissLabel: string;
  /** What the Dismiss button reads; "Dismiss" when absent. */
  dismissText?: string;
  onDismiss: () => void;
}

/** What the stack must never cover: a composer with its send button, and a
 *  pane's own bottom-right overlay. */
export const CORNER_NOTICE_OBSTACLES = ".thread-composer, .conv-composer, .terminal-startup-download";

/**
 * The main area's notices, a small card each in its bottom-right corner
 * (owner, 2026-11-15; a full-width bar across the top before). Newest first,
 * three at most with "N more". It sits above any composer it would cover.
 */
export function CornerNotices({ items, inert = false }: {
  items: readonly CornerNoticeItem[];
  /** The composer or Settings is over the main area: the stack stays under it. */
  inert?: boolean;
}) {
  const stackRef = useRef<HTMLElement>(null);
  const seen = useRef<{ map: ReadonlyMap<string, number>; next: number }>({ map: new Map(), next: 0 });
  const [expanded, setExpanded] = useState(false);
  const [lift, setLift] = useState(CORNER_NOTICE_INSET);

  const ids = items.map(item => item.id);
  const ordered = orderNewestFirst(ids, seen.current.map, seen.current.next);
  seen.current = { map: ordered.seen, next: ordered.next };
  const byId = new Map(items.map(item => [item.id, item]));
  const { shown, folded } = foldCornerNotices(ordered.order.map(id => byId.get(id)!), expanded);
  const present = items.length > 0;
  if (!present && expanded) setExpanded(false);

  // Measured only while something shows: rises above a composer that the
  // corner would cover, and follows it as it grows or the area resizes.
  useLayoutEffect(() => {
    const stack = stackRef.current;
    const area = stack?.parentElement;
    if (!present || stack === null || area === null || area === undefined) return;
    let frame = 0;
    const observed = new Set<Element>();
    const sizes = new ResizeObserver(() => { schedule(); });
    const measure = (): void => {
      frame = 0;
      const obstacles = Array.from(area.querySelectorAll(CORNER_NOTICE_OBSTACLES))
        .filter(node => !stack.contains(node) && node.getClientRects().length > 0);
      for (const node of obstacles) if (!observed.has(node)) { observed.add(node); sizes.observe(node); }
      const next = cornerLift(area.getBoundingClientRect(), stack.offsetWidth,
        obstacles.map(node => node.getBoundingClientRect()));
      setLift(current => current === next ? current : next);
    };
    function schedule(): void { if (frame === 0) frame = window.requestAnimationFrame(measure); }
    sizes.observe(area);
    sizes.observe(stack);
    // A composer that mounts or leaves; text inside panes is not looked at.
    const tree = new MutationObserver(records => {
      const touches = (node: Node): boolean => node instanceof Element
        && (node.matches(CORNER_NOTICE_OBSTACLES) || node.querySelector(CORNER_NOTICE_OBSTACLES) !== null);
      if (records.some(record => [...record.addedNodes, ...record.removedNodes].some(touches))) schedule();
    });
    tree.observe(area, { childList: true, subtree: true });
    measure();
    return () => {
      if (frame !== 0) window.cancelAnimationFrame(frame);
      sizes.disconnect();
      tree.disconnect();
    };
  }, [present]);

  if (!present) return null;
  const notice = (item: CornerNoticeItem, isFolded: boolean) => (
    <div key={item.id} className={`corner-notice is-${item.tone}${isFolded ? " is-folded" : ""}`}
      role={item.tone === "error" ? "alert" : "status"}
      data-corner-notice={item.id}
      title={item.detail !== undefined && item.detail !== item.message ? item.detail : undefined}
      inert={isFolded ? true : undefined}
      onAnimationEnd={event => {
        if (event.target === event.currentTarget && item.tone === "error") item.onExpire?.();
      }}>
      <span className="corner-notice-dot" aria-hidden="true" />
      <span className="corner-notice-text">
        {item.title !== undefined && <><strong>{item.title}</strong>{" "}</>}{item.message}
      </span>
      {item.onRetry !== undefined && (
        <button type="button" className="corner-notice-action" onClick={item.onRetry}>Try again</button>
      )}
      {item.action !== undefined && (
        <button type="button" className="corner-notice-action" onClick={item.action.onClick}>{item.action.label}</button>
      )}
      <button type="button" className="corner-notice-action corner-notice-dismiss"
        aria-label={item.dismissLabel} onClick={item.onDismiss}>
        {item.dismissText ?? "Dismiss"}
      </button>
    </div>
  );
  return (
    <section ref={stackRef} className="corner-notices" aria-label="Notices"
      style={{ "--corner-notice-lift": `${lift}px` } as CSSProperties}
      inert={inert ? true : undefined} aria-hidden={inert || undefined}>
      {/* One keyed list, so folding or unfolding a notice never remounts it
          (a remount would restart its countdown and announce it again).
          Folded notices stay mounted, unseen, so each still leaves on time. */}
      {[...shown, ...folded].map(item => notice(item, folded.includes(item)))}
      {items.length > CORNER_NOTICE_LIMIT && (
        <button type="button" className="corner-notice-more" aria-expanded={expanded}
          onClick={() => { setExpanded(open => !open); }}>
          {expanded ? "Show fewer" : `${folded.length} more`}
        </button>
      )}
    </section>
  );
}
