import { useLayoutEffect, type RefObject } from "react";

/** Clip scrolling rows below their sticky project label. This lets the header
 * share the sidebar's material without another tint layer or text bleeding
 * through. Geometry stays presentational; no React state updates on scroll.
 * Only translucent sidebars read the clip (translucency.css), so a solid one
 * measures nothing.
 *
 * `layoutKey` names the list's structure: which groups are drawn, where, and
 * whether each is open. The measured nodes are rebound when it changes, not
 * on every render of the pane, which streaming agents cause many times a
 * second. Between rebinds the clips follow scrolling, and a ResizeObserver
 * follows every size change that can move rows under a pinned label. */
export function useSidebarHeaderClipping(aside: RefObject<HTMLElement | null>, enabled: boolean,
    layoutKey: string): void {
  useLayoutEffect(() => {
    const node = aside.current;
    if (!node || !enabled) return;
    const pairs = [...node.querySelectorAll<HTMLElement>(".group")].flatMap(group => {
      const head = group.querySelector<HTMLElement>(":scope > .group-header-row");
      const rows = group.querySelector<HTMLElement>(":scope > .session-items");
      return head && rows ? [{ head, rows }] : [];
    });
    const update = (): void => {
      // Read all bounds before writing styles, avoiding per-group layout churn.
      const clips = pairs.map(({ head, rows }) =>
        `${Math.max(0, head.getBoundingClientRect().bottom - rows.getBoundingClientRect().top)}px`);
      pairs.forEach(({ rows }, i) => {
        if (rows.style.getPropertyValue("--sticky-header-clip") !== clips[i]) {
          rows.style.setProperty("--sticky-header-clip", clips[i]!);
        }
      });
    };
    update();
    node.addEventListener("scroll", update, true);
    const resize = new ResizeObserver(update);
    resize.observe(node);
    for (const { head, rows } of pairs) { resize.observe(head); resize.observe(rows); }
    // A group that grows (an archive confirmation under its label) or a
    // pinned section that gains a row moves every row list after it.
    for (const child of node.querySelector(".session-list")?.children ?? []) resize.observe(child);
    return () => {
      node.removeEventListener("scroll", update, true);
      resize.disconnect();
    };
  }, [aside, enabled, layoutKey]);
  useLayoutEffect(() => {
    if (enabled) return;
    for (const rows of aside.current?.querySelectorAll<HTMLElement>(".group > .session-items") ?? []) {
      rows.style.removeProperty("--sticky-header-clip");
    }
  }, [aside, enabled]);
}
