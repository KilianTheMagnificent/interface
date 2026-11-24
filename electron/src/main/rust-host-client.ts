import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { randomUUID } from "node:crypto";
import { EventEmitter } from "node:events";
import type { Readable, Writable } from "node:stream";
import { CHANNELS } from "../shared/ipc.js";
import { EXTRA_CHANNELS } from "./bridge.js";
import { plainObject } from "./json.js";
import { TerminalDataTransport } from "./terminal-data-transport.js";
import { NdjsonLineBuffer } from "./ndjson-line-buffer.js";

export const RUST_HOST_PROTOCOL_VERSION = 72;
// Eight frozen 16 MiB image inputs fit after base64 expansion, while the
// Electron/Rust stream remains explicitly bounded.
export const RUST_HOST_MAX_FRAME_BYTES = 192 * 1024 * 1024;
export const RUST_HOST_REQUEST_OPERATIONS = 127;
export const RUST_HOST_PUSH_OPERATIONS = 12;
export const RUST_HOST_MAX_PENDING_REQUESTS = 64;

// The one push-channel set: #acceptPush admits only these, and ipc-payload.ts
// excludes them from the request surface and checks the count.
export const RUST_HOST_PUSH_CHANNELS: ReadonlySet<string> = new Set<string>([
  CHANNELS.event,
  CHANNELS.registry,
  CHANNELS.updateState,
  CHANNELS.collaborationEvent,
  CHANNELS.orchestrationEvent,
  CHANNELS.terminalLifecycle,
  CHANNELS.voice,
  CHANNELS.voiceUiCall,
  CHANNELS.appearanceChanged,
  CHANNELS.threads,
  EXTRA_CHANNELS.importProgress,
  EXTRA_CHANNELS.worktree,
]);

type HostReadyFrame = {
  kind: "ready";
  version: number;
  hostVersion: string;
  processId: number;
  operations: number;
  pushes: number;
};

type HostResponseFrame = {
  kind: "response";
  version: number;
  id: string;
  ok: boolean;
  result?: unknown;
  error?: { code: string; message: string; retryable: boolean; replaySafe: boolean };
};

export type HostPushFrame = {
  kind: "push";
  version: number;
  operation: string;
  payload: unknown;
};

export type RustHostClientOptions = {
  command: string;
  args?: string[];
  cwd?: string;
  env?: NodeJS.ProcessEnv;
  maxFrameBytes?: number;
  maxPendingRequests?: number;
  readyTimeoutMs?: number;
  /** Own process group, so a signal to the caller's group cannot skip the
   * stdin-EOF teardown (hands-free sessions only; see handsfree.ts). */
  detached?: boolean;
};

type PendingRequest = {
  resolve(value: unknown): void;
  reject(error: Error): void;
};

export class RustHostError extends Error {
  constructor(
    readonly code: string,
    message: string,
    readonly retryable: boolean,
    readonly replaySafe: boolean,
  ) {
    super(message);
    this.name = "RustHostError";
  }
}

function protocolError(message: string): RustHostError {
  return new RustHostError("host-protocol", message, false, false);
}

/**
 * Claude Code stamps every process it spawns as a nested session. The Dev app
 * is routinely started from inside an agent session (`npm run dev`), and a
 * host carrying that stamp makes every Claude pane and engine process it spawns
 * run with transcript saving off, so a restart has nothing to `--resume`. The
 * host is the one spawn point for provider CLIs, so the stamp is scrubbed here.
 */
const CLAUDE_NESTING_MARKERS = [
  "CLAUDECODE", "CLAUDE_CODE_CHILD_SESSION", "CLAUDE_CODE_SESSION_ID", "CLAUDE_PID",
  "CLAUDE_CODE_ENTRYPOINT", "CLAUDE_CODE_EXECPATH", "CLAUDE_CODE_BRIDGE_SESSION_ID",
  "CLAUDE_CODE_MESSAGING_SOCKET", "CLAUDE_CODE_MESSAGING_TOKEN",
] as const;

/** The environment the host runs under: the caller's, minus Claude's nesting stamp. */
export function hostEnvironment(env: NodeJS.ProcessEnv): NodeJS.ProcessEnv {
  const clean = { ...env };
  for (const name of CLAUDE_NESTING_MARKERS) delete clean[name];
  return clean;
}

/** A tracing line at WARN or ERROR: the two levels worth a line in the main log. */
export function hostLogLineIsLoud(line: string): boolean {
  return / (WARN|ERROR) /.test(line);
}

