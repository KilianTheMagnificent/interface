import { readFileSync } from "node:fs";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { OutputPreviewResponse } from "../../shared/output-preview";
import { OutputPreviewPanel } from "./OutputPreviewPanel";

const source = readFileSync(new URL("./OutputPreviewPanel.tsx", import.meta.url), "utf8");
const css = readFileSync(new URL("./OutputPreviewPanel.css", import.meta.url), "utf8");
const rule = (selector: string): string => {
  const start = css.indexOf(`${selector} {`);
  expect(start, selector).toBeGreaterThanOrEqual(0);
  return css.slice(start, css.indexOf("}", start));
};

describe("output preview chrome", () => {
  it("draws the panes' head band, ghost actions and the app's own pickers", () => {
    // The head is the tiles' band: same height and fill, so it lines up with
    // the pane heads beside it; the column itself is the agent overview's.
    expect(rule(".output-preview-head")).toContain("height: var(--tile-head-h)");
    expect(rule(".output-preview-head")).toContain("background: var(--terminal-header-bg)");
    expect(rule(".output-preview")).toContain("background: var(--workbench-panel)");
    // No blanket outlined-box rule over every button in the panel.
    expect(css).not.toMatch(/\.output-preview button:not\(/u);
    // Version and feedback thread use AppSelect, not platform menus.
    expect(source).not.toContain("<select");
    expect(source.match(/<AppSelect compact/gu)?.length).toBe(2);

    const html = renderToStaticMarkup(createElement(OutputPreviewPanel, {
      root: "/repo/project", visible: true, api: { previewOutput: () => new Promise<OutputPreviewResponse>(() => {}) },
      onClose: () => {}, onOpenFolder: () => {},
    }));
    const head = /<header class="output-preview-head">(.*?)<\/header>/su.exec(html)?.[1] ?? "";
    expect(head).toContain("<strong>Outputs</strong>");
    expect(head).toContain('class="terminal-center-action is-active"');
    expect(head).toContain('aria-label="Close output preview"');
    expect(html).toContain('aria-label="Refresh output files"><span class="codicon codicon-refresh"');
  });
});
