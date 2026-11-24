import { EventEmitter } from "node:events";
import { outputKey, TerminalOutputWindow, type OutputIdentity } from "./terminal-output-window.js";
import type { Readable, Writable } from "node:stream";

export const TERMINAL_DATA_MAGIC = Buffer.from("ITRM", "ascii");
export const TERMINAL_DATA_VERSION = 2;
export const TERMINAL_DATA_HEADER_BYTES = 44;
export const TERMINAL_DATA_MAX_PAYLOAD_BYTES = 64 * 1024;
export const TERMINAL_DATA_MAX_RETAINED_EOF_STREAMS = 1024;
// Mirrors TerminalServiceConfig::default().max_terminals. One retirement per
// terminal id protects every visible retained generation without unbounded
// create/exit/dismiss growth.
export const TERMINAL_DATA_MAX_RETIRED_TERMINALS = 32;

const VERSION_OFFSET = 4;
const DIRECTION_OFFSET = 5;
const KIND_OFFSET = 6;
const RESERVED_OFFSET = 7;
const TERMINAL_ID_OFFSET = 8;
const GENERATION_OFFSET = 24;
const SEQUENCE_OFFSET = 32;
const PAYLOAD_LENGTH_OFFSET = 40;

export enum TerminalDataDirection {
  HostToElectron = 1,
  ElectronToHost = 2,
}

export enum TerminalDataKind {
  Data = 1,
  Eof = 2,
  OutputAck = 3,
}

export type TerminalDataFrame = {
  terminalId: string;
  generation: bigint;
  sequence: bigint;
  direction: TerminalDataDirection;
  kind: TerminalDataKind;
  payload: Uint8Array;
};

export type TerminalDataQueueLimits = {
  totalBytes: number;
  perTerminalBytes: number;
};

export type TerminalDataTransportOptions = {
  maximumPayloadBytes?: number;
  queueLimits?: Partial<TerminalDataQueueLimits>;
};

export type TerminalDataPortMessage = {
  channel: "interface-terminal-data";
  version: 2;
  terminalId: string;
  generation: string;
  sequence: string;
  direction: TerminalDataDirection;
  kind: TerminalDataKind;
  payload: ArrayBuffer;
};

export interface TerminalMessagePort {
  postMessage(message: unknown): void;
  on(event: "message", listener: (event: { data: unknown }) => void): unknown;
  off(event: "message", listener: (event: { data: unknown }) => void): unknown;
  start(): void;
}

export class TerminalDataTransportError extends Error {
  constructor(readonly code: string, message: string) {
    super(message);
    this.name = "TerminalDataTransportError";
  }
}

function protocolError(message: string): TerminalDataTransportError {
  return new TerminalDataTransportError("terminal-data-protocol", message);
}

/** A transport error passes through; anything else is filed under `code`. */
function asTransportError(error: unknown, code: string): TerminalDataTransportError {
  return error instanceof TerminalDataTransportError
    ? error
    : new TerminalDataTransportError(code, String(error));
}

function streamKey(frame: TerminalDataFrame): string {
  return `${frame.terminalId}:${frame.generation.toString()}:${frame.direction}`;
}

const TERMINAL_ID_PATTERN =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/u;

function normalizeTerminalId(terminalId: string): string {
  const normalized = terminalId.toLowerCase();
  if (!TERMINAL_ID_PATTERN.test(normalized)) {
    throw protocolError("The terminal data frame contained an invalid terminal ID.");
  }
  return normalized;
}

function terminalIdBytes(terminalId: string): Buffer {
  return Buffer.from(normalizeTerminalId(terminalId).replaceAll("-", ""), "hex");
}

