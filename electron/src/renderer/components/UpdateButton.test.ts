import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { UpdateButton } from "./UpdateButton";

const draw = (phase: "available" | "downloading" | "ready", progress: number | null, label: string) =>
  renderToStaticMarkup(createElement(UpdateButton, {
    state: { phase, progress }, label, title: `${label} — Interface 0.3.148`, onClick: () => {},
  }));

describe("UpdateButton (owner, 2026-10-01, design D4)", () => {
  it("says Update beside the tile while one is available", () => {
    const html = draw("available", null, "Update available");
    expect(html).toContain('class="workspace-toolbar-btn workspace-toolbar-update is-available"');
    expect(html).toContain('aria-label="Update available"');
    expect(html).toContain('<span class="workspace-update-tile" aria-hidden="true">');
    expect(html).toContain('<span class="workspace-update-words">Update</span>');
  });

  it("turns the tile into a ring that fills with the download", () => {
    const html = draw("downloading", 41.6, "Downloading 42%");
    expect(html).toContain('class="workspace-update-ring" style="--update-progress:42%"');
    expect(html).toContain(">Downloading 42%</span>");
    expect(html).not.toContain("workspace-update-tile");
    // No percent yet: the waiting ring, never a ring stuck at zero.
    expect(draw("downloading", null, "Downloading update"))
      .toContain('class="workspace-update-ring is-waiting"');
  });

  it("marks ready so the dot pops and the words drift", () => {
    const html = draw("ready", null, "Restart to update");
    expect(html).toContain("workspace-toolbar-update is-ready");
    expect(html).toContain(">Restart to update</span>");
  });
});
