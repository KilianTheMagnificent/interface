import { readFileSync } from "node:fs";
import { createElement, isValidElement, type ReactElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { Composer } from "./components/Composer";
import { Conversation } from "./components/Conversation";
import { PromptContextChips } from "./components/PromptContextPicker";
import type { EngineEvent } from "../shared/engine";
import { conversationItems, type ConvItem } from "./conversation-reducer";
import { mergeTranscript, newPendingEcho, type PendingEcho } from "./transcript-merge";
import {
  extractPromptTurnAttachments, promptTurnAttachments, promptTurnImages,
  type PromptContextItem, type PromptTurnAttachment,
} from "./prompt-context";

/* BOTH HALVES OF "SHOW ME THE SCREENSHOT I AM SENDING", against the real
   components. Everything here renders the shipped component and reads the
   markup a browser would receive, or presses the exact handler the DOM would
   call — never the source text of a file, which is true of a feature nobody
   can reach. */

const PREVIEW = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUg==";
const OTHER = "data:image/png;base64,R0lGODlhAQABAIAAAP///w==";

const shot = (over: Partial<PromptContextItem> = {}): PromptContextItem => ({
  id: "ctx-shot", kind: "file", label: "Screenshot.png",
  value: "/tmp/Interface/pasted-images/a.png", previewUrl: PREVIEW, ...over,
});

const chipsMarkup = (items: readonly PromptContextItem[]): string =>
  renderToStaticMarkup(createElement(PromptContextChips, {
    items, onRemove: () => undefined,
  }));

/* PromptContextChips takes no hooks, so it can be called as the plain function
   it is and its element tree walked. That is what makes a click testable in a
   node environment: renderToStaticMarkup proves the control is DRAWN, and this
   proves the control is WIRED. A button that renders and does nothing is the
   exact shape of the bug this pair exists to catch. */
function* elements(node: unknown): Generator<ReactElement<Record<string, unknown>>> {
  if (Array.isArray(node)) {
    for (const child of node) yield* elements(child);
    return;
  }
  if (!isValidElement(node)) return;
  const element = node as ReactElement<Record<string, unknown>>;
  yield element;
  yield* elements(element.props.children);
}

function press(tree: unknown, accessibleName: string): void {
  for (const element of elements(tree)) {
    if (element.props["aria-label"] !== accessibleName) continue;
    const onClick = element.props.onClick;
    if (typeof onClick !== "function") break;
    (onClick as () => void)();
    return;
  }
  throw new Error(`no pressable control named ${JSON.stringify(accessibleName)}`);
}

describe("the composer draws the screenshot it is about to send", () => {
  it("renders the thumbnail itself, named for the file it is a picture of", () => {
    const markup = chipsMarkup([shot()]);
    expect(markup).toContain(`src="${PREVIEW}"`);
    expect(markup).toContain('alt="Preview of Screenshot.png"');
    // Not "image": every one of them is an image, so only the filename says
    // which of the three on the clipboard this chip is.
    expect(markup).not.toContain('alt="image"');
    expect(markup).toContain('class="prompt-context-chip has-thumb"');
  });

  it("keeps a named, keyboard-reachable remove control on the image chip", () => {
    const markup = chipsMarkup([shot()]);
    expect(markup).toContain('aria-label="Remove File context Screenshot.png"');
    // A <button>, so Tab reaches it and Enter/Space activate it for free.
    expect(markup).toMatch(
      /<button[^>]*aria-label="Remove File context Screenshot\.png"/);
  });

  it("names each attachment separately when several are pending", () => {
    const markup = chipsMarkup([
      shot(),
      shot({ id: "ctx-two", label: "Diagram.png", value: "/tmp/b.png", previewUrl: OTHER }),
    ]);
    expect(markup).toContain('alt="Preview of Screenshot.png"');
    expect(markup).toContain('alt="Preview of Diagram.png"');
    expect(markup).toContain('aria-label="Remove File context Diagram.png"');
  });

  it("still draws an ordinary chip for a non-image attachment", () => {
    const markup = chipsMarkup([
      { id: "ctx-code", kind: "file", label: "App.tsx", value: "/repo/App.tsx" },
    ]);
    expect(markup).not.toContain("<img");
    expect(markup).not.toContain('class="prompt-context-chip has-thumb"');
    expect(markup).toContain('aria-label="Remove File context App.tsx"');
  });

  it("PRESSES remove and reports the attachment that was pressed", () => {
    const removed: string[] = [];
    const tree = PromptContextChips({
      items: [shot(), shot({ id: "ctx-two", label: "Diagram.png", value: "/tmp/b.png" })],
      onRemove: id => { removed.push(id); },
    });
    press(tree, "Remove File context Diagram.png");
    expect(removed).toEqual(["ctx-two"]);
    press(tree, "Remove File context Screenshot.png");
    expect(removed).toEqual(["ctx-two", "ctx-shot"]);
  });

  it("refuses a preview that is not inline image bytes, at the chip boundary", () => {
    // The normalizer is the boundary; the chip is where a bad value would land
    // in the DOM as an <img src>. Neither a remote URL nor a script URL nor an
    // SVG document may become one.
    for (const hostile of ["https://tracker.example/pixel.png", "javascript:alert(1)",
      "data:image/svg+xml;base64,PHN2Zz48L3N2Zz4=", "data:text/html;base64,PGgxPg=="]) {
      const markup = chipsMarkup([shot({ previewUrl: hostile })]);
      expect(markup).not.toContain("<img");
      expect(markup).not.toContain(hostile);
    }
  });

  it("draws the thumbnail on the launch card the same way", () => {
    const markup = renderToStaticMarkup(createElement(Composer, {
      open: true,
      defaults: { engine: "claude", cwd: "/w/repo", prompt: "", model: "",
        effort: "default", permissionMode: "default" },
      engines: [{ id: "claude" }],
      choicesFor: () => ({ models: [], efforts: [], permissionModes: [] }),
      onLaunch: () => undefined,
      onBrowse: () => Promise.resolve(null),
      onClose: () => undefined,
      contextItems: [shot()],
      onContextRemove: () => undefined,
    }));
    expect(markup).toContain(`src="${PREVIEW}"`);
    expect(markup).toContain('alt="Preview of Screenshot.png"');
  });
});

const echo = (over: Partial<PendingEcho> = {}): PendingEcho => ({
  id: "1", text: "what is wrong with this layout?", seen: 0, at: 0,
  historyKnown: true, ...over,
});

const pane = (items: ConvItem[], echoes: readonly PendingEcho[]): string =>
  renderToStaticMarkup(createElement(Conversation, {
    items,
    thinking: false,
    live: true,
    sessionKey: "claude#a",
    pendingEchoes: echoes,
    config: { effort: null, models: [], canSetModel: true, effortLevels: [],
      showEffort: false, canSetEffort: true, pendingModel: false,
      pendingEffort: false, error: null },
    approvals: {},
    activeApprovalIds: new Set<string>(),
    onSend: () => undefined,
    onStop: () => undefined,
    onSetModel: () => undefined,
    onSetEffort: () => undefined,
    onApprove: () => undefined,
    onDeny: () => undefined,
  }));

const transcript = (echoes: readonly PendingEcho[]): string => pane([], echoes);

/* THE REPLAY HALF consumes the checked-in cross-language oracle. Rust's
   Claude and Codex history tests derive these exact EngineEvent arrays from
   the provider-shaped `input` beside them. This renderer test therefore starts
   at the native boundary it receives in production, without retaining a
   second TypeScript provider implementation just for tests. */
type ReplayCase = { input: unknown; events: EngineEvent[] };
const replayCases = JSON.parse(readFileSync(new URL(
  "../fixtures/provider-history-user-image-events.json",
  import.meta.url,
), "utf8")) as Record<string, ReplayCase>;
const replayTurn = (name: string): string =>
  pane(conversationItems(replayCases[name]?.events ?? []), []);

describe("the transcript draws the screenshot once it is sent", () => {
  it("shows the picture beside the words, the instant the message is echoed", () => {
    const markup = transcript([echo({
      images: [{ src: PREVIEW, label: "Screenshot.png" }],
    })]);
    expect(markup).toContain("what is wrong with this layout?");
    expect(markup).toContain(`src="${PREVIEW}"`);
    expect(markup).toContain('alt="Attached image Screenshot.png"');
    expect(markup).toContain("conv-turn-images");
  });

  it("draws every attached image, each named for its own file", () => {
    const markup = transcript([echo({
      images: [
        { src: PREVIEW, label: "Before.png" },
        { src: OTHER, label: "After.png" },
      ],
    })]);
    expect(markup).toContain('alt="Attached image Before.png"');
    expect(markup).toContain('alt="Attached image After.png"');
  });

  it("leaves a plain message exactly as it was: no empty picture row", () => {
    const markup = transcript([echo()]);
    expect(markup).toContain("what is wrong with this layout?");
    expect(markup).not.toContain("conv-turn-images\"");
    expect(markup).not.toContain("Attached image");
  });

  it("still says a message with a picture was not delivered", () => {
    const markup = transcript([echo({
      images: [{ src: PREVIEW, label: "Screenshot.png" }],
      error: "the engine did not accept it",
    })]);
    expect(markup).toContain('alt="Attached image Screenshot.png"');
    expect(markup).toContain("Not delivered — the engine did not accept it");
  });

  it("carries only attachments that actually have a picture to draw", () => {
    // An image whose thumbnail could not be produced is still attached and
    // still sent; it simply has nothing to show beside the message.
    const images = promptTurnImages([
      shot(),
      { id: "ctx-code", kind: "file", label: "App.tsx", value: "/repo/App.tsx" },
      shot({ id: "ctx-heic", label: "Photo.heic", value: "/tmp/p.heic",
        previewUrl: undefined }),
    ]);
    expect(images).toEqual([{ src: PREVIEW, label: "Screenshot.png" }]);
    const markup = transcript([echo({ images })]);
    expect(markup).toContain('alt="Attached image Screenshot.png"');
    expect(markup).not.toContain("App.tsx");
  });
});

describe("images ride the echo into the merged transcript", () => {
  it("puts the attachments on the user row the echo becomes", () => {
    const images = [{ src: PREVIEW, label: "Screenshot.png" }];
    const rows = mergeTranscript([], [], [echo({ images })]);
    expect(rows).toEqual([{
      kind: "item", key: "e:1", origin: "echo",
      item: { kind: "user", id: "echo-1", text: "what is wrong with this layout?", images },
    }]);
  });

  it("adds no field at all to a message with nothing attached", () => {
    const [row] = mergeTranscript([], [], [echo()]);
    expect(row).toMatchObject({ kind: "item" });
    const item = (row as { item: Record<string, unknown> }).item;
    expect("images" in item).toBe(false);
    expect(mergeTranscript([], [], [echo({ images: [] })])[0])
      .toEqual({ kind: "item", key: "e:1", origin: "echo",
        item: { kind: "user", id: "echo-1", text: "what is wrong with this layout?" } });
  });

  it("leaves engine-replayed history untouched: a replayed turn carries no picture", () => {
    // The honest half of this feature. What the engine received was a PATH, and
    // no provider replays image data, so a reloaded transcript has the words and
    // the attachment reference and no thumbnail.
    const rows = mergeTranscript(
      [{ kind: "user", id: "user-0", text: "what is wrong with this layout?" }],
      [], []);
    expect(rows).toEqual([{
      kind: "item", key: "h:user-0", origin: "history",
      item: { kind: "user", id: "user-0", text: "what is wrong with this layout?" },
    }]);
  });
});

describe("the echo a send builds", () => {
  const base = { id: "1", text: "look at this", seen: 0, at: 3, historyKnown: true };

  it("carries the composer's pictures onto the message that was just sent", () => {
    const images = [{ src: PREVIEW, label: "Screenshot.png" }];
    expect(newPendingEcho({ ...base, images })).toEqual({ ...base, images });
    // ...and all the way through to the drawn row.
    expect(mergeTranscript([], [], [newPendingEcho({ ...base, at: 0, images })]))
      .toEqual([{ kind: "item", key: "e:1", origin: "echo",
        item: { kind: "user", id: "echo-1", text: "look at this", images } }]);
  });

  it("leaves a message with no attachments exactly as it was", () => {
    expect(newPendingEcho(base)).toEqual(base);
    expect("images" in newPendingEcho({ ...base, images: [] })).toBe(false);
  });

  it("keeps every other field of the echo intact", () => {
    expect(newPendingEcho({ ...base, seen: 2, at: 7, historyKnown: false }))
      .toEqual({ id: "1", text: "look at this", seen: 2, at: 7, historyKnown: false });
  });
});

/* END TO END from Rust-produced EngineEvent fixtures. Neither pane is handed
   an echo, so every <img> below came from a provider replay mapping rather
   than this window's optimistic send state. */
describe("the transcript draws a screenshot the ENGINE replayed", () => {
  const SHOT = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUg==";

  it("shows a Claude paste after a reload, exactly as the echo drew it", () => {
    const markup = replayTurn("claudeInline");
    expect(markup).toContain("what is wrong with this layout?");
    expect(markup).toContain(`<img class="conv-turn-image" src="${SHOT}" alt="Attached image 1"/>`);
    expect(markup).toContain('class="conv-turn-images"');
    // A REPLAYED turn, not an echoed one: mergeTranscript namespaces the two
    // lists, and an echo would key `e:`. This proves which half drew it.
    expect(markup).not.toContain("Not delivered");
  });

  it("shows a Codex inline image, with the [image] placeholder GONE", () => {
    const markup = replayTurn("codexInline");
    expect(markup).toContain("why is this broken?");
    expect(markup).toContain(`<img class="conv-turn-image" src="${SHOT}" alt="Attached image 1"/>`);
    // The literal must not survive beside the picture it stood in for.
    expect(markup).not.toContain("[image]");
  });

  it("draws a turn that was a picture and no words at all", () => {
    const markup = replayTurn("claudeImageOnly");
    expect(markup).toContain('alt="Attached image 1"');
    expect(markup).toContain("conv-turn-user");
  });

  it("keeps a replayed prompt with no attachment exactly as it was", () => {
    const markup = replayTurn("claudePlain");
    expect(markup).toContain("just words");
    // The class ATTRIBUTE, not the name: the component inlines its own
    // stylesheet, so `.conv-turn-images` is in the output either way.
    expect(markup).not.toContain('class="conv-turn-images"');
    expect(markup).not.toContain("Attached image");
  });

  it("puts NO <img> in the pane for a remote image the native boundary refused", () => {
    const markup = replayTurn("claudeRefusedRemote");
    expect(markup).toContain("look");
    expect(markup).not.toContain("<img");
    expect(markup).not.toContain("tracker.test");
  });

  it("leaves a Codex localImage as its path, with no picture and no <img>", () => {
    const markup = replayTurn("codexLocalImage");
    expect(markup).toContain("[image: /tmp/shot.png]");
    expect(markup).not.toContain("<img");
  });

  it("opens composer chips and sent-turn attachments in the in-app preview", () => {
    const previewedChips: PromptContextItem[] = [];
    const codeItem: PromptContextItem = {
      id: "ctx-code", kind: "file", label: "App.tsx", value: "/repo/App.tsx",
    };
    const urlItem: PromptContextItem = {
      id: "ctx-url", kind: "url", label: "https://example.com", value: "https://example.com",
    };
    const chipTree = PromptContextChips({
      items: [shot(), codeItem, urlItem],
      onRemove: () => undefined,
      onPreview: item => { previewedChips.push(item); },
    });
    const chipHtml = renderToStaticMarkup(chipTree);
    expect(chipHtml).toContain('aria-label="Preview File context Screenshot.png"');
    expect(chipHtml).toContain('aria-label="Preview File context App.tsx"');
    expect(chipHtml).not.toContain('aria-label="Preview URL context https://example.com"');
    press(chipTree, "Preview File context Screenshot.png");
    press(chipTree, "Preview File context App.tsx");
    expect(previewedChips).toEqual([shot(), codeItem]);

    const attachments = promptTurnAttachments([shot(), codeItem, urlItem]);
    expect(attachments).toEqual([
      { kind: "file", label: "Screenshot.png", path: "/tmp/Interface/pasted-images/a.png", previewUrl: PREVIEW },
      { kind: "file", label: "App.tsx", path: "/repo/App.tsx" },
    ]);
    expect(extractPromptTurnAttachments(
      'Check this:\n- File path reference: "/Users/me/Desktop/spec.pdf"\n[image: /tmp/shot.png]',
    )).toEqual([
      { kind: "file", label: "spec.pdf", path: "/Users/me/Desktop/spec.pdf" },
      { kind: "file", label: "shot.png", path: "/tmp/shot.png" },
    ]);

    const turnMarkup = renderToStaticMarkup(createElement(Conversation, {
      items: [],
      thinking: false,
      live: true,
      sessionKey: "claude#a",
      pendingEchoes: [echo({
        images: [{ src: PREVIEW, label: "Screenshot.png" }],
        attachments,
      })],
      config: { effort: null, models: [], canSetModel: true, effortLevels: [],
        showEffort: false, canSetEffort: true, pendingModel: false,
        pendingEffort: false, error: null },
      approvals: {},
      activeApprovalIds: new Set<string>(),
      onSend: () => undefined,
      onStop: () => undefined,
      onSetModel: () => undefined,
      onSetEffort: () => undefined,
      onApprove: () => undefined,
      onDeny: () => undefined,
      onPreviewAttachment: (_attachment: PromptTurnAttachment) => undefined,
    }));
    expect(turnMarkup).toContain('class="conv-turn-image is-clickable"');
    expect(turnMarkup).toContain('title="Preview Screenshot.png"');
    expect(turnMarkup).toContain('aria-label="Preview file App.tsx"');
  });

  it("turns local file links in assistant responses into clickable preview links", () => {
    const assistantMarkup = renderToStaticMarkup(createElement(Conversation, {
      items: [{
        kind: "text",
        id: "assistant-1",
        text: "I generated [report.md](file:///repo/docs/report.md#L12) and [diagram.png](/repo/docs/diagram.png).",
        streaming: false,
      }],
      thinking: false,
      live: true,
      sessionKey: "claude#a",
      pendingEchoes: [],
      config: { effort: null, models: [], canSetModel: true, effortLevels: [],
        showEffort: false, canSetEffort: true, pendingModel: false,
        pendingEffort: false, error: null },
      approvals: {},
      activeApprovalIds: new Set<string>(),
      onSend: () => undefined,
      onStop: () => undefined,
      onSetModel: () => undefined,
      onSetEffort: () => undefined,
      onApprove: () => undefined,
      onDeny: () => undefined,
      onPreviewAttachment: (_attachment: PromptTurnAttachment) => undefined,
    }));
    expect(assistantMarkup).toContain(
      '<a class="conv-md-link" href="#preview-file" title="Preview /repo/docs/report.md">report.md</a>',
    );
    expect(assistantMarkup).toContain(
      '<a class="conv-md-link" href="#preview-file" title="Preview /repo/docs/diagram.png">diagram.png</a>',
    );
  });
});
