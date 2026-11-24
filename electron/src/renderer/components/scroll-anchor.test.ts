import { describe, expect, it } from "vitest";
import { scrollMovesAnchor } from "./scroll-anchor";

function node(children: object[] = []): Node {
  const self = {
    contains: (other: unknown): boolean => other === self
      || children.some(child => (child as Node).contains(other as Node)),
  };
  return self as unknown as Node;
}

describe("scrollMovesAnchor", () => {
  it("counts only scrolls of the anchor's ancestors and the viewport", () => {
    const trigger = node();
    const composer = node([trigger]);
    const transcript = node();
    expect(scrollMovesAnchor(composer, trigger as unknown as Element)).toBe(true);
    expect(scrollMovesAnchor(transcript, trigger as unknown as Element)).toBe(false);
    // The visual viewport and window are not nodes: they always move it.
    expect(scrollMovesAnchor({} as EventTarget, trigger as unknown as Element)).toBe(true);
    expect(scrollMovesAnchor(transcript, null)).toBe(true);
  });
});
