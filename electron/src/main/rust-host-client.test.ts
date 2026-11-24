import { describe, expect, it } from "vitest";
import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  RUST_HOST_MAX_FRAME_BYTES, RUST_HOST_PROTOCOL_VERSION,
  RUST_HOST_REQUEST_OPERATIONS, RustHostClient, RustHostError, hostEnvironment, HostLogBuffer,
} from "./rust-host-client.js";
import {
  TerminalDataDirection, TerminalDataKind, type TerminalDataFrame,
} from "./terminal-data-transport.js";

function fakeHost(body: string, readyTimeoutMs = 2_000): RustHostClient {
  return new RustHostClient({
    command: process.execPath,
    args: ["-e", body],
    readyTimeoutMs,
  });
}

const ready = JSON.stringify({
  kind: "ready",
  version: 72,
  hostVersion: "0.2.0-test",
  processId: 42,
  operations: 127,
  pushes: 12,
});

describe("HostLogBuffer", () => {
  it("caps a fragmented unterminated line and preserves the following log boundary", () => {
    const buffer = new HostLogBuffer();
    expect(buffer.push("time WA")).toEqual([]);
    expect(buffer.push("RN " + "x".repeat(100_000))).toEqual([]);
    expect(buffer.push("y".repeat(100_000))).toEqual([]);
    expect(buffer.push("\ntime ERROR next\ntime INFO quiet\n")).toEqual([
      ("time WARN " + "x".repeat(2_000)).slice(0, 2_000), "time ERROR next",
    ]);
    expect(buffer.push("time ERROR last")).toEqual([]);
    expect(buffer.push("\n")).toEqual(["time ERROR last"]);
  });
});

describe("hostEnvironment", () => {
  it("drops the stamp Claude Code puts on nested sessions and keeps the rest", () => {
    const env = {
      PATH: "/usr/bin",
      CLAUDE_CODE_HIDE_ACCOUNT_INFO: "1",
      CLAUDECODE: "1",
      CLAUDE_CODE_CHILD_SESSION: "1",
      CLAUDE_CODE_SESSION_ID: "6011e51c-6a78-4be0-aa50-0be29ae39ae0",
      CLAUDE_PID: "90449",
      CLAUDE_CODE_ENTRYPOINT: "cli",
      CLAUDE_CODE_EXECPATH: "/usr/local/bin/claude",
      CLAUDE_CODE_BRIDGE_SESSION_ID: "session_x",
      CLAUDE_CODE_MESSAGING_SOCKET: "/tmp/cc-socks/90449.sock",
      CLAUDE_CODE_MESSAGING_TOKEN: "t",
    };
    expect(hostEnvironment(env)).toEqual({ PATH: "/usr/bin", CLAUDE_CODE_HIDE_ACCOUNT_INFO: "1" });
    // The caller's environment is left alone.
    expect(env.CLAUDE_CODE_CHILD_SESSION).toBe("1");
  });
});

