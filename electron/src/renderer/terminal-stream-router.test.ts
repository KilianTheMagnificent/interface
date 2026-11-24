import { describe, expect, it } from "vitest";
import type { TerminalControlScreenV1, TerminalDataEvent } from "../shared/ipc";
import { BRACKETED_PASTE_ON } from "./terminal-output-state";
import { MAX_REPLAY_OUTPUT_BYTES } from "./terminal-replay-buffer";
import {
  OUTPUT_TRUNCATED_NOTICE, TerminalStreamRouter, type TerminalStreamView,
} from "./terminal-stream-router";

const encoder = new TextEncoder();
const decoder = new TextDecoder();

function data(terminalId: string, generation: number, text: string | Uint8Array): TerminalDataEvent {
  return {
    terminalId, generation, sequence: "0", kind: "data",
    data: typeof text === "string" ? encoder.encode(text) : text,
  };
}

function screen(line: string): TerminalControlScreenV1 {
  return {
    lines: [line], cursorRow: 0, cursorColumn: line.length, atBottom: true,
    bracketedPaste: false, ghostText: false,
  };
}

/**
 * An emulator whose writes land only when the test drains it, as xterm's do.
 * Like TerminalView, it shows no screen before setup or while writing.
 */
class FakeView implements TerminalStreamView {
  readonly writes: string[] = [];
  private readonly held: (() => void)[] = [];
  setups = 0;
  shown: TerminalControlScreenV1 | null = null;

  write(bytes: string | Uint8Array, callback?: () => void): void {
    this.writes.push(typeof bytes === "string" ? bytes : decoder.decode(bytes));
    this.held.push(callback ?? (() => undefined));
  }

  completeSetup(): void { this.setups += 1; }

  controlScreen(): TerminalControlScreenV1 | null {
    return this.setups > 0 && this.held.length === 0 ? this.shown : null;
  }

  /** xterm has parsed everything written so far. */
  drain(): void {
    for (const callback of this.held.splice(0)) callback();
  }
}

function credits(): { complete: () => void; readonly count: number } {
  let count = 0;
  return { complete: () => { count += 1; }, get count() { return count; } };
}

/** Attach a view, drain its replay, and report what it was given. */
function remount(router: TerminalStreamRouter<FakeView>, key: string, profileId?: string) {
  const view = new FakeView();
  const ready: boolean[] = [];
  router.attach(key, view, profileId, redraw => { ready.push(redraw); });
  view.drain();
  return { view, ready, text: view.writes.join("") };
}

describe("TerminalStreamRouter chunk delivery", () => {
  it("writes the current generation into its view and releases the credit when xterm has", () => {
    const router = new TerminalStreamRouter<FakeView>();
    const view = new FakeView();
    router.attach("t:1", view, "shell", () => undefined);
    const credit = credits();

    const output = router.deliver(data("t", 1, "hello"), { generation: 1 }, credit.complete);

    expect(output).toEqual({ key: "t:1", boundary: "ground" });
    expect(view.writes).toEqual(["hello"]);
    expect(credit.count).toBe(0);
    view.drain();
    expect(credit.count).toBe(1);
  });

  it("reports the parser boundary before each chunk across split escapes", () => {
    const router = new TerminalStreamRouter<FakeView>();
    const known = { generation: 1 };
    expect(router.deliver(data("t", 1, "a\x1b["), known, () => undefined)?.boundary).toBe("ground");
    expect(router.deliver(data("t", 1, "3"), known, () => undefined)?.boundary).toBe("csi-parameter");
    expect(router.deliver(data("t", 1, "1mred"), known, () => undefined)?.boundary).toBe("csi-parameter");
    expect(router.deliver(data("t", 1, "\x1b]0;title"), known, () => undefined)?.boundary).toBe("ground");
    expect(router.deliver(data("t", 1, "\x07"), known, () => undefined)?.boundary).toBe("osc");
  });

  it("keeps output with no view, and output for an unannounced generation, for the next mount", () => {
    const router = new TerminalStreamRouter<FakeView>();
    const current = new FakeView();
    router.attach("t:2", current, "shell", () => undefined);
    const credit = credits();

    // Generation 2's view exists, but the control plane still knows generation 1.
    router.deliver(data("t", 2, "early "), { generation: 1 }, credit.complete);
    router.deliver(data("t", 3, "unmounted"), { generation: 3 }, credit.complete);

    expect(current.writes).toEqual([]);
    expect(credit.count).toBe(2);
    expect(remount(router, "t:2", "shell").text).toBe("early ");
    expect(remount(router, "t:3", "shell").text).toBe("unmounted");
  });

  it("drops superseded generations, empty chunks and end of stream, crediting each", () => {
    const router = new TerminalStreamRouter<FakeView>();
    const current = new FakeView();
    current.shown = screen("$");
    router.attach("t:2", current, "shell", () => undefined);
    const credit = credits();

    expect(router.deliver(data("t", 1, "stale"), { generation: 2 }, credit.complete)).toBeNull();
    expect(router.deliver(data("t", 2, ""), { generation: 2 }, credit.complete)).toBeNull();
    expect(router.deliver({ ...data("t", 2, "x"), kind: "eof" }, { generation: 2 }, credit.complete))
      .toBeNull();

    expect(credit.count).toBe(3);
    expect(current.writes).toEqual([]);
    expect(remount(router, "t:1").text).toBe("");
    // None of them counted as output: no byte cursor for the screen to reflect.
    expect(router.controlScreen("t:2")).toBeNull();
  });

  it("completes a write that throws and rethrows, leaving no write pending", () => {
    const router = new TerminalStreamRouter<FakeView>();
    const view = new FakeView();
    view.shown = screen("$");
    view.write = () => { throw new Error("disposed"); };
    router.attach("t:1", view, "shell", () => undefined);
    const credit = credits();

    expect(() => router.deliver(data("t", 1, "x"), { generation: 1 }, credit.complete))
      .toThrow("disposed");
    expect(credit.count).toBe(1);
    expect(router.controlScreen("t:1")).toEqual({ screen: screen("$"), cursor: "1" });
  });
});

