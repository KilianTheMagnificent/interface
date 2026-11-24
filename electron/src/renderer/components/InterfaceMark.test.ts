import { readFileSync } from "node:fs";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { InterfaceMark } from "./InterfaceMark";

// The launcher retains the Fold; the window header uses a quiet wordmark.
const sources = [
  new URL("./TerminalLauncher.tsx", import.meta.url),
].map(url => readFileSync(url, "utf8"));

describe("Interface mark", () => {
  it("renders the shared Fold artwork as a current-color inline SVG", () => {
    const html = renderToStaticMarkup(createElement(InterfaceMark, { className: "brand" }));
    expect(html).toContain('data-interface-mark="fold"');
    expect(html).toContain('stroke="currentColor"');
    expect(html.match(/<path /g)).toHaveLength(2);
    // On its plate: the app icon in miniature, not a bare stroke that reads as ».
    expect(html).toContain('class="interface-mark brand"');
    expect(html).toContain('viewBox="100 100 824 824"');
    expect(html).toContain('<rect class="interface-mark-tile" x="100" y="100" width="824" height="824" rx="186">');
  });

  it("replaces every visible IF placeholder with the shared mark", () => {
    for (const source of sources) {
      expect(source).not.toMatch(/>IF<\/span>/u);
      expect(source).toContain("<InterfaceMark");
    }
  });
});
