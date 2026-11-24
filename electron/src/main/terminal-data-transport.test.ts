import { EventEmitter } from "node:events";
import { PassThrough, Writable } from "node:stream";
import { describe, expect, it } from "vitest";
import {
  encodeTerminalDataFrame,
  FairTerminalFrameQueue,
  frameFromPortMessage,
  frameToPortMessage,
  TERMINAL_DATA_HEADER_BYTES,
  TERMINAL_DATA_MAX_PAYLOAD_BYTES,
  TERMINAL_DATA_MAX_RETIRED_TERMINALS,
  TERMINAL_DATA_MAX_RETAINED_EOF_STREAMS,
  TerminalDataDecoder,
  TerminalDataDirection,
  TerminalDataKind,
  type TerminalDataFrame,
  type TerminalDataPortMessage,
  TerminalDataTransport,
  type TerminalDataTransportError,
  type TerminalMessagePort,
} from "./terminal-data-transport.js";

const A = "00000000-0000-4000-8000-000000000001";
const B = "00000000-0000-4000-8000-000000000002";

function frame(
  terminalId: string,
  sequence: bigint,
  direction = TerminalDataDirection.HostToElectron,
  text = "x",
): TerminalDataFrame {
  return {
    terminalId,
    generation: 1n,
    sequence,
    direction,
    kind: TerminalDataKind.Data,
    payload: new TextEncoder().encode(text),
  };
}

function decodeText(value: TerminalDataFrame): string {
  return new TextDecoder().decode(value.payload);
}

function numberedId(value: number): string {
  return `00000000-0000-4000-8000-${value.toString(16).padStart(12, "0")}`;
}

async function turn(): Promise<void> {
  await new Promise<void>(resolve => { setImmediate(resolve); });
}

class FakePort extends EventEmitter implements TerminalMessagePort {
  readonly posted: unknown[] = [];
  started = false;

  postMessage(message: unknown): void {
    this.posted.push(message);
  }

  start(): void {
    this.started = true;
  }
}

