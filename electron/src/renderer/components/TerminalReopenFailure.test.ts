import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { TerminalReopenFailure } from "./TerminalReopenFailure";

describe("TerminalReopenFailure", () => {
  it("says why a saved pane did not come back instead of leaving it blank", () => {
    const html = renderToStaticMarkup(createElement(TerminalReopenFailure, {
      message: "Could not open a terminal.\nThat workspace is missing, linked, or no longer a stable directory.",
    }));
    expect(html).toContain("This terminal did not reopen");
    expect(html).toContain("That workspace is missing, linked, or no longer a stable directory.");
    expect(html).not.toContain("<button");
  });

  it("offers to reopen a pane whose folder moved in the folder it moved to", () => {
    const html = renderToStaticMarkup(createElement(TerminalReopenFailure, {
      message: "Could not open a terminal.",
      moved: { from: "/Users/me/Desktop/Projects/app", to: "/Users/me/Developer/app" },
      onReopenMoved: () => undefined,
    }));
    expect(html).toContain("~/Desktop/Projects/app");
    expect(html).toContain('title="/Users/me/Developer/app"');
    expect(html).toMatch(/<button[^>]*type="button"[^>]*>Reopen in ~\/Developer\/app<\/button>/u);
  });
});
