import { flushSync } from "react-dom";
import type { Root } from "react-dom/client";
import type { TerminalSummary, TerminalWorkspaceRecord } from "../../shared/ipc";
import type { OutputPreviewSnapshot, OutputReview } from "../../shared/output-preview";
import { OutputPreviewPanel, type OutputPreviewPanelProps } from "../components/OutputPreviewPanel";
import { IntegratedTerminalCenter, type TerminalRendererApi } from "../components/IntegratedTerminalCenter";
import { APP_SELECT_CSS } from "../components/AppSelect";
import type { TerminalWorkspaceSwitcher } from "../components/TerminalWorkspace";
import { EMPTY_TERMINAL_WORKSPACE_STATE, openTerminalTab, serializeTerminalWorkspaceState } from "../terminal-workspace";
import { contrastRatio } from "../contrast";
import { assert, frames, untilWithin, wait } from "./fixture-support";

const until = untilWithin({ timeoutMs: 6000 });
function input(element: HTMLInputElement | HTMLTextAreaElement, text: string) {
  const prototype = element instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
  Object.getOwnPropertyDescriptor(prototype, "value")!.set!.call(element, text);
  element.dispatchEvent(new Event("input", { bubbles: true }));
}
function pdf(): number[] {
  const objects = ["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>"];
  for (let n = 0; n < 2; n++) {
    const text = `0.2 0.45 0.4 rg 30 220 240 70 re f BT /F1 18 Tf 30 170 Td (Preview page ${String(n + 1)}) Tj ET`;
    objects.push(`<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 360] /Resources << /Font << /F1 7 0 R >> >> /Contents ${String(4 + n * 2)} 0 R >>`,
      `<< /Length ${String(text.length)} >>\nstream\n${text}\nendstream`);
  }
  objects.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>");
  let result = "%PDF-1.4\n";
  const offsets = [0];
  objects.forEach((object, i) => { offsets.push(result.length); result += `${String(i + 1)} 0 obj\n${object}\nendobj\n`; });
  const xref = result.length;
  result += `xref\n0 8\n0000000000 65535 f \n${offsets.slice(1).map(n => `${String(n).padStart(10, "0")} 00000 n \n`).join("")}trailer\n<< /Size 8 /Root 1 0 R >>\nstartxref\n${String(xref)}\n%%EOF`;
  return [...new TextEncoder().encode(result)];
}
let slot: HTMLElement | null = null;
export function clearOutputPreviewFixture(root: Root) { flushSync(() => root.render(null)); slot?.remove(); slot = null; }

