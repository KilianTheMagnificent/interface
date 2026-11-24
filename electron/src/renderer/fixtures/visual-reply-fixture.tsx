// Visual replies (T3 section 8) in a real sandboxed frame: the visual-reply
// lane's main process serves `iface-visual:` with the app's own frame store
// and navigation guard, and reads the page inside the frame; these helpers
// draw the card in a Threads-width column, open the overlay and say where the
// frame and its link are. The page comes from a fake journal answer; the
// frame registration is the real preload → main call. Test code only.
import { flushSync } from "react-dom";
import type { Root } from "react-dom/client";
import type { OutputPreviewSnapshot } from "../../shared/output-preview";
import { OutputPreviewPanel, type OutputPreviewPanelProps } from "../components/OutputPreviewPanel";
import { VisualReplyCard } from "../components/VisualReply";
import { conversationItems } from "../conversation-reducer";
import { visualFrameLoader } from "../visual-reply";
import { assert, frames, until } from "./fixture-support";

const ID = "v1731630000500-0a1b2c3d";

let page = "";

const loader = visualFrameLoader({
  threadJournal: async () => ({ visual: { id: ID, html: page } }),
  visualFrame: request => window.iface.visualFrame(request),
});

/** Draws the card for `html` in `theme`, and waits for its frame. */
export async function visualReplyFixture(root: Root, theme: "dark" | "light", html: string): Promise<unknown> {
  page = html;
  document.documentElement.dataset.theme = theme;
  document.documentElement.style.colorScheme = theme;
  flushSync(() => root.render(null));
  const [item] = conversationItems([{ kind: "visual.shown", id: ID, title: "Signups by week", bytes: html.length }]);
  assert(item?.kind === "visual", "visual.shown made no visual item");
  flushSync(() => root.render(
    <div className="visual-reply-fixture" style={{ width: 760, padding: "16px 20px", boxSizing: "border-box",
      background: "var(--pane-bg, var(--chrome-bg))", color: "var(--fg)", fontFamily: "var(--font-ui)" }}>
      <p style={{ margin: "0 0 10px" }}>Here is the chart you asked for.</p>
      <VisualReplyCard item={item} sessionKey="claude#fixture" loader={loader} />
      <p style={{ margin: 0 }}>Signups rose in five of the last seven weeks.</p>
    </div>,
  ));
  await until(() => document.querySelector(".visual-reply-fixture iframe") !== null,
    () => `The card drew no frame: ${document.querySelector(".visual-reply-fixture")?.textContent ?? ""}`);
  await frames();
  return visualReplyState();
}

/** The frame(s) on screen, their URL and sandbox, and the overlay's state. */
export function visualReplyState(): unknown {
  const all = [...document.querySelectorAll<HTMLIFrameElement>("iframe")];
  const box = (element: Element | null) => {
    if (element === null) return null;
    const rect = element.getBoundingClientRect();
    return { x: rect.x, y: rect.y, width: rect.width, height: rect.height };
  };
  return {
    frames: all.map(frame => ({ src: frame.getAttribute("src"), sandbox: frame.getAttribute("sandbox"),
      srcdoc: frame.hasAttribute("srcdoc"), inOverlay: frame.closest(".visual-reply-overlay") !== null,
      box: box(frame) })),
    overlay: box(document.querySelector(".visual-reply-overlay")),
    window: { width: innerWidth, height: innerHeight },
    card: box(document.querySelector(".visual-reply")),
    title: document.querySelector(".visual-reply-title")?.textContent ?? null,
  };
}

/** Presses a card or overlay button by its words. */
export async function visualReplyPress(words: string, inOverlay = false): Promise<void> {
  const scope = inOverlay ? ".visual-reply-overlay" : ".visual-reply-fixture";
  const button = [...document.querySelectorAll<HTMLButtonElement>(`${scope} .visual-reply-button`)]
    .find(candidate => candidate.textContent === words);
  assert(button, `No "${words}" button`);
  button.click();
  await frames();
}

/**
 * Output preview's Render: an `.html` file opens as source, and Render draws
 * it in the same visual frame. The file comes from a fake preview answer.
 */
export async function visualPreviewFixture(root: Root, theme: "dark" | "light", html: string): Promise<unknown> {
  document.documentElement.dataset.theme = theme;
  document.documentElement.style.colorScheme = theme;
  flushSync(() => root.render(null));
  const project = "/fixture/visuals";
  const snapshot: OutputPreviewSnapshot = { root: project, path: "chart.html", byteLength: html.length,
    revision: "c".repeat(64), content: { kind: "text", format: "plain", text: html } };
  const api: OutputPreviewPanelProps["api"] = {
    previewOutput: async request => {
      if (request.version === 1) return request.action === "list"
        ? { version: 1, kind: "directory", root: project, path: "", entries: [{ name: "chart.html", directory: false }] }
        : { version: 1, kind: "file", snapshot };
      if (request.action === "listReviews") return { version: 2, kind: "reviews", reviews: [] };
      if (request.action === "captureReview") return { version: 2, kind: "review", review: { id: "c".repeat(64),
        revision: 1, snapshot, anchor: { kind: "file" }, note: "", threadKey: null, task: "", updatedAt: 1, delivery: null } };
      throw new Error(`Unexpected preview operation ${request.action}`);
    },
    visualFrame: request => window.iface.visualFrame(request),
  };
  flushSync(() => root.render(
    <div style={{ width: 760, height: 720, display: "flex" }}>
      <OutputPreviewPanel root={project} visible api={api} onClose={() => {}} onOpenFolder={() => {}} />
    </div>,
  ));
  const panel = () => document.querySelector<HTMLElement>(".output-preview");
  const button = (text: string) => [...(panel()?.querySelectorAll<HTMLButtonElement>("button") ?? [])]
    .find(item => item.textContent === text);
  await until(() => button("chart.html") !== undefined, "The preview listed no chart.html");
  flushSync(() => button("chart.html")!.click());
  await until(() => button("Render") !== undefined, () => `An .html file offered no Render: ${panel()?.textContent ?? ""}`);
  const source = panel()!.querySelector(".output-source")?.textContent ?? "";
  assert(source.includes("<canvas"), "An .html file did not open as source first");
  assert(panel()!.querySelector("iframe") === null, "An .html file rendered before Render was pressed");
  flushSync(() => button("Render")!.click());
  await until(() => panel()!.querySelector("iframe") !== null, "Render drew no frame");
  await frames();
  return visualReplyState();
}

export function clearVisualReplyFixture(root: Root): void {
  flushSync(() => root.render(null));
}