describe("RustHostClient", () => {
  it("leads its own process group only when detached, and still stops on stdin EOF", async () => {
    for (const detached of [false, true]) {
      const report = join(mkdtempSync(join(tmpdir(), "host-pgid-")), "pgid");
      const client = new RustHostClient({ command: process.execPath, detached, readyTimeoutMs: 2_000, args: ["-e", `
        const pgid = require("node:child_process").execFileSync("ps", ["-o", "pgid=", "-p", String(process.pid)], { encoding: "utf8" }).trim();
        require("node:fs").writeFileSync(${JSON.stringify(report)}, pgid + " " + process.pid);
        process.stdout.write(${JSON.stringify(`${ready}\n`)});
        process.stdin.resume();
        process.stdin.on("end", () => process.exit(0));
      `] });
      await client.start();
      const [pgid, pid] = readFileSync(report, "utf8").split(" ").map(Number);
      expect(pgid === pid, `detached=${String(detached)}`).toBe(detached);
      const started = Date.now();
      await client.stop(5_000);
      expect(Date.now() - started).toBeLessThan(5_000);
    }
  });

  it("creates independent fd3/fd4 terminal data pipes", async () => {
    const client = fakeHost(`
      const fs = require("node:fs");
      process.stdout.write(${JSON.stringify(`${ready}\n`)});
      process.stdin.resume();
      setImmediate(() => {
        const frame = Buffer.alloc(1024);
        const count = fs.readSync(4, frame, 0, frame.length, null);
        frame[5] = 1;
        fs.writeSync(3, frame.subarray(0, count));
      });
    `);
    await client.start();
    const transport = client.terminalDataTransport;
    expect(transport).toBeDefined();
    const returned = new Promise<TerminalDataFrame>((resolve) => {
      transport?.onFrame(resolve);
    });
    transport?.send({
      terminalId: "00000000-0000-4000-8000-000000000001",
      generation: 1n,
      sequence: 0n,
      direction: TerminalDataDirection.ElectronToHost,
      kind: TerminalDataKind.Data,
      payload: new TextEncoder().encode("pipe"),
    });
    await expect(returned).resolves.toMatchObject({
      direction: TerminalDataDirection.HostToElectron,
      payload: new TextEncoder().encode("pipe"),
    });
    await client.stop(100);
  });

  it("requires the exact ready surface and correlates named-object requests", async () => {
    const client = fakeHost(`
      process.stdout.write(${JSON.stringify(`${ready}\n`)});
      let input = "";
      process.stdin.setEncoding("utf8");
      process.stdin.on("data", chunk => {
        input += chunk;
        const newline = input.indexOf("\\n");
        if (newline < 0) return;
        const request = JSON.parse(input.slice(0, newline));
        process.stdout.write(JSON.stringify({kind:"push",version:72,
          operation:"iface:registry",payload:[]}) + "\\n");
        process.stdout.write(JSON.stringify({kind:"response",version:72,
          id:request.id,ok:true,result:{operation:request.operation,payload:request.payload}})
          + "\\n");
      });
    `);
    const pushes: string[] = [];
    client.onPush(frame => { pushes.push(frame.operation); });
    expect(RUST_HOST_PROTOCOL_VERSION).toBe(72);
    expect(RUST_HOST_REQUEST_OPERATIONS).toBe(127);
    await expect(client.start()).resolves.toMatchObject({
      version: 72, operations: 127, pushes: 12,
    });
    await expect(client.invoke("iface:send", { sessionKey: "codex#1", text: "hello" }))
      .resolves.toEqual({
        operation: "iface:send",
        payload: { sessionKey: "codex#1", text: "hello" },
      });
    expect(pushes).toEqual(["iface:registry"]);
    await client.stop(10);
  });

  it("preserves typed host errors without exposing protocol wrapper text", async () => {
    const client = fakeHost(`
      process.stdout.write(${JSON.stringify(`${ready}\n`)});
      let input = "";
      process.stdin.setEncoding("utf8");
      process.stdin.on("data", chunk => {
        input += chunk;
        const newline = input.indexOf("\\n");
        if (newline < 0) return;
        const request = JSON.parse(input.slice(0, newline));
        process.stdout.write(JSON.stringify({kind:"response",version:72,id:request.id,ok:false,
          error:{code:"invalid-session",message:"No such session.",retryable:false,
            replaySafe:true}}) + "\\n");
      });
    `);
    await client.start();
    await expect(client.invoke("iface:send", { sessionKey: "missing", text: "hello" }))
      .rejects.toMatchObject({
        code: "invalid-session",
        message: "No such session.",
        retryable: false,
        replaySafe: true,
      });
    await client.stop();
  });

  it("preserves host-declared unknown effect certainty", async () => {
    const client = fakeHost(`
      process.stdout.write(${JSON.stringify(`${ready}\n`)});
      process.stdin.once("data", chunk => {
        const request = JSON.parse(String(chunk).trim());
        process.stdout.write(JSON.stringify({kind:"response",version:72,id:request.id,ok:false,
          error:{code:"effect-outcome-unknown",message:"Delivery is unknown.",retryable:false,
            replaySafe:false}}) + "\\n");
      });
    `);
    await client.start();
    await expect(client.invoke("iface:send", { sessionKey: "codex#one", text: "hello" }))
      .rejects.toMatchObject({ code: "effect-outcome-unknown", replaySafe: false });
    await client.stop();
  });

  it("marks an in-flight request unknown when the host exits", async () => {
    const client = fakeHost(`
      process.stdout.write(${JSON.stringify(`${ready}\n`)});
      process.stdin.once("data", () => process.exit(9));
    `);
    await client.start();
    let error: unknown;
    try {
      await client.invoke("iface:send", { sessionKey: "codex#1", text: "hello" });
    } catch (caught) {
      error = caught;
    }
    expect(error).toBeInstanceOf(RustHostError);
    expect(error).toMatchObject({ code: "host-exited", replaySafe: false });
  });

  it("reports an unexpected idle host exit without waiting for another request", async () => {
    const client = fakeHost(`
      process.stdout.write(${JSON.stringify(`${ready}\n`)});
      setTimeout(() => process.exit(17), 20);
    `);
    const failures: RustHostError[] = [];
    const failure = new Promise<RustHostError>((resolve) => {
      client.onFailure((error) => { failures.push(error); resolve(error); });
    });
    await client.start();
    await expect(failure).resolves.toMatchObject({
      code: "host-exited",
      message: "The Rust host stopped unexpectedly (exit 17).",
      retryable: true,
      replaySafe: false,
    });
    expect(failures).toHaveLength(1);
  });

  it("does not report the application's own host shutdown as a failure", async () => {
    const client = fakeHost(`
      process.stdout.write(${JSON.stringify(`${ready}\n`)});
      process.stdin.resume();
    `);
    const failures: RustHostError[] = [];
    client.onFailure((error) => { failures.push(error); });
    await client.start();
    await client.stop(100);
    expect(failures).toEqual([]);
  });

  // stop() ends stdin. A request written after that would emit a stream
  // 'error' with no listener, i.e. an uncaughtException during shutdown.
  it("refuses requests after stop() without emitting a stream error", async () => {
    const client = fakeHost(`
      process.stdout.write(${JSON.stringify(`${ready}\n`)});
      process.stdin.resume();
    `);
    const failures: RustHostError[] = [];
    client.onFailure((error) => { failures.push(error); });
    await client.start();
    const stopping = client.stop(100);
    await expect(client.invoke("getAppearance", {}))
      .rejects.toMatchObject({ code: "host-stopped" });
    await stopping;
    expect(failures).toEqual([]);
  });

  // The host can die between the readiness handshake and the next write, so
  // the write lands on a closed pipe (EPIPE). That must surface as a typed
  // host failure, not as an unhandled stream 'error'.
  it("treats a broken stdin pipe as a host failure, not an uncaught exception", async () => {
    const client = fakeHost(`
      process.stdout.write(${JSON.stringify(`${ready}\n`)});
      process.stdin.once("data", () => process.exit(0));
    `);
    const failures: RustHostError[] = [];
    client.onFailure((error) => { failures.push(error); });
    await client.start();
    const rejections: unknown[] = [];
    void client.invoke("a", {}).catch((error: unknown) => { rejections.push(error); });
    await new Promise((resolve) => { setTimeout(resolve, 50); });
    try {
      await client.invoke("b", {});
      rejections.push(new Error("the second request resolved after the host died"));
    } catch (caught) {
      rejections.push(caught);
    }
    expect(rejections.length).toBeGreaterThanOrEqual(1);
    for (const rejection of rejections) expect(rejection).toBeInstanceOf(RustHostError);
    expect(failures.length).toBeLessThanOrEqual(1);
    await client.stop(100);
  });

  // A push listener refuses a malformed frame by throwing. That throw unwinds
  // through the child's stdout `data` handler, so without a catch it is an
  // uncaughtException that takes the whole main process down instead of the
  // fail-closed "backend stopped" path.
  it("fails closed when a push listener refuses a frame", async () => {
    const client = fakeHost(`
      process.stdout.write(${JSON.stringify(`${ready}\n`)});
      process.stdout.write(JSON.stringify({kind:"push",version:72,
        operation:"iface:registry",payload:[]}) + "\\n");
      process.stdin.resume();
    `);
    const failure = new Promise<RustHostError>((resolve) => { client.onFailure(resolve); });
    client.onPush(() => { throw new TypeError("iface:event requires a {key,event} payload."); });
    await client.start();
    await expect(failure).resolves.toMatchObject({
      code: "host-protocol",
      message: "iface:event requires a {key,event} payload.",
      retryable: false,
      replaySafe: false,
    });
    await client.stop(100);
  });

  it("forces a host that stays alive after stdin EOF to stop at the shutdown deadline", async () => {
    const client = fakeHost(`
      process.stdout.write(${JSON.stringify(`${ready}\n`)});
      process.stdin.resume();
      setInterval(() => {}, 1_000);
    `);
    await client.start();
    const startedAt = Date.now();
    await client.stop(30);
    expect(Date.now() - startedAt).toBeGreaterThanOrEqual(20);
  });

  it("bounds renderer-side in-flight requests before writing more work", async () => {
    const client = new RustHostClient({
      command: process.execPath,
      args: ["-e", `process.stdout.write(${JSON.stringify(`${ready}\n`)});
        process.stdin.resume(); setInterval(()=>{}, 1000);`],
      maxPendingRequests: 1,
    });
    await client.start();
    const first = client.invoke("iface:send", { sessionKey: "codex#1", text: "first" });
    await expect(client.invoke("iface:send", { sessionKey: "codex#1", text: "second" }))
      .rejects.toMatchObject({ code: "host-backpressure", replaySafe: true });
    const stoppedFirst = expect(first)
      .rejects.toMatchObject({ code: "host-stopped", replaySafe: false });
    await client.stop(10);
    await stoppedFirst;
  });

  it("rejects malformed and over-limit stdout before accepting traffic", async () => {
    const malformed = fakeHost(`process.stdout.write("not-json\\n"); setInterval(()=>{}, 1000);`);
    await expect(malformed.start()).rejects.toMatchObject({ code: "host-protocol" });

    expect(RUST_HOST_MAX_FRAME_BYTES).toBe(192 * 1024 * 1024);
    const oversized = new RustHostClient({
      command: process.execPath,
      args: ["-e", `process.stdout.write("x".repeat(1025));`],
      maxFrameBytes: 1024,
      readyTimeoutMs: 2_000,
    });
    await expect(oversized.start()).rejects.toMatchObject({ code: "host-protocol" });

    // 400 BMP CJK code units are 1,200 UTF-8 bytes. A length-only pre-check
    // would miss this; the 3x code-unit bound must still fail closed.
    const oversizedUtf8 = new RustHostClient({
      command: process.execPath,
      args: ["-e", `process.stdout.write(${JSON.stringify("\u4e00".repeat(400))});`],
      maxFrameBytes: 1024,
      readyTimeoutMs: 2_000,
    });
    await expect(oversizedUtf8.start()).rejects.toMatchObject({ code: "host-protocol" });
  });

  it("rejects the previous v4 ready envelope after the v5 session-kind addition", async () => {
    const previous = fakeHost(`process.stdout.write(JSON.stringify({
      kind:"ready",version:4,hostVersion:"0.2.0-test",processId:42,operations:104,pushes:10
    }) + "\\n"); setInterval(()=>{}, 1000);`);
    await expect(previous.start()).rejects.toMatchObject({ code: "host-protocol" });
  });
});
