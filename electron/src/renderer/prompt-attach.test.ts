import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import {
  attachedScreenshotPaths, attachmentContextItems,
  draftAfterScreenshotAttach,
  pastedImagePayloads,
  TERMINAL_CURSOR_TO_LINE_END, terminalImagePasteInput,
  terminalPathInsertPrefix,
  withAttachmentThumbnails,
} from "./prompt-attach";
import { PROMPT_IMAGE_LIMITS } from "../shared/prompt-images";
import type { PastedFile } from "./prompt-paste";

const PREVIEW = "data:image/png;base64,iVBORw0KGgo=";

describe("thumbnails for granted attachments", () => {
  it("asks for a picture for image files and attaches what comes back", async () => {
    const asked: string[] = [];
    const out = await withAttachmentThumbnails(async path => {
      asked.push(path);
      return PREVIEW;
    }, [{ path: "/tmp/shot.png", kind: "file" }]);
    expect(asked).toEqual(["/tmp/shot.png"]);
    expect(out).toEqual([{ path: "/tmp/shot.png", kind: "file", previewUrl: PREVIEW }]);
  });

  it("never asks for a folder, a non-image file, or a grant that already has one", async () => {
    const asked: string[] = [];
    const read = async (path: string): Promise<string | null> => {
      asked.push(path);
      return PREVIEW;
    };
    const out = await withAttachmentThumbnails(read, [
      { path: "/repo/src", kind: "folder" },
      { path: "/repo/App.tsx", kind: "file" },
      { path: "/tmp/pasted.png", kind: "file", previewUrl: PREVIEW },
    ]);
    expect(asked).toEqual([]);
    expect(out).toEqual([
      { path: "/repo/src", kind: "folder" },
      { path: "/repo/App.tsx", kind: "file" },
      { path: "/tmp/pasted.png", kind: "file", previewUrl: PREVIEW },
    ]);
  });

  it("leaves an ordinary file chip when there is no picture to draw", async () => {
    // A .heic that Electron cannot decode is still attached and still sent.
    expect(await withAttachmentThumbnails(async () => null,
      [{ path: "/tmp/photo.heic", kind: "file" }]))
      .toEqual([{ path: "/tmp/photo.heic", kind: "file" }]);
  });

  it("survives a thumbnail read that rejects, rather than losing the attachment", async () => {
    expect(await withAttachmentThumbnails(
      async () => { throw new Error("main went away"); },
      [{ path: "/tmp/shot.png", kind: "file" }]))
      .toEqual([{ path: "/tmp/shot.png", kind: "file" }]);
  });

  it("keeps attachment order, whichever thumbnail resolves first", async () => {
    const out = await withAttachmentThumbnails(async path => {
      if (path.endsWith("a.png")) {
        await new Promise(resolve => { setTimeout(resolve, 5); });
      }
      return PREVIEW;
    }, [
      { path: "/tmp/a.png", kind: "file" },
      { path: "/tmp/b.png", kind: "file" },
    ]);
    expect(out.map(entry => entry.path)).toEqual(["/tmp/a.png", "/tmp/b.png"]);
  });
});

const clipboardFile = (bytes: number, fail = false): PastedFile => ({
  type: "image/png",
  arrayBuffer: async () => {
    if (fail) throw new Error("clipboard entry went away");
    return new ArrayBuffer(bytes);
  },
});