describe("TerminalStreamRouter detach", () => {
  it("credits every write a disposed view will never finish, exactly once", () => {
    const router = new TerminalStreamRouter<FakeView>();
    const view = new FakeView();
    router.attach("t:1", view, "shell", () => undefined);
    view.drain();
    const credit = credits();
    router.deliver(data("t", 1, "a"), { generation: 1 }, credit.complete);
    router.deliver(data("t", 1, "b"), { generation: 1 }, credit.complete);

    router.detach("t:1");
    expect(credit.count).toBe(2);
    expect(router.view("t:1")).toBeUndefined();
    // xterm finishing a write after disposal does not credit it twice.
    view.drain();
    expect(credit.count).toBe(2);
  });

  it("keeps the replay tail so a remount restores what the pane showed", () => {
    const router = new TerminalStreamRouter<FakeView>();
    const first = new FakeView();
    router.attach("t:1", first, "shell", () => undefined);
    router.deliver(data("t", 1, "one "), { generation: 1 }, () => undefined);
    router.detach("t:1");
    router.deliver(data("t", 1, "two"), { generation: 1 }, () => undefined);

    const { text, view, ready } = remount(router, "t:1", "shell");
    expect(text).toBe("one two");
    expect(view.setups).toBe(1);
    expect(ready).toEqual([false]);
  });
});

describe("TerminalStreamRouter screens", () => {
  it("reads a live screen only once every write has landed, with the bytes it reflects", () => {
    const router = new TerminalStreamRouter<FakeView>();
    const view = new FakeView();
    view.shown = screen("$ ");
    router.attach("t:1", view, "shell", () => undefined);
    view.drain();
    // No output yet: nothing proves which bytes the screen reflects.
    expect(router.controlScreen("t:1")).toBeNull();
    expect(router.screenEvidence("t:1")).toEqual({ screen: screen("$ "), outputCursor: "0" });

    router.deliver(data("t", 1, "ls\r\n"), { generation: 1 }, () => undefined);
    expect(router.controlScreen("t:1")).toBeNull();
    expect(router.screenEvidence("t:1")).toEqual({});
    view.drain();
    expect(router.controlScreen("t:1")).toEqual({ screen: screen("$ "), cursor: "4" });
    expect(router.screenEvidence("t:1")).toEqual({ screen: screen("$ "), outputCursor: "4" });
  });

  it("shows the host a detached view's last screen until a new view has drawn", () => {
    const router = new TerminalStreamRouter<FakeView>();
    const first = new FakeView();
    first.shown = screen("idle");
    router.attach("t:1", first, "claude", () => undefined);
    router.deliver(data("t", 1, "abc"), { generation: 1 }, () => undefined);
    first.drain();
    router.detach("t:1");
    router.deliver(data("t", 1, "de"), { generation: 1 }, () => undefined);

    expect(router.screenEvidence("t:1")).toEqual({ screen: screen("idle"), outputCursor: "3" });
    const second = new FakeView();
    second.shown = screen("redrawn");
    router.attach("t:1", second, "claude", () => undefined);
    expect(router.screenEvidence("t:1")).toEqual({ screen: screen("idle"), outputCursor: "3" });
    second.drain();
    expect(router.screenEvidence("t:1")).toEqual({ screen: screen("redrawn"), outputCursor: "5" });
  });

  it("notices OpenTUI's pixel-size query across the whole stream", () => {
    const router = new TerminalStreamRouter<FakeView>();
    router.deliver(data("t", 1, "\x1b[1"), { generation: 1 }, () => undefined);
    expect(router.queriedPixelSize("t:1")).toBe(false);
    router.deliver(data("t", 1, "4t"), { generation: 1 }, () => undefined);
    expect(router.queriedPixelSize("t:1")).toBe(true);
  });
});

