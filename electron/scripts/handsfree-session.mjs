// The hands-free provider fence the TUI session (handsfree-tui.mjs) builds.
// src/main/handsfree.ts keeps the desktop copy, because packaged main cannot
// import from scripts/; src/main/handsfree.test.ts asserts the two are equal.

/** Provider overrides the host honours ahead of any PATH lookup. */
export const FAKE_PROVIDER_VARIABLES = [
  "CLAUDE_CODE_EXECUTABLE", "CODEX_EXECUTABLE", "GROK_EXECUTABLE", "QWEN_EXECUTABLE",
  "ANTIGRAVITY_EXECUTABLE", "QWEN_MLX_SERVER_EXECUTABLE",
];

/** Every provider command resolved by name; each is a link to the fake. */
export const FAKE_PROVIDER_COMMANDS = [
  "claude", "codex", "grok", "qwen", "agy", "omp", "opencode", "muse", "kimi", "qwen-mlx-server",
];

/** The only PATH entries after the session's bin: system tools. */
export const SYSTEM_PATH = ["/usr/bin", "/bin", "/usr/sbin", "/sbin"];

const shellQuoted = value => `'${value.replaceAll("'", "'\\''")}'`;

/** The session's .zprofile and .profile: login shells run path_helper, which
 * puts Homebrew first; this puts the fakes back in front. */
export function sessionProfileScript(bin) {
  return "# Hands-free session: provider commands are the fake "
    + `(electron/src/main/handsfree.ts).\nexport PATH=${shellQuoted(bin)}:"$PATH"\n`;
}
