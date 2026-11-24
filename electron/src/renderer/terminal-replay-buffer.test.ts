import { expect, it } from "vitest";
import { TerminalReplayBuffer, MAX_REPLAY_OUTPUT_BYTES, MAX_REPLAY_OUTPUT_CHUNKS } from "./terminal-replay-buffer";
import { replayTerminalOutputChunks } from "./terminal-output-state";

const encode = (text: string): Uint8Array => new TextEncoder().encode(text);
const replay = (buffer: TerminalReplayBuffer): string => {
  const decoder = new TextDecoder();
  return replayTerminalOutputChunks(buffer.chunks).map(chunk => decoder.decode(chunk, { stream: true })).join("") + decoder.decode();
};

it("preserves split Unicode and escapes across remounts and a clear", () => {
  const buffer = new TerminalReplayBuffer();
  const text = "hello 界\x1b[31mred\x1b[0m";
  for (const byte of encode(text)) buffer.push(Uint8Array.of(byte));
  expect(replay(buffer)).toBe(text);
  expect(replay(buffer)).toBe(text);
  buffer.push(encode("\x1b]0;hidden"));
  buffer.clear();
  buffer.push(encode("title\x07visible"));
  expect(replay(buffer)).toBe("visible");
});

it("bounds tiny fragments, compacts repeatedly, and preserves retained order", () => {
  const buffer = new TerminalReplayBuffer();
  for (let index = 0; index < MAX_REPLAY_OUTPUT_CHUNKS * 8; index++) buffer.push(Uint8Array.of(65 + index % 26));
  expect(buffer.length).toBe(MAX_REPLAY_OUTPUT_CHUNKS);
  expect(buffer.bytes).toBe(MAX_REPLAY_OUTPUT_CHUNKS);
  expect(buffer.truncated).toBe(true);
  const expected = Array.from({ length: MAX_REPLAY_OUTPUT_CHUNKS }, (_, offset) => String.fromCharCode(65 + (MAX_REPLAY_OUTPUT_CHUNKS * 7 + offset) % 26)).join("");
  expect(replay(buffer)).toBe(expected);
});

it("bounds a single oversized chunk as well as sustained bulk output", () => {
  const buffer = new TerminalReplayBuffer();
  for (let index = 0; index < 10; index++) buffer.push(new Uint8Array(MAX_REPLAY_OUTPUT_BYTES * 2).fill(120));
  expect(buffer.bytes).toBe(MAX_REPLAY_OUTPUT_BYTES);
  expect(buffer.length).toBe(1);
  expect(buffer.truncated).toBe(true);
});

it("retains OpenCode input modes when startup bytes are evicted, including inside one oversized frame", () => {
  const buffer = new TerminalReplayBuffer();
  buffer.push(encode("\x1b[?1049h\x1b[?1000;1002;1003;1006;2004h" + "x".repeat(MAX_REPLAY_OUTPUT_BYTES)));
  expect(buffer.truncated).toBe(true);
  expect(replay(buffer)).not.toContain("\x1b");
  expect(buffer.modes.restoreSequence).toBe("\x1b[?1049h\x1b[?1003h\x1b[?1006h\x1b[?2004h");
});

it("knows a shell-launched full-screen TUI and OpenTUI's resize query after eviction", () => {
  const buffer = new TerminalReplayBuffer();
  expect(buffer.modes.alternateScreen).toBe(false);
  expect(buffer.modes.queriedPixelSize).toBe(false);
  buffer.push(encode("$ opencode\r\n\x1b]0;OC | [14t\x07\x1b[?1049h\x1b[14t" + "x".repeat(MAX_REPLAY_OUTPUT_BYTES)));
  expect(buffer.truncated).toBe(true);
  expect(buffer.modes.alternateScreen).toBe(true);
  expect(buffer.modes.queriedPixelSize).toBe(true);
  buffer.push(encode("\x1b[?1049l$ "));
  expect(buffer.modes.alternateScreen).toBe(false);
  const titled = new TerminalReplayBuffer();
  titled.push(encode("\x1b]0;OC | [14t\x07"));
  expect(titled.modes.queriedPixelSize).toBe(false);
});

