import { spawn, spawnSync, type ChildProcessWithoutNullStreams } from "node:child_process";
import { randomUUID } from "node:crypto";
import fs from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it } from "vitest";

// The scripted Claude a hands-free session runs for structured sessions
// (scripts/handsfree-fake-claude.mjs). These tests read its transcript with
// the rules crates/engine-claude/src/store.rs applies and its stream with the
// event shapes crates/engine-claude/src/mapper.rs maps.
const scripts = fileURLToPath(new URL("../../scripts/", import.meta.url));
const FAKE_CLAUDE = path.join(scripts, "handsfree-fake-claude.mjs");
const FAKE_PROVIDER = path.join(scripts, "handsfree-fake-provider.sh");
/** The argument prefix crates/engine-claude/src/launch.rs always builds. */
const CLAUDE_PREFIX = ["--output-format", "stream-json", "--verbose", "--input-format", "stream-json"];

type Frame = Record<string, any>;
const temporary: string[] = [];

function directory(): string {
  const created = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "fake-claude-")));
  temporary.push(created);
  return created;
}

afterEach(() => {
  for (const created of temporary.splice(0)) fs.rmSync(created, { recursive: true, force: true });
});

class FakeClaude {
  readonly frames: Frame[] = [];
  /** When each frame was read, in step with `frames`. */
  readonly arrivals: number[] = [];
  private readonly child: ChildProcessWithoutNullStreams;
  private readonly exited: Promise<number | null>;

  constructor(argumentsList: string[], readonly cwd: string, readonly home: string, viaShell = false,
      environment: Record<string, string> = {}) {
    const [command, prefix] = viaShell ? ["/bin/sh", [FAKE_PROVIDER]] : [process.execPath, [FAKE_CLAUDE]];
    this.child = spawn(command, [...prefix, ...CLAUDE_PREFIX, ...argumentsList, "--permission-prompt-tool", "stdio"], {
      cwd, env: { HOME: home, PATH: "/usr/bin:/bin", ...environment },
    });
    let buffered = "";
    this.child.stdout.setEncoding("utf8");
    this.child.stdout.on("data", (chunk: string) => {
      buffered += chunk;
      const lines = buffered.split("\n");
      buffered = lines.pop() ?? "";
      for (const line of lines) {
        if (!line.trim()) continue;
        this.frames.push(JSON.parse(line) as Frame);
        this.arrivals.push(Date.now());
      }
    });
    this.exited = new Promise(resolve => { this.child.on("exit", code => { resolve(code); }); });
  }

  send(frame: Frame): void { this.child.stdin.write(`${JSON.stringify(frame)}\n`); }

  prompt(sessionId: string, text: string): void {
    this.send({ type: "user", session_id: sessionId, message: { role: "user", content: [{ type: "text", text }] }, parent_tool_use_id: null });
  }

  async initialize(): Promise<Frame> {
    this.send({ type: "control_request", request_id: "init-1", request: { subtype: "initialize" } });
    return this.waitFor(frame => frame.type === "control_response" && frame.response.request_id === "init-1");
  }

  async waitFor(match: (frame: Frame) => boolean, after = 0): Promise<Frame> {
    for (const deadline = Date.now() + 10_000; Date.now() < deadline;) {
      const found = this.frames.slice(after).find(match);
      if (found !== undefined) return found;
      await new Promise(resolve => setTimeout(resolve, 20));
    }
    throw new Error(`no matching frame in ${JSON.stringify(this.frames.slice(after))}`);
  }

  async results(count: number): Promise<Frame[]> {
    await this.waitFor(() => this.frames.filter(frame => frame.type === "result").length >= count);
    return this.frames.filter(frame => frame.type === "result");
  }

  async close(): Promise<number | null> {
    this.child.stdin.end();
    return this.exited;
  }
}

/** store.rs `project_dir_name`. */
const projectDirName = (cwd: string) => cwd.replace(/[^A-Za-z0-9]/gu, "-");
const transcriptPath = (home: string, cwd: string, sessionId: string) =>
  path.join(home, ".claude", "projects", projectDirName(cwd), `${sessionId}.jsonl`);
const readTranscript = (file: string): Frame[] =>
  fs.readFileSync(file, "utf8").split("\n").filter(Boolean).map(line => JSON.parse(line) as Frame);

/** store.rs `text_blocks` and `has_tool_result`. */
function texts(message: Frame | undefined): string[] {
  if (typeof message?.content === "string") return [message.content];
  return (message?.content ?? []).filter((block: Frame) => block.type === "text" && block.text).map((block: Frame) => block.text);
}
const hasToolResult = (record: Frame) =>
  Array.isArray(record.message?.content) && record.message.content.some((block: Frame) => block.type === "tool_result");

