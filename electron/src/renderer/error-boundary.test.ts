import { beforeAll, describe, expect, it, vi } from "vitest";
import { createElement, isValidElement, type ErrorInfo, type ReactElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { readFileSync } from "node:fs";

/* The root error boundary, which lives in main.tsx because that is the only
   place a ROOT boundary can live — it has to wrap the element handed to
   createRoot, and anything below it leaves the mount itself uncovered.

   main.tsx is an effectful shell: it imports App (and therefore Monaco's
   browser worker, which cannot load in this project's deliberately node-only
   Vitest environment), reaches for window and document at module scope, and
   mounts on import. Four stand-ins below reduce it to the part under test —
   the boundary class and the tree it actually mounts — without changing a line
   of production code to be test-shaped. Everything the boundary decides is
   then exercised for real: the static transition, the logged componentStack,
   and the markup react-dom/server renders from its fallback. */

const mounted = vi.hoisted(() => ({ tree: null as unknown, heldForAppearance: false }));

vi.mock("./App", () => ({ App: () => null }));
vi.mock("react-dom/client", () => ({
  createRoot: () => ({ render: (tree: unknown) => { mounted.tree = tree; } }),
}));

type Boundary = typeof import("./main")["ErrorBoundary"];
let ErrorBoundary: Boundary;
let App: unknown;

const source = readFileSync(new URL("./main.tsx", import.meta.url), "utf8");
const theme = readFileSync(new URL("./theme.css", import.meta.url), "utf8");

const failed = (): ReactElement => {
  const boundary = new ErrorBoundary({ children: createElement("p", null, "children") });
  boundary.state = ErrorBoundary.getDerivedStateFromError();
  return boundary.render() as ReactElement;
};

beforeAll(async () => {
  const document = Object.assign(new EventTarget(), {
    getElementById: () => ({}),
    // window-activity.ts marks the root from focus and visibility at load.
    documentElement: { dataset: {} },
    visibilityState: "visible",
    hasFocus: () => true,
  });
  // The document loads while the host starts: the first render waits for the
  // host's appearance answer. This one fails, which must still render.
  let answer!: () => void;
  const iface = { getAppearance: () => new Promise((_, reject) => {
    answer = () => { reject(new Error("no host here")); };
  }) };
  vi.stubGlobal("window", Object.assign(new EventTarget(), { document, iface }));
  vi.stubGlobal("document", document);
  ErrorBoundary = (await import("./main")).ErrorBoundary;
  App = (await import("./App")).App;
  mounted.heldForAppearance = mounted.tree === null;
  answer();
  await new Promise(resolve => setTimeout(resolve, 0));
});

describe("the mounted tree", () => {
  it("waits for the host's appearance before its first render, and renders on failure", () => {
    expect(mounted.heldForAppearance).toBe(true);
    expect(mounted.tree).not.toBeNull();
  });

  it("wraps the app in the boundary, so no render can unmount the root", () => {
    const tree = mounted.tree;
    expect(isValidElement(tree)).toBe(true);
    const root = tree as ReactElement<{ children?: unknown }>;
    expect(root.type).toBe(ErrorBoundary);
    const child = root.props.children;
    expect(isValidElement(child)).toBe(true);
    expect((child as ReactElement).type).toBe(App);
  });
});

describe("ErrorBoundary", () => {
  it("renders its children while nothing has thrown", () => {
    const markup = renderToStaticMarkup(
      createElement(ErrorBoundary, null, createElement("p", null, "the app")));
    expect(markup).toBe("<p>the app</p>");
  });

  it("flips to the fallback on a render exception", () => {
    expect(ErrorBoundary.getDerivedStateFromError()).toEqual({ failed: true });
  });

  it("catches a real render exception and swaps in the fallback, not the child", () => {
    // A genuine throwing component — the exact shape React's reconciler hits — and
    // its real error object, driven through the boundary's contract. (Node-env
    // Vitest has no DOM and react-dom/server does not engage boundaries, so the
    // boundary is exercised directly rather than by mounting.)
    const Boom = (): ReactElement => { throw new Error("child render exploded"); };
    let thrown: unknown;
    try {
      renderToStaticMarkup(createElement(Boom));
    } catch (err) {
      thrown = err;
    }
    expect(thrown).toBeInstanceOf(Error);

    const boundary = new ErrorBoundary({ children: createElement(Boom) });
    boundary.state = ErrorBoundary.getDerivedStateFromError();
    const logged = vi.spyOn(console, "error").mockImplementation(() => undefined);
    try {
      boundary.componentDidCatch(thrown as Error, { componentStack: "\n    in Boom" });
    } finally {
      logged.mockRestore();
    }
    const markup = renderToStaticMarkup(boundary.render() as ReactElement);
    expect(markup).toContain("Interface hit a rendering error.");
    expect(markup).not.toContain("child render exploded");
  });

  it("logs the error AND the componentStack, which window.onerror never sees", () => {
    const logged = vi.spyOn(console, "error").mockImplementation(() => undefined);
    try {
      const boundary = new ErrorBoundary({ children: null });
      const err = new Error("Cannot read properties of undefined");
      const info: ErrorInfo = { componentStack: "\n    in CenterPane\n    in App" };
      boundary.componentDidCatch(err, info);
      expect(logged).toHaveBeenCalledTimes(1);
      const call = logged.mock.calls[0];
      expect(call).toContain(err);
      expect(call).toContain(info.componentStack);
    } finally {
      logged.mockRestore();
    }
  });

  it("renders a sentence and a Reload button instead of a black window", () => {
    const markup = renderToStaticMarkup(failed());
    expect(markup).toContain("Interface hit a rendering error.");
    expect(markup).toContain("Reload");
    // Announced, so the fallback is not silent to a screen reader.
    expect(markup).toContain('role="alert"');
    // theme.css classes only: the root reserves the titlebar strip it sits under,
    // the inner copy reuses theme.css's zero-state classes. A fallback needing CSS
    // a component injects at render is a fallback that can fail to paint.
    expect(markup).toContain('class="crash-fallback"');
    expect(markup).toContain('class="empty"');
    expect(markup).toContain('class="btn btn-primary"');
    expect(markup).toContain('type="button"');
  });

  it("reserves and drags the titlebar strip the crash root sits under", () => {
    // The fallback is the one view mounted at the document root, directly under the
    // macOS traffic lights (window is titleBarStyle hiddenInset). Without this its
    // first line renders behind the close/minimise buttons and the crashed window
    // cannot be dragged. Static markup carries no CSS, so the rule is pinned as
    // source text — the way the trunk pins its main-side wiring.
    const markup = renderToStaticMarkup(failed());
    expect(markup).toContain('class="crash-fallback"');
    const from = theme.indexOf(".crash-fallback::before");
    const strip = theme.slice(from, theme.indexOf("}", from) + 1);
    expect(strip).toContain("height: var(--titlebar-h)");
    expect(strip).toContain("-webkit-app-region: drag");
    // The control opts back out, so Reload stays clickable in the drag region.
    expect(theme).toContain(".crash-fallback button { -webkit-app-region: no-drag; }");
  });

  it("keeps Reload reachable by keyboard: a native, enabled button in the tab order", () => {
    const markup = renderToStaticMarkup(failed());
    // A real <button> is focusable and Enter/Space-activatable with no extra
    // wiring; a div-with-onClick would not be. Guard both regressions: not a
    // non-button, and not a button pulled out of the tab order or disabled.
    expect(markup).toMatch(/<button[^>]*>[\s\S]*Reload[\s\S]*<\/button>/);
    expect(markup).not.toContain('tabindex="-1"');
    expect(markup).not.toContain("disabled");
  });

  it("says the sessions survive, and does not show the raw exception", () => {
    const markup = renderToStaticMarkup(failed());
    expect(markup).toContain("Engines and sessions keep running");
    expect(markup).not.toContain("children");   // the crashed tree is gone, not printed
  });

  // renderToStaticMarkup drops event handlers, so the one thing the button DOES
  // is pinned in source rather than in markup. It must be a renderer reload:
  // remounting the same crashed state would only crash again, and anything that
  // restarted the app would take every running engine session down with it.
  it("reloads the renderer only", () => {
    expect(source).toContain("location.reload()");
    expect(source).not.toContain("window.close");
  });
});
