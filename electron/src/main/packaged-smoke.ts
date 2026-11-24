import { lstatSync, realpathSync } from "node:fs";
import { homedir } from "node:os";
import { isAbsolute, join, resolve, sep } from "node:path";
import { app, BrowserWindow, clipboard, dialog, Menu, session, shell } from "electron";
import { handsfreeRefusal } from "./handsfree.js";

export const PACKAGED_SMOKE_MARKER = "iface-packaged-smoke-isolation-v1";

/** The release smoke runner supplies all three private roots. Never interpret a
 * partial configuration as permission to fall back to the owner's profile. */
export function smokeProfile(env: NodeJS.ProcessEnv, argv: readonly string[]): string | undefined {
  const profile = env.INTERFACE_PACKAGED_SMOKE_PROFILE;
  const data = env.INTERFACE_HOST_SMOKE_DATA_ROOT;
  const home = env.INTERFACE_HOST_SMOKE_HOME;
  if (profile === undefined && data === undefined && home === undefined) return undefined;
  if (profile === undefined || data === undefined || home === undefined) {
    throw new Error("Packaged smoke requires a profile, data root and home together.");
  }
  for (const directory of [profile, data, home]) {
    const info = lstatSync(directory);
    if (!isAbsolute(directory) || !info.isDirectory() || info.isSymbolicLink()
        || (info.mode & 0o077) !== 0 || info.uid !== process.getuid?.()
        || realpathSync(directory) !== resolve(directory)) {
      throw new Error("Packaged smoke directories must be private, owned, canonical directories.");
    }
  }
  const ownerHome = realpathSync(homedir());
  if (profile === ownerHome || profile.startsWith(ownerHome + sep)
      || data !== join(profile, "data") || home !== join(profile, "home")
      || !argv.includes(`--user-data-dir=${profile}`)) {
    throw new Error("Packaged smoke roots must match the disposable profile outside the process home.");
  }
  const policy = join(profile, "host.sb");
  const policyInfo = lstatSync(policy);
  if (!policyInfo.isFile() || policyInfo.isSymbolicLink() || (policyInfo.mode & 0o077) !== 0
      || policyInfo.uid !== process.getuid?.()) {
    throw new Error("Packaged smoke requires an owner-private host sandbox policy.");
  }
  return profile;
}

/** A packaged smoke stays hidden without enabling the unpackaged HANDSFREE
 * entry point or loading any external code. The runner also applies an OS
 * filesystem/network sandbox to the Rust host and an allowlisted environment.
 * Chromium retains its production sandbox; macOS refuses nested sandbox_init. */
export function startPackagedSmoke(): boolean {
  let profile: string | undefined;
  try { profile = smokeProfile(process.env, process.argv); }
  catch (error) {
    console.error("packaged-smoke: refusing to start:", error);
    process.exit(78);
  }
  if (profile === undefined) return false;
  console.log(PACKAGED_SMOKE_MARKER);
  app.setPath("userData", profile);
  app.commandLine.appendSwitch("use-mock-keychain");
  app.commandLine.appendSwitch("mute-audio");
  if (process.platform === "darwin") app.setActivationPolicy("prohibited");
  app.focus = () => {};
  app.setBadgeCount = () => true;
  for (const method of ["show", "showInactive", "focus", "moveTop", "restore"] as const) {
    BrowserWindow.prototype[method] = () => {};
  }
  shell.openExternal = async () => { throw new Error("Packaged smoke cannot open external applications."); };
  shell.openPath = async () => "Packaged smoke cannot open external applications.";
  shell.showItemInFolder = () => {};
  dialog.showErrorBox = (title, content) => { console.error(`packaged-smoke: ${title}: ${content}`); };
  dialog.showOpenDialog = async () => ({ canceled: true, filePaths: [] });
  dialog.showSaveDialog = async () => ({ canceled: true, filePath: "" });
  dialog.showMessageBoxSync = () => 0;
  dialog.showMessageBox = async () => ({ response: 0, checkboxChecked: false });
  const setMenu = Menu.setApplicationMenu.bind(Menu);
  Menu.setApplicationMenu = () => {};
  let copied = "";
  clipboard.writeText = text => { copied = text; };
  clipboard.readText = () => copied;
  void app.whenReady().then(() => {
    setMenu(null);
    session.defaultSession.webRequest.onBeforeRequest((details, decide) => {
      const url = new URL(details.url);
      const external = ["http:", "https:", "ws:", "wss:"].includes(url.protocol)
        && !["localhost", "127.0.0.1", "[::1]"].includes(url.hostname);
      decide({ cancel: external });
    });
  });
  app.on("browser-window-created", (_event, win) => {
    win.webContents.setAudioMuted(true);
    const debuggerClient = win.webContents.debugger;
    debuggerClient.attach("1.3");
    void debuggerClient.sendCommand("Page.enable").then(() => debuggerClient.sendCommand(
      "Page.addScriptToEvaluateOnNewDocument", { runImmediately: true, source: `(() => {
        let text = "";
        if (navigator.clipboard) Object.assign(navigator.clipboard, {
          writeText: async value => { text = String(value); }, readText: async () => text,
          write: async () => {}, read: async () => [],
        });
        const exec = document.execCommand.bind(document);
        document.execCommand = (command, ...args) => {
          if (/^(copy|cut)$/i.test(command)) { text = String(getSelection()); return true; }
          if (/^paste$/i.test(command)) return false;
          return exec(command, ...args);
        };
        if (globalThis.speechSynthesis) speechSynthesis.speak = () => {};
      })();` },
    )).catch(error => { console.error("Packaged smoke page isolation failed", error); app.exit(78); });
    for (const method of ["copy", "cut", "paste", "pasteAndMatchStyle"] as const) {
      win.webContents[method] = () => {};
    }
  });
  return true;
}

export function guardPackagedSmoke(enabled: boolean, operation: string, payload: unknown): void {
  if (!enabled) return;
  const reason = handsfreeRefusal(operation, payload);
  if (reason !== undefined) throw new Error(`A packaged smoke cannot ${reason}.`);
}

export function packagedSmokeHostCommand(enabled: boolean, command: string): { command: string; args?: string[] } {
  if (!enabled) return { command };
  return { command: "/usr/bin/sandbox-exec", args: ["-f", join(app.getPath("userData"), "host.sb"), command] };
}