/** Delayed durable saves must finish before any panel exit or version replacement. */
async function outputDraftCloseFixture(root: Root): Promise<void> {
  const project = "/fixture/draft-close";
  let changed = false, failSave = false, closes = 0;
  let releaseSave: (() => void) | null = null;
  const saved = new Map<string, OutputReview>();
  const snapshot = (): OutputPreviewSnapshot => ({ root: project, path: "note.md", byteLength: 4,
    revision: (changed ? "b" : "a").repeat(64), content: { kind: "text", format: "markdown", text: changed ? "New version" : "Original version" } });
  const api: OutputPreviewPanelProps["api"] = { previewOutput: async request => {
    if (request.version === 1) return request.action === "list"
      ? { version: 1, kind: "directory", root: project, path: "", entries: [{ name: "note.md", directory: false }] }
      : { version: 1, kind: "file", snapshot: snapshot() };
    if (request.action === "listReviews") return { version: 2, kind: "reviews", reviews: [...saved.values()] };
    if (request.action === "captureReview") {
      const value = snapshot(), id = value.revision!;
      const review: OutputReview = saved.get(id) ?? { id, revision: 1, snapshot: value, anchor: { kind: "file" }, note: "", threadKey: null, task: "", updatedAt: 1, delivery: null };
      saved.set(id, review); return { version: 2, kind: "review", review };
    }
    if (request.action !== "saveReview") throw new Error("Unexpected draft test operation");
    await new Promise<void>(resolve => { releaseSave = resolve; });
    if (failSave) throw new Error("Draft storage is unavailable");
    const previous = saved.get(request.id)!;
    assert(previous.revision === request.revision, "Draft close used a stale version");
    const review = { ...previous, revision: previous.revision + 1, note: request.note, anchor: request.anchor, threadKey: request.threadKey, task: request.task };
    saved.set(review.id, review); return { version: 2, kind: "review", review };
  } };
  const panel = () => document.querySelector<HTMLElement>(".output-preview")!;
  const button = (text: string) => [...panel().querySelectorAll<HTMLButtonElement>("button")]
    .find(item => item.textContent === text || item.getAttribute("aria-label") === text)!;
  const textarea = () => panel().querySelector<HTMLTextAreaElement>("textarea")!;
  const close = () => panel().querySelector<HTMLButtonElement>('[aria-label="Close output preview"]')!;
  const release = () => { const resolve = releaseSave; releaseSave = null; assert(resolve !== null, "No draft save to release"); resolve(); };
  flushSync(() => root.render(<OutputPreviewPanel root={project} visible api={api} onClose={() => { closes += 1; }} onOpenFolder={() => {}} />));
  await until(() => button("note.md") !== undefined, "Draft fixture files did not list");
  flushSync(() => button("note.md").click());
  await until(() => textarea() !== null, "Draft fixture did not open");
  flushSync(() => input(textarea(), "Keep the final keystroke."));
  flushSync(() => close().click());
  await until(() => releaseSave !== null, "Immediate close did not flush its draft");
  assert(Number(closes) === 0 && textarea().disabled, "The panel closed or stayed editable during its durable save");
  let unload = new Event("beforeunload", { cancelable: true }); window.dispatchEvent(unload);
  assert(unload.defaultPrevented, "An in-flight draft save did not guard window close");
  release();
  await until(() => closes === 1, "Panel did not close after saving");
  assert(saved.get("a".repeat(64))?.note === "Keep the final keystroke.", "Immediate close lost the latest note");
  failSave = true;
  flushSync(() => input(textarea(), "Keep this even when storage fails."));
  flushSync(() => close().click());
  await until(() => releaseSave !== null, "Failed-save case did not try to preserve the note");
  release();
  await until(() => panel().textContent!.includes("Draft storage is unavailable") && !textarea().disabled, "Failed save did not restore editable draft");
  assert(Number(closes) === 1 && textarea().value === "Keep this even when storage fails.", "Save refusal discarded the unsaved draft");
  unload = new Event("beforeunload", { cancelable: true }); window.dispatchEvent(unload);
  assert(unload.defaultPrevented, "A refused draft save did not guard window close");
  failSave = false;
  flushSync(() => close().click());
  await until(() => releaseSave !== null, "Draft close could not retry after storage recovered");
  release(); await until(() => closes === 2, "Retried draft was not saved before close");
  changed = true;
  flushSync(() => button("Check for changes").click());
  await until(() => button("Load latest") !== undefined, "Changed output was not detected");
  flushSync(() => input(textarea(), "Preserve this on the reviewed version."));
  flushSync(() => button("Load latest").click());
  await until(() => releaseSave !== null, "Version transition did not save its prior draft");
  assert(textarea().disabled && panel().querySelector<HTMLButtonElement>('[aria-label^="Saved output version"]')!.disabled,
    "Delayed version transition allowed new edits to be discarded");
  release();
  await until(() => textarea().value === "" && panel().textContent!.includes("New version"), "Latest version did not open after preserving its predecessor");
  assert(saved.get("a".repeat(64))?.note === "Preserve this on the reviewed version.", "Version transition lost the prior note");
  flushSync(() => root.render(null));
}

