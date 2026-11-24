import { describe, expect, it } from "vitest";
import {
  BROWSER_EVENT_CHANNEL, browserDownloadNotice, browserKeyAction, browserZoomFactor, parseBrowserEvent,
  type BrowserKeyInput,
} from "./browser-events";

describe("Browser events from main", () => {
  it("is a main-only channel, named like the others", () => {
    expect(BROWSER_EVENT_CHANNEL).toBe("iface:browserEvent");
  });

  it("accepts exactly the events main sends", () => {
    const tab = { version: 1, kind: "open-tab", page: 12, url: "https://example.com/a", background: true };
    expect(parseBrowserEvent(tab)).toEqual(tab);
    expect(parseBrowserEvent({ version: 1, kind: "find", page: 3 })).toEqual({ version: 1, kind: "find", page: 3 });
    expect(parseBrowserEvent({ version: 1, kind: "address", page: 3 })).toEqual({ version: 1, kind: "address", page: 3 });
    const download = { version: 1, kind: "download", page: 3, state: "saved", name: "report (1).pdf" };
    expect(parseBrowserEvent(download)).toEqual(download);
  });

  it("refuses anything else", () => {
    const tab = { version: 1, kind: "open-tab", page: 12, url: "https://example.com/a", background: false };
    for (const bad of [
      null, [], "find", { ...tab, version: 2 }, { ...tab, page: 0 }, { ...tab, page: 1.5 }, { ...tab, page: "12" },
      { ...tab, url: "file:///etc/hosts" }, { ...tab, url: "javascript:alert(1)" }, { ...tab, url: "http://u:p@x.com/" },
      { ...tab, background: "yes" }, { ...tab, extra: 1 }, { version: 1, kind: "find", page: 3, url: "x" },
      { version: 1, kind: "close", page: 3 },
      { version: 1, kind: "download", page: 3, state: "saved", name: "../evil" },
      { version: 1, kind: "download", page: 3, state: "saved", name: "" },
      { version: 1, kind: "download", page: 3, state: "lost", name: "a.pdf" },
      { version: 1, kind: "download", page: 3, state: "saved", name: "a\u0007.pdf" },
    ]) {
      expect(parseBrowserEvent(bad), JSON.stringify(bad)).toBeNull();
    }
  });

  it("says what became of a download in plain words", () => {
    expect(browserDownloadNotice("started", "report.pdf")).toBe("Downloading report.pdf…");
    expect(browserDownloadNotice("saved", "report.pdf")).toBe("Saved report.pdf to Downloads");
    expect(browserDownloadNotice("failed", "report.pdf")).toBe("report.pdf could not be downloaded.");
  });

  it("zooms by Chrome's steps", () => {
    expect(browserZoomFactor(1, "in")).toBe(1.1);
    expect(browserZoomFactor(1, "out")).toBe(0.9);
    expect(browserZoomFactor(1.1, "out")).toBe(1);
    expect(browserZoomFactor(1.05, "in")).toBe(1.1);
    expect(browserZoomFactor(5, "in")).toBe(5);
    expect(browserZoomFactor(0.25, "out")).toBe(0.25);
    expect(browserZoomFactor(3, "reset")).toBe(1);
  });

  it("claims the page's chords and none of the app's", () => {
    const key = (value: string, modifiers: Partial<Omit<BrowserKeyInput, "key">> = {}) => browserKeyAction({
      key: value, meta: false, control: false, alt: false, shift: false, ...modifiers,
    });
    const cmd = { meta: true };
    expect([key("=", cmd), key("+", { meta: true, shift: true }), key("-", cmd), key("0", cmd)])
      .toEqual(["zoom-in", "zoom-in", "zoom-out", "zoom-reset"]);
    expect([key("r", cmd), key("R", { meta: true, shift: true }), key("[", cmd), key("]", cmd),
      key("f", cmd), key("l", cmd)]).toEqual(["reload", "hard-reload", "back", "forward", "find", "address"]);
    // ⌘W, ⌘T, ⌘Q, the app's palette, splits, tile cycling and the rest stay the app's.
    for (const [value, modifiers] of [["w", cmd], ["t", cmd], ["q", cmd], ["p", { meta: true, shift: true }],
      ["p", cmd], ["d", cmd], ["n", cmd], ["k", cmd], [",", cmd], ["{", { meta: true, shift: true }],
      ["}", { meta: true, shift: true }], ["f", { meta: true, shift: true }], ["r", { control: true }],
      ["=", { meta: true, alt: true }], ["-", { meta: true, control: true }], ["r", {}], ["f", {}]] as const) {
      expect(key(value, modifiers), `${value} ${JSON.stringify(modifiers)}`).toBeNull();
    }
  });
});
