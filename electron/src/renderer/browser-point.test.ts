import { describe, expect, it } from "vitest";
import {
  BROWSER_POINT_LIMITS, BROWSER_VIEWPORTS, addBrowserNoteToDraft, appendBrowserNote, browserCapture,
  browserCaptureNote, browserCaptureSummary, browserCropRect, browserNoteAddedMessage, browserPickFromPage,
  browserViewportFrame, browserViewportPreset, cleanComment, cleanPageText, cleanSelector, pngBytesFromDataUrl,
  type BrowserElementPick, type BrowserNoteDraftPort,
} from "./browser-point";
import { BROWSER_POINT_CANCEL_SCRIPT, browserPointScript } from "./browser-point-page";

const viewport = { width: 1280, height: 800 };
const rawButton = {
  kind: "element", selector: "#save", tag: "BUTTON", text: "Save", name: "Save", role: "button", frame: false,
  box: { x: 40.4, y: 60.6, width: 120, height: 32 }, viewport,
};

describe("Browser point: page results are untrusted", () => {
  it("keeps a well-formed element pick, rounded and cleaned", () => {
    expect(browserPickFromPage(rawButton)).toEqual({
      kind: "element", selector: "#save", tag: "button", text: "Save", name: "Save", role: "button", frame: false,
      box: { x: 40, y: 61, width: 120, height: 32 }, viewport,
    });
  });

  it("refuses anything that is not a pick", () => {
    for (const raw of [null, undefined, 1, "pick", [], [rawButton], {}, { ...rawButton, kind: "script" },
      { ...rawButton, box: null }, { ...rawButton, box: { x: "1", y: 2, width: 3, height: 4 } },
      { ...rawButton, box: { x: Number.NaN, y: 0, width: 1, height: 1 } },
      { ...rawButton, viewport: { width: 0, height: 10 } }, { ...rawButton, viewport: "1280x800" },
      { kind: "region", box: { x: 0, y: 0, width: 0, height: 10 }, viewport }]) {
      expect(browserPickFromPage(raw), JSON.stringify(raw) ?? String(raw)).toBeNull();
    }
  });

  it("bounds huge strings and clamps huge numbers", () => {
    const pick = browserPickFromPage({ ...rawButton, text: "x".repeat(5_000_000), name: "n".repeat(10_000),
      selector: `div${" > div".repeat(200)}`, tag: "a".repeat(500), role: "r".repeat(300),
      box: { x: -1e12, y: 1e300, width: 1e9, height: 5 } }) as BrowserElementPick | null;
    expect(pick).not.toBeNull();
    expect(Array.from(pick!.text)).toHaveLength(BROWSER_POINT_LIMITS.text);
    expect(pick!.text.endsWith("…")).toBe(true);
    expect(Array.from(pick!.name!)).toHaveLength(BROWSER_POINT_LIMITS.name);
    // A selector too long to keep is dropped, never cut into a different one.
    expect(pick!.selector).toBeNull();
    expect(pick!.tag).toBe("element");
    expect(pick!.role).toBeNull();
    expect(browserPickFromPage({ ...rawButton, box: { x: -1e12, y: 1e300, width: 1e9, height: 5 } }))
      .toMatchObject({ box: { x: -1_000_000, y: 1_000_000, width: 1_000_000, height: 5 } });
    expect(browserPickFromPage({ ...rawButton, box: { x: 0, y: 0, width: 1, height: Infinity } })).toBeNull();
  });

  it("drops wrong types to safe defaults", () => {
    expect(browserPickFromPage({ ...rawButton, selector: 42, tag: { toString: () => "a" }, text: ["Save"],
      name: null, role: 7, frame: "yes" })).toMatchObject({
      selector: null, tag: "element", text: "", name: null, role: null, frame: false,
    });
  });

  it("strips control, invisible and reordering characters and keeps HTML as inert text", () => {
    expect(cleanPageText("Sa\u0000ve\nnow‮​⁦ \t ok\u0085", 200)).toBe("Sa ve now ok");
    expect(cleanPageText("<img src=x onerror=alert(1)>", 200)).toBe("<img src=x onerror=alert(1)>");
    expect(cleanPageText("\ud800lone", 20)).toBe("lone");
    expect(cleanPageText(undefined, 10)).toBe("");
    expect(cleanPageText("😀".repeat(20), 5)).toBe("😀😀😀😀…");
    expect(cleanSelector("#a\u0000b")).toBe("#a b");
    expect(cleanSelector("   ")).toBeNull();
    expect(cleanSelector("a".repeat(BROWSER_POINT_LIMITS.selector))).toHaveLength(BROWSER_POINT_LIMITS.selector);
    expect(cleanSelector("a".repeat(BROWSER_POINT_LIMITS.selector + 1))).toBeNull();
  });

  it("keeps a region with its box only", () => {
    expect(browserPickFromPage({ kind: "region", box: { x: 10, y: 20, width: 300, height: 200 }, viewport,
      text: "ignored", selector: "ignored" })).toEqual({ kind: "region", box: { x: 10, y: 20, width: 300, height: 200 }, viewport });
  });

  it("bounds the page's address and title", () => {
    const pick = browserPickFromPage(rawButton)!;
    const capture = browserCapture(pick, `https://example.com/${"a".repeat(5000)}`, `Home\u0007${"t".repeat(500)}`);
    expect(Array.from(capture.url)).toHaveLength(BROWSER_POINT_LIMITS.url);
    expect(Array.from(capture.title)).toHaveLength(BROWSER_POINT_LIMITS.title);
    expect(capture.title.startsWith("Home t")).toBe(true);
    expect(browserCapture(pick, null, 3)).toMatchObject({ url: "", title: "" });
  });
});

