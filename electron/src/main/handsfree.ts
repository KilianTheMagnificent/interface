import { mkdirSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { isAbsolute, join } from "node:path";
import { CHANNELS } from "../shared/ipc.js";

/**
 * Hands-free agent testing. `scripts/handsfree.mjs start` sets HANDSFREE=1 and
 * a throwaway HANDSFREE_PROFILE; the vendored kit hook
 * (`scripts/handsfree-electron.cjs`) then keeps the app hidden, inactive and on
 * that profile, with a DevTools port for the driver. This module decides
 * whether the hook loads, what the Rust host inherits and which renderer
 * requests are refused, so a session reads and writes only session state:
 * never the owner's profile or single-instance lock, home directory, provider
 * accounts, running Interface, microphone, network listeners or other apps.
 */
export interface HandsfreeHook {
  readonly enabled: boolean;
  install(): void;
}

export interface HandsfreeMode {
  readonly enabled: boolean;
  /** The environment the Rust host starts with. */
  hostEnvironment(base: NodeJS.ProcessEnv): NodeJS.ProcessEnv;
  /** Throws, before the host sees it, for a renderer request whose effect
   * would leave the session. */
  guard(operation: string, payload: unknown): void;
}

/** Creates the session's private directories, dotfiles and command links. */
export interface SessionFiles {
  directory(path: string): void;
  file(path: string, text: string): void;
  link(target: string, path: string): void;
}

const OFF: HandsfreeMode = { enabled: false, hostEnvironment: base => base, guard: () => {} };

/** Variables that would route a session at the owner's live state, running
 * Interface, agent session or provider accounts. The host supplies its own
 * INTERFACE_* values to panes. */
const OWNER_STATE = /^(INTERFACE_|IFACE_|CLAUDE|CODEX_|GROK_|QWEN_|ANTIGRAVITY_|OPENCODE|MUSE_|KIMI_|ANTHROPIC_|OPENAI_|XAI_|GEMINI_|XDG_[A-Z]+_HOME$|ZDOTDIR$|HISTFILE$)|(_API_KEY|_TOKEN|_SECRET)$/u;

/** Provider overrides the host honours ahead of any PATH lookup. This fence
 * (with FAKE_PROVIDER_COMMANDS, SYSTEM_PATH and sessionProfileScript) is
 * repeated in scripts/handsfree-session.mjs for the TUI session, which
 * handsfree.test.ts holds equal; packaged main cannot import from scripts/. */
export const FAKE_PROVIDER_VARIABLES = [
  "CLAUDE_CODE_EXECUTABLE", "CODEX_EXECUTABLE", "GROK_EXECUTABLE", "QWEN_EXECUTABLE",
  "ANTIGRAVITY_EXECUTABLE", "QWEN_MLX_SERVER_EXECUTABLE",
] as const;

/** Every command the host or a pane resolves by name to reach a provider,
 * including OMP, OpenCode, Muse Code and Kimi Code, which have no override variable. Each one is a
 * link to the fake in the session's bin directory, which leads PATH. */
export const FAKE_PROVIDER_COMMANDS = [
  "claude", "codex", "grok", "qwen", "agy", "omp", "opencode", "muse", "kimi", "qwen-mlx-server",
] as const;

/** The only inherited PATH entries: system tools, never the owner's
 * ~/.local/bin, npm, bun or Homebrew installs. */
export const SYSTEM_PATH = ["/usr/bin", "/bin", "/usr/sbin", "/sbin"] as const;

/** A voice helper that exits at once: never the microphone or the speakers. */
export const HANDSFREE_VOICE_HELPER = "/usr/bin/false";

const shellQuoted = (value: string): string => `'${value.replaceAll("'", "'\\''")}'`;

/** The session's .zprofile and .profile. Login shells (the host's provider
 * lookup and every shell pane) run path_helper, which puts Homebrew ahead of
 * an inherited PATH; this puts the fakes back in front. */
export function sessionProfileScript(bin: string): string {
  return "# Hands-free session: provider commands are the fake "
    + `(electron/src/main/handsfree.ts).\nexport PATH=${shellQuoted(bin)}:"$PATH"\n`;
}

function field(payload: unknown, name: string): unknown {
  return typeof payload === "object" && payload !== null && !Array.isArray(payload)
    ? (payload as Record<string, unknown>)[name] : undefined;
}

/** Remote requests that open a LAN listener, a relay connection or a pairing. */
function opensRemoteAccess(payload: unknown): boolean {
  const action = field(payload, "action");
  if (action === "enable" || action === "pair" || action === "pairDirect") return true;
  const command = field(payload, "command");
  return action === "connection" && (command === "enable" || command === "pair");
}

/** Why a renderer request is refused in a hands-free session, or undefined.
 * Each refused effect reaches past the session: `/usr/bin/open` launching or
 * activating another app on the owner's screen, the microphone and speakers,
 * or a network listener. */
export function handsfreeRefusal(operation: string, payload: unknown): string | undefined {
  switch (operation) {
    case CHANNELS.openTerminal:
      return "open an external terminal app";
    case CHANNELS.openWorkspaceTool:
      return "open Finder, the browser or Simulator";
    case CHANNELS.voiceSetEnabled:
      return field(payload, "enabled") === true
        ? "turn on the voice assistant (microphone and speakers)" : undefined;
    case CHANNELS.agentControl:
      return field(payload, "kind") === "revealAgentHome" ? "reveal an agent home in Finder" : undefined;
    case CHANNELS.diagnostics:
      return field(payload, "kind") === "reveal" ? "reveal a file or folder in Finder" : undefined;
    case CHANNELS.remoteAccess:
      return opensRemoteAccess(payload) ? "open phone remote access" : undefined;
    default:
      return undefined;
  }
}

export function resolveHandsfree(input: {
  readonly env: NodeJS.ProcessEnv;
  readonly isPackaged: boolean;
  readonly fakeProvider: string;
  loadHook(): HandsfreeHook;
  readonly files: SessionFiles;
  log(line: string): void;
}): HandsfreeMode {
  if (input.env.HANDSFREE !== "1") return OFF;
  // Packaged builds do not ship the hook, and would otherwise open the owner's
  // profile and hand `second-instance` to the owner's running window.
  if (input.isPackaged) throw new Error("hands-free mode runs only the unpackaged app");
  const profile = input.env.HANDSFREE_PROFILE;
  if (profile === undefined || !isAbsolute(profile)) {
    throw new Error("HANDSFREE_PROFILE must be an absolute session directory");
  }
  const hook = input.loadHook();
  if (!hook.enabled) throw new Error("the hands-free hook did not enable itself");
  hook.install();
  const home = join(profile, "home");
  const bin = join(profile, "bin");
  return {
    enabled: true,
    hostEnvironment(base) {
      input.files.directory(home);
      input.files.directory(bin);
      for (const command of FAKE_PROVIDER_COMMANDS) input.files.link(input.fakeProvider, join(bin, command));
      const profileScript = sessionProfileScript(bin);
      input.files.file(join(home, ".zprofile"), profileScript);
      input.files.file(join(home, ".profile"), profileScript);
      const env: NodeJS.ProcessEnv = {};
      for (const [name, value] of Object.entries(base)) {
        if (!OWNER_STATE.test(name)) env[name] = value;
      }
      for (const name of FAKE_PROVIDER_VARIABLES) env[name] = input.fakeProvider;
      // One directory holds the Electron profile and the host stores, as in
      // production and Dev. HOME moves too: shells, provider stores and even a
      // release host's platform data root then land inside the session, and
      // Foundation preferences of anything the host starts follow it.
      return {
        ...env,
        HOME: home,
        CFFIXED_USER_HOME: home,
        PATH: [bin, ...SYSTEM_PATH].join(":"),
        INTERFACE_HOST_TEST_DATA_ROOT: profile,
        INTERFACE_HOST_TEST_HOME: home,
        INTERFACE_VOICE_HELPER: HANDSFREE_VOICE_HELPER,
      };
    },
    guard(operation, payload) {
      const refused = handsfreeRefusal(operation, payload);
      if (refused === undefined) return;
      input.log(`handsfree: blocked ${operation} (${refused})`);
      throw new Error(`A hands-free test session cannot ${refused}.`);
    },
  };
}

const sessionFiles: SessionFiles = {
  directory: path => { mkdirSync(path, { recursive: true, mode: 0o700 }); },
  file: (path, text) => { writeFileSync(path, text, { mode: 0o600 }); },
  link: (target, path) => {
    rmSync(path, { force: true });
    symlinkSync(target, path);
  },
};

/** Called at module scope, before the single-instance lock and any userData
 * read. A refused hands-free start exits rather than open the owner's state. */
export function startHandsfree(mainDirectory: string, isPackaged: boolean): HandsfreeMode {
  try {
    return resolveHandsfree({
      env: process.env,
      isPackaged,
      fakeProvider: join(mainDirectory, "..", "..", "scripts", "handsfree-fake-provider.sh"),
      loadHook: () => createRequire(import.meta.url)(
        join(mainDirectory, "..", "..", "scripts", "handsfree-electron.cjs"),
      ) as HandsfreeHook,
      files: sessionFiles,
      log: line => { console.log(line); },
    });
  } catch (error) {
    console.error(`handsfree: refusing to start: ${error instanceof Error ? error.message : String(error)}`);
    process.exit(78);
  }
}
