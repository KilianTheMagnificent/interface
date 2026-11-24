import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { EngineEvent } from "../shared/engine";
import { wrapVisualDocument } from "../shared/visual-frame";
import { sameConversationItemProps } from "./components/ConversationRows";
import { VisualFrame, VisualReplyCard } from "./components/VisualReply";
import { conversationItems, type ConvItem } from "./conversation-reducer";
import { threadTranscriptPlan } from "./thread-transcript";
import { mergeTranscript, type TranscriptRow } from "./transcript-merge";
import { buildTranscriptSearchCorpus, searchTranscriptCorpus } from "./transcript-search";
import {
  appColorScheme, previewFrameUrl, renderableKind, visualFrameLoader, visualSizeLabel,
} from "./visual-reply";

const ID = "v1731630000500-0a1b2c3d";
const shown: EngineEvent = { kind: "visual.shown", id: ID, title: "Signups by week", bytes: 1834 };
const PAGE = "<!doctype html><html><body><svg viewBox=\"0 0 4 4\"></svg></body></html>";

describe("visual replies in the transcript (T3 section 8)", () => {
  it("reduces visual.shown to a visual item keyed by Rust's id, between the text around it", () => {
    const items = conversationItems([
      { kind: "message.delta", text: "Here it is:" },
      shown,
      { kind: "message.delta", text: "Signups rose." },
    ]);
    expect(items.map(item => item.kind)).toEqual(["text", "visual", "text"]);
    expect(items[1]).toEqual({ kind: "visual", id: `visual-${ID}`, visualId: ID,
      title: "Signups by week", bytes: 1834 });
    expect(items[0]).toMatchObject({ streaming: false });
    // The id is the same however many events came first: live and replayed agree.
    expect(conversationItems([{ kind: "message.delta", text: "x" }, shown])[1]?.id).toBe(`visual-${ID}`);
  });

  it("shows a visual once when history and the live stream both hold it", () => {
    const [visual] = conversationItems([shown]);
    const rows = mergeTranscript([visual], [visual]);
    expect(rows.filter(row => row.kind === "item" && row.item.kind === "visual")).toHaveLength(1);
  });

  it("finds a visual by its title, and never by its page", () => {
    const rows: TranscriptRow[] = conversationItems([shown]).map(item =>
      ({ kind: "item", key: `l:${item.id}`, origin: "live", item }));
    const corpus = buildTranscriptSearchCorpus(rows);
    expect(searchTranscriptCorpus(corpus, "signups").matches).toEqual([
      { key: `l:visual-${ID}`, kind: "visual" },
    ]);
    expect(searchTranscriptCorpus(corpus, "svg").matches).toEqual([]);
  });

  it("keeps a visual above the reply in a finished thread turn, never folded into the work", () => {
    const items: ConvItem[] = [
      { kind: "user", id: "user-0", text: "chart signups" },
      { kind: "tool", id: "call-1", tool: "mcp__iface_team__show_visual", summary: "show_visual",
        input: {}, result: { ok: true, summary: "ok" } },
      ...conversationItems([shown]),
      { kind: "text", id: "text-9", text: "Signups rose.", streaming: false },
    ];
    const rows: TranscriptRow[] = items.map(item => ({ kind: "item", key: item.id, origin: "history", item }));
    const plan = threadTranscriptPlan(rows);
    expect(plan.foldedIn.has("call-1")).toBe(true);
    expect(plan.foldedIn.has(`visual-${ID}`)).toBe(false);
    expect(plan.turns[0]?.answerKey).toBe("text-9");
  });

  it("re-renders a visual row only when its loader changes", () => {
    type ConversationItemProps = Parameters<typeof sameConversationItemProps>[0];
    const [item] = conversationItems([shown]);
    const loader = { load: vi.fn(), release: vi.fn() };
    const all = { sessionKey: "claude#a", visualFrames: loader } as unknown as ConversationItemProps["all"];
    const props = (context: ConversationItemProps["all"]) =>
      ({ item, findKey: item.id, all: context, find: { register: undefined } }) as unknown as ConversationItemProps;
    const base = props(all);
    expect(sameConversationItemProps(base, props({ ...all }))).toBe(true);
    expect(sameConversationItemProps(base,
      props({ ...all, visualFrames: { load: vi.fn(), release: vi.fn() } }))).toBe(false);
  });
});