it("tracks fragmented mode changes and mutually exclusive resets while retaining bounded output", () => {
  const buffer = new TerminalReplayBuffer();
  for (const byte of encode("\x1b[?1049;1003;1006;1;2004h\x1b[?47;1000;1016;1;2004l")) {
    buffer.push(Uint8Array.of(byte));
  }
  expect(buffer.modes.restoreSequence).toBe("\x1b[?47l\x1b[?1000l\x1b[?1016l\x1b[?1l\x1b[?2004l");
  for (let index = 0; index < MAX_REPLAY_OUTPUT_CHUNKS + 1; index++) buffer.push(encode("."));
  expect(buffer.truncated).toBe(true);
  expect(buffer.modes.restoreSequence).not.toContain("h");
  // A new stream/generation starts without inheriting a retired TUI's modes.
  expect(new TerminalReplayBuffer().modes.restoreSequence).toBe("");
});

it("ignores mode-looking strings, cancelled commands, unsupported modes and overlong parameters", () => {
  const buffer = new TerminalReplayBuffer();
  const decset = "\x1b[?1049;1003;1006;2004h";
  for (const text of [
    `\x1b]0;title ${decset}\x07`, `\x1bPpayload ${decset}\x1b\\`,
    "\x1b[?1049\x18h", "\x1b[?1003\x1ah", "\x1b[?1006\x1b[0mh",
    "\x1b[?2026;2027;2031h", `\x1b[?${"1;".repeat(1000)}1049h`,
  ]) for (const byte of encode(text)) buffer.push(Uint8Array.of(byte));
  expect(buffer.modes.restoreSequence).toBe("");
  buffer.push(encode(decset));
  expect(buffer.modes.restoreSequence).toBe("\x1b[?1049h\x1b[?1003h\x1b[?1006h\x1b[?2004h");
});

it("preserves modes and an incomplete parser across a clear, then honors later disables", () => {
  const buffer = new TerminalReplayBuffer();
  buffer.push(encode("\x1b[?1049;1003;1006h\x1b[?200"));
  buffer.clear();
  expect(buffer.length).toBe(0);
  expect(buffer.initialModes).toBe("\x1b[?1049h\x1b[?1003h\x1b[?1006h");
  buffer.push(encode("4h\x1b[?1049;1003l"));
  buffer.push(new Uint8Array(MAX_REPLAY_OUTPUT_BYTES + 1).fill(120));
  expect(buffer.modes.restoreSequence).toBe("\x1b[?1049l\x1b[?1003l\x1b[?1006h\x1b[?2004h");
});

it("honors terminal hard and soft resets without re-enabling a finished TUI", () => {
  const buffer = new TerminalReplayBuffer();
  buffer.push(encode("\x1b[?1049;1003;1006;1;2004h\x1b[!p"));
  expect(buffer.modes.restoreSequence).toBe("\x1b[?1049h\x1b[?1003h\x1b[?1006h\x1b[?1l\x1b[?2004l");
  buffer.push(encode("\x1bc"));
  expect(buffer.modes.restoreSequence).not.toContain("h");
});

it("restores Codex's kitty keyboard flags on the screen that pushed them", () => {
  const buffer = new TerminalReplayBuffer();
  // Codex 0.157 startup: main-screen push, then the alternate screen, pointer capture and its own push.
  buffer.push(encode("\x1b[?2004h\x1b[>7u\x1b[?1049h\x1b[?1000h\x1b[?1002h\x1b[?1006h\x1b[?1003h\x1b[>7u"
    + "x".repeat(MAX_REPLAY_OUTPUT_BYTES)));
  expect(buffer.truncated).toBe(true);
  expect(buffer.modes.restoreSequence)
    .toBe("\x1b[>7u\x1b[?2004h\x1b[?1049h\x1b[?1003h\x1b[?1006h\x1b[>7u");
  // Exit: pop the alternate entry, leave the screen, pop the main entry.
  buffer.push(encode("\x1b[<u\x1b[?1006l\x1b[?1003l\x1b[?1049l\x1b[<u"));
  expect(buffer.modes.restoreSequence).not.toMatch(/\x1b\[>\d*u/);
  // A later alternate-screen program starts from an empty stack; RIS clears the main one.
  buffer.push(encode("\x1b[>1u\x1b[?1049h"));
  expect(buffer.modes.restoreSequence).toBe("\x1b[>1u\x1b[?2004h\x1b[?1049h\x1b[?1003l\x1b[?1006l");
  buffer.push(encode("\x1bc"));
  expect(buffer.modes.restoreSequence).not.toContain("u");
});

it("ignores kitty-looking queries and other CSI u sequences", () => {
  const buffer = new TerminalReplayBuffer();
  buffer.push(encode("\x1b[?u\x1b[u\x1b[5u\x1b[>4;1m\x1b]0;\x1b[>7u\x07"));
  expect(buffer.modes.restoreSequence).toBe("");
});
