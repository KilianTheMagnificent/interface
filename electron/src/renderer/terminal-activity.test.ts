import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import {
  createTerminalActivityMonitor,
  feedTerminalActivity,
  noteTerminalInput,
  noteTerminalQuiet,
  terminalActivitySawWork,
  withTerminalActivity,
  markTerminalDone,
  clearTerminalDone,
  isTerminalFocusReport,
  isTerminalProtocolReply,
  setTerminalActivityProfile,
  codexOsc9NotificationKind,
  titleKind,
  type TerminalActivityMonitor,
} from "./terminal-activity";
import { terminalOutputStreamKey } from "./terminal-output-state";

const encoder = new TextEncoder();

describe("terminal activity scan work", () => {
  it.each(["x\x1b[0m", "x\x07", "x\x1b[0my\x07"])(
    "searches each delimiter over at most one chunk for %j", pattern => {
      const bytes = encoder.encode(pattern.repeat(4_000));
      let searched = 0;
      const original = bytes.indexOf.bind(bytes);
      bytes.indexOf = (value, start = 0) => {
        const found = original(value, start);
        searched += (found < 0 ? bytes.length : found + 1) - start;
        return found;
      };
      const monitor = createTerminalActivityMonitor();
      feedTerminalActivity(monitor, bytes);
      expect(searched).toBeLessThanOrEqual(bytes.length * 2);
      expect(monitor.activity).toBe(pattern.includes('\x07') ? 'waiting' : 'working');
    },
  );

  it("matches byte-wise parsing across mixed Unicode, OSC, CSI and control strings", () => {
    const bytes = encoder.encode('héllo\x1b[0m\x07\x1b]2;interface-claude-activity:working\x07'
      + 'x\x1bPignored\x07payload\x1b\\中文\x1b]2;interface-claude-activity:idle\x1b\\tail');
    const reference = createTerminalActivityMonitor();
    for (const byte of bytes) feedTerminalActivity(reference, new Uint8Array([byte]));
    for (let split = 0; split <= bytes.length; split++) {
      const monitor = createTerminalActivityMonitor();
      feedTerminalActivity(monitor, bytes.subarray(0, split));
      feedTerminalActivity(monitor, bytes.subarray(split));
      expect(monitor.activity).toBe(reference.activity);
      expect(monitor.completedTurns).toBe(reference.completedTurns);
    }
  });
});

describe("Rust-projected OpenCode activity v1", () => {
  const frame = (signal: string) => encoder.encode(`\x1b]2;interface-terminal-activity:v1:${signal}\x07`);
  it("matches the Rust wire fixture and counts only explicit completion transitions", () => {
    const monitor = createTerminalActivityMonitor();
    setTerminalActivityProfile(monitor, "opencode");
    const fixture = readFileSync(new URL("../fixtures/terminal-opencode-activity-v1.txt", import.meta.url), "utf8");
    for (const line of fixture.trim().split("\n")) {
      const [signal] = line.split("\t");
      // Every byte boundary is legal on the terminal data plane.
      for (const byte of frame(signal!)) feedTerminalActivity(monitor, new Uint8Array([byte]));
      expect(monitor.activity).toBe(signal === "complete" ? "idle" : signal);
    }
    expect(monitor.completedTurns).toBe(2);
  });
  it("holds working through quiet periods and waiting through navigation or focus input", () => {
    const monitor = createTerminalActivityMonitor();
    setTerminalActivityProfile(monitor, "opencode");
    feedTerminalActivity(monitor, frame("working"));
    expect(noteTerminalQuiet(monitor)).toBeNull();
    feedTerminalActivity(monitor, osc("OC | Code session not reopening after restart"));
    expect(monitor.activity).toBe("working");
    feedTerminalActivity(monitor, frame("waiting"));
    expect(noteTerminalInput(monitor)).toBeNull();
    feedTerminalActivity(monitor, osc9("Permission needs input"));
    expect(monitor.activity).toBe("waiting");
    feedTerminalActivity(monitor, frame("working"));
    expect(monitor.activity).toBe("working");
    feedTerminalActivity(monitor, frame("complete"));
    feedTerminalActivity(monitor, text("\x07"));
    expect(monitor.activity).toBe("idle");
    expect(monitor.completedTurns).toBe(1);
  });
});

function osc(title: string): Uint8Array {
  return encoder.encode(`\x1b]0;${title}\x07`);
}

function osc9(message: string): Uint8Array {
  return encoder.encode(`\x1b]9;${message}\x07`);
}

function text(value: string): Uint8Array {
  return encoder.encode(value);
}

function activity(monitor: TerminalActivityMonitor): string {
  return monitor.activity;
}

