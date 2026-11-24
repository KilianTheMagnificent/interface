import { readFileSync } from "node:fs";
import { describe, expect, it, vi } from "vitest";

const fixture = JSON.parse(readFileSync(new URL("../fixtures/opencode-go-usage-v1.json", import.meta.url), "utf8"));
const source = readFileSync(new URL("../../../crates/engine-acp/src/opencode_usage_transport.mjs", import.meta.url), "utf8");
const PREFIX = "INTERFACE_OPENCODE_GO_USAGE_V1 ";

async function run(fetch: typeof globalThis.fetch, auth: unknown = { type: "api", key: "synthetic-test-only" }) {
  const lines: string[] = [];
  const plugin = new Function("process", "fetch", source.replace("export default", "return"))(
    { stdout: { write: (line: string) => lines.push(line) } }, fetch);
  const hooks = await plugin();
  expect(hooks.auth.provider).toBe("opencode-go");
  expect(await hooks.auth.loader(async () => auth)).toEqual({});
  return lines.map(line => {
    expect(line.startsWith(PREFIX)).toBe(true);
    return JSON.parse(line.slice(PREFIX.length));
  });
}

describe("OpenCode Go private usage transport v1", () => {
  it("uses the provider callback for one fixed read-only request and emits only quota fields", async () => {
    const fetch = vi.fn<typeof globalThis.fetch>(async () => new Response(JSON.stringify({
      ...fixture, account: "must stay in the provider process",
    }), { status: 200 }));
    const frames = await run(fetch);
    expect(frames).toEqual([{ version: 1, type: "ready" },
      { version: 1, type: "response", status: 200, body: fixture }]);
    expect(fetch).toHaveBeenCalledExactlyOnceWith("https://opencode.ai/zen/go/v1/usage", {
      method: "GET", redirect: "error", signal: expect.any(AbortSignal),
      headers: { Authorization: "Bearer synthetic-test-only", Accept: "application/json",
        "User-Agent": "Interface/OpenCode-Go-Usage" },
    });
    expect(JSON.stringify(frames)).not.toContain("synthetic-test-only");
    expect(JSON.stringify(frames)).not.toContain("account");
  });

  it("reports missing credentials without a request, authentication mutation, or fabricated zero", async () => {
    const fetch = vi.fn<typeof globalThis.fetch>();
    expect(await run(fetch, null)).toEqual([{ version: 1, type: "ready" },
      { version: 1, type: "response", status: 401 }]);
    expect(fetch).not.toHaveBeenCalled();
  });

  it.each([401, 403, 429, 500])("forwards HTTP %i without copying its error body", async status => {
    const frames = await run(vi.fn<typeof globalThis.fetch>(async () =>
      new Response("private error metadata", { status })));
    expect(frames[1]).toEqual({ version: 1, type: "response", status });
  });

  it.each(["not json", " ".repeat(8193)])("bounds and rejects invalid successful response bodies", async body => {
    const frames = await run(vi.fn<typeof globalThis.fetch>(async () => new Response(body)));
    expect(frames[1]).toEqual({ version: 1, type: "response", status: 0 });
  });

  it("treats timeouts and rejected redirects as unavailable without exposing exception text", async () => {
    const frames = await run(vi.fn<typeof globalThis.fetch>(async () => { throw new Error("private failure"); }));
    expect(frames[1]).toEqual({ version: 1, type: "response", status: 0 });
  });
});