describe("terminal data codec", () => {
  it("releases only consumed output across EOF, document replacement, and late callbacks", async () => {
    const output = new PassThrough();
    const input = new PassThrough();
    const sent: TerminalDataFrame[] = [];
    const decoder = new TerminalDataDecoder(TerminalDataDirection.ElectronToHost);
    input.on("data", bytes => { sent.push(...decoder.push(bytes)); });
    const transport = new TerminalDataTransport(output, input);
    const failures: Error[] = [];
    transport.onFailure(error => failures.push(error));
    const first = new FakePort();
    const detach = transport.attachPort(first);
    const staleListener = first.listeners("message")[0];
    const ack = (sequence: bigint): TerminalDataFrame => ({
      ...frame(A, sequence, TerminalDataDirection.ElectronToHost),
      kind: TerminalDataKind.OutputAck, payload: new Uint8Array(),
    });
    expect(() => transport.send(ack(0n))).toThrow(/completion/u);
    output.write(encodeTerminalDataFrame(frame(A, 0n)));
    output.write(encodeTerminalDataFrame({ ...frame(A, 1n), kind: TerminalDataKind.Eof, payload: new Uint8Array() }));
    await turn();
    expect(sent).toEqual([]);
    first.emit("message", { data: frameToPortMessage(ack(1n)) });
    await turn();
    expect(sent).toEqual([]); // EOF cannot release the unfinished write before it.
    first.emit("message", { data: frameToPortMessage(ack(0n)) });
    await turn();
    expect(sent.map(value => value.sequence)).toEqual([1n]);
    output.write(encodeTerminalDataFrame(frame(B, 0n)));
    detach(); detach();
    await turn();
    expect(sent.map(value => [value.terminalId, value.sequence])).toEqual([[A, 1n], [B, 0n]]);
    const second = new FakePort();
    const detachSecond = transport.attachPort(second);
    output.write(encodeTerminalDataFrame(frame(B, 1n)));
    staleListener({ data: frameToPortMessage({ ...ack(1n), terminalId: B }) });
    await turn();
    expect(sent).toHaveLength(2);
    second.emit("message", { data: frameToPortMessage({ ...ack(1n), terminalId: B }) });
    await turn();
    expect(sent.at(-1)?.sequence).toBe(1n);
    expect(failures).toEqual([]);
    detachSecond(); transport.close();
  });

  it("acknowledges every frame finished in one turn with one pipe write", async () => {
    const output = new PassThrough();
    const input = new PassThrough();
    let writes = 0;
    const write = input.write.bind(input);
    input.write = ((chunk: Uint8Array) => { writes += 1; return write(chunk); }) as typeof input.write;
    const sent: TerminalDataFrame[] = [];
    const decoder = new TerminalDataDecoder(TerminalDataDirection.ElectronToHost);
    input.on("data", bytes => { sent.push(...decoder.push(bytes)); });
    const transport = new TerminalDataTransport(output, input);
    const port = new FakePort();
    transport.attachPort(port);
    const ack = (terminalId: string, sequence: bigint): TerminalDataFrame => ({
      ...frame(terminalId, sequence, TerminalDataDirection.ElectronToHost),
      kind: TerminalDataKind.OutputAck, payload: new Uint8Array(),
    });
    for (const id of [A, B]) {
      for (const sequence of [0n, 1n, 2n]) output.write(encodeTerminalDataFrame(frame(id, sequence)));
    }
    await turn();
    for (const id of [A, B]) {
      for (const sequence of [0n, 1n, 2n]) port.emit("message", { data: frameToPortMessage(ack(id, sequence)) });
    }
    await turn();
    expect(writes).toBe(1);
    expect(sent.map(value => [value.terminalId, value.sequence])).toEqual([[A, 2n], [B, 2n]]);
    transport.close();
  });

  it("decodes fragmented and coalesced frames in sequence", () => {
    const first = frame(A, 0n, TerminalDataDirection.HostToElectron, "one");
    const second = frame(A, 1n, TerminalDataDirection.HostToElectron, "two");
    const bytes = Buffer.concat([
      encodeTerminalDataFrame(first),
      encodeTerminalDataFrame(second),
    ]);
    const decoder = new TerminalDataDecoder(TerminalDataDirection.HostToElectron);
    const decoded: TerminalDataFrame[] = [];
    for (let offset = 0; offset < bytes.length; offset += 7) {
      decoded.push(...decoder.push(bytes.subarray(offset, offset + 7)));
    }
    decoder.finish();
    expect(decoded.map(decodeText)).toEqual(["one", "two"]);
    expect(decoded.map(value => value.sequence)).toEqual([0n, 1n]);
  });

  it("handles maximum payloads at every fragmentation scale without sharing output storage", () => {
    const payload = Uint8Array.from({ length: TERMINAL_DATA_MAX_PAYLOAD_BYTES }, (_, i) => i % 251);
    const data = { ...frame(A, 0n), payload };
    const eof = { ...frame(A, 1n), kind: TerminalDataKind.Eof, payload: new Uint8Array() };
    for (const chunkSize of [1, 7, 44, 1024, 128 * 1024]) {
      const bytes = Buffer.concat([encodeTerminalDataFrame(data), encodeTerminalDataFrame(eof)]);
      const decoder = new TerminalDataDecoder(TerminalDataDirection.HostToElectron);
      const decoded: TerminalDataFrame[] = [];
      for (let start = 0; start < bytes.length; start += chunkSize) {
        decoded.push(...decoder.push(bytes.subarray(start, start + chunkSize)));
        expect(decoder.push(new Uint8Array())).toEqual([]);
      }
      decoder.finish();
      bytes.fill(0);
      expect(decoded.map(item => [item.sequence, item.kind])).toEqual([[0n, TerminalDataKind.Data], [1n, TerminalDataKind.Eof]]);
      expect(Buffer.from(decoded[0].payload).equals(Buffer.from(payload))).toBe(true);
      expect(decoded[1].payload.byteLength).toBe(0);
    }
  });

  it("rejects a fragmented oversized header before accepting its payload", () => {
    const bytes = encodeTerminalDataFrame(frame(A, 0n));
    bytes.writeUInt32LE(TERMINAL_DATA_MAX_PAYLOAD_BYTES + 1, 40);
    const decoder = new TerminalDataDecoder(TerminalDataDirection.HostToElectron);
    expect(decoder.push(bytes.subarray(0, 43))).toEqual([]);
    expect(() => decoder.push(bytes.subarray(43, 44))).toThrow(/exceeded/u);
  });

  it("preserves fairness and FIFO order while a deep queue drains and refills", () => {
    const queue = new FairTerminalFrameQueue({ totalBytes: 4 * 1024 * 1024, perTerminalBytes: 2 * 1024 * 1024 });
    for (let i = 0; i < 30_000; i++) expect(queue.push(frame(A, BigInt(i)))).toBe(true);
    for (let i = 0; i < 20_000; i++) {
      expect(queue.push(frame(B, BigInt(i)))).toBe(true);
      expect(queue.pop()?.sequence).toBe(BigInt(i));
      expect(queue.pop()?.terminalId).toBe(B);
    }
    for (let i = 20_000; i < 30_000; i++) expect(queue.pop()?.sequence).toBe(BigInt(i));
    expect(queue.empty).toBe(true);
    expect(queue.totalBytes).toBe(0);
    expect(queue.pop()).toBeUndefined();
    expect(queue.push(frame(B, 30_000n))).toBe(true);
    queue.clear();
    expect(queue.empty).toBe(true);
    expect(queue.totalBytes).toBe(0);
  });

  it("rejects invalid, oversized, wrong-direction, and out-of-order frames", () => {
    const encoded = encodeTerminalDataFrame(frame(A, 0n));
    encoded[0] = 0;
    expect(() => new TerminalDataDecoder(TerminalDataDirection.HostToElectron).push(encoded))
      .toThrow(/magic/u);

    const oversized = encodeTerminalDataFrame(frame(A, 0n));
    oversized.writeUInt32LE(TERMINAL_DATA_MAX_PAYLOAD_BYTES + 1, 40);
    expect(() => new TerminalDataDecoder(TerminalDataDirection.HostToElectron).push(oversized))
      .toThrow(/exceeded/u);

    expect(() => new TerminalDataDecoder(TerminalDataDirection.ElectronToHost)
      .push(encodeTerminalDataFrame(frame(A, 0n))))
      .toThrow(/direction/u);
    expect(() => new TerminalDataDecoder(TerminalDataDirection.HostToElectron)
      .push(encodeTerminalDataFrame(frame(A, 1n))))
      .toThrow(/expected 0/u);
  });

  it("enforces terminal EOF and reports truncated pipe EOF", () => {
    const eof: TerminalDataFrame = {
      ...frame(A, 0n),
      kind: TerminalDataKind.Eof,
      payload: new Uint8Array(),
    };
    const decoder = new TerminalDataDecoder(TerminalDataDirection.HostToElectron);
    expect(decoder.push(encodeTerminalDataFrame(eof))).toHaveLength(1);
    expect(() => decoder.push(encodeTerminalDataFrame(frame(A, 1n))))
      .toThrow(/after EOF/u);

    const partial = new TerminalDataDecoder(TerminalDataDirection.HostToElectron);
    partial.push(encodeTerminalDataFrame(frame(B, 0n)).subarray(0, 9));
    expect(() => partial.finish()).toThrow(/9 buffered bytes/u);
  });

  it("prunes completed sequence state and bounds retained EOF tombstones", () => {
    const decoder = new TerminalDataDecoder(TerminalDataDirection.HostToElectron);
    for (let index = 0; index < TERMINAL_DATA_MAX_RETAINED_EOF_STREAMS + 20; index += 1) {
      const eof: TerminalDataFrame = {
        ...frame(numberedId(index + 1), 0n),
        kind: TerminalDataKind.Eof,
        payload: new Uint8Array(),
      };
      decoder.push(encodeTerminalDataFrame(eof));
    }
    expect(decoder.retainedSequenceStreams).toBe(0);
    expect(decoder.retainedEofStreams).toBe(TERMINAL_DATA_MAX_RETAINED_EOF_STREAMS);
  });
});

