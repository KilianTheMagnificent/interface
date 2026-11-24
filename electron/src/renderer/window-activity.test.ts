import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { installWindowActivity } from "./window-activity";

const RENDERER = __dirname;

function sources(directory: string): string[] {
  return readdirSync(directory).flatMap(name => {
    const path = join(directory, name);
    if (statSync(path).isDirectory()) return sources(path);
    return /\.(?:css|tsx?)$/u.test(name) && !/\.test\.|fixture/u.test(name) ? [path] : [];
  });
}

/** Every innermost `selector { body }` block, from sheets and CSS template strings alike. */
function rules(text: string): { selector: string; body: string }[] {
  return [...text.replace(/\/\*[\s\S]*?\*\//gu, "").matchAll(/([^{}]+)\{([^{}]*)\}/gu)]
    .map(([, selector, body]) => ({ selector: selector.trim(), body }));
}

describe("looping animations rest and step (owner, 2026-10-22, battery)", () => {
  const loops = sources(RENDERER).flatMap(path => {
    const all = rules(readFileSync(path, "utf8"));
    return all
      .filter(rule => /animation\s*:[^;]*\binfinite\b/u.test(rule.body))
      .map(rule => ({ path, rule, all }));
  });

  it("finds the loops it guards", () => {
    expect(loops.length).toBeGreaterThan(15);
  });

  it("steps every loop at about 12 frames a second and rests it while Interface is not in front", () => {
    for (const { path, rule, all } of loops) {
      const where = `${path}: ${rule.selector}`;
      // The Effort popover declares its loops paused and runs them only while
      // it is hovered or holds focus.
      if (/animation-play-state\s*:\s*paused/u.test(rule.body)) continue;
      const shorthand = /animation\s*:([^;]*\binfinite\b[^;]*)/u.exec(rule.body)?.[1] ?? "";
      expect(shorthand, where).toMatch(/\bsteps?\(|step-end/u);
      const rests = [rule, ...all.filter(other => other.selector === rule.selector)]
        .some(other => /animation-play-state\s*:\s*var\(--ambient-play-state\)/u.test(other.body));
      expect(rests, where).toBe(true);
    }
  });

  it("pauses the ambient loops only while the window is inactive", () => {
    const theme = readFileSync(join(RENDERER, "theme.css"), "utf8");
    expect(theme).toContain(":root { --ambient-play-state: running; }");
    expect(theme).toContain(':root[data-window-active="false"] { --ambient-play-state: paused; }');
  });

  it("lets a ready update say so three times, then rest, and again on return", () => {
    const theme = readFileSync(join(RENDERER, "theme.css"), "utf8");
    const ready = rules(theme).filter(rule => /\.workspace-toolbar-update\.is-ready/u.test(rule.selector)
      && /animation\s*:/u.test(rule.body) && !/animation\s*:\s*none/u.test(rule.body));
    expect(ready.map(rule => rule.body.trim())).toEqual([
      "animation: workspace-update-pop 1.8s ease-in-out 3;",
      "animation: workspace-update-drift 3s linear 3;",
    ]);
    for (const rule of ready) expect(rule.selector).toMatch(/^:root:not\(\[data-window-active="false"\]\) /u);
  });
});

describe("installWindowActivity", () => {
  function fakeWindow(state: { focused: boolean; visible: boolean }) {
    const listeners = new Map<string, () => void>();
    const documentListeners = new Map<string, () => void>();
    const timers: (() => void)[] = [];
    const root = { dataset: {} as Record<string, string> };
    const win = {
      document: {
        documentElement: root,
        get visibilityState() { return state.visible ? "visible" : "hidden"; },
        hasFocus: () => state.focused,
        addEventListener: (type: string, listener: () => void) => { documentListeners.set(type, listener); },
        removeEventListener: (type: string) => { documentListeners.delete(type); },
      },
      addEventListener: (type: string, listener: () => void) => { listeners.set(type, listener); },
      removeEventListener: (type: string) => { listeners.delete(type); },
      setTimeout: (callback: () => void) => { timers.push(callback); return timers.length; },
      clearTimeout: () => {},
    };
    const flush = (): void => { for (const timer of timers.splice(0)) timer(); };
    return { win: win as unknown as Window, root, listeners, documentListeners, flush };
  }

  it("marks the document active only while it is visible and focused", () => {
    const state = { focused: true, visible: true };
    const fake = fakeWindow(state);
    const remove = installWindowActivity(fake.win);
    expect(fake.root.dataset.windowActive).toBe("true");

    state.focused = false;
    fake.listeners.get("blur")?.();
    // Focus may still be moving into a Browser pane's webview: decide a task later.
    expect(fake.root.dataset.windowActive).toBe("true");
    fake.flush();
    expect(fake.root.dataset.windowActive).toBe("false");

    state.focused = true;
    fake.listeners.get("focus")?.();
    fake.flush();
    expect(fake.root.dataset.windowActive).toBe("true");

    state.visible = false;
    fake.documentListeners.get("visibilitychange")?.();
    fake.flush();
    expect(fake.root.dataset.windowActive).toBe("false");

    remove();
    expect(fake.listeners.size).toBe(0);
    expect(fake.documentListeners.size).toBe(0);
  });

  it("keeps a webview's focus inside Interface", () => {
    const state = { focused: true, visible: true };
    const fake = fakeWindow(state);
    installWindowActivity(fake.win);
    fake.listeners.get("blur")?.();
    fake.flush();
    expect(fake.root.dataset.windowActive).toBe("true");
  });
});
