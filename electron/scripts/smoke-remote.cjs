// Real Rust host + PTY + sandboxed Chromium at phone sizes. All data and grants
// are disposable; no provider accounts, live sessions or network tunnels.
const { app, BrowserWindow } = require("electron");
const assert = require("node:assert/strict");
const { randomUUID } = require("node:crypto");
const fs = require("node:fs/promises");
const path = require("node:path");
const { pathToFileURL } = require("node:url");
const { tmpdir } = require("node:os");
app.on("window-all-closed", () => {});
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
async function waitFor(read, description) {
  const deadline = Date.now() + 15000;
  while (Date.now() < deadline) { const value = await read(); if (value) return value; await delay(40); }
  throw new Error(`Timed out: ${description}`);
}
const root = path.resolve(__dirname, "../..");
let host, window, temporary, output;
const deadline = setTimeout(() => { console.error("Remote smoke exceeded 120 seconds"); app.exit(1); }, 120000);
async function run() {
  temporary = await fs.realpath(await fs.mkdtemp(path.join(tmpdir(), "irphone-")));
  output = await fs.mkdtemp(path.join(tmpdir(), "interface-remote-evidence-"));
  app.setPath("userData", path.join(temporary, "profile"));
  await app.whenReady();
  const { RustHostClient, RUST_HOST_PROTOCOL_VERSION, RUST_HOST_REQUEST_OPERATIONS } = await import(pathToFileURL(path.join(root, "electron/dist-main/main/rust-host-client.js")));
  const { TerminalDataDirection, TerminalDataKind } = await import(pathToFileURL(path.join(root, "electron/dist-main/main/terminal-data-transport.js")));
  const { cargoBinary } = await import(pathToFileURL(path.join(root, "electron/scripts/cargo-target.mjs")));
  const home = path.join(temporary, "home"), data = path.join(temporary, "data");
  const workspace = path.join(home, "Desktop/Projects");
  await fs.mkdir(workspace, { recursive: true, mode: 0o700 });
  await fs.mkdir(data, { recursive: true, mode: 0o700 });
  const shell = path.join(temporary, "shell");
  await fs.writeFile(shell, "#!/bin/sh\nexec /bin/sh -i\n", { mode: 0o700 });
  const chatId = randomUUID(), chatTerminalId = randomUUID();
  const claude = path.join(temporary, 'claude');
  await fs.writeFile(claude, "#!/bin/sh\nprintf '\\033[?2004h❯ '\nwhile IFS= read -r line; do printf '%s\\n' \"$line\" >> phone-replies; if [ -f next-screen ]; then cat next-screen; rm next-screen; else printf '\\033[2J\\033[H❯ '; fi; done\n", { mode: 0o700 });
  const project = path.join(home, '.claude/projects', workspace.replace(/[^a-zA-Z0-9]/g, '-'));
  await fs.mkdir(project, { recursive: true, mode: 0o700 });
  const transcript = [
    { type: 'user', uuid: randomUUID(), sessionId: chatId, cwd: workspace, timestamp: '2026-09-11T00:00:00Z', message: { role: 'user', content: 'Show my Interface sessions on iPhone.' } },
    { type: 'assistant', uuid: randomUUID(), sessionId: chatId, cwd: workspace, timestamp: '2026-09-11T00:00:01Z', message: { role: 'assistant', content: [{ type: 'text', text: 'The phone shares the same conversation view and connects to the sessions running on your Mac.\n\n### Ready for your review\n\nYour workspace names and sessions stay together. You can read the conversation and send a reply from here.\n\n- Messages stay in sync with your Mac.\n- Tool activity opens when you need the details.\n\n```swift\nlet session = workspace.sessions.first\n// A deliberately long line verifies that code scrolls inside its own block without widening the phone screen.\n```\n\nThe session will keep running when you put your phone away.' }] } },
  ];
  await fs.writeFile(path.join(project, `${chatId}.jsonl`), transcript.map(row => JSON.stringify(row)).join('\n') + '\n', { mode: 0o600 });
  const hostOptions = { command: cargoBinary(root, "debug", "interface-host"), cwd: root, env: {
    PATH: "/usr/bin:/bin:/usr/sbin:/sbin", SHELL: shell,
    INTERFACE_HOST_TEST_HOME: home, INTERFACE_HOST_TEST_DATA_ROOT: data,
    INTERFACE_REMOTE_ASSETS: path.join(root, "electron/dist-remote"),
    CODEX_EXECUTABLE: "/usr/bin/false", CLAUDE_CODE_EXECUTABLE: claude,
    GROK_EXECUTABLE: "/usr/bin/false", QWEN_EXECUTABLE: "/usr/bin/false",
  }};
  host = new RustHostClient(hostOptions);
  const ready = await host.start();
  assert.equal(ready.version, RUST_HOST_PROTOCOL_VERSION);
  assert.equal(ready.operations, RUST_HOST_REQUEST_OPERATIONS);
  const manage = request => host.invoke("iface:remoteAccess", request);
  assert.equal((await manage({ action: "status" })).enabled, false);
  const terminalId = randomUUID();
  let text = "", sequence = 0n;
  const transport = host.terminalDataTransport;
  transport.onFrame(frame => { if (frame.terminalId === terminalId && frame.kind === TerminalDataKind.Data) text += Buffer.from(frame.payload).toString(); });
  await host.invoke("iface:createTerminal", { id: terminalId, cwd: workspace, title: "Remote companion", size: { columns: 80, rows: 24 } });
  const desktop = command => transport.send({ terminalId, generation: 1n, sequence: sequence++, direction: TerminalDataDirection.ElectronToHost, kind: TerminalDataKind.Data, payload: Buffer.from(command) });
  desktop("stty -echo; printf 'ORIGINAL_PID:%s\\n' \"$$\"\n");
  const pid = await waitFor(() => text.match(/ORIGINAL_PID:(\d+)/)?.[1], "original shell PID");
  await host.invoke('iface:createTerminal', { id: chatTerminalId, cwd: workspace, title: 'Phone conversation', size: { columns: 80, rows: 24 }, launch: { version: 1, profileId: 'claude' }, launchSession: { version: 1, id: chatId, resume: true } });
  await host.invoke('iface:setTerminalWorkspaces', { workspaces: [{ id: randomUUID(), name: 'Interface project', createdAt: '2026-09-11T00:00:00Z', updatedAt: '2026-09-11T00:00:00Z', terminals: [
    { id: terminalId, cwd: workspace, title: 'Remote companion' },
    { id: chatTerminalId, cwd: workspace, title: 'Phone conversation', launch: { version: 1, profileId: 'claude' }, launchSession: { version: 1, id: chatId, resume: true } },
  ] }] });
  const enabled = await manage({ action: "enable", origin: "" });
  const url = enabled.localUrl;
  console.log("remote smoke: host enabled");
  const api = `/api/v1/terminals/${terminalId}/1`;
  assert.equal((await fetch(`${url}/api/v1/terminals`)).status, 401);
  assert.equal(await new Promise((resolve, reject) => {
    const req = require('node:http').get(url, { headers: { Host: 'evil.test' } }, response => { response.resume(); resolve(response.statusCode); }); req.on('error', reject);
  }), 403);
  assert.equal((await fetch(`${url}/api/v1/pair`, { method: "POST", headers: { Origin: "https://evil.test", "Content-Type": "application/json" }, body: "{}" })).status, 403);
  const publicPage = await fetch(url);
  assert.equal(publicPage.headers.get("cache-control"), "no-store");
  assert.equal(publicPage.headers.get("access-control-allow-origin"), null);
  window = new BrowserWindow({ show: false, width: 390, height: 844, webPreferences: { contextIsolation: true, sandbox: true, nodeIntegration: false, backgroundThrottling: false } });
  await window.loadURL("about:blank");
  window.webContents.debugger.attach("1.3");
  await window.webContents.debugger.sendCommand("Page.enable");
  await window.webContents.debugger.sendCommand("Emulation.setFocusEmulationEnabled", { enabled: true });
  await window.webContents.debugger.sendCommand("Page.addScriptToEvaluateOnNewDocument", { source: `
    window.remoteSockets = [];
    const OriginalSocket = WebSocket;
    window.WebSocket = class extends OriginalSocket { constructor(...args) { super(...args); window.remoteSockets.push(this); } };
  ` });
  const js = source => window.webContents.executeJavaScript(source);
  const has = value => js(`document.body.innerText.includes(${JSON.stringify(value)})`);
  const click = value => js(`Array.from(document.querySelectorAll('button')).find(b => b.textContent.trim() === ${JSON.stringify(value)})?.click()`);
  const fill = (selector, value) => js(`(() => { const e = document.querySelector(${JSON.stringify(selector)}); Object.getOwnPropertyDescriptor(e.tagName === 'TEXTAREA' ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype, 'value').set.call(e, ${JSON.stringify(value)}); e.dispatchEvent(new Event('input', { bubbles: true })); })()`);
  const shot = async name => fs.writeFile(path.join(output, `${name}.png`), (await window.webContents.capturePage()).toPNG());
  console.log("remote smoke: loading phone view");
  await window.loadURL(url);
  console.log("remote smoke: phone view loaded");
  await waitFor(() => has("Connect iPhone"), "pairing screen");
  await shot("pairing-390");
  const pairing = await manage({ action: "pair" });
  await fill("#pair-code", pairing.pairingCode);
  
  // Text nodes can contain optional whitespace; submit through the real form.
  if (!await has("Sessions")) await js("document.querySelector('form')?.requestSubmit()");
  await waitFor(() => has("Remote companion"), "paired session list");
  await shot("sessions-390");
  console.log("remote smoke: pairing passed");
  assert.equal(await js("document.cookie"), "", "grant must be HttpOnly");
  const cookies = await window.webContents.session.cookies.get({ url });
  const cookie = cookies.find(c => c.name === "interface_remote");
  assert.ok(cookie.httpOnly); assert.equal(cookie.sameSite, "strict");
  const auth = { Cookie: `interface_remote=${cookie.value}`, Origin: url };
  const post = (suffix, body, headers = {}) => fetch(`${url}${api}${suffix}`, { method: "POST", headers: { ...auth, ...headers }, body });
  assert.equal((await fetch(`${url}${api}/draft`)).status, 401);
  const target = { kind: 'terminal', terminalId, generation: 1 };
  const originalDraft = await (await fetch(`${url}${api}/draft`, { headers: auth })).json();
  const shared = await host.invoke('iface:harnessControl', { kind: 'saveDraft', target, expectedRevision: originalDraft.revision, text: 'saved on desktop' });
  assert.equal((await (await fetch(`${url}${api}/draft`, { headers: auth })).json()).text, 'saved on desktop');
  assert.equal((await post('/draft', JSON.stringify({ expectedRevision: originalDraft.revision, text: 'stale phone edit' }), { 'Content-Type': 'application/json' })).status, 400);
  const clearedDraft = await post('/draft', JSON.stringify({ expectedRevision: shared.draft.revision, text: '' }), { 'Content-Type': 'application/json' });
  assert.equal(clearedDraft.status, 200);
  const results = await (await fetch(`${url}${api}/results`, { headers: auth })).json();
  assert.equal(results.version, 1); assert.equal(results.terminalId, terminalId); assert.equal(results.generation, 1);
  assert.deepEqual(results.results, []);
  assert.equal((await fetch(`${url}/api/v1/terminals/${terminalId}/2/results`, { headers: auth })).status, 400);

  assert.equal((await fetch(`${url}/api/v2/workspaces`)).status, 401);
  assert.equal((await fetch(`${url}/api/v2/terminals/${chatTerminalId}/1/history`)).status, 401);
  const roster = await (await fetch(`${url}/api/v2/workspaces`, { headers: auth })).json();
  assert.equal(roster.version, 2); assert.equal(roster.workspaces[0].name, 'Interface project');
  assert.deepEqual(roster.workspaces[0].terminals.map(t => t.id), [terminalId, chatTerminalId]);
  assert.equal((await fetch(`${url}/api/v2/terminals/${chatTerminalId}/2/history`, { headers: auth })).status, 400);
  const history = await (await fetch(`${url}/api/v2/terminals/${chatTerminalId}/1/history`, { headers: auth })).json();
  assert.equal(history.version, 2); assert.ok(history.history.events.some(e => e.text?.includes('same conversation view')), 'real Rust provider transcript read');
  await js("document.querySelectorAll('.remote-session')[1].click()");
  await waitFor(() => has('The phone shares the same conversation view'), 'shared desktop conversation');
  for (const [width, height] of [[390, 844], [430, 932], [844, 390]]) {
    window.setContentSize(width, height); await delay(200);
    assert.equal(await js('document.documentElement.scrollWidth <= innerWidth'), true);
    assert.equal(await js("document.querySelector('.remote-composer').getBoundingClientRect().bottom <= innerHeight + 1"), true);
    await shot(`conversation-${width}`);
  }
  window.setContentSize(390, 844);
  await click('Terminal'); await waitFor(() => has('Live terminal'), 'conversation terminal view');
  await click('Conversation'); await waitFor(() => has('The phone shares'), 'conversation survives toggle');
  await js("document.querySelector('.remote-back').click()");
  const lease = (await (await post("/control")).json()).lease;
  const command = randomUUID(), bytes = "printf 'REMOTE_ONCE\\n'; printf x >> count\n";
  const headers = { "X-Interface-Control": lease, "X-Interface-Command": command };
  assert.equal((await post("/input", bytes, headers)).status, 202);
  assert.equal((await post("/input", bytes, headers)).status, 202);
  await waitFor(async () => { try { return await fs.readFile(path.join(workspace, "count"), "utf8") === "x"; } catch { return false; } }, "one remote effect");
  assert.equal((await post("/input", "different", headers)).status, 409);
  assert.equal((await post("/input", "x".repeat(16385), { ...headers, "X-Interface-Command": randomUUID() })).status, 413);
  assert.equal((await fetch(`${url}/api/v1/terminals/${terminalId}/2/control`, { method: "POST", headers: auth })).status, 400);
  desktop("printf 'DESKTOP_TAKEOVER\\n'\n");
  await waitFor(() => text.includes("DESKTOP_TAKEOVER"), "desktop takeover");
  assert.equal((await post("/input", "printf bad\n", { ...headers, "X-Interface-Command": randomUUID() })).status, 409);
  console.log("remote smoke: opening terminal");
  await js("document.querySelector('.remote-session').click()");
  await waitFor(() => has("Live terminal"), "terminal websocket");
  await click("Take control");
  await waitFor(() => has("iPhone controls input"), "control lease");
  await fill(".remote-composer textarea", "printf 'PHONE_PID:%s\\n' \"$$\"");
  await js("document.querySelector('.remote-composer form').requestSubmit()");
  await waitFor(() => text.includes(`PHONE_PID:${pid}`), "phone sends to the original shell");
  console.log("remote smoke: composer reached original PTY");
  await delay(300);
  for (const [width, height] of [[390, 844], [430, 932], [844, 390]]) {
    window.setContentSize(width, height); await delay(200);
    assert.equal(await js("document.documentElement.scrollWidth <= innerWidth"), true, "page must not overflow horizontally");
    assert.equal(await js("document.querySelector('.remote-composer').getBoundingClientRect().bottom <= innerHeight + 1"), true, "composer must remain visible");
    await shot(`terminal-${width}`);
  }
  window.setContentSize(390, 844);
  console.log("remote smoke: screenshots captured; testing reconnect");
  const socketsBefore = await js("window.remoteSockets.length");
  await js("window.remoteSockets.at(-1).close()");
  desktop("printf 'RECONNECTED_PID:%s\\n' \"$$\"\n");
  await waitFor(() => text.includes(`RECONNECTED_PID:${pid}`), "output during phone disconnect");
  await waitFor(() => js(`window.remoteSockets.length > ${socketsBefore} && window.remoteSockets.at(-1).readyState === 1`), "phone reconnect");
  assert.ok(await js(`Number(new URL(window.remoteSockets.at(-1).url).searchParams.get('cursor')) > 0`), "reconnect resumes a byte cursor");
  await waitFor(() => js(`document.querySelector('.xterm-rows')?.textContent.includes('RECONNECTED_PID:${pid}')`), "reconnected bytes rendered");
  console.log("remote smoke: reconnect passed");
  // Lose the response after the server accepts input. The actual UI must keep
  // the ambiguous command blocked across reload instead of repeating it.
  await click("Renew control");
  await waitFor(() => js("document.querySelector('.remote-control-bar button').textContent === 'Renew control' && !document.querySelector('.remote-control-bar button').disabled"), "control renewed");
  await js(`window.originalFetch = window.fetch; window.fetch = async (...args) => { const response = await window.originalFetch(...args); if (String(args[0]).endsWith('/input')) { window.fetch = window.originalFetch; throw new TypeError('Fixture lost the response'); } return response; }; void 0;`);
  await fill(".remote-composer textarea", "printf z >> uncertain; printf 'UNKNOWN_ONCE\\n'");
  await js("document.querySelector('.remote-composer form').requestSubmit()");
  await waitFor(() => has("The last input may have arrived"), "unknown outcome warning");
  await waitFor(async () => { try { return await fs.readFile(path.join(workspace, "uncertain"), "utf8") === "z"; } catch { return false; } }, "unknown input arrived once");
  // Session storage disappears on app termination. The persistent opaque
  // marker must retain uncertainty without retaining the submitted text.
  await js('sessionStorage.clear()');
  window.reload();
  await waitFor(() => has("Remote companion"), "session list after reload");
  console.log("remote smoke: opening terminal");
  await js("document.querySelector('.remote-session').click()");
  await waitFor(() => has("The last input may have arrived"), "unknown receipt survives reload");
  assert.equal(await js("document.querySelector('.remote-send').disabled"), true);
  await waitFor(() => js("document.querySelector('.xterm-rows')?.textContent.includes('UNKNOWN_ONCE')"), "reloaded terminal displays the uncertain effect");
  await shot("unknown-outcome-390");
  const device = (await manage({ action: "status" })).devices[0];
  await manage({ action: "revoke", id: device.id });
  assert.equal((await fetch(`${url}/api/v1/terminals`, { headers: auth })).status, 401);
  await waitFor(() => has("Connect iPhone"), "revoked phone returns to pairing");
  // Native trusted access uses the same Rust gateway and survives a real host restart.
  const trust = command => manage({ action: 'trusted', version: 3, ...command });
  const phone = { version: 3, id: randomUUID(), secret: require('node:crypto').randomBytes(32).toString('hex') };
  const native = (route, body, grant) => fetch(`${url}/api/v3/${route}`, { method: 'POST', headers: { Origin: url, 'Content-Type': 'application/json', ...(grant ? {Cookie:grant} : {}) }, body: JSON.stringify(body) });
  const enrollment = await (await native('enrollment', {...phone, name:'Fixture iPhone'})).json();
  assert.equal(enrollment.approved, false);
  assert.equal((await native('unlock',phone)).status,401);
  const pending = await trust({command:'status'});
  assert.equal(pending.pending[0].verification,enrollment.verification);
  assert.equal(JSON.stringify(pending).includes(phone.secret),false);
  await trust({command:'approve',id:phone.id});
  assert.equal((await (await native('enrollment/status',phone)).json()).approved,true);
  const unlocked = await native('unlock',phone); assert.equal(unlocked.status,200);
  const nativeCookie = unlocked.headers.get('set-cookie').split(';')[0];
  assert.ok(unlocked.headers.get('set-cookie').includes('HttpOnly'));
  assert.ok(!unlocked.headers.get('set-cookie').includes('Max-Age'));
  assert.equal((await fetch(`${url}/api/v2/workspaces`,{headers:{Cookie:nativeCookie}})).status,200);
  // Native routing must show the shared session directly and fence stale generations.
  await window.webContents.session.cookies.set({url, name:'interface_remote',value:nativeCookie.split('=')[1],httpOnly:true,sameSite:'strict'});
  await window.loadURL(`${url}/?native=1&terminal=${chatTerminalId}&generation=1`);
  await waitFor(() => has('The phone shares the same conversation view'), 'native shared session');
  assert.equal(await js("document.querySelector('.remote-back') === null"),true);
  assert.equal(await js("document.querySelector('.mobile-view-switch') === null && document.querySelector('.remote-control-bar') === null && document.querySelector('.remote-keys') === null"),true,'native conversation has no terminal UI');
  assert.equal(await js("document.querySelector('.remote-terminal-scroll').inert && getComputedStyle(document.querySelector('.remote-terminal-scroll')).visibility === 'hidden'"),true);
  await waitFor(() => js("document.querySelector('.phone-composer textarea') !== null"), 'phone message composer');
  await fill('.phone-composer textarea', 'A reply from the refined phone');
  await waitFor(() => js("!document.querySelector('.remote-send').disabled"), 'one-step reply enabled');
  await js("document.querySelector('.phone-composer form').requestSubmit()");
  await waitFor(async () => (await fs.readFile(path.join(workspace,'phone-replies'),'utf8').catch(() => '')).includes('A reply from the refined phone'), 'refined reply reached original CLI');
  for (const [width,height] of [[390,844],[430,932],[375,440],[844,390]]) {
    window.setContentSize(width,height); await delay(200);
    assert.equal(await js('document.documentElement.scrollWidth <= innerWidth'),true);
    assert.equal(await js("document.querySelector('.phone-composer').getBoundingClientRect().bottom <= innerHeight + 1"),true);
    assert.equal(await js("parseFloat(getComputedStyle(document.querySelector('.conv-markdown')).fontSize) >= 16"),true);
    await shot(`native-refined-${width}-${height}`);
  }
  window.setContentSize(390,844);
  const draftBeforeApproval = 'Draft kept while reviewing\nSecond line\nThird line\nFourth line';
  await fill('.phone-composer textarea', draftBeforeApproval);
  assert.equal(await js("document.querySelector('.phone-composer textarea').getBoundingClientRect().height > 70"), true, 'multiline reply expands');
  // Emit a complete standard Claude permission dialog through the original PTY.
  // The fixture executable never runs a provider or performs the displayed command.
  const approvalScreen = '\x1b[2J\x1b[HBash command\r\n  printf hello\r\nDo you want to proceed?\r\n❯ 1. Yes\r\n  2. Yes, and don\x27t ask again\r\n  3. No\r\nEnter to confirm · Esc to cancel';
  // Disposable CLI file is read on the next input; no user profile or live session.
  const chatWrite = data => transport.send({terminalId:chatTerminalId,generation:1n,sequence:0n,direction:TerminalDataDirection.ElectronToHost,kind:TerminalDataKind.Data,payload:Buffer.from(data)});
  await fs.writeFile(path.join(workspace,'next-screen'),approvalScreen);
  chatWrite('\n');
  await waitFor(() => has('Needs your approval'),'refined permission card');
  assert.equal(await js("document.querySelector('.phone-approval pre').textContent.includes('printf hello')"),true);
  assert.equal(await js("getComputedStyle(document.querySelector('.phone-reply-form')).display === 'none'"), true, 'approval replaces reply controls');
  assert.equal(await js("document.querySelector('.phone-composer textarea').value"), draftBeforeApproval, 'approval preserves draft');
  await shot('native-approval-390');
  window.setContentSize(375,440); await delay(200);
  assert.equal(await js("document.querySelector('.phone-composer').getBoundingClientRect().bottom <= innerHeight + 1"),true,'approval and keyboard-sized viewport fit');
  await shot('native-approval-keyboard-375');
  window.setContentSize(390,844); await delay(200);
  const beforeApproval = await fs.readFile(path.join(workspace,'phone-replies'),'utf8');
  await click('Allow once');
  await waitFor(async () => (await fs.readFile(path.join(workspace,'phone-replies'),'utf8')).length > beforeApproval.length,'single-use approval reached original CLI');
  await waitFor(() => js("document.querySelector('.phone-approval') === null"),'resolved approval removed');
  assert.equal(await js("document.querySelector('.phone-composer textarea').value"), draftBeforeApproval, 'resolved approval restores draft');
  assert.notEqual(await js("getComputedStyle(document.querySelector('.phone-reply-form')).display"), 'none');
  await window.webContents.debugger.sendCommand('Emulation.setEmulatedMedia', {features:[{name:'prefers-reduced-motion',value:'reduce'}]});
  assert.equal(await js("matchMedia('(prefers-reduced-motion: reduce)').matches"),true);
  await window.loadURL(`${url}/?native=1&terminal=${terminalId}&generation=1`);
  await waitFor(() => has('This session lives on your Mac'),'unsupported session explanation');
  assert.equal(await js("document.querySelector('.remote-composer') === null && document.querySelector('.mobile-view-switch') === null"),true);
  await shot('native-unsupported-390');
  await window.loadURL(`${url}/?native=1&terminal=${chatTerminalId}&generation=2`);
  await waitFor(() => has('This session has changed or closed'), 'native stale generation refusal');
  await native('lock',{},nativeCookie);
  assert.equal((await fetch(`${url}/api/v2/workspaces`,{headers:{Cookie:nativeCookie}})).status,401);
  await host.stop(10000); host = new RustHostClient(hostOptions); await host.start();
  const restored = await manage({action:'status'}); assert.equal(restored.enabled,true); assert.equal(restored.localUrl,url);
  assert.equal((await fetch(`${url}/api/v2/workspaces`,{headers:{Cookie:nativeCookie}})).status,401);
  const renewed = await native('unlock',phone); assert.equal(renewed.status,200);
  const renewedCookie = renewed.headers.get('set-cookie').split(';')[0];
  await trust({command:'forget',id:phone.id});
  assert.equal((await native('unlock',phone)).status,401);
  assert.equal((await fetch(`${url}/api/v2/workspaces`,{headers:{Cookie:renewedCookie}})).status,401);
  await manage({ action: "disable" });
  assert.equal((await manage({ action: "status" })).enabled, false);
  await assert.rejects(fetch(url));
  await host.stop(10000); host = new RustHostClient(hostOptions); await host.start();
  assert.equal((await manage({action:"status"})).enabled,false);
  const connection = command => manage({action:'connection',version:5,...command});
  assert.equal((await connection({command:'status'})).enabled,false);
  await assert.rejects(manage({action:'connection',version:99,command:'enable'}));
  const direct = await connection({command:'enable',relayOrigin:null});
  assert.equal(direct.enabled,true); assert.match(direct.macId,/^[a-f0-9]{64}$/);
  assert.equal(direct.relayOnline,false);
  const invitation = await connection({command:'pair'});
  assert.equal(invitation.invitation.macId,direct.macId);
  assert.equal(invitation.invitation.profile,'development');
  assert.ok(invitation.qrRows.length > 20);
  assert.equal(JSON.stringify(invitation).includes('privateKey'),false);
  await assert.rejects(manage({action:'pairDirect',version:5}));
  await assert.rejects(manage({action:'pairDirect',version:6,origin:'http://10.0.0.1:5000'}));
  const routed = await manage({action:'pairDirect',version:6});
  assert.equal(routed.version,6); assert.equal(routed.invitation.version,6);
  assert.equal(routed.invitation.invitation.macId,direct.macId);
  assert.equal(routed.invitation.invitation.version,5);
  assert.ok(routed.qrRows.length > 20);
  const listener = JSON.parse(await fs.readFile(path.join(data,'phone-listener.v1.json'),'utf8'));
  assert.equal(listener.version,1); assert.ok(listener.port >= 1024);
  for (const origin of routed.invitation.nearbyOrigins) assert.equal(new URL(origin).port,String(listener.port));
  await host.stop(10000); host = new RustHostClient(hostOptions); await host.start();
  const directRestored = await connection({command:'status'});
  assert.equal(directRestored.enabled,true); assert.equal(directRestored.macId,direct.macId);
  assert.deepEqual(JSON.parse(await fs.readFile(path.join(data,'phone-listener.v1.json'),'utf8')),listener);
  await connection({command:'disable'});
  assert.equal((await manage({action:'status'})).enabled,false);
  await host.stop(10000); host = new RustHostClient(hostOptions); await host.start();
  assert.equal((await connection({command:'status'})).enabled,false);
  assert.equal((await manage({action:'status'})).enabled,false);
  console.log('remote smoke: v5 host identity, QR, restart and persistent disable passed');
  assert.equal(await fs.readFile(path.join(workspace, "count"), "utf8"), "x");
  assert.equal(await fs.readFile(path.join(workspace, "uncertain"), "utf8"), "z");
  console.log(JSON.stringify({ remoteSmoke: "passed", originalPtyPreserved: true, checks: ["shared-draft-cas", "result-generation-fence", "refined-native-view", "one-step-native-reply", "approval-v4-original-pty", "unsupported-conversation", "keyboard-sized-layout", "reduced-motion", "native-approval-v3", "native-shared-view", "native-generation-fence", "trusted-host-restart", "remembered-listener", "lock", "forget", "persistent-disable", "workspaces-v2", "provider-history-v2", "shared-conversation", "pairing", "host-origin", "httpOnly", "idempotency", "desktop-takeover", "stale-generation", "body-limit", "phone-input", "reconnect-cursor", "unknown-outcome-reload", "revocation", "shutdown"], artifacts: output }));
}
run().then(() => finish(0), async error => {
  console.error(error.stack);
  if (output && window && !window.isDestroyed()) {
    await fs.writeFile(path.join(output,'failure.png'),(await window.webContents.capturePage()).toPNG());
    console.error(JSON.stringify({artifacts:output, screen:await window.webContents.executeJavaScript('document.body.innerText')}));
  }
  void finish(1);
});
async function finish(code) {
  clearTimeout(deadline);
  if (window && !window.isDestroyed()) window.destroy();
  if (host) await host.stop();
  if (temporary) await fs.rm(temporary, { recursive: true, force: true });
  app.exit(code);
}
