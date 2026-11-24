import { describe, expect, it } from "vitest";
import { isPromptImagePreviewUrl, PROMPT_IMAGE_LIMITS } from "./prompt-images";

describe("preview URL boundary", () => {
  it("admits a bounded base64 data image", () => {
    expect(isPromptImagePreviewUrl("data:image/png;base64,iVBORw0KGgo=")).toBe(true);
    expect(isPromptImagePreviewUrl("data:image/jpeg;base64,/9j/4AAQ")).toBe(true);
    expect(isPromptImagePreviewUrl("data:image/webp;base64,UklGRg==")).toBe(true);
  });

  it("refuses every source that is not bounded inline image bytes", () => {
    expect(isPromptImagePreviewUrl("https://example.com/shot.png")).toBe(false);
    expect(isPromptImagePreviewUrl("javascript:alert(1)")).toBe(false);
    expect(isPromptImagePreviewUrl("data:text/html;base64,PGgxPg==")).toBe(false);
    // SVG is a script-bearing document format and is deliberately not a thumbnail.
    expect(isPromptImagePreviewUrl("data:image/svg+xml;base64,PHN2Zz4=")).toBe(false);
    expect(isPromptImagePreviewUrl("data:image/png,notbase64")).toBe(false);
    expect(isPromptImagePreviewUrl("data:image/png;base64,has spaces")).toBe(false);
    expect(isPromptImagePreviewUrl("data:image/png;base64,")).toBe(false);
    expect(isPromptImagePreviewUrl(42)).toBe(false);
    expect(isPromptImagePreviewUrl(undefined)).toBe(false);
  });

  it("refuses a preview past the length ceiling rather than truncating it", () => {
    const body = "A".repeat(PROMPT_IMAGE_LIMITS.maxPreviewChars);
    expect(isPromptImagePreviewUrl(`data:image/png;base64,${body}`)).toBe(false);
  });
});
