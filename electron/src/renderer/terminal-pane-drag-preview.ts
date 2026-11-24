/** Transient DOM feedback only. The existing drop callback owns layout changes.
 * Keep the source identity here because HTML dragover protects getData(). */
let active: { paneId: string; dispose: () => void } | undefined;

// A loaded transparent pixel. An empty drag image (a canvas outside the
// document paints nothing) makes Chromium on macOS draw its globe icon beside
// the cursor instead. Built lazily so non-DOM tests can import this module.
let transparentPixel: HTMLImageElement | undefined;
function blankDragImage(): HTMLImageElement {
  transparentPixel ??= Object.assign(new Image(1, 1), {
    src: "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7",
  });
  return transparentPixel;
}
if (typeof Image !== "undefined") blankDragImage();

export function terminalPaneDragSource(): string | undefined {
  return active?.paneId;
}

export function endTerminalPaneDrag(paneId?: string): void {
  if (paneId === undefined || active?.paneId === paneId) active?.dispose();
}

export function beginTerminalPaneDrag(dataTransfer: DataTransfer, paneId: string,
    tab: HTMLElement, clientX: number, clientY: number): void {
  endTerminalPaneDrag();
  const preview = document.createElement("div");
  preview.className = "terminal-pane-drag-preview";
  preview.setAttribute("aria-hidden", "true");
  const identity = document.createElement("span");
  identity.className = "terminal-pane-drag-identity";
  // Clone only the displayed status, provider mark and title, with no tab IDs,
  // controls or event handlers. Long titles retain the same truncation.
  for (const child of tab.children) {
    if (!child.classList.contains("terminal-sr-only")) identity.append(child.cloneNode(true));
  }
  const action = document.createElement("span");
  action.className = "terminal-pane-drag-action";
  const icon = document.createElement("span");
  icon.className = "codicon codicon-arrow-swap";
  action.append(icon, "Swap");
  preview.append(identity, action);
  document.body.append(preview);
  // Suppress Chromium's screenshot of the full header; our badge remains
  // readable and can change to Swap only while over another pane.
  dataTransfer.setDragImage(blankDragImage(), 0, 0);
  const position = (x: number, y: number): void => {
    const left = Math.max(8, Math.min(x + 16, window.innerWidth - preview.offsetWidth - 8));
    const top = Math.max(8, Math.min(y + 16, window.innerHeight - preview.offsetHeight - 8));
    preview.style.transform = `translate(${left}px, ${top}px)`;
  };
  const move = (event: DragEvent): void => {
    const target = event.target instanceof Element
      ? event.target.closest<HTMLElement>("[data-terminal-pane-id]") : null;
    preview.classList.toggle("is-swap", target !== null && target.dataset.terminalPaneId !== paneId);
    preview.style.visibility = "visible";
    position(event.clientX, event.clientY);
  };
  const leave = (event: DragEvent): void => {
    if (event.relatedTarget === null) preview.style.visibility = "hidden";
  };
  const cancel = (event: KeyboardEvent): void => {
    if (event.key === "Escape") endTerminalPaneDrag();
  };
  const dispose = (): void => {
    preview.remove();
    window.removeEventListener("dragover", move, true);
    window.removeEventListener("dragleave", leave, true);
    window.removeEventListener("dragend", dispose, true);
    window.removeEventListener("drop", dispose, true);
    window.removeEventListener("blur", dispose);
    window.removeEventListener("keydown", cancel, true);
    active = undefined;
  };
  active = { paneId, dispose };
  window.addEventListener("dragover", move, true);
  window.addEventListener("dragleave", leave, true);
  window.addEventListener("dragend", dispose, true);
  window.addEventListener("drop", dispose, true);
  window.addEventListener("blur", dispose);
  window.addEventListener("keydown", cancel, true);
  position(clientX, clientY);
}
