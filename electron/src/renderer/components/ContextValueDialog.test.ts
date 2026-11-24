import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import {
  CONTEXT_VALUE_DIALOG_CSS,
  ContextValueDialog,
  isValidContextUrl,
  type ContextValueDialogProps,
} from "./ContextValueDialog";

const render = (over: Partial<ContextValueDialogProps> = {}): string =>
  renderToStaticMarkup(createElement(ContextValueDialog, {
    open: true,
    onSubmit: () => undefined,
    onCancel: () => undefined,
    ...over,
  }));

describe("context URL validation", () => {
  it("accepts complete HTTP(S) URLs and trims surrounding whitespace", () => {
    expect(isValidContextUrl("https://example.com/path?q=one")).toBe(true);
    expect(isValidContextUrl("  http://localhost:3000/docs  ")).toBe(true);
    expect(isValidContextUrl("HTTPS://EXAMPLE.COM")).toBe(true);
  });

  it("rejects empty, relative, incomplete, and non-web URLs", () => {
    for (const value of ["", "   ", "/docs", "example.com", "https://", "file:///tmp/a",
      "javascript:alert(1)", "mailto:hello@example.com",
      "https://user:pass@example.com/private", "https://user@example.com/private"]) {
      expect(isValidContextUrl(value), value).toBe(false);
    }
  });
});

describe("dialog keyboard containment", () => {
  it("uses the shared modal focus trap", () => {
    const source = readFileSync(new URL("./ContextValueDialog.tsx", import.meta.url), "utf8");
    expect(source).toContain("useModalFocus({");
    expect(source).toContain("onEscape: onCancel");
    expect(source).not.toContain("contextValueDialogKeyAction");
    expect(source).not.toContain("enabledControls");
  });
});

describe("ContextValueDialog markup", () => {
  it("renders a labelled modal URL form with an initially disabled submit", () => {
    const html = render();
    expect(html).toContain('role="dialog"');
    expect(html).toContain('aria-modal="true"');
    expect(html).toMatch(/aria-labelledby="([^"]+)-title"/);
    expect(html).toMatch(/id="([^"]+)-title"[^>]*>Add URL context</);
    expect(html).toContain('type="url"');
    expect(html.toLowerCase()).toContain('inputmode="url"');
    expect(html).toContain("Web address");
    expect(html).toContain("https://example.com/page");
    expect(html).toContain("codicon-link-external");
    expect(html).toMatch(/<button[^>]*type="submit"[^>]*disabled=""[^>]*>Add URL<\/button>/);
  });

  it("renders nothing while closed", () => {
    expect(render({ open: false })).toBe("");
  });

  it("uses app overlay, input, selection, focus, and typography tokens", () => {
    for (const token of ["var(--widget-bg)", "var(--overlay-shadow)", "var(--input-bg)",
      "var(--input-border)", "var(--focus)", "var(--font-ui)"]) {
      expect(CONTEXT_VALUE_DIALOG_CSS).toContain(token);
    }
    expect(CONTEXT_VALUE_DIALOG_CSS).toContain("position:fixed");
    expect(CONTEXT_VALUE_DIALOG_CSS).not.toContain("linear-gradient");
    // The prior-chat picker never had a production mount; its listbox is gone.
    expect(CONTEXT_VALUE_DIALOG_CSS).not.toContain("context-value-listbox");
    expect(CONTEXT_VALUE_DIALOG_CSS).not.toContain("context-value-option");
    const source = ContextValueDialog.toString();
    expect(source).not.toContain("window.prompt");
    expect(source).not.toContain("<select");
  });
});