function terminalIdFromBytes(bytes: Uint8Array): string {
  const hex = Buffer.from(bytes).toString("hex");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

function validateFrame(frame: TerminalDataFrame, maximumPayloadBytes: number): void {
  normalizeTerminalId(frame.terminalId);
  if (frame.generation <= 0n || frame.generation > 0xffff_ffff_ffff_ffffn) {
    throw protocolError("The terminal data frame contained an invalid generation.");
  }
  if (frame.sequence < 0n || frame.sequence > 0xffff_ffff_ffff_ffffn) {
    throw protocolError("The terminal data frame contained an invalid sequence.");
  }
  if (frame.direction !== TerminalDataDirection.HostToElectron
      && frame.direction !== TerminalDataDirection.ElectronToHost) {
    throw protocolError("The terminal data frame contained an invalid direction.");
  }
  if (frame.kind !== TerminalDataKind.Data && frame.kind !== TerminalDataKind.Eof
      && frame.kind !== TerminalDataKind.OutputAck) {
    throw protocolError("The terminal data frame contained an invalid kind.");
  }
  if (frame.payload.byteLength > maximumPayloadBytes) {
    throw new TerminalDataTransportError(
      "terminal-data-too-large",
      `The terminal data payload exceeded ${maximumPayloadBytes.toString()} bytes.`,
    );
  }
  if (frame.kind === TerminalDataKind.Eof && frame.payload.byteLength !== 0) {
    throw protocolError("A terminal EOF frame contained a payload.");
  }
  if (frame.kind === TerminalDataKind.OutputAck
      && (frame.direction !== TerminalDataDirection.ElectronToHost || frame.payload.byteLength !== 0)) {
    throw protocolError("A terminal output acknowledgement was invalid.");
  }
}

export function encodeTerminalDataFrame(
  frame: TerminalDataFrame,
  maximumPayloadBytes = TERMINAL_DATA_MAX_PAYLOAD_BYTES,
): Buffer {
  validateFrame(frame, maximumPayloadBytes);
  const encoded = Buffer.alloc(TERMINAL_DATA_HEADER_BYTES + frame.payload.byteLength);
  TERMINAL_DATA_MAGIC.copy(encoded, 0);
  encoded.writeUInt8(TERMINAL_DATA_VERSION, VERSION_OFFSET);
  encoded.writeUInt8(frame.direction, DIRECTION_OFFSET);
  encoded.writeUInt8(frame.kind, KIND_OFFSET);
  encoded.writeUInt8(0, RESERVED_OFFSET);
  terminalIdBytes(frame.terminalId).copy(encoded, TERMINAL_ID_OFFSET);
  encoded.writeBigUInt64LE(frame.generation, GENERATION_OFFSET);
  encoded.writeBigUInt64LE(frame.sequence, SEQUENCE_OFFSET);
  encoded.writeUInt32LE(frame.payload.byteLength, PAYLOAD_LENGTH_OFFSET);
  Buffer.from(frame.payload.buffer, frame.payload.byteOffset, frame.payload.byteLength)
    .copy(encoded, TERMINAL_DATA_HEADER_BYTES);
  return encoded;
}

/**
 * The streams whose EOF has already been seen, bounded and forgotten oldest
 * first. The order array is deliberately not deduplicated: a repeated key is
 * pushed again exactly as the Set-plus-array pair always behaved, so the
 * ceiling counts insertions rather than distinct streams.
 */
class BoundedEofStreams {
  readonly #keys = new Set<string>();

  get size(): number {
    return this.#keys.size;
  }

  has(key: string): boolean {
    return this.#keys.has(key);
  }

  add(key: string): void {
    // Refresh insertion order so re-adding an already retired stream key does
    // not evict it prematurely when older entries roll off the window.
    this.#keys.delete(key);
    this.#keys.add(key);
    while (this.#keys.size > TERMINAL_DATA_MAX_RETAINED_EOF_STREAMS) {
      const expired = this.#keys.values().next().value;
      if (expired !== undefined) this.#keys.delete(expired);
    }
  }

  delete(key: string): void {
    this.#keys.delete(key);
  }

  clear(): void {
    this.#keys.clear();
  }
}

export class TerminalDataDecoder {
  readonly #expectedDirection: TerminalDataDirection;
  readonly #maximumPayloadBytes: number;
  readonly #nextSequences = new Map<string, bigint>();
  readonly #endedStreams = new BoundedEofStreams();
  #buffer: Buffer<ArrayBufferLike> = Buffer.alloc(TERMINAL_DATA_HEADER_BYTES);
  #bufferedBytes = 0;
  #frameLength = TERMINAL_DATA_HEADER_BYTES;

  constructor(
    expectedDirection: TerminalDataDirection,
    maximumPayloadBytes = TERMINAL_DATA_MAX_PAYLOAD_BYTES,
  ) {
    this.#expectedDirection = expectedDirection;
    this.#maximumPayloadBytes = maximumPayloadBytes;
  }

  get retainedSequenceStreams(): number {
    return this.#nextSequences.size;
  }

  get retainedEofStreams(): number {
    return this.#endedStreams.size;
  }

  push(bytes: Uint8Array): TerminalDataFrame[] {
    const incoming = Buffer.from(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    const frames: TerminalDataFrame[] = [];
    let offset = 0;
    while (offset < incoming.byteLength) {
      // Complete frames decode directly from the incoming chunk. Only a partial
      // frame is copied, into one allocation sized from its validated header.
      if (this.#bufferedBytes === 0 && incoming.byteLength - offset >= TERMINAL_DATA_HEADER_BYTES) {
        const remaining = incoming.subarray(offset);
        const length = this.#readFrameLength(remaining);
        if (remaining.byteLength >= length) {
          frames.push(this.#decodeFrame(remaining.subarray(0, length)));
          offset += length;
          continue;
        }
        this.#buffer = Buffer.alloc(length);
        remaining.copy(this.#buffer);
        this.#bufferedBytes = remaining.byteLength;
        this.#frameLength = length;
        break;
      }
      if (this.#bufferedBytes < TERMINAL_DATA_HEADER_BYTES) {
        const count = Math.min(TERMINAL_DATA_HEADER_BYTES - this.#bufferedBytes,
          incoming.byteLength - offset);
        incoming.copy(this.#buffer, this.#bufferedBytes, offset, offset + count);
        this.#bufferedBytes += count;
        offset += count;
        if (this.#bufferedBytes < TERMINAL_DATA_HEADER_BYTES) break;
        this.#frameLength = this.#readFrameLength(this.#buffer);
        if (this.#frameLength > this.#buffer.byteLength) {
          const frame = Buffer.alloc(this.#frameLength);
          this.#buffer.copy(frame, 0, 0, this.#bufferedBytes);
          this.#buffer = frame;
        }
      }
      const count = Math.min(this.#frameLength - this.#bufferedBytes,
        incoming.byteLength - offset);
      incoming.copy(this.#buffer, this.#bufferedBytes, offset, offset + count);
      this.#bufferedBytes += count;
      offset += count;
      if (this.#bufferedBytes === this.#frameLength) {
        frames.push(this.#decodeFrame(this.#buffer.subarray(0, this.#frameLength)));
        this.#buffer = Buffer.alloc(TERMINAL_DATA_HEADER_BYTES);
        this.#bufferedBytes = 0;
        this.#frameLength = TERMINAL_DATA_HEADER_BYTES;
      }
    }
    return frames;
  }

  finish(): void {
    if (this.#bufferedBytes !== 0) {
      throw protocolError(
        `The terminal data pipe ended with ${this.#bufferedBytes.toString()} buffered bytes.`,
      );
    }
  }

  #decodeFrame(bytes: Buffer): TerminalDataFrame {
    const frame: TerminalDataFrame = {
      terminalId: terminalIdFromBytes(bytes.subarray(TERMINAL_ID_OFFSET, GENERATION_OFFSET)),
      generation: bytes.readBigUInt64LE(GENERATION_OFFSET),
      sequence: bytes.readBigUInt64LE(SEQUENCE_OFFSET),
      direction: bytes.readUInt8(DIRECTION_OFFSET) as TerminalDataDirection,
      kind: bytes.readUInt8(KIND_OFFSET) as TerminalDataKind,
      // Returned frames own their payload: later pushes cannot mutate it or
      // pin an entire coalesced input chunk through a small output view.
      payload: new Uint8Array(bytes.subarray(TERMINAL_DATA_HEADER_BYTES)),
    };
    validateFrame(frame, this.#maximumPayloadBytes);
    this.#acceptSequence(frame);
    return frame;
  }

  #readFrameLength(header: Buffer): number {
    if (!header.subarray(0, 4).equals(TERMINAL_DATA_MAGIC)) {
      throw protocolError("The terminal data frame magic did not match.");
    }
    if (header.readUInt8(VERSION_OFFSET) !== TERMINAL_DATA_VERSION) {
      throw protocolError("The terminal data protocol version is unsupported.");
    }
    if (header.readUInt8(DIRECTION_OFFSET) !== this.#expectedDirection) {
      throw protocolError("The terminal data direction did not match this endpoint.");
    }
    const kind = header.readUInt8(KIND_OFFSET);
    if (kind !== TerminalDataKind.Data && kind !== TerminalDataKind.Eof && kind !== TerminalDataKind.OutputAck) {
      throw protocolError("The terminal data frame kind is invalid.");
    }
    if (header.readUInt8(RESERVED_OFFSET) !== 0) {
      throw protocolError("The terminal data frame reserved bits were set.");
    }
    const payloadLength = header.readUInt32LE(PAYLOAD_LENGTH_OFFSET);
    if (payloadLength > this.#maximumPayloadBytes) {
      throw new TerminalDataTransportError(
        "terminal-data-too-large",
        `The terminal data payload exceeded ${this.#maximumPayloadBytes.toString()} bytes.`,
      );
    }
    return TERMINAL_DATA_HEADER_BYTES + payloadLength;
  }

  #acceptSequence(frame: TerminalDataFrame): void {
    if (frame.kind === TerminalDataKind.OutputAck) return;
    const key = streamKey(frame);
    if (this.#endedStreams.has(key)) {
      throw protocolError("Terminal data arrived after EOF.");
    }
    const expected = this.#nextSequences.get(key) ?? 0n;
    if (frame.sequence !== expected) {
      throw protocolError(
        `The terminal data sequence was ${frame.sequence.toString()}; expected ${expected.toString()}.`,
      );
    }
    if (frame.kind === TerminalDataKind.Eof) {
      this.#nextSequences.delete(key);
      this.#endedStreams.add(key);
    } else {
      this.#nextSequences.set(key, expected + 1n);
    }
  }
}

/** Amortized constant-time dequeue, releasing consumed payloads immediately. */
class FifoQueue<T> {
  #items: Array<T | undefined> = [];
  #head = 0;

  get length(): number { return this.#items.length - this.#head; }
  push(value: T): void { this.#items.push(value); }
  pop(): T | undefined {
    if (this.length === 0) return undefined;
    const value = this.#items[this.#head];
    this.#items[this.#head++] = undefined;
    if (this.#head === this.#items.length) this.clear();
    else if (this.#head >= 1024 && this.#head * 2 >= this.#items.length) {
      this.#items = this.#items.slice(this.#head);
      this.#head = 0;
    }
    return value;
  }
  clear(): void { this.#items = []; this.#head = 0; }
}

export class FairTerminalFrameQueue {
  readonly #limits: TerminalDataQueueLimits;
  readonly #queues = new Map<string, FifoQueue<TerminalDataFrame>>();
  readonly #terminalBytes = new Map<string, number>();
  readonly #active = new FifoQueue<string>();
  #totalBytes = 0;

  constructor(limits: TerminalDataQueueLimits) {
    this.#limits = limits;
  }

  get totalBytes(): number {
    return this.#totalBytes;
  }

  get empty(): boolean {
    return this.#active.length === 0;
  }

  /**
   * Queues one frame, or reports that it does not fit. A full queue is flow
   * control, never a protocol failure: refusing leaves the queue, this
   * terminal's stream, and every other terminal exactly as they were.
   */
  push(frame: TerminalDataFrame): boolean {
    const terminalId = normalizeTerminalId(frame.terminalId);
    const frameBytes = TERMINAL_DATA_HEADER_BYTES + frame.payload.byteLength;
    const currentTerminalBytes = this.#terminalBytes.get(terminalId) ?? 0;
    if (this.#totalBytes + frameBytes > this.#limits.totalBytes
        || currentTerminalBytes + frameBytes > this.#limits.perTerminalBytes) {
      return false;
    }
    const queue = this.#queues.get(terminalId) ?? new FifoQueue<TerminalDataFrame>();
    if (queue.length === 0) this.#active.push(terminalId);
    queue.push(frame);
    this.#queues.set(terminalId, queue);
    this.#terminalBytes.set(terminalId, currentTerminalBytes + frameBytes);
    this.#totalBytes += frameBytes;
    return true;
  }

  pop(): TerminalDataFrame | undefined {
    const terminalId = this.#active.pop();
    if (terminalId === undefined) return undefined;
    const queue = this.#queues.get(terminalId);
    if (queue === undefined || queue.length === 0) {
      throw new TerminalDataTransportError(
        "terminal-data-state",
        "The terminal data fairness queue was inconsistent.",
      );
    }
    const frame = queue.pop() as TerminalDataFrame;
    const frameBytes = TERMINAL_DATA_HEADER_BYTES + frame.payload.byteLength;
    this.#totalBytes -= frameBytes;
    const terminalBytes = (this.#terminalBytes.get(terminalId) ?? frameBytes) - frameBytes;
    if (queue.length === 0) {
      this.#queues.delete(terminalId);
      this.#terminalBytes.delete(terminalId);
    } else {
      this.#terminalBytes.set(terminalId, terminalBytes);
      this.#active.push(terminalId);
    }
    return frame;
  }

  clear(): void {
    this.#queues.clear();
    this.#terminalBytes.clear();
    this.#active.clear();
    this.#totalBytes = 0;
  }
}

export class TerminalDataTransport {
  readonly #acknowledgements = new Map<string, TerminalDataFrame>();
  #portAttached = false;
  readonly #hostOutput: Readable;
  readonly #hostInput: Writable;
  readonly #decoder: TerminalDataDecoder;
  readonly #queue: FairTerminalFrameQueue;
  readonly #events = new EventEmitter();
  readonly #nextSequences = new Map<string, bigint>();
  readonly #endedStreams = new BoundedEofStreams();
  // One maximum retired generation per terminal id. Lifecycle membership lets
  // deterministic eviction prefer already-dismissed ids if cross-pipe event
  // ordering briefly delivers their EOF after removal.
  readonly #retiredInputGenerations = new Map<string, bigint>();
  readonly #retainedTerminalIds = new Set<string>();
  readonly #maximumPayloadBytes: number;
  #droppedInputFrames = 0;
  #writing = false;
  /** Acks wait for the end of this event-loop turn, so every frame the
   *  renderer finished in one burst is acknowledged in one pipe write. */
  #ackFlush: ReturnType<typeof setImmediate> | undefined;
  #closed = false;
  #failure: TerminalDataTransportError | undefined;

  constructor(
    hostOutput: Readable,
    hostInput: Writable,
    options: TerminalDataTransportOptions = {},
  ) {
    this.#hostOutput = hostOutput;
    this.#hostInput = hostInput;
    this.#maximumPayloadBytes = options.maximumPayloadBytes
      ?? TERMINAL_DATA_MAX_PAYLOAD_BYTES;
    this.#decoder = new TerminalDataDecoder(
      TerminalDataDirection.HostToElectron,
      this.#maximumPayloadBytes,
    );
    this.#queue = new FairTerminalFrameQueue({
      totalBytes: options.queueLimits?.totalBytes ?? 8 * 1024 * 1024,
      perTerminalBytes: options.queueLimits?.perTerminalBytes ?? 1024 * 1024,
    });
    hostOutput.on("data", this.#onData);
    hostOutput.once("end", this.#onEnd);
    hostOutput.once("error", this.#onReadableError);
    hostInput.once("error", this.#onWritableError);
  }

  get retainedInputSequenceStreams(): number {
    return this.#nextSequences.size;
  }

  get retainedInputEofStreams(): number {
    return this.#endedStreams.size;
  }

  get retainedInputRetirementTerminals(): number {
    return this.#retiredInputGenerations.size;
  }

  /** Renderer-input frames a saturated per-terminal queue refused so far. */
  get droppedInputFrames(): number {
    return this.#droppedInputFrames;
  }

  retainTerminal(terminalId: string): void {
    this.#retainedTerminalIds.add(normalizeTerminalId(terminalId));
  }

  releaseTerminal(terminalId: string): void {
    const normalized = normalizeTerminalId(terminalId);
    this.#retainedTerminalIds.delete(normalized);
    this.#retiredInputGenerations.delete(normalized);
  }

  onFrame(listener: (frame: TerminalDataFrame) => void): () => void {
    this.#events.on("frame", listener);
    return () => { this.#events.off("frame", listener); };
  }

  onFailure(listener: (error: TerminalDataTransportError) => void): () => void {
    this.#events.on("failure", listener);
    if (this.#failure !== undefined) listener(this.#failure);
    return () => { this.#events.off("failure", listener); };
  }

  /**
   * Queues one renderer-input frame for the host pipe. Returns false when the
   * bounded queue refused it; a protocol violation still throws.
   */
  send(frame: TerminalDataFrame): boolean {
    if (this.#failure !== undefined) throw this.#failure;
    if (this.#closed) {
      throw new TerminalDataTransportError(
        "terminal-data-closed",
        "The terminal data transport is closed.",
      );
    }
    validateFrame(frame, this.#maximumPayloadBytes);
    if (frame.direction !== TerminalDataDirection.ElectronToHost) {
      throw protocolError("Electron attempted to send a host-output terminal frame.");
    }
    if (frame.kind === TerminalDataKind.OutputAck) {
      throw protocolError("Output acknowledgements must come from a delivered frame completion.");
    }
    const key = streamKey(frame);
    if (this.#endedStreams.has(key)) {
      throw protocolError("Terminal data was queued after EOF.");
    }
    const expected = this.#nextSequences.get(key) ?? 0n;
    if (frame.sequence !== expected) {
      throw protocolError(
        `The terminal data sequence was ${frame.sequence.toString()}; expected ${expected.toString()}.`,
      );
    }
    if (!this.#queue.push(frame)) {
      // One saturated terminal must never fence the pipes every other terminal
      // shares. The per-terminal ceiling already guarantees the others keep
      // their room, so the excess is discarded for this stream alone — what a
      // real tty does with input its process has stopped reading. The stream's
      // expected sequence deliberately does not advance, so the host still
      // sees a gapless stream and the next accepted frame takes this number.
      this.#droppedInputFrames += 1;
      return false;
    }
    if (frame.kind === TerminalDataKind.Eof) {
      this.#nextSequences.delete(key);
      this.#endedStreams.add(key);
    } else {
      this.#nextSequences.set(key, expected + 1n);
    }
    this.#scheduleWrite();
    return true;
  }

  attachPort(port: TerminalMessagePort): () => void {
    if (this.#failure !== undefined) throw this.#failure;
    if (this.#closed) {
      throw new TerminalDataTransportError(
        "terminal-data-closed",
        "The terminal data transport is closed.",
      );
    }
    if (this.#portAttached) throw protocolError("A terminal document already owns this transport.");
    this.#portAttached = true;
    let attached = true;
    const output = new TerminalOutputWindow();
    const removeFrame = this.onFrame((frame) => {
      output.sent(frame, frame.payload.byteLength);
      port.postMessage(frameToPortMessage(frame));
    });
    // Renderer-input counters are scoped to one authenticated document/port.
    // The host pipe is process-scoped and survives a renderer reload, so its
    // counters must continue independently across port attachments.
    const portNextSequences = new Map<string, bigint>();
    const portEndedStreams = new BoundedEofStreams();
    const listener = (event: { data: unknown }): void => {
      if (!attached) return;
      try {
        const portFrame = frameFromPortMessage(event.data);
        if (portFrame.kind === TerminalDataKind.OutputAck) {
          const sequence = output.complete(portFrame);
          if (sequence !== undefined) this.#acknowledge({ ...portFrame, sequence });
          return;
        }
        const key = streamKey(portFrame);
        const retiredGeneration = this.#retiredInputGenerations.get(portFrame.terminalId);
        if ((retiredGeneration !== undefined && portFrame.generation <= retiredGeneration)
            || this.#endedStreams.has(key)) {
          // A valid late frame for a sealed generation is a per-stream no-op,
          // not a shared transport protocol failure. Malformed envelopes still
          // fail closed above in frameFromPortMessage.
          return;
        }
        if (portEndedStreams.has(key)) {
          throw protocolError("Terminal data arrived after EOF on this renderer port.");
        }
        const portExpected = portNextSequences.get(key) ?? 0n;
        if (portFrame.sequence !== portExpected) {
          throw protocolError(
            `The renderer-port terminal data sequence was ${portFrame.sequence.toString()}; expected ${portExpected.toString()}.`,
          );
        }
        // Rewrite the per-document counter to the continuous host-pipe counter.
        // This preserves replay/order checks on both sides without requiring a
        // fresh preload to know how many frames an earlier document submitted.
        const hostSequence = this.#nextSequences.get(key) ?? 0n;
        this.send({ ...portFrame, sequence: hostSequence });
        if (portFrame.kind === TerminalDataKind.Eof) {
          portNextSequences.delete(key);
          portEndedStreams.add(key);
        } else {
          portNextSequences.set(key, portExpected + 1n);
        }
      } catch (error) {
        this.#fail(asTransportError(error, "terminal-data-port"));
      }
    };
    try {
      port.on("message", listener);
      port.start();
    } catch (error) {
      this.#fail(new TerminalDataTransportError("terminal-data-port", String(error)));
    }
    return () => {
      if (!attached) return;
      attached = false;
      removeFrame();
      port.off("message", listener);
      this.#portAttached = false;
      for (const frame of output.abandon()) this.#acknowledge(frame);
    };
  }

  close(): void {
    if (this.#closed) return;
    this.#terminatePipes();
  }

  readonly #onData = (chunk: Buffer): void => {
    if (this.#closed || this.#failure !== undefined) return;
    try {
      for (const frame of this.#decoder.push(chunk)) {
        if (frame.kind === TerminalDataKind.Eof) this.#retireInputGeneration(frame);
        this.#events.emit("frame", frame);
        // No document: bytes are intentionally discarded, not queued in main.
        if (!this.#portAttached) this.#acknowledge(frame);
      }
    } catch (error) {
      this.#fail(asTransportError(error, "terminal-data-read"));
    }
  };

  readonly #onEnd = (): void => {
    if (this.#closed || this.#failure !== undefined) return;
    try {
      this.#decoder.finish();
      this.#fail(new TerminalDataTransportError(
        "terminal-data-eof",
        "The Rust host terminal output pipe closed unexpectedly.",
      ));
    } catch (error) {
      this.#fail(asTransportError(error, "terminal-data-read"));
    }
  };

  readonly #onReadableError = (error: Error): void => {
    this.#fail(new TerminalDataTransportError("terminal-data-read", error.message));
  };

  readonly #onWritableError = (error: Error): void => {
    this.#fail(new TerminalDataTransportError("terminal-data-write", error.message));
  };

  #scheduleWrite(): void {
    if (this.#writing) return;
    this.#writing = true;
    queueMicrotask(() => { this.#writeNext(); });
  }

  #writeNext(): void {
    if (this.#closed || this.#failure !== undefined) {
      this.#writing = false;
      return;
    }
    try {
      let bytes: Uint8Array;
      if (this.#acknowledgements.size > 0) {
        // Every pending ack in one write: acks are tiny and never reordered
        // against each other's streams.
        const acks = [...this.#acknowledgements.values()]
          .map(frame => encodeTerminalDataFrame(frame, this.#maximumPayloadBytes));
        this.#acknowledgements.clear();
        bytes = acks.length === 1 ? acks[0] : Buffer.concat(acks);
      } else {
        const frame = this.#queue.pop();
        if (frame === undefined) {
          this.#writing = false;
          return;
        }
        bytes = encodeTerminalDataFrame(frame, this.#maximumPayloadBytes);
      }
      const ready = this.#hostInput.write(bytes);
      if (ready) {
        queueMicrotask(() => { this.#writeNext(); });
      } else {
        this.#hostInput.once("drain", this.#onDrain);
      }
    } catch (error) {
      this.#fail(new TerminalDataTransportError("terminal-data-write", String(error)));
    }
  }

  readonly #onDrain = (): void => { this.#writeNext(); };

  #acknowledge(frame: OutputIdentity): void {
    if (this.#closed || this.#failure !== undefined) return;
    const key = outputKey(frame);
    if (!this.#acknowledgements.has(key) && this.#acknowledgements.size >= 4096) {
      this.#fail(protocolError("Too many pending terminal acknowledgements."));
      return;
    }
    const previous = this.#acknowledgements.get(key);
    if (previous !== undefined && previous.sequence >= frame.sequence) return;
    this.#acknowledgements.set(key, { ...frame, direction: TerminalDataDirection.ElectronToHost,
      kind: TerminalDataKind.OutputAck, payload: new Uint8Array() });
    // Dedicated, bounded, coalescing lane: saturated PTY input cannot drop acks.
    // Keystrokes still flush on a microtask and carry any pending acks along.
    if (this.#writing || this.#ackFlush !== undefined) return;
    this.#ackFlush = setImmediate(() => {
      this.#ackFlush = undefined;
      this.#scheduleWrite();
    });
  }

  #retireInputGeneration(frame: TerminalDataFrame): void {
    const inputKey = streamKey({ ...frame, direction: TerminalDataDirection.ElectronToHost });
    this.#nextSequences.delete(inputKey);
    // Keep the ended mark once the host counter is gone: releaseTerminal drops
    // the retired generation, and a late renderer frame must stay a no-op
    // rather than reach the host at sequence 0 and fail every terminal's input.
    this.#endedStreams.add(inputKey);
    const terminalId = normalizeTerminalId(frame.terminalId);
    const previous = this.#retiredInputGenerations.get(terminalId);
    if (previous === undefined || frame.generation > previous) {
      this.#retiredInputGenerations.delete(terminalId);
      this.#retiredInputGenerations.set(terminalId, frame.generation);
    }
    this.#trimInputRetirements();
  }

  #trimInputRetirements(): void {
    while (this.#retiredInputGenerations.size > TERMINAL_DATA_MAX_RETIRED_TERMINALS) {
      let oldest = this.#retiredInputGenerations.keys().next().value as string | undefined;
      for (const terminalId of this.#retiredInputGenerations.keys()) {
        if (!this.#retainedTerminalIds.has(terminalId)) {
          oldest = terminalId;
          break;
        }
      }
      if (oldest === undefined) return;
      this.#retiredInputGenerations.delete(oldest);
    }
  }

  #terminatePipes(): void {
    this.#closed = true;
    this.#writing = false;
    if (this.#ackFlush !== undefined) clearImmediate(this.#ackFlush);
    this.#ackFlush = undefined;
    this.#hostOutput.off("data", this.#onData);
    this.#hostInput.off("drain", this.#onDrain);
    this.#queue.clear();
    this.#acknowledgements.clear();
    this.#nextSequences.clear();
    this.#endedStreams.clear();
    this.#retiredInputGenerations.clear();
    this.#retainedTerminalIds.clear();
    if (!this.#hostOutput.destroyed) this.#hostOutput.destroy();
    if (!this.#hostInput.destroyed) this.#hostInput.destroy();
  }

  #fail(error: TerminalDataTransportError): void {
    if (this.#failure !== undefined || this.#closed) return;
    this.#failure = error;
    this.#terminatePipes();
    this.#events.emit("failure", error);
  }
}

export function frameToPortMessage(frame: TerminalDataFrame): TerminalDataPortMessage {
  validateFrame(frame, TERMINAL_DATA_MAX_PAYLOAD_BYTES);
  const p = frame.payload;
  const payload = p.byteOffset === 0 && p.byteLength === p.buffer.byteLength
      && p.buffer instanceof ArrayBuffer
    ? p.buffer
    : new Uint8Array(p).buffer;
  return {
    channel: "interface-terminal-data",
    version: TERMINAL_DATA_VERSION,
    terminalId: normalizeTerminalId(frame.terminalId),
    generation: frame.generation.toString(),
    sequence: frame.sequence.toString(),
    direction: frame.direction,
    kind: frame.kind,
    payload,
  };
}

export function frameFromPortMessage(value: unknown): TerminalDataFrame {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw protocolError("The terminal MessagePort payload was invalid.");
  }
  const message = value as Partial<TerminalDataPortMessage>;
  if (message.channel !== "interface-terminal-data" || message.version !== TERMINAL_DATA_VERSION
      || typeof message.terminalId !== "string" || typeof message.generation !== "string"
      || typeof message.sequence !== "string" || !(message.payload instanceof ArrayBuffer)) {
    throw protocolError("The terminal MessagePort envelope was invalid.");
  }
  let generation: bigint;
  let sequence: bigint;
  try {
    if (!/^(0|[1-9][0-9]*)$/u.test(message.generation)
        || !/^(0|[1-9][0-9]*)$/u.test(message.sequence)) {
      throw new Error("non-canonical integer");
    }
    generation = BigInt(message.generation);
    sequence = BigInt(message.sequence);
  } catch {
    throw protocolError("The terminal MessagePort counters were invalid.");
  }
  const frame: TerminalDataFrame = {
    // Normalized here, not merely validated: every downstream guard keys a Map
    // or Set on this string, so an id that kept the caller's casing would open
    // a second, unsealed stream for the same terminal.
    terminalId: normalizeTerminalId(message.terminalId),
    generation,
    sequence,
    direction: message.direction as TerminalDataDirection,
    kind: message.kind as TerminalDataKind,
    payload: new Uint8Array(message.payload),
  };
  validateFrame(frame, TERMINAL_DATA_MAX_PAYLOAD_BYTES);
  return frame;
}
