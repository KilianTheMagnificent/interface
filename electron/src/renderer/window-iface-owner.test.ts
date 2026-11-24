import { readdirSync, readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

// App.tsx is the one renderer module that talks to the host. Hooks and
// components take the api as a parameter or a prop (`useThreads(window.iface)`),
// so a direct call cannot slip in while state moves out of App. main.tsx's
// pre-mount `getAppearance()` paints the first frame before App exists.
const ALLOWED = ["App.tsx", "main.tsx"];

function withoutComments(source: string): string {
  return source
    .replace(/\/\*[\s\S]*?\*\//gu, "")
    .replace(/(^|[^:])\/\/.*$/gmu, "$1");
}

describe("window.iface owner", () => {
  it("is called only from App.tsx and main.tsx", () => {
    const root = new URL("./", import.meta.url);
    const sources = (readdirSync(root, { recursive: true }) as string[])
      .map(name => name.replaceAll("\\", "/"))
      .filter(name => /\.tsx?$/u.test(name) && !/\.test\.tsx?$/u.test(name) && !name.startsWith("fixtures/"));
    expect(sources.length).toBeGreaterThan(100);
    const callers = sources.filter(name =>
      /\bwindow\.iface\b/u.test(withoutComments(readFileSync(new URL(name, root), "utf8"))));
    expect(callers.sort()).toEqual(ALLOWED);
  });
});
