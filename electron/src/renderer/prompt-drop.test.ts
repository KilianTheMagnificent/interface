import { describe, expect, it } from "vitest";
import {
  isPromptDropImageName,
  nextDropHoverCount,
  promptDropAccepts,
  promptDropEffect,
  promptDropZone,
  splitDroppedFiles,
  type PromptDropEvent,
  type PromptDropPath,
} from "./prompt-drop";

describe("prompt drop acceptance", () => {
  it("accepts OS files and nothing else", () => {
    expect(promptDropAccepts(["Files"])).toBe(true);
    expect(promptDropAccepts(["text/plain", "text/uri-list"])).toBe(false);
    expect(promptDropEffect(["Files"])).toBe("copy");
    expect(promptDropEffect(["text/html"])).toBe("none");
  });

  it("tracks nested dragenter/leave so a child does not cancel the drop target", () => {
    expect(nextDropHoverCount(0, "dragenter")).toBe(1);
    expect(nextDropHoverCount(1, "dragenter")).toBe(2);
    expect(nextDropHoverCount(2, "dragleave")).toBe(1);
    expect(nextDropHoverCount(1, "drop")).toBe(0);
    expect(nextDropHoverCount(4, "dragend")).toBe(0);
    expect(nextDropHoverCount(0, "dragleave")).toBe(0);
  });
});

describe("splitting a drop for a store-capable target", () => {
  it("keeps on-disk paths and holds back only the pathless images", () => {
    const lookup = new Map([
      ["notes.md", "/Users/me/Desktop/notes.md"],
      ["shot.png", "/Users/me/Desktop/shot.png"],
    ]);
    const split = splitDroppedFiles([
      { name: "notes.md", type: "text/markdown" },
      { name: "shot.png", type: "image/png" },
      { name: "notes.md", type: "text/markdown" },
      { name: "image.png", type: "image/png" },
      { name: "capture.pdf", type: "application/pdf" },
    ], file => lookup.get(file.name) ?? "");
    expect(split.paths).toEqual([
      "/Users/me/Desktop/notes.md",
      "/Users/me/Desktop/shot.png",
    ]);
    expect(split.images.map(file => file.name)).toEqual(["image.png"]);
  });

  it("stores images whose path is a temporary copy the source is about to delete", () => {
    const lookup = new Map([
      ["Screenshot 2026-08-31 at 10.33.48 AM.png",
        "/var/folders/_b/x/T/TemporaryItems/NSIRD_screencaptureui_ab/"
          + "Screenshot 2026-08-31 at 10.33.48 AM.png"],
      ["mail.jpg", "/private/tmp/mail.jpg"],
      ["shot.png", "/Users/me/Desktop/shot.png"],
      ["report.pdf", "/var/folders/_b/x/T/TemporaryItems/report.pdf"],
      ["typeless.png", "/var/folders/_b/x/T/TemporaryItems/typeless.png"],
    ]);
    const split = splitDroppedFiles([
      { name: "Screenshot 2026-08-31 at 10.33.48 AM.png", type: "image/png" },
      { name: "mail.jpg", type: "image/jpeg" },
      { name: "shot.png", type: "image/png" },
      { name: "report.pdf", type: "application/pdf" },
      { name: "typeless.png", type: "" },
    ], file => lookup.get(file.name) ?? "");
    // The PDF keeps today's behavior: only an image has a byte route to take.
    expect(split.paths).toEqual([
      "/Users/me/Desktop/shot.png",
      "/var/folders/_b/x/T/TemporaryItems/report.pdf",
    ]);
    expect(split.images.map(file => file.name)).toEqual([
      "Screenshot 2026-08-31 at 10.33.48 AM.png",
      "mail.jpg",
      "typeless.png",
    ]);
  });
});

describe("image name detection", () => {
  it("recognizes common image extensions regardless of case", () => {
    expect(isPromptDropImageName("Shot.PNG")).toBe(true);
    expect(isPromptDropImageName("diagram.webp")).toBe(true);
    expect(isPromptDropImageName("App.tsx")).toBe(false);
    expect(isPromptDropImageName("archive.tar.gz")).toBe(false);
  });
});