describe("Browser point: the note", () => {
  const button = browserPickFromPage(rawButton)!;
  const capture = browserCapture(button, "http://localhost:5173/settings", "Settings");

  it("says where, what and the owner's comment", () => {
    expect(browserCaptureNote(capture, "Make this green\nand wider")).toBe([
      "In the browser at http://localhost:5173/settings (\"Settings\"):",
      "Element button \"Save\" (selector `#save`, 120×32 at 40,61)",
      "> Make this green",
      "> and wider",
    ].join("\n"));
  });

  it("leaves out what the page did not give", () => {
    const bare = browserCapture({ ...button, selector: null, text: "", name: null } as BrowserElementPick, "https://a.test/", "");
    expect(browserCaptureNote(bare, "  ")).toBe("In the browser at https://a.test/:\nElement button (120×32 at 40,61)");
  });

  it("names an element whose accessible name differs from its text", () => {
    const icon = browserCapture({ ...button, text: "×", name: "Close dialog" } as BrowserElementPick, "https://a.test/", "A");
    expect(browserCaptureNote(icon, "")).toContain("Element button \"×\" named \"Close dialog\"");
  });

  it("fences a selector that carries backticks", () => {
    const odd = browserCapture({ ...button, selector: "#a\\`b" } as BrowserElementPick, "https://a.test/", "");
    expect(browserCaptureNote(odd, "")).toContain("selector ``#a\\`b``,");
    const edge = browserCapture({ ...button, selector: "`x" } as BrowserElementPick, "https://a.test/", "");
    expect(browserCaptureNote(edge, "")).toContain("selector `` `x ``,");
  });

  it("describes a region and an embedded frame", () => {
    const region = browserCapture({ kind: "region", box: { x: 0, y: 10, width: 300, height: 120 }, viewport },
      "https://a.test/", "A");
    expect(browserCaptureNote(region, "too tall")).toBe(
      "In the browser at https://a.test/ (\"A\"):\nRegion 300×120 at 0,10 of a 1280×800 view (picture attached)\n> too tall");
    const frame = browserCapture({ ...button, tag: "iframe", text: "", name: null, frame: true } as BrowserElementPick,
      "https://a.test/", "A");
    expect(browserCaptureNote(frame, "")).toContain("Embedded frame <iframe> (selector `#save`, 120×32 at 40,61); its contents could not be inspected");
    expect(browserCaptureSummary(frame)).toContain("can't look inside this embedded frame");
  });

  it("drops controls from the comment but keeps its lines", () => {
    expect(cleanComment("a\u0000b\r\nc‮")).toBe("a b\nc");
    expect(cleanComment("x".repeat(10_000))).toHaveLength(BROWSER_POINT_LIMITS.comment);
  });

  it("goes after the draft's words, a blank line apart", () => {
    expect(appendBrowserNote("", "N")).toBe("N\n\n");
    expect(appendBrowserNote("fix this  \n\n", "N")).toBe("fix this\n\nN\n\n");
  });

  it("says what was captured in plain words", () => {
    expect(browserCaptureSummary(capture)).toBe("A button “Save”, 120×32");
    expect(browserCaptureSummary(browserCapture({ ...button, tag: "img", text: "", name: "Logo" } as BrowserElementPick, "", "")))
      .toBe("An image “Logo”, 120×32");
    expect(browserCaptureSummary(browserCapture({ ...button, tag: "x-widget", text: "", name: null, role: null } as BrowserElementPick, "", "")))
      .toBe("A <x-widget> element, 120×32");
  });
});

