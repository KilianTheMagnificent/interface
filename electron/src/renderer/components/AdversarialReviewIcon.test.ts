import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { AdversarialReviewIcon } from "./AdversarialReviewIcon";

describe("AdversarialReviewIcon", () => {
  it("renders an opposed-verdict silhouette as a current-color inline SVG", () => {
    const html = renderToStaticMarkup(createElement(AdversarialReviewIcon, {
      className: "review-mark",
    }));
    expect(html).toContain('class="adversarial-review-icon review-mark"');
    expect(html).toContain('data-adversarial-review-icon="opposed-verdict"');
    expect(html).toContain('viewBox="0 0 24 24"');
    expect(html).toContain('width="16"');
    expect(html).toContain('height="16"');
    expect(html).toContain('aria-hidden="true"');
    expect(html).toContain('focusable="false"');
    expect(html.match(/<path /gu)).toHaveLength(3);
    expect(html.match(/stroke="currentColor"/gu)).toHaveLength(2);
    expect(html).toContain('fill="currentColor"');
    expect(html).not.toContain("<text");
    expect(html).not.toContain("<img");
  });

  it.each([16, 24, 28])("keeps an explicit intrinsic box at %ipx", size => {
    const html = renderToStaticMarkup(createElement(AdversarialReviewIcon, { size }));
    expect(html).toContain(`width="${size}"`);
    expect(html).toContain(`height="${size}"`);
  });
});
