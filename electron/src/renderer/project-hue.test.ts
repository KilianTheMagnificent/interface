import { readFileSync } from "node:fs";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { DiffstatBar, FileBadge, ProjectTile } from "./components/ProjectHueParts";
import {
  diffstatBlocks, fileBadgeFor, NO_PROJECT_HUE_CLASS, PROJECT_HUE_COUNT, projectHueClass,
  projectHueIndex, projectInitial,
} from "./project-hue";

describe("a project's hue", () => {
  it("is stable for a folder, whatever its trailing slash or case", () => {
    const index = projectHueIndex("/Users/owner/Developer/Projects/atlas");
    expect(index).not.toBeNull();
    expect(projectHueIndex("/Users/owner/Developer/Projects/atlas/")).toBe(index);
    expect(projectHueIndex("/users/owner/developer/projects/ATLAS")).toBe(index);
    expect(projectHueClass("/Users/owner/Developer/Projects/atlas")).toBe(`project-hue-${index}`);
  });

  it("spreads projects across all eight pairs", () => {
    const seen = new Set<number>();
    for (let n = 0; n < 200; n += 1) seen.add(projectHueIndex(`/w/project-${n}`)!);
    expect(seen.size).toBe(PROJECT_HUE_COUNT);
  });

  it("is the neutral class for No project", () => {
    expect(projectHueIndex(null)).toBeNull();
    expect(projectHueIndex("  ")).toBeNull();
    expect(projectHueClass(undefined)).toBe(NO_PROJECT_HUE_CLASS);
  });

  it("has a light and a dark colour for every pair in the stylesheet", () => {
    const css = readFileSync(new URL("./components/ProjectHue.css", import.meta.url), "utf8");
    for (let index = 0; index < PROJECT_HUE_COUNT; index += 1) {
      expect(css).toMatch(new RegExp(`\\.project-hue-${index} \\{ --project-h: light-dark\\(#[0-9a-f]{6}, #[0-9a-f]{6}\\); --project-h2: light-dark\\(`, "u"));
    }
    expect(css).toContain(".project-hue-none {");
  });
});

describe("a project tile", () => {
  it("shows the name's first letter or digit, upper-cased", () => {
    expect(projectInitial("atlas")).toBe("A");
    expect(projectInitial("_3d-viewer")).toBe("3");
    expect(projectInitial("")).toBe("·");
    const html = renderToStaticMarkup(createElement(ProjectTile, { root: "/w/atlas", name: "atlas" }));
    expect(html).toMatch(/class="project-tile project-hue-\d"/u);
    expect(html).toContain(">A<");
    const chat = renderToStaticMarkup(createElement(ProjectTile, { root: null, name: "No project" }));
    expect(chat).toContain(NO_PROJECT_HUE_CLASS);
    expect(chat).toContain(">·<");
  });
});

describe("file badges and the diffstat bar", () => {
  it("names a file's type by extension, and an unknown one by its first letters", () => {
    expect(fileBadgeFor("src/auth.ts")).toEqual({ label: "TS", kind: "ts" });
    expect(fileBadgeFor("crates/x/src/lib.rs")).toEqual({ label: "RS", kind: "rs" });
    expect(fileBadgeFor("README.md").kind).toBe("md");
    expect(fileBadgeFor("data.parquet")).toEqual({ label: "PAR", kind: "other" });
    expect(fileBadgeFor("Makefile")).toEqual({ label: "·", kind: "other" });
    expect(renderToStaticMarkup(createElement(FileBadge, { path: "a.json" })))
      .toBe('<span class="file-badge kind-json" aria-hidden="true">{ }</span>');
  });

  it("splits five squares by added and removed lines, showing each side that changed", () => {
    expect(diffstatBlocks(26, 1)).toEqual(["added", "added", "added", "added", "removed"]);
    expect(diffstatBlocks(16, 0)).toEqual(["added", "added", "added", "added", "added"]);
    expect(diffstatBlocks(1, 40)).toEqual(["added", "removed", "removed", "removed", "removed"]);
    expect(diffstatBlocks(0, 0)).toEqual(["neutral", "neutral", "neutral", "neutral", "neutral"]);
    const html = renderToStaticMarkup(createElement(DiffstatBar, { added: 3, removed: 2 }));
    expect(html).toBe('<span class="diffstat-bar" aria-hidden="true"><i class="is-added"></i>'
      + '<i class="is-added"></i><i class="is-added"></i><i class="is-removed"></i><i class="is-removed"></i></span>');
  });
});