/** The owner prompts store.rs `history` shows, in order. */
const prompts = (records: Frame[]) => records
  .filter(record => record.type === "user" && !record.isSidechain && !record.parent_tool_use_id && !hasToolResult(record))
  .map(record => texts(record.message).join("\n\n").trim());

/** store.rs `fork_cut`: the last main-thread assistant entry before prompt `keep + 1`. */
function forkCut(records: Frame[], keep: number): Frame | undefined {
  let seen = 0;
  let cut: Frame | undefined;
  for (const record of records) {
    if (record.type === "user" && !hasToolResult(record)) {
      if (seen === keep) break;
      seen += 1;
    } else if (seen === keep && record.type === "assistant" && !record.parent_tool_use_id) {
      cut = record;
    }
  }
  return cut;
}

/** Every entry is a transcript message chained to the one before it. */
function expectChained(records: Frame[], sessionId: string, cwd: string, firstParent: string | null): void {
  let parent = firstParent;
  for (const record of records) {
    expect(["user", "assistant"]).toContain(record.type);
    expect(record.uuid).toMatch(/^[0-9a-f-]{36}$/u);
    expect(record.parentUuid).toBe(parent);
    expect(record.sessionId).toBe(sessionId);
    expect(record.cwd).toBe(cwd);
    expect(record.isSidechain).toBe(false);
    expect(Number.isNaN(Date.parse(record.timestamp))).toBe(false);
    expect(record.message.role).toBe(record.type);
    parent = record.uuid;
  }
}

const streamedText = (frames: Frame[]) => frames
  .filter(frame => frame.type === "stream_event" && frame.event.type === "content_block_delta" && frame.event.delta.type === "text_delta")
  .map(frame => frame.event.delta.text).join("");