describe("Browser point: adding to a draft", () => {
  const port = (canAttach: boolean, attach: BrowserNoteDraftPort["attachImage"]) => {
    const drafts = new Map<string, string>([["claude:1", "earlier words"]]);
    const attached: Uint8Array[] = [];
    return {
      drafts, attached,
      port: {
        getDraft: key => drafts.get(key) ?? "",
        setDraft: (key, text) => { drafts.set(key, text); },
        canAttach: () => canAttach,
        attachImage: async (key, png) => { attached.push(png); return attach(key, png); },
      } satisfies BrowserNoteDraftPort,
    };
  };
  const png = new Uint8Array([0x89, 0x50, 0x4e, 0x47]);

  it("appends the note and attaches the picture, never sending", async () => {
    const { drafts, attached, port: draftPort } = port(true, async () => ["/images/a.png"]);
    await expect(addBrowserNoteToDraft(draftPort, "claude:1", "NOTE", png)).resolves.toBe("attached");
    expect(drafts.get("claude:1")).toBe("earlier words\n\nNOTE\n\n");
    expect(attached).toEqual([png]);
  });

  it("keeps the note when the picture cannot go", async () => {
    const chat = port(false, async () => ["/x.png"]);
    await expect(addBrowserNoteToDraft(chat.port, "claude:1", "NOTE", png)).resolves.toBe("unsupported");
    expect(chat.attached).toEqual([]);
    expect(chat.drafts.get("claude:1")).toContain("NOTE");
    const failing = port(true, async () => { throw new Error("disk full"); });
    await expect(addBrowserNoteToDraft(failing.port, "claude:1", "NOTE", png)).resolves.toBe("failed");
    const refused = port(true, async () => []);
    await expect(addBrowserNoteToDraft(refused.port, "claude:1", "NOTE", png)).resolves.toBe("failed");
    await expect(addBrowserNoteToDraft(refused.port, "claude:2", "NOTE", null)).resolves.toBe("none");
    expect(refused.drafts.get("claude:2")).toBe("NOTE\n\n");
  });

  it("says where the note went", () => {
    expect(browserNoteAddedMessage("Fix header", "attached")).toBe("Added to the draft in “Fix header”, with its picture. Review it there and send.");
    expect(browserNoteAddedMessage("", "unsupported")).toContain("“the thread”");
  });
});

describe("Browser point: pictures", () => {
  const pick = browserPickFromPage(rawButton)!;

  it("crops the element with a margin, clipped to the view and scaled by zoom", () => {
    expect(browserCropRect(pick)).toEqual({ x: 32, y: 53, width: 136, height: 48 });
    expect(browserCropRect(pick, 2)).toEqual({ x: 64, y: 106, width: 272, height: 96 });
    expect(browserCropRect({ ...pick, box: { x: -50, y: 780, width: 100, height: 100 } })).toEqual({ x: 0, y: 772, width: 58, height: 28 });
    expect(browserCropRect({ ...pick, box: { x: 2000, y: 0, width: 10, height: 10 } })).toBeNull();
    expect(browserCropRect({ kind: "region", box: { x: 10, y: 10, width: 50, height: 40 }, viewport }, Number.NaN))
      .toEqual({ x: 10, y: 10, width: 50, height: 40 });
  });

  it("reads PNG bytes from a data URL and nothing else", () => {
    const bytes = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3]);
    const encoded = btoa(String.fromCharCode(...bytes));
    expect(pngBytesFromDataUrl(`data:image/png;base64,${encoded}`)).toEqual(bytes);
    expect(pngBytesFromDataUrl(`data:image/jpeg;base64,${encoded}`)).toBeNull();
    expect(pngBytesFromDataUrl(`data:image/png;base64,${btoa("GIF89a")}`)).toBeNull();
    expect(pngBytesFromDataUrl("data:image/png;base64,***")).toBeNull();
    expect(pngBytesFromDataUrl("data:image/png;base64,")).toBeNull();
    expect(pngBytesFromDataUrl(42)).toBeNull();
  });
});

describe("Browser point: viewport presets", () => {
  it("offers Fit, Phone 390, Tablet 768 and Desktop 1280", () => {
    expect(BROWSER_VIEWPORTS.map(preset => [preset.id, preset.width])).toEqual([
      ["fit", null], ["phone", 390], ["tablet", 768], ["desktop", 1280]]);
    expect(browserViewportPreset("nonsense").id).toBe("fit");
  });

  it("centres a preset in the slot and never overflows it", () => {
    expect(browserViewportFrame(1000, "phone")).toEqual({ width: 390, left: 305 });
    expect(browserViewportFrame(1001.6, "tablet")).toEqual({ width: 768, left: 116 });
    expect(browserViewportFrame(1000, "desktop")).toEqual({ width: 1000, left: 0 });
    expect(browserViewportFrame(1000, "fit")).toEqual({ width: 1000, left: 0 });
    expect(browserViewportFrame(300, "phone")).toEqual({ width: 300, left: 0 });
    expect(browserViewportFrame(Number.NaN, "phone")).toEqual({ width: 0, left: 0 });
  });
});

describe("Browser point: the in-page script", () => {
  it("is one self-contained expression for each mode", () => {
    for (const mode of ["element", "region"] as const) {
      const script = browserPointScript(mode);
      expect(() => new Function(`return ${script};`)).not.toThrow();
      expect(script).toContain(JSON.stringify(mode));
      // Never markup, never eval inside the page.
      expect(script).not.toMatch(/innerHTML|outerHTML|insertAdjacentHTML|\beval\(|new Function/u);
    }
    expect(() => new Function(BROWSER_POINT_CANCEL_SCRIPT)).not.toThrow();
  });
});