describe("terminal activity — agent CLI titles", () => {
  it("keeps known Codex panes idle through focus redraws and title-only reloads", () => {
    const monitor = createTerminalActivityMonitor();
    setTerminalActivityProfile(monitor, "codex");
    feedTerminalActivity(monitor, osc("Respond to greeting"));
    for (let index = 0; index < 20; index += 1) {
      expect(feedTerminalActivity(monitor, text("\x1b[1;1H› Ask Codex to do anything"))).toBeNull();
      expect(terminalActivitySawWork(monitor)).toBe(false);
      expect(activity(monitor)).toBe("idle");
    }
    expect(feedTerminalActivity(monitor, osc("⠋ Respond to greeting"))).toBe("working");
    expect(feedTerminalActivity(monitor, osc("Respond to greeting"))).toBe("idle");
  });

  it("adopts late host identity, resumes shell output after demotion, and preserves waiting", () => {
    const monitor = createTerminalActivityMonitor();
    feedTerminalActivity(monitor, text("prompt redraw"));
    expect(setTerminalActivityProfile(monitor, "codex")).toBe("idle");
    // Codex is on OSC 9: bare BEL must not mark waiting.
    expect(feedTerminalActivity(monitor, text("\x07"))).toBeNull();
    expect(activity(monitor)).toBe("idle");
    setTerminalActivityProfile(monitor, "claude");
    expect(feedTerminalActivity(monitor, text("\x07"))).toBe("waiting");
    expect(activity(monitor)).toBe("waiting");
    noteTerminalInput(monitor);
    setTerminalActivityProfile(monitor, "shell");
    expect(feedTerminalActivity(monitor, text("compiling"))).toBe("working");
  });

  it("distinguishes focus protocol reports from actual terminal input", () => {
    for (const input of ["\x1b[I", "\x1b[O"]) expect(isTerminalFocusReport(text(input))).toBe(true);
    for (const input of ["i", "\r", "\x1b[A", "\x1b[Ihello", "\x1b[?1;2c"]) {
      expect(isTerminalFocusReport(text(input))).toBe(false);
    }
  });

  it("does not treat Claude's persistent brand mark as work", () => {
    // Measured against live Claude Code: the OSC title is "✳ Claude Code" at
    // startup and "✳ <session name>" at rest. The asterisk is identity, not a
    // spinner. A workspace of idle Claude panes must not keep pulsing green.
    const monitor = createTerminalActivityMonitor();
    expect(feedTerminalActivity(monitor, osc("✳ Claude Code"))).toBeNull();
    expect(activity(monitor)).toBe("idle");
    expect(feedTerminalActivity(monitor, osc("✳ Zebra Widget"))).toBeNull();
    expect(activity(monitor)).toBe("idle");
  });

  it("collapses Codex braille spinner frames onto one working state", () => {
    const monitor = createTerminalActivityMonitor();
    expect(feedTerminalActivity(monitor, osc("\u{280b} zebra-parser"))).toBe("working");
    expect(feedTerminalActivity(monitor, osc("\u{2819} zebra-parser"))).toBeNull();
    expect(feedTerminalActivity(monitor, osc("\u{2839} zebra-parser"))).toBeNull();
    expect(activity(monitor)).toBe("working");
  });

  it("returns to idle when a spinner title drops the activity mark", () => {
    const monitor = createTerminalActivityMonitor();
    feedTerminalActivity(monitor, osc("\u{280b} zebra-parser"));
    expect(feedTerminalActivity(monitor, osc("zebra-parser"))).toBe("idle");
    expect(activity(monitor)).toBe("idle");
  });

  it("does not treat later TUI redraws as work once an agent title has been seen", () => {
    const monitor = createTerminalActivityMonitor();
    feedTerminalActivity(monitor, osc("✳ Zebra Widget"));
    expect(feedTerminalActivity(monitor, text("\x1b[1;1H\x1b[0m> "))).toBeNull();
    expect(activity(monitor)).toBe("idle");
  });

  it("treats Grok's self-named title as agent identity so TUI frames stay idle", () => {
    // Grok opens with OSC "grok" and then repaints the prompt. Those frames
    // are not work; without agent-mode they used to keep the workspace dot
    // pulsing for the lifetime of the pane.
    const monitor = createTerminalActivityMonitor();
    expect(feedTerminalActivity(monitor, osc("grok"))).toBeNull();
    expect(activity(monitor)).toBe("idle");
    expect(feedTerminalActivity(monitor, text("\x1b[1;1H\x1b[0mGrok> "))).toBeNull();
    expect(activity(monitor)).toBe("idle");
  });

  it("treats Antigravity's self-named titles as agent identity", () => {
    for (const title of ["Antigravity", "agy"]) {
      const monitor = createTerminalActivityMonitor();
      expect(feedTerminalActivity(monitor, osc(title))).toBeNull();
      expect(feedTerminalActivity(monitor, text("\x1b[1;1H\x1b[0m> "))).toBeNull();
      expect(activity(monitor)).toBe("idle");
    }
  });
});