describe("hands-free fake Claude", () => {
  it("is what the fake provider runs for Claude's structured launch, and answers initialize with models", async () => {
    const home = directory();
    const cwd = directory();
    const claude = new FakeClaude(["--session-id", randomUUID(), "--no-session-persistence", "--setting-sources="], cwd, home, true);
    const initialized = await claude.initialize();
    expect(initialized.response.subtype).toBe("success");
    expect(initialized.response.response.models.map((model: Frame) => model.value)).toEqual(["default", "opus", "haiku"]);
    expect(await claude.close()).toBe(0);
    expect(fs.existsSync(path.join(home, ".claude"))).toBe(false);
  }, 20_000);

  it("keeps the shell fake for every other invocation", () => {
    const version = spawnSync("/bin/sh", [FAKE_PROVIDER, "--version"], { encoding: "utf8" });
    expect(version.stdout).toBe("handsfree-fake-provider 1.0.0\n");
    // The Antigravity worker also speaks stream-json, with another prefix.
    const agy = spawnSync("/bin/sh", [FAKE_PROVIDER, "--agent", "x", "--input-format", "stream-json", "--output-format", "stream-json"], {
      encoding: "utf8", input: "{}\n", timeout: 5_000,
    });
    expect([agy.status, agy.stdout]).toEqual([0, ""]);
  });

  it("answers a one-shot print call: a title, or Write with AI's commit message and pull request text", () => {
    /** crates/engine-claude/src/title.rs `one_shot_arguments`, with `system` as the system prompt. */
    const oneShot = (system: string, input: string) => spawnSync("/bin/sh", [FAKE_PROVIDER,
      "-p", "--output-format", "json", "--model", "claude-haiku-5-5", "--no-session-persistence", "--tools", "",
      "--setting-sources=", "--strict-mcp-config", "--mcp-config", "{\"mcpServers\":{}}", "--safe-mode",
      "--disable-slash-commands", "--no-chrome", "--permission-mode", "dontAsk", "--system-prompt", system,
    ], { encoding: "utf8", input, timeout: 10_000, env: { HOME: directory(), PATH: "/usr/bin:/bin" } });
    const result = (run: ReturnType<typeof oneShot>): Frame => {
      expect(run.status, run.stderr).toBe(0);
      const frame = JSON.parse(run.stdout) as Frame;
      expect(frame).toMatchObject({ type: "result", subtype: "success", is_error: false });
      return frame;
    };
    const title = result(oneShot("You name conversations. Reply with the title only.",
      "Write a 3-8 word title…\n\n<first-prompt>\nfix the flaky login test today\n</first-prompt>\n"));
    expect(title.result).toBe("Hands-free fix the flaky login");
    const system = "You write git commit messages and pull request descriptions. Reply with the text only.";
    const changes = "<changes>\n M HANDSFREE-NOTES.md | +1 -0\n</changes>\n";
    expect(result(oneShot(system, `Write a git commit message for the changes below.\n\n${changes}`)).result)
      .toBe("Update HANDSFREE-NOTES.md\n\nWritten by the hands-free fake Claude from HANDSFREE-NOTES.md.");
    expect(result(oneShot(system, `Write a pull request for the changes below.\n\n${changes}`)).result)
      .toMatch(/^Update HANDSFREE-NOTES\.md \(hands-free\)\n\n/u);
  });

  it("writes the file, streams the turn and leaves the transcript store.rs reads", async () => {
    const home = directory();
    const cwd = directory();
    const sessionId = randomUUID();
    const claude = new FakeClaude(["--session-id", sessionId, "--include-partial-messages"], cwd, home);
    await claude.initialize();
    claude.prompt(sessionId, "write hello.txt hi there");
    const [result] = await claude.results(1);
    expect(await claude.close()).toBe(0);

    expect(fs.readFileSync(path.join(cwd, "hello.txt"), "utf8")).toBe("hi there\n");
    expect(result).toMatchObject({ subtype: "success", is_error: false, session_id: sessionId, result: "Wrote `hello.txt`." });
    expect(claude.frames.find(frame => frame.type === "system")).toMatchObject({ subtype: "init", session_id: sessionId, cwd });
    expect(streamedText(claude.frames)).toBe("I'll write `hello.txt`.Wrote `hello.txt`.");
    const toolUse = claude.frames.flatMap(frame => frame.type === "assistant" ? frame.message.content : [])
      .find((block: Frame) => block.type === "tool_use");
    expect(toolUse).toMatchObject({ name: "Write", input: { file_path: path.join(cwd, "hello.txt"), content: "hi there\n" } });
    expect(claude.frames.find(frame => frame.type === "user")?.message.content[0]).toMatchObject({ type: "tool_result", tool_use_id: toolUse.id });

    const records = readTranscript(transcriptPath(home, cwd, sessionId));
    expectChained(records, sessionId, cwd, null);
    expect(records.map(record => record.type)).toEqual(["user", "assistant", "assistant", "user", "assistant"]);
    expect(prompts(records)).toEqual(["write hello.txt hi there"]);
    expect(records[2].message.content[0]).toEqual(toolUse);
    expect(records[3].message.content[0]).toMatchObject({ type: "tool_result", tool_use_id: toolUse.id });
    expect(forkCut(records, 1)).toBe(records[4]);
    expect(texts(records[4].message)).toEqual(["Wrote `hello.txt`."]);
  }, 20_000);

  it("writes its transcript under CLAUDE_CONFIG_DIR when a provider account sets it", async () => {
    const home = directory();
    const cwd = directory();
    const account = path.join(directory(), "accounts", "claude", "acct-0123456789ab");
    const sessionId = randomUUID();
    const claude = new FakeClaude(["--session-id", sessionId], cwd, home, false,
      { CLAUDE_CONFIG_DIR: account });
    await claude.initialize();
    claude.prompt(sessionId, "write account.txt on the work account");
    await claude.results(1);
    expect(await claude.close()).toBe(0);

    const file = path.join(account, "projects", projectDirName(cwd), `${sessionId}.jsonl`);
    expect(prompts(readTranscript(file))).toEqual(["write account.txt on the work account"]);
    expect(fs.existsSync(path.join(home, ".claude", "projects"))).toBe(false);
  }, 30_000);

  it("answers an Away loop's rounds with the AWAY_RESULT line Away reads (review B3)", async () => {
    const home = directory();
    const cwd = directory();
    const sessionId = randomUUID();
    const claude = new FakeClaude(["--session-id", sessionId, "--permission-mode", "acceptEdits"], cwd, home);
    await claude.initialize();
    claude.prompt(sessionId, "You are executing a bounded Interface Away loop in /w. Iteration 1/2.\nOwner objective: x");
    const [work] = await claude.results(1);
    expect(fs.readFileSync(path.join(cwd, "HANDSFREE-NOTES.md"), "utf8")).toBe("Away round 1: one more line.\n");
    expect(work?.result).toBe('Appended a line to `HANDSFREE-NOTES.md`.\nAWAY_RESULT {"outcome":"continue",'
      + '"summary":"Round 1: appended a line to HANDSFREE-NOTES.md.","checks":["cat HANDSFREE-NOTES.md: passed"]}');
    // A review changes nothing; it says continue before the last round and complete on it.
    claude.prompt(sessionId, "You are executing a bounded Interface Away loop in /w. Iteration 1/2.\n"
      + "You are a fresh independent reviewer.");
    claude.prompt(sessionId, "You are executing a bounded Interface Away loop in /w. Iteration 2/2.\n"
      + "You are a fresh independent reviewer.");
    const [, first, last] = await claude.results(3);
    expect(await claude.close()).toBe(0);
    expect(first?.result).toMatch(/^Reviewed round 1: .*\nAWAY_RESULT \{"outcome":"continue",/u);
    expect(last?.result).toMatch(/\nAWAY_RESULT \{"outcome":"complete","summary":"Round 2 checked; no justified work remains\."/u);
    expect(fs.readFileSync(path.join(cwd, "HANDSFREE-NOTES.md"), "utf8")).toBe("Away round 1: one more line.\n");
  }, 20_000);

  it("resumes onto the same transcript and forks a copy cut at --resume-session-at", async () => {
    const home = directory();
    const cwd = directory();
    const sessionId = randomUUID();
    const first = new FakeClaude(["--session-id", sessionId], cwd, home);
    await first.initialize();
    first.prompt(sessionId, "write a.txt one");
    await first.results(1);
    first.prompt(sessionId, "append a.txt two");
    await first.results(2);
    await first.close();
    const original = readTranscript(transcriptPath(home, cwd, sessionId));
    const firstTurnEnd = forkCut(original, 1);
    expect(firstTurnEnd).toBeDefined();

    const resumed = new FakeClaude([`--resume=${sessionId}`], cwd, home);
    await resumed.initialize();
    resumed.prompt(sessionId, "append a.txt three");
    await resumed.results(1);
    await resumed.close();
    expect(fs.readFileSync(path.join(cwd, "a.txt"), "utf8")).toBe("one\ntwo\nthree\n");
    const continued = readTranscript(transcriptPath(home, cwd, sessionId));
    expectChained(continued, sessionId, cwd, null);
    expect(prompts(continued)).toEqual(["write a.txt one", "append a.txt two", "append a.txt three"]);

    const forkId = randomUUID();
    const fork = new FakeClaude([`--resume=${sessionId}`, "--fork-session", `--session-id=${forkId}`, "--resume-session-at", firstTurnEnd!.uuid], cwd, home);
    await fork.initialize();
    expect(fs.existsSync(transcriptPath(home, cwd, forkId))).toBe(false);
    fork.prompt(forkId, "note this down");
    await fork.results(1);
    await fork.close();
    const forked = readTranscript(transcriptPath(home, cwd, forkId));
    expectChained(forked, forkId, cwd, null);
    expect(prompts(forked)).toEqual(["write a.txt one", "note this down"]);
    expect(fs.readFileSync(path.join(cwd, "HANDSFREE-NOTES.md"), "utf8")).toBe("note this down\n");
    expect(readTranscript(transcriptPath(home, cwd, sessionId))).toEqual(continued);
  }, 30_000);

  it("asks the host before an `ask` edit and keeps its answer", async () => {
    const home = directory();
    const cwd = directory();
    const sessionId = randomUUID();
    const claude = new FakeClaude(["--session-id", sessionId], cwd, home);
    await claude.initialize();
    const answer = (request: Frame, response: Frame) => {
      claude.send({ type: "control_response", response: { subtype: "success", request_id: request.request_id, response } });
    };

    claude.prompt(sessionId, "ask write denied.txt no");
    const denied = await claude.waitFor(frame => frame.type === "control_request");
    expect(denied.request).toMatchObject({ subtype: "can_use_tool", tool_name: "Write", input: { file_path: path.join(cwd, "denied.txt") } });
    answer(denied, { behavior: "deny", message: "Not this one" });
    const [refused] = await claude.results(1);
    expect(refused.permission_denials).toHaveLength(1);
    expect(fs.existsSync(path.join(cwd, "denied.txt"))).toBe(false);

    const seen = claude.frames.length;
    claude.prompt(sessionId, "ask write allowed.txt yes");
    answer(await claude.waitFor(frame => frame.type === "control_request", seen), { behavior: "allow" });
    await claude.results(2);
    await claude.close();
    expect(fs.readFileSync(path.join(cwd, "allowed.txt"), "utf8")).toBe("yes\n");
    const records = readTranscript(transcriptPath(home, cwd, sessionId));
    expect(records.find(record => hasToolResult(record))?.message.content[0]).toMatchObject({ is_error: true, content: "Not this one" });
  }, 20_000);

  it("streams a `burst` reply the way Opus does: a dozen deltas at once, then a pause", async () => {
    const home = directory();
    const cwd = directory();
    const chatId = randomUUID();
    const chat = new FakeClaude(["--session-id", chatId, "--tools", "", "--include-partial-messages"], cwd, home);
    await chat.initialize();
    chat.prompt(chatId, "burst the light at the top of the tower keeps signalling to ships that are not there");
    await chat.results(1);
    await chat.close();
    const deltaTimes = chat.frames.flatMap((frame, index) => frame.type === "stream_event"
      && frame.event.type === "content_block_delta" && frame.event.delta.type === "text_delta" ? [chat.arrivals[index]] : []);
    const bursts: number[] = [];
    deltaTimes.forEach((at, index) => {
      if (index === 0 || at - deltaTimes[index - 1] > 400) bursts.push(0);
      bursts[bursts.length - 1] += 1;
    });
    expect(bursts.length).toBeGreaterThan(1);
    expect(bursts.every(size => size <= 12)).toBe(true);
    expect(bursts.slice(0, -1).every(size => size === 12)).toBe(true);
  }, 20_000);

  it("starts and waits for a thread and schedules a prompt through the iface_team server its --mcp-config names (`team start`, `team wait`, `team schedule`)", async () => {
    const home = directory();
    const cwd = directory();
    const socket = path.join(directory(), "s.sock");
    const calls: Frame[] = [];
    const server = net.createServer(connection => {
      let buffered = "";
      connection.setEncoding("utf8");
      connection.on("data", (chunk: string) => {
        buffered += chunk;
        for (let newline = buffered.indexOf("\n"); newline >= 0; newline = buffered.indexOf("\n")) {
          const message = JSON.parse(buffered.slice(0, newline)) as Frame;
          buffered = buffered.slice(newline + 1);
          let result: Frame = {};
          if (message.method === "tools/call") {
            calls.push(message.params);
            const answer = message.params.name === "schedule_prompt"
              ? { id: "s-0123456789ab", when: message.params.arguments.when, nextAt: 1, nextAtIso: "2026-11-15T09:00:00Z" }
              : message.params.name === "start_thread"
              ? { thread: "claude#child-1", title: "append child.txt hello from the child", provider: "claude", model: null, status: "running" }
              : { thread: "claude#child-1", title: "append child.txt hello from the child", status: "done",
                answer: "Wrote `child.txt`.", turn: 0, waitTimedOut: false };
            result = { content: [{ type: "text", text: JSON.stringify(answer) }] };
          }
          connection.write(`${JSON.stringify({ jsonrpc: "2.0", id: message.id, result })}\n`);
        }
      });
    });
    await new Promise<void>(resolve => { server.listen(socket, resolve); });
    try {
      const sessionId = randomUUID();
      const config = JSON.stringify({ mcpServers: { iface_team: { type: "stdio", command: "/usr/bin/nc", args: ["-U", socket], env: {} } } });
      const claude = new FakeClaude(["--session-id", sessionId, "--mcp-config", config], cwd, home);
      await claude.initialize();
      claude.prompt(sessionId, "team start append child.txt hello from the child");
      const [started] = await claude.results(1);
      expect(started?.result).toBe(
        "The thread \"append child.txt hello from the child\" is done. It answered: Wrote `child.txt`.");
      // The task went to the started thread; this one changed nothing itself.
      expect(fs.existsSync(path.join(cwd, "child.txt"))).toBe(false);
      expect(calls.map(call => call.name)).toEqual(["start_thread", "wait_thread"]);
      expect(calls[0]?.arguments.task).toBe("append child.txt hello from the child");
      expect(typeof calls[0]?.arguments.requestId).toBe("string");
      expect(calls[1]?.arguments).toEqual({ thread: "claude#child-1", timeoutSec: 600 });
      const init = claude.frames.find(frame => frame.type === "system" && frame.subtype === "init");
      expect(init?.mcp_servers).toEqual([{ name: "iface_team", status: "connected" }]);
      const uses = claude.frames.filter(frame => frame.type === "assistant")
        .flatMap(frame => frame.message.content).filter((block: Frame) => block.type === "tool_use");
      expect(uses.map((block: Frame) => block.name)).toEqual(["mcp__iface_team__start_thread", "mcp__iface_team__wait_thread"]);
      const results = claude.frames.filter(frame => frame.type === "user")
        .flatMap(frame => frame.message.content).filter((block: Frame) => block.type === "tool_result");
      expect(results.map((block: Frame) => block.tool_use_id)).toEqual(uses.map((block: Frame) => block.id));

      claude.prompt(sessionId, "team wait");
      const [, waited] = await claude.results(2);
      expect(waited?.result).toContain("It answered: Wrote `child.txt`.");
      expect(calls.at(-1)).toEqual({ name: "wait_thread", arguments: { thread: "claude#child-1", timeoutSec: 600 } });

      // `team schedule <minutes> <prompt>` (T3 section 7): a prompt to this thread later.
      claude.prompt(sessionId, "team schedule 2 append later.txt from the schedule");
      const [, , scheduled] = await claude.results(3);
      expect(scheduled?.result).toBe("Scheduled s-0123456789ab for 2026-11-15T09:00:00Z.");
      expect(calls.at(-1)).toEqual({ name: "schedule_prompt",
        arguments: { prompt: "append later.txt from the schedule", when: { kind: "once", inMinutes: 2 } } });
      expect(fs.existsSync(path.join(cwd, "later.txt"))).toBe(false);
      expect(await claude.close()).toBe(0);
    } finally {
      await new Promise<void>(resolve => { server.close(() => { resolve(); }); });
    }

    // Without the server the verb says so and touches nothing.
    const bare = new FakeClaude(["--session-id", randomUUID()], cwd, home);
    await bare.initialize();
    bare.prompt(randomUUID(), "team start append child.txt hi");
    const [refused] = await bare.results(1);
    expect(refused?.result).toBe("Team tools are not available in this session.");
    expect(fs.existsSync(path.join(cwd, "child.txt"))).toBe(false);
    expect(await bare.close()).toBe(0);
  }, 30_000);

  it("shows a chart through show_visual for `visual <title>`, then answers in words", async () => {
    const home = directory();
    const cwd = directory();
    const socket = path.join(directory(), "s.sock");
    const calls: Frame[] = [];
    const server = net.createServer(connection => {
      let buffered = "";
      connection.setEncoding("utf8");
      connection.on("data", (chunk: string) => {
        buffered += chunk;
        for (let newline = buffered.indexOf("\n"); newline >= 0; newline = buffered.indexOf("\n")) {
          const message = JSON.parse(buffered.slice(0, newline)) as Frame;
          buffered = buffered.slice(newline + 1);
          let result: Frame = {};
          if (message.method === "tools/call") {
            calls.push(message.params);
            result = { content: [{ type: "text", text: JSON.stringify({ id: "v1731630000500-0a1b2c3d", bytes: 1200 }) }] };
          }
          connection.write(`${JSON.stringify({ jsonrpc: "2.0", id: message.id, result })}\n`);
        }
      });
    });
    await new Promise<void>(resolve => { server.listen(socket, resolve); });
    try {
      const sessionId = randomUUID();
      const config = JSON.stringify({ mcpServers: { iface_team: { type: "stdio", command: "/usr/bin/nc", args: ["-U", socket], env: {} } } });
      const claude = new FakeClaude(["--session-id", sessionId, "--mcp-config", config], cwd, home);
      await claude.initialize();
      claude.prompt(sessionId, "visual Signups <by> week");
      const [shown] = await claude.results(1);
      expect(shown?.result).toBe("Above is **Signups <by> week**: signups rose in five of the last seven weeks.");
      expect(calls.map(call => call.name)).toEqual(["show_visual"]);
      const { title, html } = calls[0]?.arguments as { title: string; html: string };
      expect(title).toBe("Signups <by> week");
      expect(html.startsWith("<!doctype html>")).toBe(true);
      expect(html).toContain("<h1>Signups &#60;by&#62; week</h1>");
      expect(html).toContain("prefers-color-scheme: dark");
      expect(html).toContain("<canvas id=\"chart\"");
      expect(html).not.toMatch(/https?:\/\//u);
      const uses = claude.frames.filter(frame => frame.type === "assistant")
        .flatMap(frame => frame.message.content).filter((block: Frame) => block.type === "tool_use");
      expect(uses.map((block: Frame) => block.name)).toEqual(["mcp__iface_team__show_visual"]);
      claude.prompt(sessionId, "visual");
      const [, plain] = await claude.results(2);
      expect(plain?.result).toContain("**Weekly signups**");
      expect(await claude.close()).toBe(0);
    } finally {
      await new Promise<void>(resolve => { server.close(() => { resolve(); }); });
    }
    const bare = new FakeClaude(["--session-id", randomUUID()], cwd, home);
    await bare.initialize();
    bare.prompt(randomUUID(), "visual Chart");
    const [refused] = await bare.results(1);
    expect(refused?.result).toBe("Visuals are not available in this session.");
    expect(await bare.close()).toBe(0);
  }, 30_000);

  it("asks for a secret through request_secret for `secret <name>`, and never prints the value it was given", async () => {
    const home = directory();
    const cwd = directory();
    const vault = directory();
    const socket = path.join(directory(), "s.sock");
    const secretFile = path.join(vault, "STRIPE_TEST_KEY");
    fs.writeFileSync(secretFile, "sk_test_value_1234", { mode: 0o600 });
    const calls: Frame[] = [];
    let declineNext = false;
    const server = net.createServer(connection => {
      let buffered = "";
      connection.setEncoding("utf8");
      connection.on("data", (chunk: string) => {
        buffered += chunk;
        for (let newline = buffered.indexOf("\n"); newline >= 0; newline = buffered.indexOf("\n")) {
          const message = JSON.parse(buffered.slice(0, newline)) as Frame;
          buffered = buffered.slice(newline + 1);
          let result: Frame = {};
          if (message.method === "tools/call") {
            calls.push(message.params);
            const answer = declineNext ? { declined: true }
              : { path: secretFile, note: "Read the value from this file; do not print it." };
            result = { content: [{ type: "text", text: JSON.stringify(answer) }] };
          }
          connection.write(`${JSON.stringify({ jsonrpc: "2.0", id: message.id, result })}\n`);
        }
      });
    });
    await new Promise<void>(resolve => { server.listen(socket, resolve); });
    try {
      const sessionId = randomUUID();
      const config = JSON.stringify({ mcpServers: { iface_team: { type: "stdio", command: "/usr/bin/nc", args: ["-U", socket], env: {} } } });
      const claude = new FakeClaude(["--session-id", sessionId, "--mcp-config", config], cwd, home);
      await claude.initialize();
      claude.prompt(sessionId, "secret STRIPE_TEST_KEY To run the payment tests.");
      const [given] = await claude.results(1);
      expect(given?.result).toBe(`I have STRIPE_TEST_KEY (18 characters) from \`${secretFile}\`; I won't print it.`);
      expect(calls[0]).toEqual({ name: "request_secret",
        arguments: { name: "STRIPE_TEST_KEY", reason: "To run the payment tests." } });
      declineNext = true;
      claude.prompt(sessionId, "secret DB_PASSWORD");
      const [, declined] = await claude.results(2);
      expect(declined?.result).toBe("You declined DB_PASSWORD, so I'll go on without it.");
      expect(await claude.close()).toBe(0);
      expect(JSON.stringify(claude.frames)).not.toContain("sk_test_value_1234");
    } finally {
      await new Promise<void>(resolve => { server.close(() => { resolve(); }); });
    }
  }, 30_000);
  it("closes its iface_team connection when a `secret` turn is interrupted while the owner has not answered", async () => {
    const home = directory();
    const cwd = directory();
    const socket = path.join(directory(), "s.sock");
    let asked = false;
    let closed = false;
    const connections: net.Socket[] = [];
    // Never answers request_secret: the turn waits for the owner.
    const server = net.createServer(connection => {
      connections.push(connection);
      let buffered = "";
      connection.setEncoding("utf8");
      connection.on("close", () => { closed = true; });
      connection.on("data", (chunk: string) => {
        buffered += chunk;
        for (let newline = buffered.indexOf("\n"); newline >= 0; newline = buffered.indexOf("\n")) {
          const message = JSON.parse(buffered.slice(0, newline)) as Frame;
          buffered = buffered.slice(newline + 1);
          if (message.method === "tools/call") { asked = true; continue; }
          connection.write(`${JSON.stringify({ jsonrpc: "2.0", id: message.id, result: {} })}\n`);
        }
      });
    });
    await new Promise<void>(resolve => { server.listen(socket, resolve); });
    try {
      const sessionId = randomUUID();
      const config = JSON.stringify({ mcpServers: { iface_team: { type: "stdio", command: "/usr/bin/nc", args: ["-U", socket], env: {} } } });
      const claude = new FakeClaude(["--session-id", sessionId, "--mcp-config", config], cwd, home);
      await claude.initialize();
      claude.prompt(sessionId, "secret STOP_KEY To test a stop.");
      for (const deadline = Date.now() + 10_000; !asked && Date.now() < deadline;) {
        await new Promise(resolve => setTimeout(resolve, 20));
      }
      expect(asked).toBe(true);
      claude.send({ type: "control_request", request_id: "stop-1", request: { subtype: "interrupt" } });
      await claude.waitFor(frame => frame.type === "control_response" && frame.response.request_id === "stop-1");
      for (const deadline = Date.now() + 5_000; !closed && Date.now() < deadline;) {
        await new Promise(resolve => setTimeout(resolve, 20));
      }
      // The host sees the call go, as it does when Claude Code abandons it.
      expect(closed).toBe(true);
      expect(await claude.close()).toBe(0);
    } finally {
      for (const connection of connections) connection.destroy();
      await new Promise<void>(resolve => { server.close(() => { resolve(); }); });
    }
  }, 30_000);
  it("ends a `limit <seconds>` turn as a rate-limited Claude Code does, and refuses every prompt until the reset", async () => {
    const home = directory();
    const cwd = directory();
    const sessionId = randomUUID();
    const claude = new FakeClaude(["--session-id", sessionId, "--include-partial-messages"], cwd, home);
    await claude.initialize();
    const before = Math.floor(Date.now() / 1_000);
    claude.prompt(sessionId, "limit 600");
    const [limited] = await claude.results(1);
    // The frame map_rate_limit reads: a rejected status, the reset in Unix seconds.
    const event = claude.frames.find(frame => frame.type === "rate_limit_event");
    expect(event).toMatchObject({ session_id: sessionId, rate_limit_info: { status: "rejected", rateLimitType: "five_hour" } });
    const resetsAt = event?.rate_limit_info.resetsAt as number;
    expect(Number.isInteger(resetsAt)).toBe(true);
    expect(resetsAt).toBeGreaterThanOrEqual(before + 600);
    expect(resetsAt).toBeLessThanOrEqual(Math.ceil(Date.now() / 1_000) + 600);
    expect(claude.frames.indexOf(event as Frame))
      .toBeLessThan(claude.frames.findIndex(frame => frame.type === "result"));
    expect(claude.frames.find(frame => frame.type === "assistant")).toMatchObject({ error: "rate_limit" });
    expect(limited).toMatchObject({ is_error: true, session_id: sessionId });
    expect(limited.result).toMatch(/^You've hit your limit · resets \d{2}:\d{2}$/u);
    // A prompt before the reset is refused the same way; nothing is edited.
    claude.prompt(sessionId, "write hello.txt hi");
    const results = await claude.results(2);
    expect(results[1]).toMatchObject({ is_error: true });
    expect(claude.frames.filter(frame => frame.type === "rate_limit_event")).toHaveLength(2);
    expect(await claude.close()).toBe(0);
    expect(fs.existsSync(path.join(cwd, "hello.txt"))).toBe(false);

    // `limit 0` refuses its own turn only: the reset has come by the next
    // prompt (the resume's), which is answered.
    const again = randomUUID();
    const reset = new FakeClaude(["--session-id", again], cwd, home);
    await reset.initialize();
    reset.prompt(again, "limit 0");
    reset.prompt(again, "[Interface] Resumed after the usage limit reset:\nContinue from where you stopped.");
    const [refused, resumed] = await reset.results(2);
    expect(await reset.close()).toBe(0);
    expect(refused).toMatchObject({ is_error: true });
    expect(resumed).toMatchObject({ is_error: false, subtype: "success" });
    expect(fs.readFileSync(path.join(cwd, "HANDSFREE-NOTES.md"), "utf8")).toContain("Continue from where you stopped.");
  }, 20_000);

  it("only replies in a tool-less chat, stops a slow turn on interrupt and never writes outside the folder", async () => {
    const home = directory();
    const cwd = directory();
    const chatId = randomUUID();
    const chat = new FakeClaude(["--session-id", chatId, "--tools", "", "--include-partial-messages"], cwd, home);
    await chat.initialize();
    chat.prompt(chatId, "write chat.txt hello");
    await chat.results(1);
    await chat.close();
    expect(streamedText(chat.frames)).toContain("> write chat.txt hello");
    expect(fs.readdirSync(cwd)).toEqual([]);

    const sessionId = randomUUID();
    const claude = new FakeClaude(["--session-id", sessionId], cwd, home);
    await claude.initialize();
    claude.prompt(sessionId, "slow write late.txt x");
    claude.send({ type: "control_request", request_id: "stop-1", request: { subtype: "interrupt" } });
    await claude.waitFor(frame => frame.type === "control_response" && frame.response.request_id === "stop-1");
    claude.prompt(sessionId, "write ../escaped.txt x");
    const [result] = await claude.results(1);
    expect(await claude.close()).toBe(0);
    expect(result.result).toContain("outside the working directory");
    expect(fs.readdirSync(cwd)).toEqual([]);
    expect(fs.existsSync(path.join(cwd, "..", "escaped.txt"))).toBe(false);
  }, 20_000);
});
