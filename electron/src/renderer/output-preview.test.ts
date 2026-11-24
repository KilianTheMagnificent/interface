import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import type { OutputPreviewRequest, OutputPreviewResponse, OutputPreviewSnapshot } from "../shared/output-preview";
import { CHANNELS } from "../shared/ipc";
import { encodeRendererPayload } from "../main/ipc-payload";
import {
  attachmentPreviewTarget, inlineImageSnapshot, inlineTextSnapshot,
  outputPreviewTarget, outputFeedback, outputImageUrl, outputKey, outputRegion, outputTextAnchor,
} from "./output-preview";

describe("output previews", () => {
  it("opens a thread output only inside its own worktree", () => {
    expect(outputPreviewTarget("report.md", "/project/worktree")).toEqual({ root: "/project/worktree", path: "report.md" });
    expect(outputPreviewTarget("/project/worktree/reports/brief.pdf", "/project/worktree/"))
      .toEqual({ root: "/project/worktree", path: "reports/brief.pdf" });
    for (const path of ["../secret.md", "/project/worktree-other/report.md", "report.ts", "folder/../report.md"])
      expect(outputPreviewTarget(path, "/project/worktree")).toBeNull();
  });
  it("resolves in-project and external attachment preview targets", () => {
    expect(attachmentPreviewTarget("src/main.rs", "/project"))
      .toEqual({ root: "/project", path: "src/main.rs" });
    expect(attachmentPreviewTarget("/project/docs/spec.pdf", "/project/"))
      .toEqual({ root: "/project", path: "docs/spec.pdf" });
    expect(attachmentPreviewTarget("/Users/me/Desktop/design.pdf", "/project"))
      .toEqual({ root: "/Users/me/Desktop", path: "design.pdf" });
    expect(attachmentPreviewTarget("/tmp/Interface/pasted-images/a.png", ""))
      .toEqual({ root: "/tmp/Interface/pasted-images", path: "a.png" });
    for (const path of ["../secret.txt", "/Users/me/../secret.txt", "relative.txt", "/"])
      expect(attachmentPreviewTarget(path, path === "relative.txt" ? "" : "/project")).toBeNull();
    expect(inlineImageSnapshot("/attachments", "shot.png", "data:image/png;base64,AQID")).toEqual({
      root: "/attachments", path: "shot.png", byteLength: 3, revision: null,
      content: { kind: "image", mediaType: "image/png", bytes: [1, 2, 3] },
    });
    expect(inlineImageSnapshot("/attachments", "bad.svg", "data:image/svg+xml;base64,PHN2Zz4=")).toBeNull();
    expect(inlineTextSnapshot("/repo", "Working tree diff.diff", "+added", "diff")).toEqual({
      root: "/repo", path: "Working tree diff.diff", byteLength: 6, revision: null,
      content: { kind: "text", format: "diff", text: "+added" },
    });
  });
  it("translates the shared Rust requests without granting paths or changing wire bytes", () => {
    const fixture = JSON.parse(readFileSync(new URL("../fixtures/output-preview-v1.json", import.meta.url), "utf8")) as {
      requests: OutputPreviewRequest[]; responses: OutputPreviewResponse[];
    };
    for (const request of fixture.requests) expect(encodeRendererPayload(CHANNELS.previewOutput, [request])).toEqual(request);
    expect(fixture.responses.map(response => response.kind === "file" ? response.snapshot.content.kind : response.kind))
      .toEqual(["directory", "text", "image", "pdf", "unsupported"]);
  });
  it("keeps feedback attached to the reviewed hash and quotes paths and selected text", () => {
    const snapshot: OutputPreviewSnapshot = { root: "/project", path: "report.md", byteLength: 20,
      revision: "first-revision", content: { kind: "text", format: "markdown", text: "original paragraph" } };
    const feedback = outputFeedback(snapshot, { kind: "text", quote: "original paragraph", startLine: 1, endLine: 1 }, "Make this clearer.");
    expect(feedback).toContain('"/project/report.md"');
    expect(feedback).toContain("SHA-256): first-revision");
    expect(feedback).toContain("reconcile this feedback");
    expect(feedback).toContain("Line 1");
    expect(feedback).toContain('Selected text: "original paragraph"');
    expect(feedback).toContain("Make this clearer.");
  });
  it("claims line numbers only for a unique literal text selection", () => {
    expect(outputTextAnchor("first\nsecond\nthird", "second\nthird"))
      .toEqual({ kind: "text", quote: "second\nthird", startLine: 2, endLine: 3 });
    expect(outputTextAnchor("same\nsame", "same")).toEqual({ kind: "text", quote: "same" });
    expect(outputTextAnchor("**bold** text", "bold text")).toEqual({ kind: "text", quote: "bold text" });
    expect(outputTextAnchor("", "   ")).toBeNull();
    expect(outputTextAnchor("x".repeat(9000), "x".repeat(9000))).toEqual({ kind: "text", quote: "x".repeat(2000) });
  });
  it("scopes tabs to full project identities and clamps reverse image drags", () => {
    expect(outputKey("/a", "b/c")).not.toBe(outputKey("/a/b", "c"));
    expect(outputRegion({ x: 1.2, y: .8 }, { x: -.1, y: .2 }))
      .toEqual({ kind: "region", x: 0, y: .2, width: 1, height: .6 });
    expect(outputImageUrl("image/svg+xml", [60, 62])).toBe("");
    expect(outputImageUrl("image/png", [1, 2, 3])).toBe("data:image/png;base64,AQID");
  });
});