describe("terminal activity — needs input", () => {
  it("promotes a ground-state bell to waiting", () => {
    const monitor = createTerminalActivityMonitor();
    expect(feedTerminalActivity(monitor, text("\x07"))).toBe("waiting");
    expect(activity(monitor)).toBe("waiting");
  });

  it("does not treat the OSC terminator BEL as a request for input", () => {
    const monitor = createTerminalActivityMonitor();
    expect(feedTerminalActivity(monitor, osc("Zebra Widget"))).toBeNull();
    expect(activity(monitor)).toBe("idle");
  });

  it("lets a later busy title clear waiting", () => {
    const monitor = createTerminalActivityMonitor();
    feedTerminalActivity(monitor, text("\x07"));
    expect(feedTerminalActivity(monitor, osc("\u{280b} Fixing tests"))).toBe("working");
  });

  it("clears waiting when the human types", () => {
    const monitor = createTerminalActivityMonitor();
    feedTerminalActivity(monitor, text("\x07"));
    expect(noteTerminalInput(monitor)).toBe("idle");
    expect(noteTerminalInput(monitor)).toBeNull();
  });

  it("keeps waiting through prompt redraws after a bell", () => {
    const monitor = createTerminalActivityMonitor();
    feedTerminalActivity(monitor, osc("✳ Zebra Widget"));
    feedTerminalActivity(monitor, text("\x07"));
    expect(feedTerminalActivity(monitor, osc("Zebra Widget"))).toBeNull();
    expect(activity(monitor)).toBe("waiting");
  });
});

describe("terminal activity — plain shells", () => {
  it("treats ordinary output as working until a quiet tick", () => {
    const monitor = createTerminalActivityMonitor();
    expect(feedTerminalActivity(monitor, text("compiling\r\n"))).toBe("working");
    expect(noteTerminalQuiet(monitor)).toBe("idle");
    expect(noteTerminalQuiet(monitor)).toBeNull();
  });

  it("idles a spinner title once the stream goes quiet", () => {
    const monitor = createTerminalActivityMonitor();
    feedTerminalActivity(monitor, osc("\u{280b} zebra-parser"));
    expect(noteTerminalQuiet(monitor)).toBe("idle");
    expect(activity(monitor)).toBe("idle");
  });

  it("does not treat CSI-only redraws as work", () => {
    const monitor = createTerminalActivityMonitor();
    expect(feedTerminalActivity(monitor, text("\x1b[2J\x1b[1;1H"))).toBeNull();
    expect(activity(monitor)).toBe("idle");
  });

  it("survives a title split across chunks", () => {
    const monitor = createTerminalActivityMonitor();
    expect(feedTerminalActivity(monitor, text("\x1b]0;\u{280b} ze"))).toBeNull();
    expect(feedTerminalActivity(monitor, text("bra\x07"))).toBe("working");
  });
});

describe("terminal activity — ground-state fast-forward", () => {
  // The Ground state charges one "output" signal per plain-byte RUN instead of
  // one per byte. These pin the equivalence: feeding the same stream a byte at
  // a time has to land on the same state and the same work verdict.
  const feed = (chunks: readonly Uint8Array[], perByte: boolean) => {
    const monitor = createTerminalActivityMonitor();
    let sawWork = false;
    for (const chunk of chunks) {
      sawWork = false;
      if (!perByte) {
        feedTerminalActivity(monitor, chunk);
        sawWork = terminalActivitySawWork(monitor);
        continue;
      }
      // sawWork is a per-CALL flag, so OR it across the bytes of one chunk.
      for (const byte of chunk) {
        feedTerminalActivity(monitor, Uint8Array.of(byte));
        sawWork = sawWork || terminalActivitySawWork(monitor);
      }
    }
    return { activity: activity(monitor), sawWork };
  };
  const sameAsByteLoop = (chunks: readonly Uint8Array[]): void => {
    expect(feed(chunks, false)).toEqual(feed(chunks, true));
  };

  it("treats a chunk of pure shell output as one working run", () => {
    const monitor = createTerminalActivityMonitor();
    expect(feedTerminalActivity(monitor, text("total 48\nsrc\ntarget\n"))).toBe("working");
    expect(terminalActivitySawWork(monitor)).toBe(true);
    sameAsByteLoop([text("total 48\nsrc\ntarget\n")]);
  });

  it("stops fast-forwarding at an ESC in the middle of a chunk", () => {
    // Output, then an OSC title naming an agent: the title has to be parsed,
    // not swallowed by the plain-output run in front of it.
    const monitor = createTerminalActivityMonitor();
    feedTerminalActivity(monitor, text("hello\x1b]0;\u{280b} zebra\x07 after"));
    expect(activity(monitor)).toBe("working");
    sameAsByteLoop([text("hello\x1b]0;\u{280b} zebra\x07 after")]);
    sameAsByteLoop([text("\x1b[1;1Hhello\x1b[0m")]);
  });

  it("still reads a BEL inside a run of plain output as needing input", () => {
    const monitor = createTerminalActivityMonitor();
    expect(feedTerminalActivity(monitor, text("password: \x07"))).toBe("waiting");
    expect(activity(monitor)).toBe("waiting");
    sameAsByteLoop([text("password: \x07")]);
    sameAsByteLoop([text("a\x07b")]);
  });

  it("does not rescan the remainder once per BEL in a bell-dense chunk", () => {
    // A control byte AT the cursor takes the per-byte path with no indexOf, so
    // a chunk that is nothing but bells costs one pass, not one per bell.
    sameAsByteLoop([text("\x07".repeat(64))]);
    sameAsByteLoop([text(`${"\x07".repeat(8)}out${"\x07".repeat(8)}`)]);
    sameAsByteLoop([text("\x1b".repeat(4))]);
    const monitor = createTerminalActivityMonitor();
    expect(feedTerminalActivity(monitor, text("\x07".repeat(64)))).toBe("waiting");
    expect(activity(monitor)).toBe("waiting");
  });

  it("keeps a sequence split across chunks out of the plain-output path", () => {
    sameAsByteLoop([text("ready \x1b]0;\u{280b} ze"), text("bra\x07 done")]);
    sameAsByteLoop([text("\x1b"), text("]0;grok\x07"), text("output")]);
    const monitor = createTerminalActivityMonitor();
    feedTerminalActivity(monitor, text("ready \x1b]0;\u{280b} ze"));
    feedTerminalActivity(monitor, text("bra\x07 done"));
    expect(activity(monitor)).toBe("working");
  });
});