export async function outputPreviewFixture(root: Root, theme: string, chrome: string): Promise<unknown> {
  clearOutputPreviewFixture(root);
  document.documentElement.dataset.theme = theme;
  document.documentElement.dataset.chrome = chrome;
  await outputDraftCloseFixture(root);
  window.localStorage.removeItem("interface.integrated-terminals.agent-overview.v1");
  const project = "/fixture/preview", other = "/fixture/other";
  const stamp = "2026-09-24T00:00:00Z";
  const terminals: TerminalSummary[] = [project, other].map((cwd, i) => ({
    id: `00000000-0000-4000-8000-00000000098${String(i)}`, generation: 1, title: `Preview agent ${String(i + 1)}`,
    root: cwd, cwd, state: "running", size: { columns: 80, rows: 24 },
    launch: { version: 1, profileId: "shell" }, createdAt: stamp, updatedAt: stamp,
  }));
  const workspaces: TerminalWorkspaceRecord[] = terminals.map((terminal, i) => ({
    id: `preview-workspace-${String(i)}`, name: `Review ${String(i + 1)}`, createdAt: stamp, updatedAt: stamp, terminals: [terminal],
    layout: serializeTerminalWorkspaceState(openTerminalTab(EMPTY_TERMINAL_WORKSPACE_STATE, `preview-pane-${String(i)}`, terminal.id)),
  }));
  // The largest diff the host reads (output_preview.rs: 10,000 lines), so
  // typing feedback beneath it shows whether the lines are rebuilt per key.
  const diffLines = ["@@ -1,5000 +1,5000 @@", ...Array.from({ length: 9_999 },
    (_, i) => i % 2 === 0 ? `-Previous line ${String(i)}` : `+Improved line ${String(i)}`)];
  const canvas = document.createElement("canvas"); canvas.width = 360; canvas.height = 180;
  const context = canvas.getContext("2d")!; context.fillStyle = "#317868"; context.fillRect(0, 0, 170, 180);
  context.fillStyle = "#c6a56c"; context.fillRect(190, 0, 170, 180);
  const bytes = [...atob(canvas.toDataURL("image/png").split(",")[1])].map(c => c.charCodeAt(0));
  let changed = false;
  let switcher: TerminalWorkspaceSwitcher | null = null;
  let copied = "";
  const inputs: string[] = [], errors: string[] = [];
  const unexpected = (): never => { throw new Error("Preview attempted a provider or native-app operation"); };
  const snapshots = (path: string, root: string): OutputPreviewSnapshot => ({
    root, path, byteLength: 150, revision: path === "deck.pptx" ? null : changed && path === "report.md" ? "b".repeat(64) : "a".repeat(64),
    content: path === "report.md" ? { kind: "text", format: "markdown", text: changed ? "# New report\n\nChanged by the agent." : "# Launch report\n\nSelect this paragraph for a revision.\n\n- Clear evidence\n- Linked feedback" }
      : path === "layout.png" ? { kind: "image", mediaType: "image/png", bytes }
        : path === "brief.pdf" ? { kind: "pdf", bytes: pdf() }
          : path === "changes.patch" ? { kind: "text", format: "diff", text: diffLines.join("\n") }
            : path === "untrusted.html" ? { kind: "text", format: "plain", text: "<script>window.__outputScriptRan=true</script><h1>Untrusted output</h1>" }
              : { kind: "unsupported", reason: "Open this format in its own application." },
  });
  const reviews = new Map<string, OutputReview>();
  const feedback: { key: string; text: string }[] = [];
  const noop = () => {};
  const api: TerminalRendererApi = {
    listTerminals: async () => ({ revision: 1, terminals }), getTerminalWorkspaces: async () => workspaces,
    setTerminalWorkspaces: async records => records, listTerminalLaunchProfiles: async () => [{ id: "shell", label: "Terminal", available: true }],
    onTerminalData: () => noop, onTerminalLifecycle: () => noop, onTerminalDataFailure: () => noop,
    sendTerminalInput: (_id, _generation, data) => { inputs.push(new TextDecoder().decode(data)); },
    resizeTerminal: async request => ({ ...terminals.find(t => t.id === request.id)!, size: request.size }),
    createTerminal: unexpected, closeTerminal: unexpected, dismissTerminal: unexpected,
    pathForFile: unexpected, pickDirectory: unexpected, savePastedImages: unexpected, startAdversarialReview: unexpected,
    previewOutput: async request => {
      if (request.version === 1) return request.action === "list"
        ? { version: 1, kind: "directory", root: request.root, path: request.path,
            entries: ["report.md", "layout.png", "brief.pdf", "changes.patch", "untrusted.html", "deck.pptx"].map(name => ({ name, directory: false })) }
        : { version: 1, kind: "file", snapshot: snapshots(request.path, request.root) };
      if (request.action === "listReviews") return { version: 2, kind: "reviews", reviews: [...reviews.values()].filter(review => review.snapshot.root === request.root) };
      if (request.action === "captureReview") {
        const snapshot = snapshots(request.path, request.root), id = `${request.root}:${request.path}:${snapshot.revision}`;
        const review = reviews.get(id) ?? { id, revision: 1, snapshot, note: "", anchor: { kind: "file" }, threadKey: null, task: "", updatedAt: Date.now(), delivery: null };
        reviews.set(id, review); return { version: 2, kind: "review", review };
      }
      const old = reviews.get(request.id)!;
      assert(old.revision === request.revision, "Fixture received a stale write");
      if (request.action === "deleteReview") { reviews.delete(request.id); return { version: 2, kind: "deletedReview", id: request.id }; }
      const review: OutputReview = request.action === "saveReview" ? { ...old, note: request.note, anchor: request.anchor, threadKey: request.threadKey, task: request.task, revision: old.revision + 1 }
        : { ...old, revision: old.revision + 1, delivery: request.action === "reserveFeedback" ? "sending" : request.delivery };
      reviews.set(review.id, review); return { version: 2, kind: "review", review };
    },
  };
  const copyDescriptor = Object.getOwnPropertyDescriptor(navigator.clipboard, "writeText");
  Object.defineProperty(navigator.clipboard, "writeText", { configurable: true, value: async (text: string) => { copied = text; } });
  slot = document.createElement("div"); document.body.appendChild(slot);
  const panel = () => document.querySelector<HTMLElement>(".output-preview")!;
  const button = (label: string) => [...panel().querySelectorAll<HTMLButtonElement>("button")]
    .find(b => b.textContent === label || b.getAttribute("aria-label") === label)!;
  const toggle = () => slot!.querySelector<HTMLButtonElement>('button[aria-label="Output preview"]')!;
  const browse = () => panel().querySelector<HTMLButtonElement>('button[aria-label="Browse project files"]')!;
  const closePreview = async () => {
    await until(() => button("Save draft")?.disabled !== true, "A draft save never settled before close");
    flushSync(() => panel().querySelector<HTMLButtonElement>('button[aria-label="Close output preview"]')!.click());
    await until(() => panel().hidden === true, "Preview closed before its drafts settled or never completed closing");
  };
  const choose = async (name: string) => {
    if (browse().getAttribute("aria-pressed") !== "true") flushSync(() => browse().click());
    flushSync(() => button(name).click());
    await until(() => panel().querySelector(".output-file-head strong")?.textContent === name, `Did not open ${name}`);
    // Let the new file's passive effects settle before the next synthetic
    // click; seeing its header alone can precede its check-request reset.
    await frames();
  };
  try {
    flushSync(() => root.render(<div style={{ height: "100vh", display: "flex" }}>
      <style>{APP_SELECT_CSS}</style>
      <IntegratedTerminalCenter api={api} activeCwd={project} visible toolbarSlot={slot}
        outputFeedbackTargets={[{ key: "claude#review", title: "Review thread" }]}
        onSendOutputFeedback={async (key, text) => { feedback.push({ key, text }); return "sent"; }}
        onSwitcherChange={value => { switcher = value; }} onError={message => errors.push(message)} onOpenWorkspaceTool={unexpected} />
    </div>));
    await until(() => switcher !== null && toggle() !== null, "Terminal workspace did not mount");
    flushSync(() => switcher!.onSelect(workspaces[0].id));
    await until(() => document.querySelector(".terminal-xterm .xterm-helper-textarea") !== null, "xterm did not mount");
    const terminalElement = document.querySelector(".terminal-xterm")!;
    const terminalInput = terminalElement.querySelector<HTMLTextAreaElement>(".xterm-helper-textarea")!;
    flushSync(() => toggle().click());
    await until(() => button("report.md") !== undefined, "Files did not list");
    await choose("report.md");
    assert(panel().querySelector("h1")?.textContent === "Launch report", "Markdown was not rendered");
    const paint = document.createElement("canvas").getContext("2d", { willReadFrequently: true })!;
    const painted = (value: string, under = "#000000"): string => {
      paint.fillStyle = under; paint.fillRect(0, 0, 1, 1);
      paint.fillStyle = value; paint.fillRect(0, 0, 1, 1);
      return `#${[...paint.getImageData(0, 0, 1, 1).data].slice(0, 3).map(v => v.toString(16).padStart(2, "0")).join("")}`;
    };
    const panelStyle = getComputedStyle(panel());
    const background = painted(panelStyle.backgroundColor);
    assert(contrastRatio(painted(panelStyle.color, background), background) >= 4.5,
      "Output review text is unreadable against its panel in this theme");
    const paragraph = panel().querySelector(".conv-markdown p")!;
    const range = document.createRange(); range.selectNodeContents(paragraph);
    window.getSelection()!.removeAllRanges(); window.getSelection()!.addRange(range);
    paragraph.dispatchEvent(new MouseEvent("mouseup", { bubbles: true }));
    await until(() => panel().querySelector("blockquote") !== null, "Text selection did not attach");
    flushSync(() => input(panel().querySelector("textarea")!, "Make this introduction clearer."));
    flushSync(() => button("Copy feedback").click());
    await until(() => copied.includes("Make this introduction clearer."), "Feedback did not copy");
    assert(copied.includes("a".repeat(64)) && copied.includes("Select this paragraph"), "Feedback lost its version or quote");
    await until(() => [...reviews.values()].some(review => review.note === "Make this introduction clearer."), "Feedback draft was not saved durably");
    flushSync(() => panel().querySelector<HTMLButtonElement>('[aria-label^="Output feedback thread"]')!.click());
    const reviewThread = [...document.querySelectorAll<HTMLElement>('.app-select-popover [role="option"]')]
      .find(option => option.textContent?.includes("Review thread") === true);
    assert(reviewThread !== undefined, "The feedback thread picker did not list its threads");
    flushSync(() => reviewThread.click());
    await until(() => button("Send feedback")?.disabled === false, "Feedback target did not enable send");
    flushSync(() => button("Send feedback").click());
    await until(() => feedback.length === 1 && [...reviews.values()].some(review => review.delivery === "sent"), "Feedback delivery did not settle");
    assert(feedback[0].key === "claude#review" && feedback[0].text.includes("a".repeat(64)), "Feedback lost the selected target or immutable version");
    assert(button("Send feedback").disabled, "Already sent feedback could be replayed");
    changed = true;
    flushSync(() => button("Check for changes").click());
    await until(() => panel().textContent!.includes("The file changed."), "Stale version was not identified");
    assert(panel().querySelector("h1")!.textContent === "Launch report", "Refresh silently replaced the reviewed version");
    await closePreview();
    assert(terminalElement.isConnected && document.querySelector(".terminal-xterm") === terminalElement, "Preview closing remounted the terminal");
    flushSync(() => toggle().click());
    await until(() => panel().querySelector("h1") !== null, "Review did not reopen");
    assert(panel().querySelector("textarea")!.value === "Make this introduction clearer.", "Closing preview lost the draft");
    flushSync(() => button("Compare versions").click());
    assert(panel().querySelector(".output-comparison")!.textContent!.includes("Launch report")
      && panel().querySelector(".output-comparison")!.textContent!.includes("New report"), "Comparison did not show both immutable versions");
    flushSync(() => button("Load latest").click());
    await until(() => panel().querySelector("h1")?.textContent === "New report", "Latest version did not load explicitly");
    assert([...reviews.values()].some(review => review.note === "Make this introduction clearer."), "Loading a revision destroyed earlier feedback");
    assert(panel().querySelector("textarea")!.value === "", "An old draft moved to a new version");

    await choose("untrusted.html");
    assert(panel().querySelector(".output-content script") === null && panel().querySelector(".output-source")!.textContent!.includes("<script>"), "HTML became executable DOM");
    await choose("changes.patch");
    assert(panel().querySelectorAll(".output-source.is-diff > span").length === diffLines.length
      && panel().querySelectorAll(".is-added").length === diffLines.filter(line => line.startsWith("+")).length
      && panel().querySelectorAll(".is-removed").length === diffLines.filter(line => line.startsWith("-")).length
      && panel().querySelectorAll(".is-hunk").length === 1, "Diff was not rendered line by line");
    // Typing feedback beneath the diff must not rebuild its 10,000 lines.
    const firstDiffLine = panel().querySelector(".output-source.is-diff > span");
    let diffKeystrokeMs = 0;
    for (const note of ["T", "Ti", "Tig", "Tight", "Tighten", "Tighten this hunk."]) {
      const started = performance.now();
      flushSync(() => input(panel().querySelector("textarea")!, note));
      diffKeystrokeMs = Math.max(diffKeystrokeMs, performance.now() - started);
    }
    assert(panel().querySelector("textarea")!.value === "Tighten this hunk."
      && panel().querySelector(".output-source.is-diff > span") === firstDiffLine, "Feedback typing disturbed the diff");
    flushSync(() => input(panel().querySelector("textarea")!, ""));
    await choose("deck.pptx"); assert(panel().textContent!.includes("Open project in Finder"), "Unsupported format had no honest fallback");
    flushSync(() => button("Check for changes").click());
    await until(() => panel().textContent!.includes("Preview refreshed."), "Unsupported preview did not refresh");
    assert(!panel().textContent!.includes("The file changed."), "An unversioned format falsely reported a change");
    await choose("layout.png");
    await until(() => panel().querySelector<HTMLImageElement>("img")?.naturalWidth === 360, "Image was not decoded");
    const target = panel().querySelector<HTMLElement>(".output-image")!;
    const box = target.getBoundingClientRect();
    // Synthetic pointer events have no browser pointer capture; drive the same
    // handlers with a local capture shim, never the owner's mouse.
    target.setPointerCapture = noop; target.hasPointerCapture = () => false;
    target.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, button: 0, pointerId: 1, clientX: box.left + 20, clientY: box.top + 20 }));
    target.dispatchEvent(new PointerEvent("pointerup", { bubbles: true, button: 0, pointerId: 1, clientX: box.left + 120, clientY: box.top + 90 }));
    await until(() => panel().querySelector(".output-image-region") !== null, "Image region did not attach");
    await choose("brief.pdf");
    await until(() => (panel().querySelector(".output-pdf canvas") as HTMLCanvasElement | null)?.width !== 300
      && !panel().textContent!.includes("Rendering page") && button("Comment on page")?.disabled === false, "PDF page did not render");
    flushSync(() => panel().querySelector<HTMLButtonElement>('button[aria-label="Next PDF page"]')!.click());
    await until(() => button("Comment on page")?.disabled === false, "Second page did not render");
    flushSync(() => button("Comment on page").click());
    await until(() => panel().querySelector(".output-feedback-label")!.textContent!.includes("Page 2"), "PDF feedback did not name page 2");
    flushSync(() => input(panel().querySelector("textarea")!, "Revise the second page."));
    flushSync(() => panel().querySelector<HTMLButtonElement>('button[aria-label="Previous PDF page"]')!.click());
    assert(panel().querySelector(".output-feedback-label")!.textContent!.includes("Page 2"), "Page navigation silently rebound feedback");
    flushSync(() => switcher!.onSelect(workspaces[1].id));
    await wait(60);
    assert(panel().hidden, "Other workspace inherited this project's preview");
    flushSync(() => toggle().click());
    await until(() => button("report.md") !== undefined, "Other project did not list");
    assert(panel().querySelector(".output-tabs") === null, "Other project inherited open files");
    flushSync(() => switcher!.onSelect(workspaces[0].id));
    await until(() => panel().querySelector("textarea")?.value === "Revise the second page.", "Project switch lost feedback");
    await closePreview();
    await until(() => terminalElement.isConnected, "Terminal identity was lost on project return");
    terminalInput.focus();
    terminalInput.dispatchEvent(new KeyboardEvent("keydown", { key: "x", code: "KeyX", keyCode: 88, bubbles: true }));
    assert(inputs.includes("x"), "Terminal input stopped after preview/project changes");
    flushSync(() => toggle().click());
    await until(() => panel().hidden === false, "Preview did not reopen for screenshot");
    await until(() => button("Comment on page")?.disabled === false, "Reopened PDF did not render");
    const pdfCanvas = panel().querySelector<HTMLCanvasElement>(".output-pdf canvas")!;
    const pixels = pdfCanvas.getContext("2d")!.getImageData(0, 0, pdfCanvas.width, pdfCanvas.height).data;
    assert(pixels.some((value, i) => i % 4 === 0 && value < 220 && pixels[i + 3] > 0), "PDF canvas contained no rendered content");
    const bounds = panel().getBoundingClientRect();
    assert(bounds.right <= window.innerWidth + 1 && bounds.left >= -1, `Preview overflowed the window: ${JSON.stringify({left: bounds.left, right: bounds.right, width: window.innerWidth, theme, chrome})}`);
    assert(panel().scrollWidth <= panel().clientWidth + 1, "Preview chrome overflowed horizontally");
    assert(errors.length === 0, errors.join("; "));
    // The toolbar slot sits below the 100vh column; a picker's focus can
    // scroll the page to it. The capture is of the column.
    window.scrollTo(0, 0);
    return { theme, chrome, width: window.innerWidth, formats: 6, staleRevision: true, terminalPreserved: true, pdfPages: 2,
      diffLines: diffLines.length, diffKeystrokeMs };
  } finally {
    if (copyDescriptor) Object.defineProperty(navigator.clipboard, "writeText", copyDescriptor);
    else Reflect.deleteProperty(navigator.clipboard, "writeText");
  }
}
