/**
 * Whether a scroll event can have moved `anchor`: the document, the visual
 * viewport or one of the anchor's own scrolling ancestors. A scroll anywhere
 * else (a streaming transcript beside the composer, another pane, the
 * sidebar) leaves the anchor where it was, so an open menu or hover card
 * measured against it is still true.
 */
export function scrollMovesAnchor(target: EventTarget | null, anchor: Element | null): boolean {
  if (anchor === null || target === null) return true;
  const contains = (target as { contains?: unknown }).contains;
  if (typeof contains !== "function") return true;
  return (target as Node).contains(anchor);
}