describe("withTerminalActivity", () => {
  it("stamps known activity and leaves a starting process idle", () => {
    const stamped = withTerminalActivity([
      { id: "a", generation: 3, state: "running" },
      { id: "b", generation: 1, state: "starting" },
      { id: "c", generation: 8, state: "running" },
    ], new Map([[terminalOutputStreamKey("a", 3), "waiting"]]));
    expect(stamped.map(item => item.activity)).toEqual(["waiting", "idle", "idle"]);
  });

  it("re-stamping an unchanged row keeps its identity, and the array keeps its own", () => {
    // The stamped array is stored as state and read by memoized tabs and
    // sidebar rows: minting new objects for rows that did not move repaints
    // every terminal's chrome for one terminal's transition.
    const byStream = new Map([[terminalOutputStreamKey("a", 3), "working" as const]]);
    const first = withTerminalActivity([
      { id: "a", generation: 3, state: "running" },
      { id: "b", generation: 1, state: "running" },
    ], byStream);
    expect(withTerminalActivity(first, byStream)).toBe(first);

    const moved = withTerminalActivity(first, new Map([
      [terminalOutputStreamKey("a", 3), "waiting" as const],
    ]));
    expect(moved).not.toBe(first);
    expect(moved[0]).not.toBe(first[0]);
    expect(moved[1]).toBe(first[1]);
    expect(moved.map(row => row.activity)).toEqual(["waiting", "idle"]);
  });

  it("does not inherit activity across a restart or display future-generation output", () => {
    const activityByStream = new Map([
      [terminalOutputStreamKey("same-terminal", 4), "waiting" as const],
      [terminalOutputStreamKey("same-terminal", 6), "working" as const],
    ]);
    const restarted = withTerminalActivity([
      { id: "same-terminal", generation: 5, state: "running" },
    ], activityByStream);
    expect(restarted[0]?.activity).toBe("idle");

    const caughtUp = withTerminalActivity([
      { id: "same-terminal", generation: 6, state: "running" },
    ], activityByStream);
    expect(caughtUp[0]?.activity).toBe("working");
  });
});

describe("done dot", () => {
  const key = terminalOutputStreamKey("a", 3);

  it("marks a finished turn in a pane nobody is looking at, never the focused one", () => {
    const none: ReadonlySet<string> = new Set();
    const done = markTerminalDone(none, key, terminalOutputStreamKey("b", 1));
    expect([...done]).toEqual([key]);
    expect(markTerminalDone(none, key, key)).toBe(none);
    // Marking twice keeps the same set, so the stamped rows keep their identity.
    expect(markTerminalDone(done, key, null)).toBe(done);
  });

  it("clears on focus or new activity and leaves unrelated panes alone", () => {
    const done: ReadonlySet<string> = new Set([key, "other:1"]);
    expect([...clearTerminalDone(done, key)]).toEqual(["other:1"]);
    expect(clearTerminalDone(done, "absent:1")).toBe(done);
    expect(clearTerminalDone(done, null)).toBe(done);
  });

  it("stamps done onto its own generation only, keeping identity when nothing moved", () => {
    const rows = [
      { id: "a", generation: 3, state: "running" },
      { id: "b", generation: 1, state: "running" },
    ];
    const done: ReadonlySet<string> = new Set([key]);
    const stamped = withTerminalActivity(rows, new Map(), new Map(), done);
    expect(stamped.map(row => row.done)).toEqual([true, false]);
    expect(withTerminalActivity(stamped, new Map(), new Map(), done)).toBe(stamped);
    const cleared = withTerminalActivity(stamped, new Map(), new Map(), new Set());
    expect(cleared[0]?.done).toBe(false);
    expect(cleared[1]).toBe(stamped[1]);
    const restarted = withTerminalActivity([{ id: "a", generation: 4, state: "running" }],
      new Map(), new Map(), done);
    expect(restarted[0]?.done).toBe(false);
  });
});