describe("terminal data fairness and stream bridge", () => {
  it("round-robins terminals and refuses per-terminal queue overflow", () => {
    const bytes = TERMINAL_DATA_HEADER_BYTES + 1;
    const queue = new FairTerminalFrameQueue({
      totalBytes: bytes * 4,
      perTerminalBytes: bytes * 2,
    });
    expect(queue.push(frame(A, 0n, TerminalDataDirection.ElectronToHost, "a"))).toBe(true);
    expect(queue.push(frame(A, 1n, TerminalDataDirection.ElectronToHost, "b"))).toBe(true);
    expect(queue.push(frame(A, 2n, TerminalDataDirection.ElectronToHost, "c"))).toBe(false);
    expect(queue.totalBytes).toBe(bytes * 2);
    expect(queue.push(frame(B, 0n, TerminalDataDirection.ElectronToHost, "d"))).toBe(true);
    expect([queue.pop(), queue.pop(), queue.pop()].map(value => value?.terminalId))
      .toEqual([A, B, A]);
    expect(queue.empty).toBe(true);
    expect(queue.totalBytes).toBe(0);
  });

  it("forwards frames fairly and fences both pipes when host output ends", async () => {
    const hostOutput = new PassThrough();
    const hostInput = new PassThrough();
    const written: Buffer[] = [];
    hostInput.on("data", (chunk: Buffer) => { written.push(Buffer.from(chunk)); });
    const transport = new TerminalDataTransport(hostOutput, hostInput);
    const received: TerminalDataFrame[] = [];
    const failures: TerminalDataTransportError[] = [];
    transport.onFrame(value => { received.push(value); });
    transport.onFailure(error => { failures.push(error); });

    const hostA = frame(A, 0n, TerminalDataDirection.HostToElectron, "host");
    const encoded = encodeTerminalDataFrame(hostA);
    hostOutput.write(encoded.subarray(0, 5));
    hostOutput.write(encoded.subarray(5));
    await turn();
    expect(received.map(decodeText)).toEqual(["host"]);

    transport.send(frame(A, 0n, TerminalDataDirection.ElectronToHost, "a"));
    transport.send(frame(A, 1n, TerminalDataDirection.ElectronToHost, "b"));
    transport.send(frame(B, 0n, TerminalDataDirection.ElectronToHost, "c"));
    await turn();
    const decoder = new TerminalDataDecoder(TerminalDataDirection.ElectronToHost);
    const output = decoder.push(Buffer.concat(written));
    expect(output.filter(value => value.kind === TerminalDataKind.OutputAck)).toHaveLength(1);
    expect(output.filter(value => value.kind === TerminalDataKind.Data).map(value => `${value.terminalId}:${decodeText(value)}`))
      .toEqual([`${A}:a`, `${B}:c`, `${A}:b`]);

    hostOutput.end();
    await turn();
    expect(failures[0]?.code).toBe("terminal-data-eof");
    expect(failures[0]?.message).toMatch(/closed unexpectedly/u);
    expect(hostOutput.destroyed).toBe(true);
    expect(hostInput.destroyed).toBe(true);
  });

  it("retains a decoder failure for late observers and destroys both pipes", async () => {
    const hostOutput = new PassThrough();
    const hostInput = new PassThrough();
    const transport = new TerminalDataTransport(hostOutput, hostInput);
    const failures: Error[] = [];
    hostOutput.end(Buffer.from("ITRM"));
    await turn();
    transport.onFailure(error => { failures.push(error); });
    expect(failures).toHaveLength(1);
    expect(failures[0]?.message).toMatch(/4 buffered bytes/u);
    expect(hostOutput.destroyed).toBe(true);
    expect(hostInput.destroyed).toBe(true);
  });

  it("destroys both pipes and retains asynchronous write failures", async () => {
    const hostOutput = new PassThrough();
    const hostInput = new Writable({
      write(_chunk, _encoding, callback): void { callback(new Error("write exploded")); },
    });
    const transport = new TerminalDataTransport(hostOutput, hostInput);
    transport.send(frame(A, 0n, TerminalDataDirection.ElectronToHost, "input"));
    await turn();
    const failures: Error[] = [];
    transport.onFailure(error => { failures.push(error); });
    expect(failures[0]?.message).toContain("write exploded");
    expect(hostOutput.destroyed).toBe(true);
    expect(hostInput.destroyed).toBe(true);
  });
});

