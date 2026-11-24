// Starts the `iface` Ratatui TUI hands-free: no window, in a private tmux server
// (its own socket, no config), with a throwaway HOME, IFACE_DATA_DIR and
// Foundation home, a PATH of system directories led by a bin directory whose
// provider commands are handsfree-fake-provider.sh, and the provider
// *_EXECUTABLE overrides pointed at the same fake. Nothing reaches the owner's
// accounts, CLIs or data. Mirrors hostEnvironment in src/main/handsfree.ts; the
// provider fence itself is handsfree-session.mjs, held equal to it by a test.
//   node scripts/handsfree-tui.mjs start [--no-build] [--session name]
//   tmux -S <socket> send-keys -t iface '/help' Enter; tmux -S <socket> capture-pane -p -t iface
//   node scripts/handsfree-tui.mjs stop [--session name]
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { cargoBinary } from "./cargo-target.mjs";
import {
  FAKE_PROVIDER_COMMANDS, FAKE_PROVIDER_VARIABLES, SYSTEM_PATH, sessionProfileScript,
} from "./handsfree-session.mjs";

const electronRoot = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
const workspaceRoot = path.join(electronRoot, "..");
const fake = path.join(electronRoot, "scripts", "handsfree-fake-provider.sh");

const [command, ...args] = process.argv.slice(2);
const named = args.indexOf("--session");
const session = (named >= 0 && args[named + 1]) || "iface-tui";
// Short on purpose: iface's agent socket lives in IFACE_DATA_DIR, and $TMPDIR
// paths overflow the 104-byte Unix socket limit.
const directory = path.join("/private/tmp", `handsfree-tui-${session}`);
const socket = path.join(directory, "tmux.sock");
const die = message => { console.error(`handsfree-tui: ${message}`); process.exit(1); };
// Found on the caller's PATH: the session's own PATH holds system tools only.
const isFile = candidate => { try { return fs.statSync(candidate).isFile(); } catch { return false; } };
const tmuxBinary = (process.env.PATH ?? "").split(path.delimiter).concat("/opt/homebrew/bin", "/usr/local/bin")
  .filter(Boolean).map(entry => path.join(entry, "tmux")).find(isFile);
const tmux = (...tmuxArgs) => spawnSync(tmuxBinary ?? die("tmux is not installed"), ["-S", socket, "-f", "/dev/null", ...tmuxArgs], {
  env: sessionEnvironment(), stdio: ["ignore", "pipe", "pipe"], encoding: "utf8",
});

function sessionEnvironment() {
  const home = path.join(directory, "home");
  const bin = path.join(directory, "bin");
  return {
    ...Object.fromEntries(FAKE_PROVIDER_VARIABLES.map(name => [name, fake])),
    HOME: home,
    CFFIXED_USER_HOME: home,
    IFACE_DATA_DIR: path.join(directory, "data"),
    PATH: [bin, ...SYSTEM_PATH].join(":"),
    SHELL: "/bin/zsh",
    TERM: "xterm-256color",
    LANG: "en_US.UTF-8",
    TMPDIR: path.join(directory, "tmp"),
  };
}

if (command === "start") {
  if (fs.existsSync(directory)) die(`session ${session} exists; stop it or pass --session`);
  if (!args.includes("--no-build")) {
    const cargoPath = [process.env.PATH, path.join(os.homedir(), ".cargo", "bin")].filter(Boolean).join(path.delimiter);
    const built = spawnSync("nice", ["-n", "10", "cargo", "build", "--package", "iface"], {
      cwd: workspaceRoot, env: { ...process.env, PATH: cargoPath }, stdio: ["ignore", 2, 2],
    });
    if (built.status !== 0) die("cargo build --package iface failed");
  }
  const iface = cargoBinary(workspaceRoot, "debug", "iface");
  if (!fs.existsSync(iface)) die(`${iface} is missing; run without --no-build`);
  const env = sessionEnvironment();
  for (const made of [env.HOME, env.IFACE_DATA_DIR, env.TMPDIR, path.join(directory, "bin")]) {
    fs.mkdirSync(made, { recursive: true, mode: 0o700 });
  }
  for (const name of FAKE_PROVIDER_COMMANDS) fs.symlinkSync(fake, path.join(directory, "bin", name));
  const profile = sessionProfileScript(path.join(directory, "bin"));
  for (const dotfile of [".zprofile", ".profile"]) fs.writeFileSync(path.join(env.HOME, dotfile), profile, { mode: 0o600 });
  const started = tmux("new-session", "-d", "-s", "iface", "-x", "160", "-y", "48", "-c", env.HOME, iface);
  if (started.status !== 0) {
    fs.rmSync(directory, { recursive: true, force: true });
    die(`tmux could not start: ${started.stderr.trim()}`);
  }
  console.log(JSON.stringify({ session, socket, target: "iface", home: env.HOME }));
} else if (command === "stop") {
  if (!fs.existsSync(directory)) die(`no session ${session}`);
  tmux("kill-server");
  fs.rmSync(directory, { recursive: true, force: true });
  console.log(`stopped ${session}`);
} else {
  die("usage: handsfree-tui.mjs start [--no-build] [--session name] | stop [--session name]");
}
