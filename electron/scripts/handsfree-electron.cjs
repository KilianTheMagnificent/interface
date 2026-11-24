// Hands-free mode for agent testing, switched on by `handsfree start` (HANDSFREE=1). The app
// runs against a throwaway profile, opens a DevTools port for the driver, never activates,
// and keeps every window hidden (CDP input and screenshots work on hidden windows). Nothing it
// does may reach the owner: showing or focusing windows, global shortcuts, the Dock, the
// owner's browser, keychain, speakers, modal dialogs and notifications are all neutralised here.
// Call install() first thing in the main process, before "ready" and before any
// requestSingleInstanceLock() or store that reads userData. Windows must still be created
// with `show: !handsfree.enabled`: the constructor shows natively, past these patches.
const { app, BrowserWindow, clipboard, dialog, globalShortcut, Menu, nativeImage, Notification, shell } = require("electron");
const fs = require("node:fs");

const enabled = process.env.HANDSFREE === "1" && !!process.env.HANDSFREE_PROFILE;
const blocked = what => console.log(`handsfree: blocked ${what}`);
// Runs in every frame before page scripts: speech is silent and copy/paste use a page-local
// clipboard, so neither the owner's speakers nor their pasteboard are reachable from a page.
const PAGE_GUARD = `(() => {
  const log = what => console.log("handsfree: blocked " + what);
  try { if (globalThis.speechSynthesis) speechSynthesis.speak = () => log("speech"); } catch {}
  const board = { text: "" };
  try {
    if (navigator.clipboard) Object.assign(navigator.clipboard, {
      writeText: async text => { board.text = String(text); },
      readText: async () => board.text,
      write: async () => log("clipboard.write"),
      read: async () => [],
    });
    const exec = document.execCommand.bind(document);
    document.execCommand = (command, ...rest) => {
      if (/^(copy|cut)$/i.test(command)) { board.text = String(getSelection()); return true; }
      if (/^paste$/i.test(command)) { log("paste"); return false; }
      return exec(command, ...rest);
    };
  } catch {}
})()`;

function install() {
  if (!enabled) return;
  // Created up front so Chromium can write DevToolsActivePort there before Electron would.
  fs.mkdirSync(process.env.HANDSFREE_PROFILE, { recursive: true });
  app.setPath("userData", process.env.HANDSFREE_PROFILE);
  // Chromium writes the chosen port to <userData>/DevToolsActivePort for the driver.
  app.commandLine.appendSwitch("remote-debugging-port", "0");
  // safeStorage/cookies must never raise a keychain prompt on the owner's screen.
  app.commandLine.appendSwitch("use-mock-keychain");
  // Silent: no audio output (the owner may be on a call); PAGE_GUARD also stubs speech.
  app.commandLine.appendSwitch("mute-audio");
  if (process.platform === "darwin") app.setActivationPolicy("accessory");
  app.setActivationPolicy = () => blocked("setActivationPolicy");
  app.focus = () => blocked("app.focus");
  if (app.dock) Object.assign(app.dock, { show: async () => blocked("dock.show"), bounce: () => 0, setBadge: () => {} });
  for (const name of ["show", "focus", "moveTop", "showInactive", "setAlwaysOnTop", "setVisibleOnAllWorkspaces"])
    BrowserWindow.prototype[name] = function () { blocked(`BrowserWindow.${name}`); };
  globalShortcut.register = () => (blocked("globalShortcut"), false);
  globalShortcut.registerAll = () => blocked("globalShortcut");
  shell.openExternal = async url => blocked(`openExternal ${url}`);
  shell.openPath = async target => (blocked(`openPath ${target}`), "");
  shell.showItemInFolder = target => blocked(`showItemInFolder ${target}`);
  Notification.prototype.show = function () { blocked(`notification ${this.title}`); };
  app.setLoginItemSettings = () => blocked("setLoginItemSettings");
  for (const name of ["showOpenDialog", "showSaveDialog"]) dialog[name] = async () => (blocked(name), { canceled: true, filePaths: [] });
  for (const name of ["showOpenDialogSync", "showSaveDialogSync"]) dialog[name] = () => (blocked(name), undefined);
  dialog.showMessageBox = async () => (blocked("showMessageBox"), { response: 0, checkboxChecked: false });
  dialog.showMessageBoxSync = () => (blocked("showMessageBoxSync"), 0);
  dialog.showErrorBox = (title, content) => blocked(`error box ${title}: ${content}`);
  // The owner's pasteboard stays untouched: an in-memory stand-in for the main process.
  const board = { text: "", html: "", rtf: "" };
  Object.assign(clipboard, {
    writeText: text => { board.text = String(text); }, readText: () => board.text,
    writeHTML: html => { board.html = String(html); }, readHTML: () => board.html,
    writeRTF: rtf => { board.rtf = String(rtf); }, readRTF: () => board.rtf,
    write: data => { for (const key of ["text", "html", "rtf"]) if (data[key] !== undefined) board[key] = String(data[key]); },
    writeImage: () => blocked("clipboard.writeImage"), readImage: () => nativeImage.createEmpty(),
    writeBookmark: () => {}, writeFindText: () => {}, readFindText: () => "", writeBuffer: () => {},
    clear: () => { board.text = board.html = board.rtf = ""; },
    availableFormats: () => (board.text ? ["text/plain"] : []), has: () => false,
  });
  // No app menu: its roles (copy/paste, Hide Others, Bring All to Front, Quit) would answer
  // Meta+ keys sent by the driver. Renderer keydown handlers still see those keys.
  // Silent: Electron itself installs a default menu at ready, so a log line would be noise.
  const setMenu = Menu.setApplicationMenu.bind(Menu);
  Menu.setApplicationMenu = () => {};
  app.whenReady().then(() => setMenu(null));
  // Pin emulated focus so the driver's short CDP sessions don't fire blur between commands,
  // and install the page guard in every frame.
  app.on("browser-window-created", (_, win) => {
    for (const name of ["copy", "cut", "paste", "pasteAndMatchStyle"]) win.webContents[name] = () => blocked(`webContents.${name}`);
    try {
      const dbg = win.webContents.debugger;
      dbg.attach("1.3");
      dbg.sendCommand("Emulation.setFocusEmulationEnabled", { enabled: true }).catch(() => {});
      dbg.sendCommand("Page.enable").catch(() => {});
      dbg.sendCommand("Page.addScriptToEvaluateOnNewDocument", { source: PAGE_GUARD, runImmediately: true }).catch(() => {});
    } catch (error) { console.error("handsfree: page guard unavailable:", error.message); }
  });
}

module.exports = { enabled, install };