describe("terminal activity — work evidence", () => {
  it("marks spinner titles and shell output as work, not identity titles or CSI", () => {
    const spinner = createTerminalActivityMonitor();
    feedTerminalActivity(spinner, osc("\u{280b} zebra-parser"));
    expect(terminalActivitySawWork(spinner)).toBe(true);

    const claude = createTerminalActivityMonitor();
    feedTerminalActivity(claude, osc("✳ Claude Code"));
    expect(terminalActivitySawWork(claude)).toBe(false);

    const grok = createTerminalActivityMonitor();
    feedTerminalActivity(grok, osc("grok"));
    expect(terminalActivitySawWork(grok)).toBe(false);
    feedTerminalActivity(grok, text("\x1b[1;1H\x1b[0mGrok> "));
    expect(terminalActivitySawWork(grok)).toBe(false);

    const csi = createTerminalActivityMonitor();
    feedTerminalActivity(csi, text("\x1b[2J\x1b[1;1H"));
    expect(terminalActivitySawWork(csi)).toBe(false);

    const shell = createTerminalActivityMonitor();
    feedTerminalActivity(shell, text("compiling\r\n"));
    expect(terminalActivitySawWork(shell)).toBe(true);
  });
});

describe("terminal activity — Codex OSC 9 notifications", () => {
  it("classifies Codex OSC 9 payloads", () => {
    expect(codexOsc9NotificationKind("Agent turn complete")).toBe("idle");
    expect(codexOsc9NotificationKind("Renamed foo to bar.")).toBe("idle");
    expect(codexOsc9NotificationKind("Approval requested: npm test")).toBe("waiting");
    expect(codexOsc9NotificationKind("Approval requested by memory")).toBe("waiting");
    expect(codexOsc9NotificationKind("Codex wants to edit src/main.rs")).toBe("waiting");
    expect(codexOsc9NotificationKind("Codex wants to edit 3 files")).toBe("waiting");
    expect(codexOsc9NotificationKind("Plan mode prompt: Ship it")).toBe("waiting");
    expect(codexOsc9NotificationKind("   ")).toBeNull();
  });

  it("maps turn-complete OSC 9 to idle and approval OSC 9 to waiting", () => {
    const monitor = createTerminalActivityMonitor();
    setTerminalActivityProfile(monitor, "codex");
    feedTerminalActivity(monitor, osc("\u{280b} Fixing tests"));
    expect(activity(monitor)).toBe("working");
    expect(feedTerminalActivity(monitor, osc9("Agent turn complete"))).toBe("idle");
    expect(activity(monitor)).toBe("idle");

    expect(feedTerminalActivity(monitor, osc9("Approval requested: cargo test"))).toBe("waiting");
    expect(activity(monitor)).toBe("waiting");
    expect(feedTerminalActivity(monitor, osc9("Looks good — tests passed."))).toBe("idle");
    expect(activity(monitor)).toBe("idle");
  });

  it("ignores OSC 9 from panes that are not Codex", () => {
    for (const profile of ["claude", "shell", undefined]) {
      const monitor = createTerminalActivityMonitor();
      setTerminalActivityProfile(monitor, profile);
      feedTerminalActivity(monitor, osc("interface-claude-activity:working"));
      const before = activity(monitor);
      expect(feedTerminalActivity(monitor, osc9("Claude is waiting for your input"))).toBeNull();
      expect(activity(monitor)).toBe(before);
    }
  });

  it("ignores bare BEL on Codex once OSC 9 is the notification channel", () => {
    const monitor = createTerminalActivityMonitor();
    setTerminalActivityProfile(monitor, "codex");
    expect(feedTerminalActivity(monitor, text("\x07"))).toBeNull();
    expect(activity(monitor)).toBe("idle");
  });

  it("still treats BEL as waiting on non-Codex panes", () => {
    const shell = createTerminalActivityMonitor();
    expect(feedTerminalActivity(shell, text("\x07"))).toBe("waiting");

    const claude = createTerminalActivityMonitor();
    setTerminalActivityProfile(claude, "claude");
    expect(feedTerminalActivity(claude, text("\x07"))).toBe("waiting");
  });

  it("does not treat the OSC 9 terminator BEL as a second waiting signal", () => {
    const monitor = createTerminalActivityMonitor();
    setTerminalActivityProfile(monitor, "codex");
    // Already idle: turn-complete OSC 9 is idempotent (no activity flip).
    expect(feedTerminalActivity(monitor, osc9("Agent turn complete"))).toBeNull();
    expect(activity(monitor)).toBe("idle");
    // From waiting, the same sequence clears attention without a second BEL wait.
    feedTerminalActivity(monitor, osc9("Approval requested: cargo test"));
    expect(activity(monitor)).toBe("waiting");
    expect(feedTerminalActivity(monitor, osc9("Agent turn complete"))).toBe("idle");
    expect(activity(monitor)).toBe("idle");
  });
});

