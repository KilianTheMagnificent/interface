import { readFileSync, writeFileSync, renameSync, unlinkSync, mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { afterEach, describe, expect, it, vi } from "vitest";

function pluginFor(env: Record<string, string> = {}) {
  const source = readFileSync(new URL("../../../crates/interface-terminal/src/opencode_activity_transport.mjs", import.meta.url), "utf8");
  return new Function("writeFileSync", "renameSync", "unlinkSync", "join", "process",
    source.replace(/^import .*;\n/gm, "").replace("export default", "return").replace(/^export /gm, ""))(
    writeFileSync, renameSync, unlinkSync, join, { env, pid: 42 }) as { id: string; tui: (api: unknown) => Promise<void> };
}

describe("OpenCode activity TUI transport", () => {
  afterEach(() => vi.useRealTimers());
  it("copies provider state, refreshes child membership, and stops on disposal", async () => {
    vi.useFakeTimers();
    // Execute the shipped dependency-free adapter with OpenCode's API shape.
    const plugin = pluginFor();
    expect(plugin.id).toBe("interface-activity-v1");
    let route: { name: string; params?: { sessionID: string } } = { name: "home" };
    let status = "busy";
    let pending = true;
    let dispose = () => {};
    const title = vi.fn();
    const children = vi.fn(async () => ({ data: [{ id: "ses_child" }] }));
    const removed = vi.fn();
    const api = {
      route: { get current() { return route; } },
      state: { ready: true, session: {
        get: (id: string) => ({ id, parentID: id === "ses_child" ? "ses_a" : undefined }),
        status: () => ({ type: status }),
        permission: (id: string) => id === "ses_child" && pending ? [{}] : [],
        question: () => [],
      } },
      client: { session: { children } },
      renderer: { setTerminalTitle: title },
      event: { on: () => removed },
      lifecycle: { onDispose: (callback: () => void) => { dispose = callback; } },
    };
    await plugin.tui(api);
    expect(title).toHaveBeenLastCalledWith("interface-opencode-state:v1:-");
    route = { name: "session", params: { sessionID: "ses_a" } };
    await vi.advanceTimersByTimeAsync(500);
    expect(children).toHaveBeenCalledWith({ sessionID: "ses_a" });
    expect(title).toHaveBeenLastCalledWith("interface-opencode-state:v1:ses_a|ses_a,-,busy,0,0|ses_child,ses_a,busy,1,0");
    pending = false;
    status = "idle";
    await vi.advanceTimersByTimeAsync(250);
    expect(title).toHaveBeenLastCalledWith("interface-opencode-state:v1:ses_a|ses_a,-,idle,0,0|ses_child,ses_a,idle,0,0");
    const beforeHeartbeat = title.mock.calls.length;
    await vi.advanceTimersByTimeAsync(1000);
    expect(title.mock.calls.length).toBeGreaterThan(beforeHeartbeat);
    dispose();
    expect(removed).toHaveBeenCalledOnce();
    const count = title.mock.calls.length;
    await vi.advanceTimersByTimeAsync(3000);
    expect(title).toHaveBeenCalledTimes(count);
  });

  it("records each pane's selected conversation and shows the recovery picker once", async () => {
    vi.useFakeTimers();
    const root = mkdtempSync(join(tmpdir(), "opencode-session-transport-"));
    const dispose: (() => void)[] = [];
    try {
      const fixture = JSON.parse(readFileSync(new URL("../fixtures/terminal-opencode-session-v1.json", import.meta.url), "utf8"));
      const tokens = [fixture.captureToken, "22222222-2222-4222-8222-222222222222"];
      const selected = [fixture.sessionId, "ses_otherPane456"];
      const pickers = [vi.fn(), vi.fn()];
      for (let index = 0; index < 2; index++) {
        mkdirSync(join(root, tokens[index]), { mode: 0o700 });
        await pluginFor({ INTERFACE_OPENCODE_CAPTURE_TOKEN: tokens[index], INTERFACE_OPENCODE_RECEIPTS_ROOT: root,
          INTERFACE_OPENCODE_RESUME_PICKER: "1" }).tui({
          route: { get current() { return { name: "session", params: { sessionID: selected[index] } }; } },
          keymap: { dispatchCommand: pickers[index] },
          state: { ready: true, path: { directory: "/project" }, session: { get: (id: string) => ({ id }),
            status: () => ({ type: "idle" }), permission: () => [], question: () => [] } },
          client: { session: { children: async () => ({ data: [] }) } },
          renderer: { setTerminalTitle: vi.fn() }, event: { on: () => () => {} },
          lifecycle: { onDispose: (callback: () => void) => dispose.push(callback) },
        });
      }
      const read = (index: number) => JSON.parse(readFileSync(join(root, tokens[index], ".interface-opencode-session.json"), "utf8"));
      expect(read(0)).toEqual(fixture);
      expect(read(1).sessionId).toBe(selected[1]);
      selected[0] = "ses_selectedOlder789";
      await vi.advanceTimersByTimeAsync(1500);
      expect(read(0).sessionId).toBe(selected[0]);
      expect(read(1).sessionId).toBe(selected[1]);
      for (const picker of pickers) {
        expect(picker).toHaveBeenCalledExactlyOnceWith("session.list");
      }
    } finally {
      for (const stop of dispose) stop();
      rmSync(root, { recursive: true, force: true });
    }
  });
});
