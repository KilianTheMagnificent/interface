import { describe, expect, it, vi } from "vitest";
import { NdjsonLineBuffer } from "./ndjson-line-buffer.js";

describe("NdjsonLineBuffer", () => {
  it("preserves Unicode across every byte boundary and multiple lines per chunk", () => {
    const bytes = Buffer.from('😀界\nsecond\n\nlast\n');
    for (let split = 0; split <= bytes.length; split += 1) {
      const decoder = new NdjsonLineBuffer(1024);
      const lines: string[] = [];
      const receive = (line: string) => { lines.push(line); return true; };
      expect(decoder.push(bytes.subarray(0, split), receive)).toBe(true);
      expect(decoder.push(bytes.subarray(split), receive)).toBe(true);
      expect(lines).toEqual(['😀界', 'second', '', 'last']);
    }
  });

  it("enforces bytes per line and does not deliver incomplete or oversized data", () => {
    const decoder = new NdjsonLineBuffer(4);
    const receive = vi.fn(() => true);
    expect(decoder.push(Buffer.from('😀\n'), receive)).toBe(true);
    expect(receive).toHaveBeenCalledWith('😀');
    receive.mockClear();
    expect(decoder.push(Buffer.from('1234'), receive)).toBe(true);
    expect(receive).not.toHaveBeenCalled();
    expect(decoder.push(Buffer.from('5\nignored\n'), receive)).toBe(false);
    expect(receive).not.toHaveBeenCalled();
    decoder.clear();
    expect(decoder.push(Buffer.from('ok\n'), receive)).toBe(true);
    expect(receive).toHaveBeenCalledWith('ok');
  });

  it("stops processing after the receiver refuses a frame", () => {
    const decoder = new NdjsonLineBuffer(32);
    const receive = vi.fn(() => false);
    expect(decoder.push(Buffer.from('one\ntwo\n'), receive)).toBe(true);
    expect(receive.mock.calls).toEqual([['one']]);
  });

  it("frames a large response from small fragments without rescanning its prefix", () => {
    const decoder = new NdjsonLineBuffer(16 * 1024 * 1024);
    const bytes = Buffer.from('x'.repeat(8 * 1024 * 1024) + '\n');
    const receive = vi.fn(() => true);
    // Observe the bytes offered to the scan primitive instead of asserting a
    // machine-dependent deadline. Every original input byte is searched once.
    const original = Buffer.prototype.indexOf;
    let searched = 0;
    const spy = vi.spyOn(Buffer.prototype, 'indexOf').mockImplementation(function(this: Buffer, value, from = 0, encoding?) {
      searched += this.length - Number(from);
      return original.call(this, value, from as number, encoding);
    });
    try {
      for (let at = 0; at < bytes.length; at += 4096) {
        expect(decoder.push(bytes.subarray(at, at + 4096), receive)).toBe(true);
      }
    } finally { spy.mockRestore(); }
    expect(searched).toBe(bytes.length);
    expect(receive).toHaveBeenCalledExactlyOnceWith('x'.repeat(8 * 1024 * 1024));
  });
});
