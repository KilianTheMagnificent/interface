import { describe, expect, it } from "vitest";
import { CHANNELS } from "../shared/ipc.js";
import {
  FAKE_PROVIDER_COMMANDS, FAKE_PROVIDER_VARIABLES, HANDSFREE_VOICE_HELPER, SYSTEM_PATH,
  handsfreeRefusal, resolveHandsfree, sessionProfileScript, type HandsfreeHook,
} from "./handsfree.js";
import * as tuiSession from "../../scripts/handsfree-session.mjs";

function hook(enabled = true): HandsfreeHook & { installed: number } {
  return { enabled, installed: 0, install() { this.installed += 1; } };
}

const FAKE = "/checkout/electron/scripts/fake";

function resolve(env: NodeJS.ProcessEnv, options: { isPackaged?: boolean; hook?: HandsfreeHook } = {}) {
  const directories: string[] = [];
  const files = new Map<string, string>();
  const links = new Map<string, string>();
  const logged: string[] = [];
  const loaded = options.hook ?? hook();
  let loads = 0;
  const mode = resolveHandsfree({
    env, isPackaged: options.isPackaged ?? false, fakeProvider: FAKE,
    loadHook: () => { loads += 1; return loaded; },
    files: {
      directory: path => { directories.push(path); },
      file: (path, text) => { files.set(path, text); },
      link: (target, path) => { links.set(path, target); },
    },
    log: line => { logged.push(line); },
  });
  return { mode, directories, files, links, logged, loads };
}

const session = { HANDSFREE: "1", HANDSFREE_PROFILE: "/tmp/handsfree/abc/profile" };
const OWNER_PATH = [
  "/Users/owner/.local/bin", "/Users/owner/.npm-global/bin", "/Users/owner/.cargo/bin",
  "/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin",
].join(":");

