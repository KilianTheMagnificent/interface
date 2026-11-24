import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it } from "vitest";

// The scripted ACP agent a hands-free session runs for Grok
// (scripts/handsfree-fake-acp.mjs, T3 section 5): the frames
// crates/engine-acp/src/adapter_tests.rs (`fake_agent`) speaks, and the
// session updates crates/engine-acp/src/mapper.rs maps.
const scripts = fileURLToPath(new URL("../../scripts/", import.meta.url));
const FAKE_ACP = path.join(scripts, "handsfree-fake-acp.mjs");
const FAKE_PROVIDER = path.join(scripts, "handsfree-fake-provider.sh");
/** How crates/engine-acp/src/adapter.rs starts Grok: its flags, then `agent stdio`. */
const GROK_ARGS = ["--no-auto-update", "agent", "--model", "grok-fake", "stdio"];
const WAIT_MS = 30_000;

type Frame = Record<string, any>;
const temporary: string[] = [];
const children: ChildProcessWithoutNullStreams[] = [];

function directory(): string {
  const created = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "fake-acp-")));
  temporary.push(created);
  return created;
}

afterEach(() => {
  for (const child of children.splice(0)) child.kill();
  for (const created of temporary.splice(0)) fs.rmSync(created, { recursive: true, force: true });
});

class FakeAcp {
  readonly frames: Frame[] = [];
  private readonly child: ChildProcessWithoutNullStreams;
  private next = 0;

  constructor(viaShell: boolean, home: string) {
    const [command, args] = viaShell ? ["/bin/sh", [FAKE_PROVIDER, ...GROK_ARGS]]
      : [process.execPath, [FAKE_ACP, ...GROK_ARGS]];
    this.child = spawn(command, args, { env: { HOME: home, PATH: `${path.dirname(process.execPath)}:/usr/bin:/bin` } });
    children.push(this.child);
    let buffered = "";
    this.child.stdout.setEncoding("utf8");
    this.child.stdout.on("data", (chunk: string) => {
      buffered += chunk;
      const lines = buffered.split("\n");
      buffered = lines.pop() ?? "";
      for (const line of lines) if (line.trim() !== "") this.frames.push(JSON.parse(line) as Frame);
    });
  }

  send(frame: Frame): void { this.child.stdin.write(`${JSON.stringify(frame)}\n`); }

  /** Sends a request and resolves its answer. */
  async request(method: string, params: Frame = {}): Promise<Frame> {
    this.next += 1;
    const id = this.next;
    this.send({ jsonrpc: "2.0", id, method, params });
    return this.until(frame => frame.id === id && frame.method === undefined);
  }

  async until(match: (frame: Frame) => boolean): Promise<Frame> {
    const deadline = Date.now() + WAIT_MS;
    for (;;) {
      const found = this.frames.find(match);
      if (found !== undefined) return found;
      if (Date.now() > deadline) throw new Error(`no matching frame in ${JSON.stringify(this.frames)}`);
      await new Promise(resolve => { setTimeout(resolve, 10); });
    }
  }

  updates(sessionId: string): Frame[] {
    return this.frames.filter(frame => frame.method === "session/update"
      && frame.params.sessionId === sessionId).map(frame => frame.params.update as Frame);
  }
}

function said(updates: Frame[]): string {
  return updates.filter(update => update.sessionUpdate === "agent_message_chunk")
    .map(update => update.content.text as string).join("");
}

describe("hands-free fake ACP agent (Grok)", () => {
  it("is what the fake provider runs for Grok over ACP: one model, a session, a reply and one edit", async () => {
    const home = directory();
    const cwd = directory();
    const agent = new FakeAcp(true, home);
    const initialized = await agent.request("initialize", { protocolVersion: 1, clientCapabilities: {} });
    expect(initialized.result.protocolVersion).toBe(1);
    expect(initialized.result.agentCapabilities.loadSession).toBe(true);
    expect(initialized.result.models.availableModels).toEqual([{ modelId: "grok-fake", name: "Grok (fake)" }]);
    const created = await agent.request("session/new", { cwd, mcpServers: [] });
    const sessionId = created.result.sessionId as string;
    expect(sessionId).toMatch(/^fake-grok-/u);
    const ended = await agent.request("session/prompt", {
      sessionId, prompt: [{ type: "text", text: "You are taking over a thread from Claude in Interface" }],
    });
    expect(ended.result).toEqual({ stopReason: "end_turn" });
    const updates = agent.updates(sessionId);
    const call = updates.find(update => update.sessionUpdate === "tool_call");
    expect(call?.kind).toBe("edit");
    expect(call?.content[0]).toMatchObject({ type: "diff", path: path.join(cwd, "HANDSFREE-NOTES.md"), oldText: "" });
    expect(updates.find(update => update.sessionUpdate === "tool_call_update")?.status).toBe("completed");
    expect(said(updates)).toBe("Grok (fake) here. Updated HANDSFREE-NOTES.md.");
    expect(fs.readFileSync(path.join(cwd, "HANDSFREE-NOTES.md"), "utf8"))
      .toBe("You are taking over a thread from Claude in Interface\n");
    const unknown = await agent.request("x.ai/models/list");
    expect(unknown.error.code).toBe(-32601);
  });

  it("writes and appends files inside its folder, refuses one outside, and loads a session again", async () => {
    const cwd = directory();
    const agent = new FakeAcp(false, directory());
    await agent.request("initialize", { protocolVersion: 1 });
    const sessionId = (await agent.request("session/new", { cwd })).result.sessionId as string;
    await agent.request("session/prompt", { sessionId, prompt: [{ type: "text", text: "write src/a.txt alpha" }] });
    await agent.request("session/prompt", { sessionId, prompt: [{ type: "text", text: "append src/a.txt beta" }] });
    expect(fs.readFileSync(path.join(cwd, "src", "a.txt"), "utf8")).toBe("alpha\nbeta\n");
    await agent.request("session/prompt", { sessionId, prompt: [{ type: "text", text: "write ../escape.txt no" }] });
    expect(said(agent.updates(sessionId))).toContain("../escape.txt is outside the working folder.");
    expect(fs.existsSync(path.join(path.dirname(cwd), "escape.txt"))).toBe(false);
    const loaded = await agent.request("session/load", { sessionId, cwd, mcpServers: [] });
    expect(loaded.result.sessionId).toBe(sessionId);
  });

  it("asks before its edit with `ask` and leaves the files alone when refused", async () => {
    const cwd = directory();
    const agent = new FakeAcp(false, directory());
    await agent.request("initialize", { protocolVersion: 1 });
    const sessionId = (await agent.request("session/new", { cwd })).result.sessionId as string;
    agent.send({ jsonrpc: "2.0", id: 50, method: "session/prompt",
      params: { sessionId, prompt: [{ type: "text", text: "ask write b.txt yes" }] } });
    const asked = await agent.until(frame => frame.method === "session/request_permission");
    expect(asked.params.toolCall.kind).toBe("edit");
    expect(asked.params.options.map((option: Frame) => option.kind)).toEqual(["allow_once", "reject_once"]);
    agent.send({ jsonrpc: "2.0", id: asked.id, result: { outcome: { outcome: "selected", optionId: "reject-once" } } });
    await agent.until(frame => frame.id === 50 && frame.result !== undefined);
    expect(fs.existsSync(path.join(cwd, "b.txt"))).toBe(false);
    expect(said(agent.updates(sessionId))).toContain("Left the files as they were.");
  });
});