describe("the visual card and frame", () => {
  it("draws a sandboxed frame by URL: scripts only, never srcdoc or same-origin", () => {
    const markup = renderToStaticMarkup(createElement(VisualFrame,
      { url: `iface-visual://frame/${"a".repeat(32)}`, title: "Chart \"one\"" }));
    expect(markup).toContain('sandbox="allow-scripts"');
    expect(markup).toContain(`src="iface-visual://frame/${"a".repeat(32)}"`);
    expect(markup).toContain('referrerPolicy="no-referrer"');
    for (const loose of ["allow-same-origin", "allow-forms", "allow-popups", "allow-top-navigation",
      "allow-downloads", "allow-modals", "srcdoc"]) {
      expect(markup).not.toContain(loose);
    }
  });

  it("names the visual and waits for its page before offering Source or Open larger", () => {
    const [item] = conversationItems([shown]);
    if (item?.kind !== "visual") throw new Error("expected a visual item");
    const markup = renderToStaticMarkup(createElement(VisualReplyCard,
      { item, sessionKey: "claude#a", loader: { load: vi.fn(), release: vi.fn() } }));
    expect(markup).toContain('aria-label="Visual: Signups by week"');
    expect(markup).toContain("Signups by week");
    expect(markup).toContain("1.8 KB");
    expect(markup).toContain("Loading the visual…");
    expect(markup).not.toContain("Open larger");
    expect(markup).not.toContain("<iframe");
  });

  it("loads a page from Rust, wraps it for the app's scheme and registers it with main", async () => {
    const threadJournal = vi.fn(async () => ({ visual: { id: ID, html: PAGE } }));
    const visualFrame = vi.fn(async () => `iface-visual://frame/${"b".repeat(32)}`);
    const loader = visualFrameLoader({ threadJournal, visualFrame });
    expect(await loader.load("claude#a", ID, "dark")).toEqual(
      { url: `iface-visual://frame/${"b".repeat(32)}`, html: PAGE });
    expect(threadJournal).toHaveBeenCalledWith({ kind: "visual", key: "claude#a", id: ID });
    expect(visualFrame).toHaveBeenCalledWith({ kind: "register", html: wrapVisualDocument(PAGE, "dark") });
    loader.release(`iface-visual://frame/${"b".repeat(32)}`);
    expect(visualFrame).toHaveBeenLastCalledWith({ kind: "release", url: `iface-visual://frame/${"b".repeat(32)}` });
    // A visual the thread no longer keeps, or another id's answer, registers nothing.
    visualFrame.mockClear();
    threadJournal.mockResolvedValueOnce({} as never);
    expect(await loader.load("claude#a", ID, "light")).toBeNull();
    threadJournal.mockResolvedValueOnce({ visual: { id: "v0000000000000-00000000", html: PAGE } });
    expect(await loader.load("claude#a", ID, "light")).toBeNull();
    expect(visualFrame).not.toHaveBeenCalled();
    visualFrame.mockResolvedValueOnce(null as never);
    await expect(loader.load("claude#a", ID, "light")).rejects.toThrow("not registered");
  });

  it("renders only .html and .svg previews, an SVG inside a minimal page", async () => {
    expect(renderableKind("/p/report.HTML")).toBe("html");
    expect(renderableKind("chart.htm")).toBe("html");
    expect(renderableKind("logo.svg")).toBe("svg");
    for (const path of ["notes.md", "page.html.txt", "image.png", "svg"]) expect(renderableKind(path)).toBeNull();
    const visualFrame = vi.fn(async () => `iface-visual://frame/${"c".repeat(32)}`);
    await previewFrameUrl({ visualFrame }, '<?xml version="1.0"?><svg/>', "svg", "light");
    const registered = (visualFrame.mock.calls[0] as unknown as [{ html: string }])[0].html;
    expect(registered).toContain("<body><svg/></body>");
    expect(registered).not.toContain("<?xml");
  });

  it("follows the app's colour scheme and says sizes plainly", () => {
    expect(appColorScheme({ style: { colorScheme: "dark" } })).toBe("dark");
    expect(appColorScheme({ style: { colorScheme: "light" } })).toBe("light");
    expect(appColorScheme({ style: { colorScheme: "" } })).toBe("light");
    expect(appColorScheme(null)).toBe("light");
    expect([0, 999, 1834, 524288].map(visualSizeLabel)).toEqual(["0 B", "999 B", "1.8 KB", "524 KB"]);
  });
});
