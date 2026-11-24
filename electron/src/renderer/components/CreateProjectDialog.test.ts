import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { ProjectsFolderView } from "../../shared/projects";
import { CreateProjectDialog, createProjectPathLine } from "./CreateProjectDialog";

const chosen: ProjectsFolderView = {
  version: 1, path: "/Users/me/code", display: "~/code", source: "chosen",
};

describe("New project's Projects folder", () => {
  it("names the folder Rust resolved, then the project's own folder", () => {
    expect(createProjectPathLine(chosen, "  web ")).toBe("~/code/web");
    expect(createProjectPathLine(chosen, "")).toBe("~/code/project-name");
    // Never a guessed path while Rust has not answered.
    expect(createProjectPathLine(null, "web")).toBe("Projects folder/web");
  });

  it("offers Change… beside the path only when the folder can be changed", () => {
    const props = { onCreate: async () => undefined, onCancel: () => undefined, folder: chosen };
    const withChange = renderToStaticMarkup(createElement(CreateProjectDialog,
      { ...props, onChangeFolder: async () => undefined }));
    expect(withChange).toContain("~/code/project-name");
    expect(withChange).toContain(">Change…</button>");
    expect(withChange).toContain('title="/Users/me/code"');
    expect(withChange).not.toContain("~/Developer/Projects");
    expect(renderToStaticMarkup(createElement(CreateProjectDialog, props))).not.toContain("Change…");
  });
});