/** Keep only the printable prefix, even when a host log never terminates. */
export class HostLogBuffer {
  #tail = "";

  push(chunk: string): string[] {
    const lines: string[] = [];
    let start = 0;
    while (start < chunk.length) {
      const newline = chunk.indexOf("\n", start);
      const end = newline < 0 ? chunk.length : newline;
      this.#tail += chunk.slice(start, Math.min(end, start + 2_000 - this.#tail.length));
      if (newline < 0) break;
      if (hostLogLineIsLoud(this.#tail)) lines.push(this.#tail);
      this.#tail = "";
      start = newline + 1;
    }
    return lines;
  }
}

export class RustHostClient {
  readonly #options: Required<Pick<RustHostClientOptions,
    "maxFrameBytes" | "maxPendingRequests" | "readyTimeoutMs">>
    & Omit<RustHostClientOptions, "maxFrameBytes" | "maxPendingRequests" | "readyTimeoutMs">;
  readonly #events = new EventEmitter();
  readonly #pending = new Map<string, PendingRequest>();
  #child: ChildProcessWithoutNullStreams | undefined;
  readonly #lines: NdjsonLineBuffer;
  #ready: HostReadyFrame | undefined;
  #readyResolve: ((ready: HostReadyFrame) => void) | undefined;
  #readyReject: ((error: Error) => void) | undefined;
  #terminalError: RustHostError | undefined;
  #terminalDataTransport: TerminalDataTransport | undefined;
  #stopping = false;

  constructor(options: RustHostClientOptions) {
    this.#options = {
      ...options,
      maxFrameBytes: options.maxFrameBytes ?? RUST_HOST_MAX_FRAME_BYTES,
      maxPendingRequests: options.maxPendingRequests ?? RUST_HOST_MAX_PENDING_REQUESTS,
      readyTimeoutMs: options.readyTimeoutMs ?? 10_000,
    };
    this.#lines = new NdjsonLineBuffer(this.#options.maxFrameBytes);
  }

  get ready(): HostReadyFrame | undefined {
    return this.#ready;
  }

  get terminalDataTransport(): TerminalDataTransport | undefined {
    return this.#terminalDataTransport;
  }

  async start(): Promise<HostReadyFrame> {
    if (this.#child !== undefined) {
      throw new RustHostError("host-already-started", "The Rust host is already started.", false, true);
    }
    if (this.#terminalError !== undefined) throw this.#terminalError;

    let timer: NodeJS.Timeout;
    const readiness = new Promise<HostReadyFrame>((resolve, reject) => {
      timer = setTimeout(() => {
        this.#fail(new RustHostError(
          "host-ready-timeout",
          "The Rust host did not complete its ready handshake.",
          true,
          true,
        ));
      }, this.#options.readyTimeoutMs);
      timer.unref();
      this.#readyResolve = ready => {
        clearTimeout(timer);
        resolve(ready);
      };
      this.#readyReject = error => {
        clearTimeout(timer);
        reject(error);
      };
    });