describe("TerminalStreamRouter replay on attach", () => {
  const oversized = (): Uint8Array => new Uint8Array(MAX_REPLAY_OUTPUT_BYTES + 16).fill(0x78);

  it("completes setup only after the last replayed chunk has been drawn", () => {
    const router = new TerminalStreamRouter<FakeView>();
    router.deliver(data("t", 1, "a"), { generation: 1 }, () => undefined);
    router.deliver(data("t", 1, "b"), { generation: 1 }, () => undefined);
    const view = new FakeView();
    const ready: boolean[] = [];
    router.attach("t:1", view, "shell", redraw => { ready.push(redraw); });

    expect(view.writes).toEqual(["a", "b"]);
    expect(view.setups).toBe(0);
    expect(ready).toEqual([]);
    view.drain();
    expect(view.setups).toBe(1);
    expect(ready).toEqual([false]);
  });

  it("sets up a view with nothing to replay at once", () => {
    const router = new TerminalStreamRouter<FakeView>();
    const view = new FakeView();
    const ready: boolean[] = [];
    router.attach("t:1", view, "claude", redraw => { ready.push(redraw); });
    expect(view.writes).toEqual([]);
    expect(view.setups).toBe(1);
    expect(ready).toEqual([false]);
  });

  it("re-asserts bracketed paste on a remounted Claude or Codex pane, never on a shell", () => {
    for (const profileId of ["claude", "codex"]) {
      const router = new TerminalStreamRouter<FakeView>();
      router.deliver(data("t", 1, "> "), { generation: 1 }, () => undefined);
      expect(remount(router, "t:1", profileId).view.writes).toEqual([BRACKETED_PASTE_ON, "> "]);
    }
    const router = new TerminalStreamRouter<FakeView>();
    router.deliver(data("t", 1, "$ "), { generation: 1 }, () => undefined);
    expect(remount(router, "t:1", "shell").view.writes).toEqual(["$ "]);
  });

  it("does not replay a torn agent frame: it restores modes and asks for a repaint", () => {
    const router = new TerminalStreamRouter<FakeView>();
    router.deliver(data("t", 1, "\x1b[?1049h\x1b[?2004h"), { generation: 1 }, () => undefined);
    router.deliver(data("t", 1, oversized()), { generation: 1 }, () => undefined);

    const { view, ready } = remount(router, "t:1", "claude");
    expect(view.writes).toEqual([
      BRACKETED_PASTE_ON, "\x1b[?1049h\x1b[?2004h", OUTPUT_TRUNCATED_NOTICE,
    ]);
    expect(view.setups).toBe(1);
    expect(ready).toEqual([true]);
  });

  it("repaints a full-screen program started inside a shell the same way", () => {
    const router = new TerminalStreamRouter<FakeView>();
    router.deliver(data("t", 1, "\x1b[?1049h"), { generation: 1 }, () => undefined);
    router.deliver(data("t", 1, oversized()), { generation: 1 }, () => undefined);

    const { view, ready } = remount(router, "t:1", "shell");
    expect(view.writes).toEqual(["\x1b[?1049h", OUTPUT_TRUNCATED_NOTICE]);
    expect(ready).toEqual([true]);
  });

  it("keeps a truncated shell tail behind the notice", () => {
    const router = new TerminalStreamRouter<FakeView>();
    router.deliver(data("t", 1, oversized()), { generation: 1 }, () => undefined);
    router.deliver(data("t", 1, "$ "), { generation: 1 }, () => undefined);

    const { view, ready } = remount(router, "t:1", undefined);
    expect(view.writes).toEqual([OUTPUT_TRUNCATED_NOTICE, "$ "]);
    expect(view.setups).toBe(1);
    expect(ready).toEqual([false]);
  });

  it("keeps the program's modes across a Clear, without the cleared text", () => {
    const router = new TerminalStreamRouter<FakeView>();
    router.deliver(data("t", 1, "\x1b[?2004hold text"), { generation: 1 }, () => undefined);
    router.clear("t:1");

    const cleared = remount(router, "t:1", "shell");
    expect(cleared.view.writes).toEqual(["\x1b[?2004h"]);
    expect(cleared.ready).toEqual([false]);
    router.deliver(data("t", 1, "new"), { generation: 1 }, () => undefined);
    expect(remount(router, "t:1", "shell").view.writes).toEqual(["\x1b[?2004h", "new"]);
  });

  it("tells a view replaced mid-replay nothing when its late writes land", () => {
    const router = new TerminalStreamRouter<FakeView>();
    router.deliver(data("t", 1, "tail"), { generation: 1 }, () => undefined);
    const first = new FakeView();
    const ready: string[] = [];
    router.attach("t:1", first, "shell", () => { ready.push("first"); });
    const second = new FakeView();
    router.attach("t:1", second, "shell", () => { ready.push("second"); });

    first.drain();
    expect(first.setups).toBe(0);
    expect(ready).toEqual([]);
    second.drain();
    expect(second.setups).toBe(1);
    expect(ready).toEqual(["second"]);
  });
});