describe("hands-free mode", () => {
  it("stays off, without loading the kit hook or refusing anything, unless the driver started the app", () => {
    const { mode, loads, directories } = resolve({ HANDSFREE_PROFILE: "/tmp/p" });
    expect(mode.enabled).toBe(false);
    expect(loads).toBe(0);
    const base = { HOME: "/Users/owner", INTERFACE_PANE_MCP_SOCKET: "/owner.sock" };
    expect(mode.hostEnvironment(base)).toBe(base);
    expect(directories).toEqual([]);
    expect(() => { mode.guard(CHANNELS.openWorkspaceTool, { version: 1, root: "/r", tool: "files" }); }).not.toThrow();
    expect(() => { mode.guard(CHANNELS.voiceSetEnabled, { enabled: true, cwd: "/r" }); }).not.toThrow();
  });

  it("refuses the packaged app, a missing profile and a hook that stayed off", () => {
    expect(() => resolve(session, { isPackaged: true })).toThrow(/unpackaged/u);
    expect(() => resolve({ HANDSFREE: "1" })).toThrow(/HANDSFREE_PROFILE/u);
    expect(() => resolve({ HANDSFREE: "1", HANDSFREE_PROFILE: "relative" })).toThrow(/absolute/u);
    expect(() => resolve(session, { hook: hook(false) })).toThrow(/did not enable/u);
  });

  it("installs the hook once and moves the host, its home and every provider into the session", () => {
    const installed = hook();
    const { mode, directories } = resolve(session, { hook: installed });
    expect(mode.enabled).toBe(true);
    expect(installed.installed).toBe(1);
    const env = mode.hostEnvironment({
      ...session, PATH: OWNER_PATH, SHELL: "/bin/zsh", HOME: "/Users/owner",
      INTERFACE_PANE_MCP_SOCKET: "/owner/panes/iface.sock", INTERFACE_DEV_SUPERVISED: "1",
      INTERFACE_HOST_SMOKE_DATA_ROOT: "/elsewhere", INTERFACE_VOICE_HELPER: "/owner/interface-voice",
      CLAUDE_CODE_CHILD_SESSION: "1", CLAUDECODE: "1",
      CLAUDE_CONFIG_DIR: "/Users/owner/.claude", CODEX_HOME: "/Users/owner/.codex", ZDOTDIR: "/Users/owner",
      XDG_CONFIG_HOME: "/Users/owner/.config", ANTHROPIC_API_KEY: "k", GITHUB_TOKEN: "t",
      CFFIXED_USER_HOME: "/Users/owner",
    });
    expect(directories).toEqual(["/tmp/handsfree/abc/profile/home", "/tmp/handsfree/abc/profile/bin"]);
    expect(env).toMatchObject({
      SHELL: "/bin/zsh", HANDSFREE: "1",
      HOME: "/tmp/handsfree/abc/profile/home",
      CFFIXED_USER_HOME: "/tmp/handsfree/abc/profile/home",
      INTERFACE_HOST_TEST_DATA_ROOT: "/tmp/handsfree/abc/profile",
      INTERFACE_HOST_TEST_HOME: "/tmp/handsfree/abc/profile/home",
      INTERFACE_VOICE_HELPER: HANDSFREE_VOICE_HELPER,
    });
    for (const name of FAKE_PROVIDER_VARIABLES) expect(env[name]).toBe(FAKE);
    for (const name of ["INTERFACE_PANE_MCP_SOCKET", "INTERFACE_DEV_SUPERVISED", "INTERFACE_HOST_SMOKE_DATA_ROOT",
      "CLAUDE_CODE_CHILD_SESSION", "CLAUDECODE", "CLAUDE_CONFIG_DIR", "CODEX_HOME", "ZDOTDIR", "XDG_CONFIG_HOME",
      "ANTHROPIC_API_KEY", "GITHUB_TOKEN"]) {
      expect(env, name).not.toHaveProperty(name);
    }
  });

  it("keeps no owner PATH entry and puts a fake ahead of every provider command, OMP and OpenCode included", () => {
    const { mode, links, files } = resolve(session);
    const env = mode.hostEnvironment({ PATH: OWNER_PATH, HOME: "/Users/owner" });
    const path = (env.PATH ?? "").split(":");
    expect(path).toEqual(["/tmp/handsfree/abc/profile/bin", ...SYSTEM_PATH]);
    for (const entry of path) {
      expect(entry.startsWith("/Users/"), entry).toBe(false);
      expect(["/opt/homebrew/bin", "/usr/local/bin"]).not.toContain(entry);
    }
    expect(FAKE_PROVIDER_COMMANDS).toEqual(expect.arrayContaining(["omp", "opencode", "qwen-mlx-server", "agy"]));
    for (const command of FAKE_PROVIDER_COMMANDS) {
      expect(links.get(`/tmp/handsfree/abc/profile/bin/${command}`), command).toBe(FAKE);
    }
    // path_helper in /etc/zprofile would put Homebrew first again in login shells.
    for (const dotfile of [".zprofile", ".profile"]) {
      expect(files.get(`/tmp/handsfree/abc/profile/home/${dotfile}`)).toContain(
        "export PATH='/tmp/handsfree/abc/profile/bin':\"$PATH\"",
      );
    }
  });

  it("refuses, with a logged reason, every renderer request that would reach past the session", () => {
    const { mode, logged } = resolve(session);
    const refused: [string, unknown][] = [
      [CHANNELS.openTerminal, { root: "/r" }],
      [CHANNELS.openWorkspaceTool, { version: 1, root: "/r", tool: "files" }],
      [CHANNELS.openWorkspaceTool, { version: 1, root: "/r", tool: "browser", url: "http://127.0.0.1:3000" }],
      [CHANNELS.openWorkspaceTool, { version: 1, root: "/r", tool: "ios-simulator" }],
      [CHANNELS.voiceSetEnabled, { enabled: true, cwd: "/r" }],
      [CHANNELS.agentControl, { kind: "revealAgentHome", agentId: "a" }],
      [CHANNELS.remoteAccess, { action: "enable", origin: "https://x" }],
      [CHANNELS.remoteAccess, { action: "pair" }],
      [CHANNELS.remoteAccess, { action: "pairDirect", version: 6 }],
      [CHANNELS.remoteAccess, { action: "connection", version: 5, command: "enable", relayOrigin: null }],
      [CHANNELS.remoteAccess, { action: "connection", version: 5, command: "pair" }],
    ];
    for (const [operation, payload] of refused) {
      expect(handsfreeRefusal(operation, payload), operation).toBeDefined();
      expect(() => { mode.guard(operation, payload); }, operation).toThrow(/hands-free test session cannot/u);
    }
    expect(logged).toHaveLength(refused.length);
    expect(logged[0]).toBe("handsfree: blocked iface:openTerminal (open an external terminal app)");
  });

  it("lets session-local requests through, including turning voice and remote access off", () => {
    const { mode, logged } = resolve(session);
    const allowed: [string, unknown][] = [
      [CHANNELS.voiceSetEnabled, { enabled: false, cwd: "/r" }],
      [CHANNELS.agentControl, { kind: "read" }],
      [CHANNELS.agentControl, { kind: "recreateAgentHome", agentId: "a" }],
      [CHANNELS.remoteAccess, { action: "status" }],
      [CHANNELS.remoteAccess, { action: "disable" }],
      [CHANNELS.remoteAccess, { action: "connection", version: 5, command: "status" }],
      [CHANNELS.remoteAccess, { action: "trusted", version: 3, command: "status" }],
      [CHANNELS.createTerminal, { version: 1 }],
      [CHANNELS.listTerminalLaunchProfiles, {}],
    ];
    for (const [operation, payload] of allowed) {
      expect(() => { mode.guard(operation, payload); }, operation).not.toThrow();
    }
    expect(logged).toEqual([]);
  });

  // The TUI session (scripts/handsfree-tui.mjs) cannot import this module, so
  // it holds its own copy of the fence; a provider added here but not there
  // would reach the real CLI from a TUI session.
  it("fences the TUI session with exactly the desktop session's providers, PATH and profile", () => {
    expect(tuiSession.FAKE_PROVIDER_VARIABLES).toEqual(FAKE_PROVIDER_VARIABLES);
    expect(tuiSession.FAKE_PROVIDER_COMMANDS).toEqual(FAKE_PROVIDER_COMMANDS);
    expect(tuiSession.SYSTEM_PATH).toEqual(SYSTEM_PATH);
    for (const bin of ["/private/tmp/handsfree-tui-iface-tui/bin", "/tmp/it's here/bin"]) {
      expect(tuiSession.sessionProfileScript(bin)).toBe(sessionProfileScript(bin));
    }
  });
});