describe("screenshots insert at the end of existing text", () => {
  it("leaves an empty draft alone: the chip is the attachment", () => {
    expect(draftAfterScreenshotAttach("", ["/tmp/shot.png"])).toBe("");
    expect(draftAfterScreenshotAttach("   ", ["/tmp/shot.png"])).toBe("   ");
    expect(draftAfterScreenshotAttach("look at this", [])).toBe("look at this");
  });

  it("appends at the end of a non-empty draft, never at a mid-sentence caret", () => {
    expect(draftAfterScreenshotAttach("look at this", ["/tmp/a.png"]))
      .toBe("look at this /tmp/a.png");
    expect(draftAfterScreenshotAttach("look at this ", ["/tmp/a.png"]))
      .toBe("look at this /tmp/a.png");
    expect(draftAfterScreenshotAttach("look at this\n", ["/tmp/a.png"]))
      .toBe("look at this\n/tmp/a.png");
    expect(draftAfterScreenshotAttach("one", ["/tmp/a.png", "/tmp/b.png"]))
      .toBe("one /tmp/a.png /tmp/b.png");
  });

  it("treats a missing attach result as no paths", () => {
    expect(attachedScreenshotPaths(undefined)).toEqual([]);
    expect(attachedScreenshotPaths(null)).toEqual([]);
    expect(attachedScreenshotPaths(["/tmp/a.png", "", 3, "/tmp/b.png"] as unknown))
      .toEqual(["/tmp/a.png", "/tmp/b.png"]);
  });

  it("uses native Ctrl+V only for clipboard images in agent CLIs", () => {
    // Claude Code, Codex, Grok and Qwen read the clipboard image themselves on
    // Ctrl+V and draw their own placeholder ("[Image #1]"). A dragged image is
    // not necessarily on the clipboard, so every pane gets its stored path
    // pasted instead.
    for (const profile of ["claude", "codex", "grok", "qwen", "gemini"] as const) {
      expect(terminalImagePasteInput(profile, "clipboard")).toBe("\x16");
      expect(terminalImagePasteInput(profile, "drop")).toBeNull();
    }
    expect(terminalImagePasteInput("shell", "clipboard")).toBeNull();
    expect(terminalImagePasteInput("shell", "drop")).toBeNull();
    expect(terminalImagePasteInput(undefined, "clipboard")).toBeNull();
    expect(terminalImagePasteInput(undefined, "drop")).toBeNull();
  });

  it("moves to the end of a line that already has text before pasting paths", () => {
    expect(terminalPathInsertPrefix("")).toBe("");
    expect(terminalPathInsertPrefix("   ")).toBe("");
    expect(terminalPathInsertPrefix("look at this")).toBe(`${TERMINAL_CURSOR_TO_LINE_END} `);
  });

  it("pastes each dropped path so the CLI attaches it and draws its own placeholder", () => {
    // Claude Code and Codex turn a bracketed-pasted image path into "[Image #1]"
    // themselves; a typed path is just text to them. xterm's paste() brackets
    // the text whenever the program enabled mode 2004, and a shell pane simply
    // shows the path, exactly as Terminal.app would. Nothing is painted over.
    const center = readFileSync(
      new URL("./components/IntegratedTerminalCenter.tsx", import.meta.url), "utf8");
    const view = readFileSync(new URL("./components/TerminalView.tsx", import.meta.url), "utf8");
    expect(center).toContain("terminalPathInsertPrefix(line)");
    expect(center).toContain("currentLine()");
    expect(center).toContain("handle?.paste(`${shellQuote(path)} `)");
    expect(center).not.toContain("queueAttachments");
    expect(center).not.toContain("terminalImagePlaceholder");
    expect(view).not.toContain("registerDecoration");
    expect(view).not.toContain("terminal-attachment-chip");
    // Search-result highlights are xterm decorations, still a proposed API.
    expect(view).toMatch(/terminalRuntimeOptions[\s\S]*allowProposedApi: true/u);
    expect(view).toContain("terminalRef.current.options = terminalRuntimeOptions(options)");
  });

  it("the composers insert screenshot paths from the same helper", () => {
    const composer = readFileSync(new URL("./components/Composer.tsx", import.meta.url), "utf8");
    const conversation = readFileSync(
      new URL("./components/ConversationComposer.tsx", import.meta.url), "utf8");
    const app = readFileSync(new URL("./App.tsx", import.meta.url), "utf8");
    const paste = readFileSync(new URL("./prompt-paste.ts", import.meta.url), "utf8");
    expect(composer).toContain("pasteImagesIntoDraft(event, {");
    expect(conversation).toContain("pasteImagesIntoDraft(event, {");
    expect(paste).toContain("draftAfterScreenshotAttach(current, paths)");
    expect(app).toContain("return result.saved.map(saved => saved.path)");
  });
});

describe("pasted image payloads", () => {
  it("reads each clipboard image into bytes", async () => {
    const payloads = await pastedImagePayloads([clipboardFile(8), clipboardFile(4)]);
    expect(payloads.map(p => p.byteLength)).toEqual([8, 4]);
  });

  it("drops an oversized entry here rather than shipping it to be refused", async () => {
    const payloads = await pastedImagePayloads([
      clipboardFile(PROMPT_IMAGE_LIMITS.maxSourceBytes + 1),
      clipboardFile(16),
    ]);
    expect(payloads.map(p => p.byteLength)).toEqual([16]);
  });

  it("drops an empty entry and one that will not yield its bytes", async () => {
    expect(await pastedImagePayloads([clipboardFile(0)])).toEqual([]);
    expect(await pastedImagePayloads([clipboardFile(8, true)])).toEqual([]);
    expect((await pastedImagePayloads([clipboardFile(8, true), clipboardFile(2)]))
      .map(p => p.byteLength)).toEqual([2]);
  });
});

describe("granted paths become chips", () => {
  let next = 0;
  const deps = (read: (path: string) => Promise<string | null>) => ({
    readThumbnail: read,
    mintId: (kind: string) => `${kind}-${next++}`,
    leaf: (path: string) => path.slice(path.lastIndexOf("/") + 1),
  });

  it("gives an attached screenshot a chip that carries its picture", async () => {
    next = 0;
    const items = await attachmentContextItems(
      deps(async () => PREVIEW), [{ path: "/tmp/Screenshot.png", kind: "file" }]);
    expect(items).toEqual([{
      id: "file-0", kind: "file", label: "Screenshot.png",
      value: "/tmp/Screenshot.png", detail: "/tmp/Screenshot.png",
      previewUrl: PREVIEW,
    }]);
  });

  it("leaves a non-image attachment as the chip it has always been", async () => {
    next = 0;
    const items = await attachmentContextItems(deps(async () => PREVIEW), [
      { path: "/repo/App.tsx", kind: "file" },
      { path: "/repo/src", kind: "folder" },
    ]);
    expect(items.every(item => item.previewUrl === undefined)).toBe(true);
    expect(items.map(item => item.label)).toEqual(["App.tsx", "src"]);
    expect(items.map(item => item.kind)).toEqual(["file", "folder"]);
  });

  it("still produces a usable chip when there is no picture to be had", async () => {
    next = 0;
    const [item] = await attachmentContextItems(
      deps(async () => null), [{ path: "/tmp/Screenshot.png", kind: "file" }]);
    expect(item.previewUrl).toBeUndefined();
    expect(item.value).toBe("/tmp/Screenshot.png");
  });

  it("keeps a pasted grant's own preview without asking for another", async () => {
    next = 0;
    const [item] = await attachmentContextItems(
      deps(async () => { throw new Error("should not be asked"); }),
      [{ path: "/tmp/pasted.png", kind: "file", previewUrl: PREVIEW }]);
    expect(item.previewUrl).toBe(PREVIEW);
  });
});
