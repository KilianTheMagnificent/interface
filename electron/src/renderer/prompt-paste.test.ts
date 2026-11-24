import { afterEach, describe, expect, it, vi } from "vitest";
import {
  pasteImagesIntoDraft, pastedImageFiles, promptPasteAction, type PastedFile,
} from "./prompt-paste";

const file = (type: string, name?: string): PastedFile => ({
  type, ...(name === undefined ? {} : { name }),
  arrayBuffer: async () => new ArrayBuffer(4),
});

const clipboard = (types: readonly string[], files: PastedFile[] = []) => ({ types, files });

describe("pasted image collection", () => {
  it("takes the images from a screenshot paste", () => {
    const shot = file("image/png", "Screenshot.png");
    expect(pastedImageFiles(clipboard(["Files"], [shot]))).toEqual([shot]);
    expect(promptPasteAction(clipboard(["Files"], [shot]), true)).toEqual([shot]);
  });

  it("leaves a paste alone when the clipboard also carries text", () => {
    // Copying a spreadsheet cell or a rich snippet puts BOTH on the clipboard.
    // Attaching a picture of the words the user meant to paste is not an option.
    const both = clipboard(["text/plain", "text/html", "Files"], [file("image/png")]);
    expect(pastedImageFiles(both)).toEqual([]);
    expect(promptPasteAction(both, true)).toBeNull();
  });

  it("ignores non-image clipboard files and a clipboard with no files at all", () => {
    expect(pastedImageFiles(clipboard(["Files"], [file("application/pdf", "paper.pdf")])))
      .toEqual([]);
    expect(pastedImageFiles(clipboard(["Files"]))).toEqual([]);
    expect(pastedImageFiles(clipboard([]))).toEqual([]);
    expect(promptPasteAction(clipboard(["Files"], [file("text/csv")]), true)).toBeNull();
  });

  it("keeps only the images when a paste mixes kinds", () => {
    const shot = file("image/jpeg", "photo.jpg");
    expect(pastedImageFiles(clipboard(["Files"], [file("application/zip"), shot])))
      .toEqual([shot]);
  });

  it("stops at the same item ceiling the drop grant uses", () => {
    const many = Array.from({ length: 20 }, () => file("image/png"));
    expect(pastedImageFiles(clipboard(["Files"], many))).toHaveLength(8);
  });

  it("survives a clipboard entry with no usable type", () => {
    const broken = { arrayBuffer: async () => new ArrayBuffer(1) } as unknown as PastedFile;
    expect(pastedImageFiles(clipboard(["Files"], [broken]))).toEqual([]);
  });
});

/* The decision both composers share. `null` is "not ours, paste it as text";
   an array is "Interface is claiming this paste". Getting the disabled cases
   wrong is what would silently eat a paste in a locked composer. */
describe("promptPasteAction", () => {
  const shot = file("image/png", "Screenshot.png");

  it("claims a screenshot paste when the composer can accept attachments", () => {
    expect(promptPasteAction(clipboard(["Files"], [shot]), true)).toEqual([shot]);
  });

  it("claims nothing while the composer cannot accept attachments", () => {
    // A locked composer, a launch in flight, or a caller with no attach
    // handler: the paste must still reach the textarea untouched.
    expect(promptPasteAction(clipboard(["Files"], [shot]), false)).toBeNull();
  });

  it("claims nothing when there is no clipboard at all", () => {
    expect(promptPasteAction(null, true)).toBeNull();
  });

  it("never claims an ordinary text paste", () => {
    expect(promptPasteAction(clipboard(["text/plain"]), true)).toBeNull();
    expect(promptPasteAction(clipboard(["text/plain", "Files"], [shot]), true)).toBeNull();
  });
});

/* The paste both prompt surfaces share: the launch card and a session's
   composer. The textarea is a stand-in and the frame runs at once. */
describe("pasting screenshots into a draft", () => {
  afterEach(() => { vi.unstubAllGlobals(); });
  const shot = file("image/png", "Screenshot.png");
  const paste = (types: readonly string[], files: PastedFile[]) => {
    const calls: string[] = [];
    return {
      calls,
      event: {
        clipboardData: { types, files },
        preventDefault: () => { calls.push("preventDefault"); },
      },
    };
  };
  const surface = (draft: string, saved: readonly string[] | void) => {
    const state = { draft, caret: -1, sent: [] as string[] };
    const textarea = { current: {
      get value() { return state.draft; },
      setSelectionRange: (start: number) => { state.caret = start; },
    } as unknown as HTMLTextAreaElement };
    return {
      state,
      options: {
        enabled: true,
        onPastedImages: async (files: readonly PastedFile[]) => {
          state.sent.push(...files.map(item => item.name ?? ""));
          return saved;
        },
        updateDraft: (update: (current: string) => string) => { state.draft = update(state.draft); },
        textarea,
      },
    };
  };
  const settle = async (): Promise<void> => {
    for (let turn = 0; turn < 4; turn += 1) await Promise.resolve();
  };

  it("attaches an image-only clipboard and puts the saved path at the end of the draft", async () => {
    vi.stubGlobal("window", { requestAnimationFrame: (run: () => void) => { run(); return 0; } });
    const { state, options } = surface("look at this", ["/tmp/interface/shot.png"]);
    const pasted = paste(["Files"], [shot]);
    pasteImagesIntoDraft(pasted.event, options);
    await settle();
    expect(pasted.calls).toEqual(["preventDefault"]);
    expect(state.sent).toEqual(["Screenshot.png"]);
    expect(state.draft).toBe("look at this /tmp/interface/shot.png");
    expect(state.caret).toBe(state.draft.length);
  });

  it("leaves text pastes, and a surface that cannot attach, to the textarea", async () => {
    for (const [types, enabled] of [[["text/plain", "Files"], true], [["Files"], false]] as const) {
      const { state, options } = surface("draft", ["/tmp/shot.png"]);
      const pasted = paste(types, [shot]);
      pasteImagesIntoDraft(pasted.event, { ...options, enabled });
      await settle();
      expect(pasted.calls).toEqual([]);
      expect(state.sent).toEqual([]);
      expect(state.draft).toBe("draft");
    }
  });

  it("writes nothing when the surface has moved on or nothing was saved", async () => {
    const moved = surface("typed", ["/tmp/shot.png"]);
    pasteImagesIntoDraft(paste(["Files"], [shot]).event, { ...moved.options, isCurrent: () => false });
    const unsaved = surface("typed", undefined);
    pasteImagesIntoDraft(paste(["Files"], [shot]).event, unsaved.options);
    await settle();
    expect(moved.state.sent).toEqual(["Screenshot.png"]);
    expect(moved.state.draft).toBe("typed");
    expect(unsaved.state.draft).toBe("typed");
  });
});
