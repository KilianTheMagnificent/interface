import type { AppearanceChange, AppearanceState } from "../shared/appearance";
import { withoutIpcErrorShape } from "./ipc-message";

type Change = AppearanceChange;
function changeKind(change: Change | null): string {
  return change === null ? "read" : typeof change === "string" ? "theme"
    : "pinnedProjects" in change ? "projects" : "terminalFontSize" in change ? "workspace" : "paneStyle" in change ? "panes"
    : "light" in change ? "system-themes" : "translucency";
}
export interface AppearanceApi {
  getAppearance(): Promise<AppearanceState>;
  setAppearance(change: Change): Promise<AppearanceState>;
}
export interface AppearanceSnapshot {
  appearance: AppearanceState;
  pending: boolean;
  error: string | null;
}
export const INITIAL_APPEARANCE: AppearanceSnapshot = {
  appearance: { preference: "system", resolved: "light" }, pending: true, error: null,
};

/** How long a slider rests mid-drag before the position it rests on is saved. */
export const SLIDER_COMMIT_DELAY_MS = 150;

/**
 * One appearance slider's drag. Every step previews at once (the caller paints
 * CSS) and only the position the slider comes to rest on is saved: after a
 * pause, or on release. A save per step was a durable write and two full
 * appearance reads in the host, and an App render, about 60 times a second.
 */
export class SliderCommit {
  private pending: number | null = null;
  private timer: ReturnType<typeof setTimeout> | null = null;
  constructor(private readonly commit: (value: number) => void,
    private readonly delayMs = SLIDER_COMMIT_DELAY_MS) {}
  input(value: number): void {
    this.pending = value;
    if (this.timer !== null) clearTimeout(this.timer);
    this.timer = setTimeout(() => { this.flush(); }, this.delayMs);
  }
  /** Release, blur, unmount: save the position now, if one is waiting. */
  flush(): void {
    if (this.timer !== null) clearTimeout(this.timer);
    this.timer = null;
    const value = this.pending;
    this.pending = null;
    if (value !== null) this.commit(value);
  }
}

/** Serialize writes and coalesce queued slider positions. Rust owns stored values.
 * A late response cannot repaint over a newer drag, and a failed final write
 * restores the last acknowledged state instead of presenting an unsaved value.
 * `confirmed` starts at the state the document already shows when there is one,
 * so the first publish repaints that state rather than the light sentinel. */
export class AppearanceController {
  private queue: (Change | null)[] = [];
  private active: Change | null = null;
  private pumping: Promise<void> | undefined;
  private stopped = false;
  private error: string | null = null;
  constructor(private api: AppearanceApi, private publish: (snapshot: AppearanceSnapshot) => void,
    private confirmed: AppearanceState = INITIAL_APPEARANCE.appearance) {}
  refresh(): void { this.enqueue(null); }
  change(change: Change): void { this.enqueue(change); }
  dispose(): void { this.stopped = true; }
  settled(): Promise<void> { return this.pumping ?? Promise.resolve(); }

  private enqueue(change: Change | null): void {
    this.error = null;
    if (typeof change === "object" && change !== null) {
      // Coalesce only the same setting: a pane choice cannot discard a slider save.
      this.queue = this.queue.filter(item => changeKind(item) !== changeKind(change));
    }
    this.queue.push(change);
    this.emit(true);
    this.pumping ??= this.drain().finally(() => { this.pumping = undefined; });
  }
  private emit(pending: boolean): void {
    if (this.stopped) return;
    let appearance = this.confirmed;
    for (const change of [this.active, ...this.queue]) {
      if (change !== null && typeof change === "object") appearance = "pinnedProjects" in change
        ? { ...appearance, projectPresentation: change } : "terminalFontSize" in change
        ? { ...appearance, workspacePresentation: change } : "paneStyle" in change
        ? { ...appearance, panePresentation: change } : "light" in change
        ? { ...appearance, systemThemes: change } : { ...appearance, translucency: change };
    }
    this.publish({ appearance, pending, error: this.error });
  }
  private async drain(): Promise<void> {
    while (this.queue.length > 0) {
      const change = this.queue.shift()!;
      this.active = change;
      try {
        this.confirmed = await (change === null ? this.api.getAppearance() : this.api.setAppearance(change));
      } catch (error) {
        this.error = error instanceof Error ? withoutIpcErrorShape(error.message) : String(error);
      }
      this.active = null;
      this.emit(this.queue.length > 0);
    }
  }
}