describe("terminal activity — Claude hook turn frames", () => {
  // Claude Code never animates its title (measured against 2.1.263: the
  // title is "✳ Claude Code", then "✳ <session name>", in both focus
  // states), so the host-installed UserPromptSubmit/Stop hooks answer with
  // this private OSC 2 frame instead.
  function turnFrame(state: string): Uint8Array {
    return encoder.encode(`\x1b]2;interface-claude-activity:${state}\x07`);
  }

  it("holds working from the prompt frame until the stop frame, without a quiet timeout", () => {
    const monitor = createTerminalActivityMonitor();
    setTerminalActivityProfile(monitor, "claude");
    feedTerminalActivity(monitor, osc("✳ Claude Code"));
    expect(feedTerminalActivity(monitor, turnFrame("working"))).toBe("working");
    expect(terminalActivitySawWork(monitor)).toBe(false);
    expect(noteTerminalQuiet(monitor)).toBeNull();
    expect(feedTerminalActivity(monitor, text("\x1b[1;1H\x1b[0m✻ Crunching…"))).toBeNull();
    expect(feedTerminalActivity(monitor, osc("✳ Zebra Widget"))).toBeNull();
    expect(activity(monitor)).toBe("working");
    expect(feedTerminalActivity(monitor, turnFrame("idle"))).toBe("idle");
    expect(noteTerminalQuiet(monitor)).toBeNull();
  });

  it("resumes the turn after a mid-turn permission prompt is answered", () => {
    const monitor = createTerminalActivityMonitor();
    setTerminalActivityProfile(monitor, "claude");
    feedTerminalActivity(monitor, turnFrame("working"));
    expect(feedTerminalActivity(monitor, text("\x07"))).toBe("waiting");
    expect(noteTerminalInput(monitor)).toBe("working");
    expect(feedTerminalActivity(monitor, turnFrame("idle"))).toBe("idle");
    expect(noteTerminalInput(monitor)).toBeNull();
  });

  it("needs you from a permission or question frame until it is answered", () => {
    const monitor = createTerminalActivityMonitor();
    setTerminalActivityProfile(monitor, "claude");
    feedTerminalActivity(monitor, turnFrame("working"));
    expect(feedTerminalActivity(monitor, turnFrame("waiting"))).toBe("waiting");
    // Redraws and renames while the dialog is up do not end the wait.
    expect(feedTerminalActivity(monitor, text("\x1b[1;1H Do you want to proceed?"))).toBeNull();
    expect(feedTerminalActivity(monitor, osc("✳ Zebra Widget"))).toBeNull();
    expect(noteTerminalQuiet(monitor)).toBeNull();
    expect(noteTerminalInput(monitor)).toBe("working");
    // Unanswered from this pane (the next tool call or the turn's end).
    expect(feedTerminalActivity(monitor, turnFrame("waiting"))).toBe("waiting");
    expect(feedTerminalActivity(monitor, turnFrame("working"))).toBe("working");
    feedTerminalActivity(monitor, turnFrame("waiting"));
    expect(feedTerminalActivity(monitor, turnFrame("idle"))).toBe("idle");
    expect(monitor.completedTurns).toBe(1);
  });

  it("lets the explicit completion clear a bell in either arrival order", () => {
    const monitor = createTerminalActivityMonitor();
    setTerminalActivityProfile(monitor, "claude");
    feedTerminalActivity(monitor, turnFrame("working"));
    feedTerminalActivity(monitor, text("\x07"));
    expect(feedTerminalActivity(monitor, turnFrame("idle"))).toBe("idle");
    expect(activity(monitor)).toBe("idle");
    expect(feedTerminalActivity(monitor, text("\x07"))).toBeNull();
    expect(monitor.completedTurns).toBe(1);
    expect(noteTerminalInput(monitor)).toBeNull();
    expect(feedTerminalActivity(monitor, text("\x07"))).toBe("waiting");
  });

  it("survives a profile refresh and never names the tab", () => {
    const monitor = createTerminalActivityMonitor();
    feedTerminalActivity(monitor, turnFrame("working"));
    expect(setTerminalActivityProfile(monitor, "claude")).toBeNull();
    expect(activity(monitor)).toBe("working");
    expect(feedTerminalActivity(monitor, turnFrame("nonsense"))).toBeNull();
    expect(activity(monitor)).toBe("working");
  });
});