/* The drop zone both prompt surfaces spread onto their drop target: the launch
   card and a session's composer. Driven with plain objects standing in for
   React's drag events. */
describe("the shared prompt drop zone", () => {
  type Dropped = { name: string; type: string; arrayBuffer: () => Promise<ArrayBuffer> };
  const dropped = (name: string, type = ""): Dropped =>
    ({ name, type, arrayBuffer: async () => new ArrayBuffer(1) });
  const drag = (types: string[], files: Dropped[] = []) => {
    const calls: string[] = [];
    const transfer = { types, files, dropEffect: "none" };
    const event = {
      dataTransfer: transfer as unknown as DataTransfer,
      preventDefault: () => { calls.push("preventDefault"); },
    } satisfies PromptDropEvent;
    return { event, transfer, calls };
  };
  const zone = (over: { enabled?: boolean; pathForFile?: (file: { name: string }) => string;
    attachPaths?: boolean } = {}) => {
    let hover = 0;
    const paths: (readonly PromptDropPath[])[] = [];
    const images: string[][] = [];
    const handlers = promptDropZone({
      enabled: over.enabled ?? true,
      pathForFile: "pathForFile" in over ? over.pathForFile
        : file => (file.name.startsWith("tmp-") ? `/var/folders/x/${file.name}`
          : file.name.startsWith("web-") ? "" : `/repo/${file.name}`),
      onDroppedPaths: over.attachPaths === false ? undefined : next => { paths.push(next); },
      onPastedImages: files => { images.push(files.map(file => file.name ?? "")); },
      setHover: next => { hover = typeof next === "function" ? next(hover) : next; },
    });
    return { handlers, paths, images, hover: () => hover };
  };

  it("counts nested enters and leaves and offers a copy while files are over it", () => {
    const { handlers, hover } = zone();
    const over = drag(["Files"]);
    handlers.onDragEnter(over.event);
    handlers.onDragEnter(over.event);
    handlers.onDragOver(over.event);
    expect(hover()).toBe(2);
    expect(over.transfer.dropEffect).toBe("copy");
    handlers.onDragLeave(over.event);
    expect(hover()).toBe(1);
    handlers.onDragEnd();
    expect(hover()).toBe(0);
    expect(over.calls).toEqual(["preventDefault", "preventDefault", "preventDefault"]);
  });

  it("attaches paths, sends dying and pathless images down the paste route", () => {
    const { handlers, paths, images, hover } = zone();
    const files = [dropped("App.tsx"), dropped("tmp-shot.png", "image/png"),
      dropped("web-photo.jpg", "image/jpeg")];
    const drop = drag(["Files"], files);
    handlers.onDragEnter(drop.event);
    handlers.onDrop(drop.event);
    expect(hover()).toBe(0);
    expect(drop.calls).toContain("preventDefault");
    expect(images).toEqual([["tmp-shot.png", "web-photo.jpg"]]);
    expect(paths).toEqual([[{ path: "/repo/App.tsx" }]]);
  });

  it("still reports a drop whose files resolved to nothing, so the caller can say so", () => {
    const { handlers, paths, images } = zone();
    handlers.onDrop(drag(["Files"], [dropped("web-notes.txt")]).event);
    expect(images).toEqual([]);
    expect(paths).toEqual([[]]);
  });

  it("leaves a drag alone when it is disabled, cannot attach, or carries no files", () => {
    for (const closed of [zone({ enabled: false }), zone({ pathForFile: undefined }),
      zone({ attachPaths: false })]) {
      const drop = drag(["Files"], [dropped("App.tsx")]);
      closed.handlers.onDragEnter(drop.event);
      closed.handlers.onDragOver(drop.event);
      closed.handlers.onDrop(drop.event);
      expect(drop.calls).toEqual([]);
      expect(closed.hover()).toBe(0);
      expect(closed.paths).toEqual([]);
    }
    const open = zone();
    const text = drag(["text/plain"]);
    open.handlers.onDragEnter(text.event);
    open.handlers.onDrop(text.event);
    expect(text.calls).toEqual([]);
    expect(open.hover()).toBe(0);
    expect(open.paths).toEqual([]);
  });
});