describe("terminal MessagePort seam", () => {
  it("forwards isolated ArrayBuffers in both directions", async () => {
    const hostOutput = new PassThrough();
    const hostInput = new PassThrough();
    const written: Buffer[] = [];
    hostInput.on("data", (chunk: Buffer) => { written.push(Buffer.from(chunk)); });
    const transport = new TerminalDataTransport(hostOutput, hostInput);
    const port = new FakePort();
    const detach = transport.attachPort(port);
    expect(port.started).toBe(true);

    hostOutput.write(encodeTerminalDataFrame(
      frame(A, 0n, TerminalDataDirection.HostToElectron, "out"),
    ));
    await turn();
    const posted = port.posted[0] as TerminalDataPortMessage;
    expect(posted.payload).toBeInstanceOf(ArrayBuffer);
    expect(new TextDecoder().decode(posted.payload)).toBe("out");

    const rendererFrame = frame(A, 0n, TerminalDataDirection.ElectronToHost, "in");
    port.emit("message", { data: frameToPortMessage(rendererFrame) });
    await turn();
    const decoder = new TerminalDataDecoder(TerminalDataDirection.ElectronToHost);
    expect(decoder.push(Buffer.concat(written)).map(decodeText)).toEqual(["in"]);

    const roundTrip = frameFromPortMessage(frameToPortMessage(rendererFrame));
    expect(roundTrip.payload).not.toBe(rendererFrame.payload);
    expect(decodeText(roundTrip)).toBe("in");
    const backing = new Uint8Array([0x41, 0x42, 0x43, 0x44]);
    const viewFrame = {
      ...rendererFrame,
      payload: backing.subarray(1, 3),
    };
    const fromView = frameFromPortMessage(frameToPortMessage(viewFrame));
    expect(Buffer.from(fromView.payload)).toEqual(Buffer.from([0x42, 0x43]));
    expect(fromView.payload.byteLength).toBe(2);
    expect(fromView.payload.buffer.byteLength).toBe(2);
    // The untrusted side may spell a valid UUID in any case; every downstream
    // guard keys on this string, so the decoded frame must carry one form.
    const lettered = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
    const envelope = frameToPortMessage(rendererFrame);
    expect(frameFromPortMessage({ ...envelope, terminalId: lettered.toUpperCase() }).terminalId)
      .toBe(lettered);
    detach();
    transport.close();
  });

  it("round-trips six independent terminal streams through one private port", async () => {
    const hostOutput = new PassThrough();
    const hostInput = new PassThrough();
    const written: Buffer[] = [];
    hostInput.on("data", (chunk: Buffer) => { written.push(Buffer.from(chunk)); });
    const transport = new TerminalDataTransport(hostOutput, hostInput);
    const port = new FakePort();
    const detach = transport.attachPort(port);
    const ids = Array.from({ length: 6 }, (_, index) => numberedId(index + 1));

    hostOutput.write(Buffer.concat(ids.map((id, index) => encodeTerminalDataFrame(
      frame(id, 0n, TerminalDataDirection.HostToElectron, `out-${index + 1}`),
    ))));
    await turn();
    expect(port.posted.map(value => {
      const message = value as TerminalDataPortMessage;
      return `${message.terminalId}:${new TextDecoder().decode(message.payload)}`;
    })).toEqual(ids.map((id, index) => `${id}:out-${index + 1}`));

    for (const [index, id] of ids.entries()) {
      port.emit("message", { data: frameToPortMessage(
        frame(id, 0n, TerminalDataDirection.ElectronToHost, `in-${index + 1}`),
      ) });
    }
    await turn();
    const decoder = new TerminalDataDecoder(TerminalDataDirection.ElectronToHost);
    expect(decoder.push(Buffer.concat(written)).map(value => (
      `${value.terminalId}:${decodeText(value)}`
    ))).toEqual(ids.map((id, index) => `${id}:in-${index + 1}`));
    detach();
    transport.close();
  });

  it("continues six host-pipe streams after a fresh renderer port starts at zero", async () => {
    const hostOutput = new PassThrough();
    const hostInput = new PassThrough();
    const written: Buffer[] = [];
    hostInput.on("data", (chunk: Buffer) => { written.push(Buffer.from(chunk)); });
    const failures: Error[] = [];
    const transport = new TerminalDataTransport(hostOutput, hostInput);
    transport.onFailure(error => { failures.push(error); });
    const ids = Array.from({ length: 6 }, (_, index) => numberedId(index + 1));

    const beforeReload = new FakePort();
    const detachBeforeReload = transport.attachPort(beforeReload);
    for (const [index, id] of ids.entries()) {
      beforeReload.emit("message", { data: frameToPortMessage(
        frame(id, 0n, TerminalDataDirection.ElectronToHost, `before-${index + 1}`),
      ) });
    }
    await turn();
    detachBeforeReload();

    // A new preload owns a new authenticated port and therefore starts each
    // local stream at zero even though the Rust pipe is still at sequence one.
    const afterReload = new FakePort();
    const detachAfterReload = transport.attachPort(afterReload);
    for (const [index, id] of ids.entries()) {
      afterReload.emit("message", { data: frameToPortMessage(
        frame(id, 0n, TerminalDataDirection.ElectronToHost, `after-${index + 1}`),
      ) });
    }
    await turn();

    const decoder = new TerminalDataDecoder(TerminalDataDirection.ElectronToHost);
    const byTerminal = new Map<string, TerminalDataFrame[]>();
    for (const value of decoder.push(Buffer.concat(written))) {
      const frames = byTerminal.get(value.terminalId) ?? [];
      frames.push(value);
      byTerminal.set(value.terminalId, frames);
    }
    for (const [index, id] of ids.entries()) {
      const frames = byTerminal.get(id) ?? [];
      expect(frames.map(value => value.sequence)).toEqual([0n, 1n]);
      expect(frames.map(decodeText)).toEqual([`before-${index + 1}`, `after-${index + 1}`]);
    }
    expect(failures).toEqual([]);
    expect(transport.retainedInputSequenceStreams).toBe(6);
    expect(hostOutput.destroyed).toBe(false);
    expect(hostInput.destroyed).toBe(false);
    detachAfterReload();
    transport.close();
  });

  it("treats malformed renderer port traffic as terminal transport failure", () => {
    const hostOutput = new PassThrough();
    const hostInput = new PassThrough();
    const transport = new TerminalDataTransport(hostOutput, hostInput);
    const port = new FakePort();
    transport.attachPort(port);
    port.emit("message", { data: { invalid: true } });
    const failures: Error[] = [];
    transport.onFailure(error => { failures.push(error); });
    expect(failures[0]?.message).toMatch(/MessagePort envelope was invalid/u);
    expect(hostOutput.destroyed).toBe(true);
    expect(hostInput.destroyed).toBe(true);
  });

  it("contains a saturated renderer stream instead of fencing every terminal", async () => {
    const hostOutput = new PassThrough();
    const hostInput = new PassThrough();
    const written: Buffer[] = [];
    hostInput.on("data", (chunk: Buffer) => { written.push(Buffer.from(chunk)); });
    const bytes = TERMINAL_DATA_HEADER_BYTES + 1;
    const transport = new TerminalDataTransport(hostOutput, hostInput, {
      queueLimits: { totalBytes: bytes * 8, perTerminalBytes: bytes * 2 },
    });
    const failures: Error[] = [];
    transport.onFailure(error => { failures.push(error); });
    const port = new FakePort();
    const detach = transport.attachPort(port);

    // Every emit lands before the queue's first microtask drain, so the third
    // frame really does meet a full per-terminal ceiling.
    for (const [index, text] of ["a", "b", "over"].entries()) {
      port.emit("message", { data: frameToPortMessage(
        frame(A, BigInt(index), TerminalDataDirection.ElectronToHost, text),
      ) });
    }
    port.emit("message", { data: frameToPortMessage(
      frame(B, 0n, TerminalDataDirection.ElectronToHost, "b-unaffected"),
    ) });
    await turn();
    expect(transport.droppedInputFrames).toBe(1);
    expect(failures).toEqual([]);
    expect(hostOutput.destroyed).toBe(false);
    expect(hostInput.destroyed).toBe(false);

    // The saturated terminal keeps its live stream: the renderer's next frame
    // takes the sequence the refused one would have used, so the host pipe
    // never sees a gap and no terminal was torn down.
    port.emit("message", { data: frameToPortMessage(
      frame(A, 3n, TerminalDataDirection.ElectronToHost, "a-after-drain"),
    ) });
    await turn();
    const decoder = new TerminalDataDecoder(TerminalDataDirection.ElectronToHost);
    expect(decoder.push(Buffer.concat(written)).map(value => (
      `${value.terminalId}:${value.sequence.toString()}:${decodeText(value)}`
    ))).toEqual([`${A}:0:a`, `${B}:0:b-unaffected`, `${A}:1:b`, `${A}:2:a-after-drain`]);
    expect(failures).toEqual([]);
    detach();
    transport.close();
  });

  it("allows an ordinary renderer port detach without fencing the pipes", () => {
    const hostOutput = new PassThrough();
    const hostInput = new PassThrough();
    const transport = new TerminalDataTransport(hostOutput, hostInput);
    const port = new FakePort();
    const detach = transport.attachPort(port);
    detach();
    port.emit("close");
    expect(hostOutput.destroyed).toBe(false);
    expect(hostInput.destroyed).toBe(false);
    transport.close();
  });

  it("refuses renderer attachment after transport close or failure", async () => {
    const closed = new TerminalDataTransport(new PassThrough(), new PassThrough());
    closed.close();
    const closedPort = new FakePort();
    expect(() => closed.attachPort(closedPort)).toThrow(/transport is closed/u);
    expect(closedPort.started).toBe(false);

    const failedOutput = new PassThrough();
    const failed = new TerminalDataTransport(failedOutput, new PassThrough());
    failedOutput.end(Buffer.from("ITRM"));
    await turn();
    const failedPort = new FakePort();
    expect(() => failed.attachPort(failedPort)).toThrow(/4 buffered bytes/u);
    expect(failedPort.started).toBe(false);
  });

  it("seals one input stream at host EOF across reattach without harming another", async () => {
    const hostOutput = new PassThrough();
    const hostInput = new PassThrough();
    const written: Buffer[] = [];
    hostInput.on("data", (chunk: Buffer) => { written.push(Buffer.from(chunk)); });
    const transport = new TerminalDataTransport(hostOutput, hostInput);
    const failures: Error[] = [];
    transport.onFailure(error => { failures.push(error); });
    const beforeReload = new FakePort();
    const detachBeforeReload = transport.attachPort(beforeReload);
    beforeReload.emit("message", { data: frameToPortMessage(
      frame(A, 0n, TerminalDataDirection.ElectronToHost, "a-before-eof"),
    ) });
    beforeReload.emit("message", { data: frameToPortMessage(
      frame(B, 0n, TerminalDataDirection.ElectronToHost, "b-before-eof"),
    ) });
    await turn();
    expect(transport.retainedInputSequenceStreams).toBe(2);

    hostOutput.write(encodeTerminalDataFrame({
      ...frame(A, 0n, TerminalDataDirection.HostToElectron),
      kind: TerminalDataKind.Eof,
      payload: new Uint8Array(),
    }));
    await turn();
    // Preload clears its counter on output EOF, so this accurately models the
    // exited pane attempting local sequence zero before a renderer reload.
    beforeReload.emit("message", { data: frameToPortMessage(
      frame(A, 0n, TerminalDataDirection.ElectronToHost, "a-rejected-before-reload"),
    ) });
    beforeReload.emit("message", { data: frameToPortMessage(
      frame(B, 1n, TerminalDataDirection.ElectronToHost, "b-still-live"),
    ) });
    await turn();
    detachBeforeReload();

    const afterReload = new FakePort();
    const detachAfterReload = transport.attachPort(afterReload);
    afterReload.emit("message", { data: frameToPortMessage(
      frame(A, 0n, TerminalDataDirection.ElectronToHost, "a-rejected-after-reload"),
    ) });
    afterReload.emit("message", { data: frameToPortMessage(
      frame(B, 0n, TerminalDataDirection.ElectronToHost, "b-after-reload"),
    ) });
    await turn();

    const decoder = new TerminalDataDecoder(TerminalDataDirection.ElectronToHost);
    const decoded = decoder.push(Buffer.concat(written)).filter(value => value.kind === TerminalDataKind.Data);
    expect(decoded.filter(value => value.terminalId === A).map(decodeText))
      .toEqual(["a-before-eof"]);
    expect(decoded.filter(value => value.terminalId === B).map(value => (
      `${value.sequence.toString()}:${decodeText(value)}`
    ))).toEqual(["0:b-before-eof", "1:b-still-live", "2:b-after-reload"]);
    expect(failures).toEqual([]);
    expect(transport.retainedInputSequenceStreams).toBe(1);
    expect(transport.retainedInputEofStreams).toBe(1);
    expect(transport.retainedInputRetirementTerminals).toBe(1);
    expect(hostOutput.destroyed).toBe(false);
    expect(hostInput.destroyed).toBe(false);
    detachAfterReload();
    transport.close();
  });

  it("keeps a late input frame a no-op after host EOF and the pane's release", async () => {
    const hostOutput = new PassThrough();
    const hostInput = new PassThrough();
    const written: Buffer[] = [];
    hostInput.on("data", (chunk: Buffer) => { written.push(Buffer.from(chunk)); });
    const transport = new TerminalDataTransport(hostOutput, hostInput);
    const failures: Error[] = [];
    transport.onFailure(error => { failures.push(error); });
    transport.retainTerminal(A);
    const port = new FakePort();
    transport.attachPort(port);
    port.emit("message", { data: frameToPortMessage(
      frame(A, 0n, TerminalDataDirection.ElectronToHost, "typed"),
    ) });
    await turn();
    hostOutput.write(encodeTerminalDataFrame({
      ...frame(A, 0n, TerminalDataDirection.HostToElectron),
      kind: TerminalDataKind.Eof,
      payload: new Uint8Array(),
    }));
    await turn();
    // The pane is removed before the renderer's last keystroke lands.
    transport.releaseTerminal(A);
    port.emit("message", { data: frameToPortMessage(
      frame(A, 1n, TerminalDataDirection.ElectronToHost, "late"),
    ) });
    port.emit("message", { data: frameToPortMessage(
      frame(B, 0n, TerminalDataDirection.ElectronToHost, "other pane"),
    ) });
    await turn();

    const decoder = new TerminalDataDecoder(TerminalDataDirection.ElectronToHost);
    const decoded = decoder.push(Buffer.concat(written)).filter(value => value.kind === TerminalDataKind.Data);
    expect(decoded.map(decodeText)).toEqual(["typed", "other pane"]);
    expect(failures).toEqual([]);
    expect(hostInput.destroyed).toBe(false);
    transport.close();
  });

  it("bounds retirement memory while protecting every retained terminal id", async () => {
    const hostOutput = new PassThrough();
    const hostInput = new PassThrough();
    const written: Buffer[] = [];
    hostInput.on("data", (chunk: Buffer) => { written.push(Buffer.from(chunk)); });
    const transport = new TerminalDataTransport(hostOutput, hostInput);
    const failures: Error[] = [];
    transport.onFailure(error => { failures.push(error); });
    const retained = Array.from(
      { length: TERMINAL_DATA_MAX_RETIRED_TERMINALS },
      (_, index) => numberedId(index + 1),
    );
    for (const id of retained) transport.retainTerminal(id);
    // Leave the last retained terminal active and retire every other visible id.
    for (const id of retained.slice(0, -1)) {
      hostOutput.write(encodeTerminalDataFrame({
        ...frame(id, 0n, TerminalDataDirection.HostToElectron),
        kind: TerminalDataKind.Eof,
        payload: new Uint8Array(),
      }));
    }

    // Model EOF arriving after lifecycle removal for many dismissed ids. The
    // oldest dismissed tombstone is evicted deterministically; retained ids are
    // never selected while the 32-terminal contract holds.
    for (let index = 0; index < TERMINAL_DATA_MAX_RETIRED_TERMINALS * 3; index += 1) {
      const id = numberedId(1_000 + index);
      transport.retainTerminal(id);
      transport.releaseTerminal(id);
      hostOutput.write(encodeTerminalDataFrame({
        ...frame(id, 0n, TerminalDataDirection.HostToElectron),
        kind: TerminalDataKind.Eof,
        payload: new Uint8Array(),
      }));
    }
    await turn();
    expect(transport.retainedInputRetirementTerminals)
      .toBe(TERMINAL_DATA_MAX_RETIRED_TERMINALS);

    const port = new FakePort();
    const detach = transport.attachPort(port);
    for (const id of retained.slice(0, -1)) {
      port.emit("message", { data: frameToPortMessage(
        frame(id, 0n, TerminalDataDirection.ElectronToHost, "must-drop"),
      ) });
    }
    const activeId = retained.at(-1) as string;
    port.emit("message", { data: frameToPortMessage(
      frame(activeId, 0n, TerminalDataDirection.ElectronToHost, "still-live"),
    ) });
    await turn();
    const decoder = new TerminalDataDecoder(TerminalDataDirection.ElectronToHost);
    expect(decoder.push(Buffer.concat(written)).filter(value => value.kind === TerminalDataKind.Data).map(value => (
      `${value.terminalId}:${decodeText(value)}`
    ))).toEqual([`${activeId}:still-live`]);
    expect(failures).toEqual([]);
    expect(hostOutput.destroyed).toBe(false);
    expect(hostInput.destroyed).toBe(false);

    transport.releaseTerminal(retained[0] as string);
    expect(transport.retainedInputRetirementTerminals)
      .toBe(TERMINAL_DATA_MAX_RETIRED_TERMINALS - 1);
    detach();
    transport.close();
  });

  it("refreshes an already-ended stream key without evicting it prematurely when the window rolls", async () => {
    const hostOutput = new PassThrough();
    const hostInput = new PassThrough();
    const transport = new TerminalDataTransport(hostOutput, hostInput);
    // Input EOF adds `${A}:1:2` once; host output EOF retires generation 1 and
    // adds `${A}:1:2` a second time.
    transport.send({
      ...frame(A, 0n, TerminalDataDirection.ElectronToHost),
      kind: TerminalDataKind.Eof,
      payload: new Uint8Array(),
    });
    hostOutput.write(encodeTerminalDataFrame({
      ...frame(A, 0n, TerminalDataDirection.HostToElectron),
      kind: TerminalDataKind.Eof,
      payload: new Uint8Array(),
    }));
    await turn();
    for (let index = 0; index < TERMINAL_DATA_MAX_RETAINED_EOF_STREAMS - 1; index += 1) {
      transport.send({
        ...frame(numberedId(2_000 + index), 0n, TerminalDataDirection.ElectronToHost),
        kind: TerminalDataKind.Eof,
        payload: new Uint8Array(),
      });
    }
    expect(transport.retainedInputEofStreams).toBe(TERMINAL_DATA_MAX_RETAINED_EOF_STREAMS);
    transport.close();
  });
});