describe("terminal activity — Antigravity OSC 7501 status frames", () => {
  function osc7501(payload: string): Uint8Array {
    return encoder.encode(`\x1b]7501;${payload}\x07`);
  }

  it("tracks working, blocked, done, error, and clear across Antigravity and shell panes", () => {
    const monitor = createTerminalActivityMonitor();
    setTerminalActivityProfile(monitor, "gemini");
    // Startup query and initial idle status do not count as work or a completed turn.
    expect(feedTerminalActivity(monitor, osc7501("?"))).toBeNull();
    expect(feedTerminalActivity(monitor, osc7501("state=idle:app=agy:title=Antigravity"))).toBeNull();
    expect(monitor.completedTurns).toBe(0);

    // Working turn holds across WindowTitle redraws and quiet ticks.
    expect(
      feedTerminalActivity(
        monitor,
        osc7501("state=working:id=t1:app=agy:title=Antigravity:msg=Thinking"),
      ),
    ).toBe("working");
    expect(terminalActivitySawWork(monitor)).toBe(false);
    expect(noteTerminalQuiet(monitor)).toBeNull();
    expect(feedTerminalActivity(monitor, osc("Antigravity"))).toBeNull();
    expect(activity(monitor)).toBe("working");

    // Permission/question/auth blocks wait until answered or resumed.
    expect(
      feedTerminalActivity(
        monitor,
        osc7501("state=blocked:id=t1:kind=permission:app=agy:title=Antigravity"),
      ),
    ).toBe("waiting");
    expect(noteTerminalInput(monitor)).toBe("working");
    expect(
      feedTerminalActivity(monitor, osc7501("state=done:id=t1:app=agy:title=Antigravity")),
    ).toBe("idle");
    expect(monitor.completedTurns).toBe(1);
    expect(
      feedTerminalActivity(monitor, osc7501("state=idle:app=agy:title=Antigravity")),
    ).toBeNull();
    expect(monitor.completedTurns).toBe(1);

    // An errored turn ends work without counting as a completed turn.
    expect(
      feedTerminalActivity(monitor, osc7501("state=working:id=t2:app=agy")),
    ).toBe("working");
    expect(
      feedTerminalActivity(monitor, osc7501("state=error:id=t2:app=agy")),
    ).toBe("idle");
    expect(monitor.completedTurns).toBe(1);

    // In a shell pane, state=clear on exit restores plain-output fallback.
    const shell = createTerminalActivityMonitor();
    setTerminalActivityProfile(shell, "shell");
    expect(feedTerminalActivity(shell, osc7501("state=working:id=t1:app=agy"))).toBe("working");
    expect(feedTerminalActivity(shell, osc7501("state=clear:app=agy"))).toBe("idle");
    expect(feedTerminalActivity(shell, text("ls -la\r\n"))).toBe("working");
  });
});

describe("terminal completion signals", () => {
  it("keeps a completion when start and stop share one chunk and leave activity idle", () => {
    const monitor = createTerminalActivityMonitor();
    setTerminalActivityProfile(monitor, "claude");
    const start = "\x1b]2;interface-claude-activity:working\x07";
    const stop = "\x1b]2;interface-claude-activity:idle\x07";
    expect(feedTerminalActivity(monitor, text(start + stop))).toBeNull();
    expect(monitor.completedTurns).toBe(1);
    feedTerminalActivity(monitor, text(stop));
    expect(monitor.completedTurns).toBe(1);
    feedTerminalActivity(monitor, text(start + stop));
    expect(monitor.completedTurns).toBe(2);
  });

  it("recognizes fragmented Codex completion while already idle, once per turn", () => {
    const monitor = createTerminalActivityMonitor();
    setTerminalActivityProfile(monitor, "codex");
    for (const byte of osc9("Agent turn complete")) feedTerminalActivity(monitor, new Uint8Array([byte]));
    expect(monitor.completedTurns).toBe(1);
    feedTerminalActivity(monitor, osc9("Agent turn complete"));
    expect(monitor.completedTurns).toBe(1);
    feedTerminalActivity(monitor, osc("\u{280b} Next turn"));
    feedTerminalActivity(monitor, osc9("Tests passed."));
    expect(monitor.completedTurns).toBe(2);
  });

  it("never treats quiet output, approvals, profile refreshes, or idle titles as completed work", () => {
    const monitor = createTerminalActivityMonitor();
    feedTerminalActivity(monitor, text("build still running"));
    noteTerminalQuiet(monitor);
    setTerminalActivityProfile(monitor, "codex");
    feedTerminalActivity(monitor, osc("Codex"));
    feedTerminalActivity(monitor, osc9("Approval requested: npm test"));
    noteTerminalInput(monitor);
    expect(monitor.completedTurns).toBe(0);
  });

  it("publishes completion independently from activity and fences it by process generation", () => {
    const rows = [{ id: "a", generation: 2 }];
    const first = withTerminalActivity(rows, new Map());
    const completed = new Map([["a:2", 1], ["a:1", 9]]);
    const next = withTerminalActivity(first, new Map(), completed);
    expect(next[0].activity).toBe("idle");
    expect(next[0].completedTurns).toBe(1);
    expect(next).not.toBe(first);
    expect(withTerminalActivity(next, new Map(), completed)).toBe(next);
    expect(withTerminalActivity([{ id: "a", generation: 3 }], new Map(), completed)[0].completedTurns).toBe(0);
  });
});