    let child: ChildProcessWithoutNullStreams;
    try {
      child = spawn(this.#options.command, this.#options.args ?? [], {
        cwd: this.#options.cwd,
        env: hostEnvironment(this.#options.env ?? process.env),
        // fd3 is host -> Electron terminal data; fd4 is Electron -> host.
        stdio: ["pipe", "pipe", "pipe", "pipe", "pipe"],
        detached: this.#options.detached === true,
      });
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      this.#fail(new RustHostError("host-spawn", message, true, true));
      return await readiness;
    }
    this.#child = child;
    const terminalOutput = child.stdio[3];
    const terminalInput = child.stdio[4];
    if (terminalOutput === null || terminalOutput === undefined
        || terminalInput === null || terminalInput === undefined) {
      this.#fail(new RustHostError(
        "host-spawn",
        "The Rust host terminal data pipes did not open.",
        true,
        true,
      ));
      return await readiness;
    }
    this.#terminalDataTransport = new TerminalDataTransport(
      terminalOutput as Readable,
      terminalInput as Writable,
    );
    child.stdout.on("data", (chunk: Buffer) => { this.#consume(chunk); });
    // Host logs (tracing on stderr) are never forwarded into the protocol or
    // the renderer. Their WARN and ERROR lines do reach the main-process log,
    // prefixed, so a shutdown stage that never settles — or any host-side
    // failure — is visible in the dev log instead of vanishing. INFO and below
    // stay out of it.
    child.stderr.setEncoding("utf8");
    const stderr = new HostLogBuffer();
    child.stderr.on("data", (chunk: string) => {
      for (const line of stderr.push(chunk)) console.error(`[host] ${line}`);
    });
    child.once("error", (error) => {
      this.#fail(new RustHostError("host-spawn", error.message, true, true));
    });
    // A write that lands after the host closed its stdin (EPIPE) or after
    // stop() ended the stream would otherwise surface as an unhandled stream
    // 'error' — an uncaughtException that blocks shutdown behind a modal.
    child.stdin.on("error", (error: Error) => {
      if (this.#stopping) return;
      this.#fail(new RustHostError("host-write", error.message, true, true));
    });
    child.once("exit", (code, signal) => {
      if (this.#stopping) return;
      const detail = signal === null ? `exit ${String(code)}` : `signal ${signal}`;
      this.#fail(new RustHostError(
        "host-exited",
        `The Rust host stopped unexpectedly (${detail}).`,
        true,
        false,
      ));
    });

    return await readiness;
  }

  onPush(listener: (frame: HostPushFrame) => void): () => void {
    this.#events.on("push", listener);
    return () => { this.#events.off("push", listener); };
  }

  /** Reports a terminal transport failure even when no request is in flight. */
  onFailure(listener: (error: RustHostError) => void): () => void {
    this.#events.on("failure", listener);
    return () => { this.#events.off("failure", listener); };
  }

  async invoke(operation: string, payload: unknown): Promise<unknown> {
    const child = this.#child;
    if (this.#ready === undefined || child === undefined || child.stdin.destroyed
        || child.stdin.writableEnded || this.#stopping) {
      throw this.#terminalError ?? new RustHostError(
        "host-not-ready",
        "The Rust host is not ready.",
        true,
        true,
      );
    }
    if (this.#pending.size >= this.#options.maxPendingRequests) {
      throw new RustHostError(
        "host-backpressure",
        "The Rust host already has the maximum number of requests in flight.",
        true,
        true,
      );
    }
    const id = randomUUID();
    const frame = JSON.stringify({
      version: RUST_HOST_PROTOCOL_VERSION,
      id,
      operation,
      payload,
    });
    const encoded = Buffer.from(`${frame}\n`, "utf8");
    if (encoded.byteLength - 1 > this.#options.maxFrameBytes) {
      throw new RustHostError("request-too-large", "The Rust host request exceeded its limit.", false, true);
    }

    return await new Promise<unknown>((resolve, reject) => {
      this.#pending.set(id, { resolve, reject });
      child.stdin.write(encoded, (error) => {
        if (error === null || error === undefined) return;
        this.#pending.delete(id);
        reject(new RustHostError("host-write", error.message, true, true));
      });
    });
  }

  async stop(timeoutMs = 2_000): Promise<void> {
    const child = this.#child;
    if (child === undefined) return;
    this.#stopping = true;
    const stopped = new RustHostError(
      "host-stopped",
      "The Rust host was stopped by the application.",
      true,
      false,
    );
    this.#terminalError ??= stopped;
    const rejectReady = this.#readyReject;
    this.#readyResolve = undefined;
    this.#readyReject = undefined;
    rejectReady?.(stopped);
    for (const pending of this.#pending.values()) pending.reject(stopped);
    this.#pending.clear();
    this.#terminalDataTransport?.close();
    if (!child.stdin.destroyed && !child.stdin.writableEnded) child.stdin.end();
    if (child.exitCode !== null || child.signalCode !== null) return;
    await new Promise<void>((resolve) => {
      let settled = false;
      let forceTimer: NodeJS.Timeout | undefined;
      const finish = (): void => {
        if (settled) return;
        settled = true;
        clearTimeout(terminateTimer);
        if (forceTimer !== undefined) clearTimeout(forceTimer);
        child.off("exit", finish);
        resolve();
      };
      const terminateTimer = setTimeout(() => {
        if (!child.kill("SIGTERM")) {
          finish();
          return;
        }
        forceTimer = setTimeout(() => {
          if (child.exitCode === null && child.signalCode === null) child.kill("SIGKILL");
          finish();
        }, 1_000);
      }, timeoutMs);
      // These deadlines deliberately stay referenced: after the only window
      // closes they are the guarantee that shutdown cannot remain headless.
      child.once("exit", finish);
    });
  }

  #consume(chunk: Buffer): void {
    if (this.#terminalError !== undefined) return;
    if (!this.#lines.push(chunk, line => {
      if (line.length > 0) this.#acceptLine(line);
      return this.#terminalError === undefined;
    })) {
      this.#fail(protocolError("The Rust host emitted an over-limit frame."));
    }
  }

  #acceptLine(line: string): void {
    let parsed: unknown;
    try {
      parsed = JSON.parse(line);
    } catch {
      this.#fail(protocolError("The Rust host emitted invalid JSON."));
      return;
    }
    const frame = plainObject(parsed);
    if (frame === undefined || frame.version !== RUST_HOST_PROTOCOL_VERSION) {
      this.#fail(protocolError("The Rust host emitted an invalid protocol envelope."));
      return;
    }
    if (frame.kind === "ready") {
      this.#acceptReady(frame);
      return;
    }
    if (this.#ready === undefined) {
      this.#fail(protocolError("The Rust host emitted traffic before its ready handshake."));
      return;
    }
    if (frame.kind === "response") {
      this.#acceptResponse(frame);
      return;
    }
    if (frame.kind === "push") {
      this.#acceptPush(frame);
      return;
    }
    this.#fail(protocolError("The Rust host emitted an unknown frame kind."));
  }

  #acceptReady(frame: Record<string, unknown>): void {
    if (this.#ready !== undefined
        || typeof frame.hostVersion !== "string"
        || typeof frame.processId !== "number"
        || frame.operations !== RUST_HOST_REQUEST_OPERATIONS
        || frame.pushes !== RUST_HOST_PUSH_OPERATIONS) {
      this.#fail(protocolError("The Rust host ready handshake did not match the frozen surface."));
      return;
    }
    const ready = frame as unknown as HostReadyFrame;
    this.#ready = ready;
    const resolve = this.#readyResolve;
    this.#readyResolve = undefined;
    this.#readyReject = undefined;
    resolve?.(ready);
  }

  #acceptResponse(frame: Record<string, unknown>): void {
    if (typeof frame.id !== "string" || typeof frame.ok !== "boolean") {
      this.#fail(protocolError("The Rust host emitted an invalid response."));
      return;
    }
    const pending = this.#pending.get(frame.id);
    if (pending === undefined) {
      this.#fail(protocolError("The Rust host emitted a response for an unknown request."));
      return;
    }
    const response = frame as unknown as HostResponseFrame;
    if (response.ok) {
      if (response.error !== undefined) {
        this.#fail(protocolError("A successful Rust host response also contained an error."));
        return;
      }
      this.#pending.delete(response.id);
      pending.resolve(response.result);
      return;
    }
    const error = plainObject(response.error);
    if (error === undefined || typeof error.code !== "string"
        || typeof error.message !== "string" || typeof error.retryable !== "boolean"
        || typeof error.replaySafe !== "boolean") {
      this.#fail(protocolError("A failed Rust host response omitted its typed error."));
      return;
    }
    if (response.result !== undefined) {
      this.#fail(protocolError("A failed Rust host response also contained a result."));
      return;
    }
    this.#pending.delete(response.id);
    pending.reject(new RustHostError(
      error.code,
      error.message,
      error.retryable,
      error.replaySafe,
    ));
  }

  #acceptPush(frame: Record<string, unknown>): void {
    if (typeof frame.operation !== "string" || !RUST_HOST_PUSH_CHANNELS.has(frame.operation)) {
      this.#fail(protocolError("The Rust host emitted an unknown push operation."));
      return;
    }
    // A push listener that refuses a malformed frame throws synchronously
    // through this emit, and the whole chain up to the child's stdout `data`
    // handler has no catch — so without this the refusal would surface as an
    // uncaughtException instead of the fail-closed shutdown path.
    try {
      this.#events.emit("push", frame as unknown as HostPushFrame);
    } catch (error) {
      this.#fail(protocolError(error instanceof Error
        ? error.message
        : "A Rust host push frame was refused by the renderer bridge."));
    }
  }

  #fail(error: RustHostError): void {
    if (this.#terminalError !== undefined) return;
    this.#terminalError = error;
    this.#lines.clear();
    const rejectReady = this.#readyReject;
    this.#readyResolve = undefined;
    this.#readyReject = undefined;
    rejectReady?.(error);
    for (const pending of this.#pending.values()) {
      pending.reject(new RustHostError(error.code, error.message, error.retryable, false));
    }
    this.#pending.clear();
    const child = this.#child;
    this.#terminalDataTransport?.close();
    if (child !== undefined && child.exitCode === null && child.signalCode === null) {
      child.kill("SIGTERM");
    }
    this.#events.emit("failure", error);
  }
}