describe("TerminalStreamRouter retire", () => {
  it("ends every stream of a removed terminal and drops its later output", () => {
    const router = new TerminalStreamRouter<FakeView>();
    const view = new FakeView();
    view.shown = screen("$");
    router.attach("t:1", view, "shell", () => undefined);
    router.deliver(data("t", 1, "old"), { generation: 1 }, () => undefined);
    view.drain();
    router.deliver(data("t", 2, "next"), { generation: 1 }, () => undefined);
    router.detach("t:1");

    router.retire("t");

    const credit = credits();
    expect(router.deliver(data("t", 1, "late"), { generation: 1 }, credit.complete)).toBeNull();
    expect(credit.count).toBe(1);
    expect(router.screenEvidence("t:1")).toEqual({});
    expect(remount(router, "t:1").text).toBe("");
    expect(remount(router, "t:2").text).toBe("");
    // A reused stream counts its bytes from zero, as the host does.
    router.retire("t", 1);
    router.deliver(data("t", 1, "ab"), { generation: 1 }, () => undefined);
    router.view("t:1")!.shown = screen("$");
    router.view("t:1")!.drain();
    expect(router.controlScreen("t:1")).toEqual({ screen: screen("$"), cursor: "2" });
  });

  it("keeps a mounted view and its unacknowledged writes until it detaches", () => {
    const router = new TerminalStreamRouter<FakeView>();
    const view = new FakeView();
    router.attach("t:1", view, "shell", () => undefined);
    const credit = credits();
    router.deliver(data("t", 1, "x"), { generation: 1 }, credit.complete);

    router.retire("t");
    expect(router.view("t:1")).toBe(view);
    expect(credit.count).toBe(0);
    router.detach("t:1");
    expect(credit.count).toBe(1);
  });

  it("retires older generations only, keeping the current and any not yet announced", () => {
    const router = new TerminalStreamRouter<FakeView>();
    for (const generation of [1, 2, 3]) {
      router.deliver(data("t", generation, `g${generation.toString()}`), { generation: 1 },
        () => undefined);
    }
    router.deliver(data("other", 1, "kept"), { generation: 1 }, () => undefined);

    router.retire("t", 2);

    expect(remount(router, "t:1").text).toBe("");
    expect(remount(router, "t:2").text).toBe("g2");
    expect(remount(router, "t:3").text).toBe("g3");
    expect(remount(router, "other:1").text).toBe("kept");
  });

  it("follows the registry: listed terminals live again, and unlisted screens go", () => {
    const router = new TerminalStreamRouter<FakeView>();
    for (const key of ["a:1", "b:1"]) {
      const view = new FakeView();
      view.shown = screen(key);
      router.attach(key, view, "shell", () => undefined);
      router.detach(key);
    }
    router.deliver(data("a", 1, "old"), { generation: 1 }, () => undefined);
    router.retire("a");

    router.sync([{ id: "a", generation: 2 }]);

    expect(router.screenEvidence("a:1")).toEqual({});
    expect(router.screenEvidence("b:1")).toEqual({});
    const credit = credits();
    expect(router.deliver(data("a", 2, "back"), { generation: 2 }, credit.complete))
      .toEqual({ key: "a:2", boundary: "ground" });
    expect(remount(router, "a:2").text).toBe("back");
  });
});