// The rules crates/interface-terminal implements too; the fixture is the
// contract both sides are held to.
describe("shared terminal activity rules v1", () => {
  const rows = readFileSync(new URL("../fixtures/terminal-agent-activity-v1.txt", import.meta.url), "utf8")
    .split("\n").filter(line => line !== "" && !line.startsWith("#"))
    .map(line => line.split("\t") as [string, string, string]);
  const sampleBytes = (sample: string): Uint8Array => Uint8Array.from(
    sample.split(/(\\x[0-9a-f]{2})/iu).flatMap(part => /^\\x[0-9a-f]{2}$/iu.test(part)
      ? [Number.parseInt(part.slice(2), 16)] : [...encoder.encode(part)]));
  const glyphs = (kind: string): Set<number> => new Set(rows
    .filter(([rule, glyphClass]) => rule === "glyph" && glyphClass === kind)
    .flatMap(([, , sample]) => {
      const range = /^U\+([0-9A-F]{4,6})\.\.U\+([0-9A-F]{4,6})$/u.exec(sample);
      if (range === null) return [sample.codePointAt(0)!];
      const [first, last] = [Number.parseInt(range[1]!, 16), Number.parseInt(range[2]!, 16)];
      return Array.from({ length: last - first + 1 }, (_, offset) => first + offset);
    }));

  it("reads every row of the fixture", () => {
    expect(rows.every(row => row.length === 3 && ["glyph", "osc9", "input"].includes(row[0]))).toBe(true);
    expect(rows.filter(([rule]) => rule === "input").map(([, kind]) => kind).sort())
      .toEqual(expect.arrayContaining(["human", "reply"]));
  });

  it("treats exactly the listed leading title glyphs as work or the agent's mark", () => {
    const spinners = glyphs("spinner");
    const brands = glyphs("brand");
    expect(spinners.size).toBeGreaterThan(256);
    expect(brands.size).toBeGreaterThan(0);
    // Every UTF-16 unit: a title's first unit decides, so this is exhaustive.
    const wrong: string[] = [];
    for (let code = 0x20; code <= 0xffff; code++) {
      if (code >= 0xd800 && code <= 0xdfff) continue;
      const kind = titleKind(`${String.fromCharCode(code)}Task`);
      if ((kind === "working") !== spinners.has(code) || (kind === "agent") !== brands.has(code)) {
        wrong.push(`U+${code.toString(16).toUpperCase()} ${String(kind)}`);
      }
    }
    expect(wrong).toEqual([]);
  });

  it("classifies Codex OSC 9 notifications as the fixture does", () => {
    for (const [rule, kind, sample] of rows) {
      if (rule === "osc9") expect(codexOsc9NotificationKind(sample), sample).toBe(kind);
    }
  });

  it("never lets xterm's own replies answer a waiting pane", () => {
    for (const [rule, kind, sample] of rows) {
      if (rule === "input") expect(isTerminalProtocolReply(sampleBytes(sample)), sample).toBe(kind === "reply");
    }
  });
});


describe("AGY native status and capture hook ordering", () => {
  const cases = JSON.parse(readFileSync(new URL("../fixtures/terminal-agy-activity-v1.json", import.meta.url), "utf8")) as {
    name: string; profile: string; steps: { output: string; activity: string; completed: number }[];
  }[];
  it("follows native AGY status despite interleaved hooks at every byte boundary", () => {
    for (const entry of cases) {
      for (const split of [false, true]) {
        const monitor = createTerminalActivityMonitor();
        setTerminalActivityProfile(monitor, entry.profile);
        for (const step of entry.steps) {
          const bytes = encoder.encode(step.output);
          if (split) for (const byte of bytes) feedTerminalActivity(monitor, Uint8Array.of(byte));
          else feedTerminalActivity(monitor, bytes);
          expect(monitor.activity, entry.name).toBe(step.activity);
          expect(monitor.completedTurns, entry.name).toBe(step.completed);
          noteTerminalQuiet(monitor);
          expect(monitor.activity, entry.name).toBe(step.activity);
        }
      }
    }
  });
});
