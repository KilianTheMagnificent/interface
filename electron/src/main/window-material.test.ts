import { expect, it, vi } from "vitest";
import { applyWindowMaterial, MAC_WINDOW_MATERIAL } from "./window-material";
it("establishes native material at construction and masks it at zero without replacing the backing", () => {
  expect(MAC_WINDOW_MATERIAL).toEqual({ vibrancy: "under-window", visualEffectState: "followWindow" });
  const win = { setVibrancy: vi.fn(), setBackgroundColor: vi.fn() };
  const glass = { version: 1, terminal: 0, sidebar: 65 } as const;
  applyWindowMaterial(win, "#07080a", glass, "darwin");
  expect(win.setBackgroundColor).toHaveBeenLastCalledWith("#00000000");
  applyWindowMaterial(win, "#f7f5f0", glass, "darwin");
  expect(win.setVibrancy).not.toHaveBeenCalled();
  const next = { setVibrancy: vi.fn(), setBackgroundColor: vi.fn() };
  applyWindowMaterial(next, "#f7f5f0", { ...glass, terminal: 100, sidebar: 0 }, "darwin");
  applyWindowMaterial(next, "#f7f5f0", { version: 1, terminal: 0, sidebar: 0 }, "darwin");
  expect(next.setVibrancy).not.toHaveBeenCalled();
  expect(next.setBackgroundColor).toHaveBeenLastCalledWith("#f7f5f0");
});
it("retains an opaque native fallback on other platforms", () => {
  const win = { setVibrancy: vi.fn(), setBackgroundColor: vi.fn() };
  applyWindowMaterial(win, "#07080a", { version: 1, terminal: 100, sidebar: 100 }, "linux");
  expect(win.setVibrancy).not.toHaveBeenCalled();
  expect(win.setBackgroundColor).toHaveBeenCalledWith("#07080a");
});
