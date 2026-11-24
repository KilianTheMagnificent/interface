"use strict";
// The browser-point renderer lane: point and fix in the Browser pane against a
// real http page in a real <webview>, pinned by the app's own main-process
// guards (no preload, sandboxed, every permission refused). This process
// serves the page on two loopback origins (so its embedded frame is
// cross-origin) and drives the native pointer and keyboard; the renderer half
// is src/renderer/fixtures/browser-point-fixture.tsx.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const http = require("node:http");
const path = require("node:path");
const { pathToFileURL } = require("node:url");
const { session } = require("electron");

const PAGE = port => `<!doctype html><html><head><meta charset="utf-8"><title>Point fixture</title>
<style>
  body { margin: 0; font: 15px/1.4 -apple-system, sans-serif; background: #f6f6f4; color: #222; }
  header { height: 44px; padding: 0 16px; display: flex; align-items: center; background: #24324a; color: #fff; }
  #save { position: absolute; left: 40px; top: 72px; width: 120px; height: 32px; border: 0; border-radius: 6px;
    background: #2a8f5c; color: #fff; font: inherit; }
  #docs { position: absolute; left: 190px; top: 78px; }
  #long { position: absolute; left: 40px; top: 124px; width: 520px; height: 42px; margin: 0; overflow: hidden; }
  .menu { position: absolute; left: 40px; top: 186px; margin: 0; padding: 0 0 0 18px; }
  #embed { position: absolute; left: 320px; top: 186px; width: 220px; height: 110px; border: 1px solid #999; }
  .card { position: absolute; left: 40px; top: 320px; width: 300px; height: 90px; border-radius: 8px;
    background: linear-gradient(90deg, #f0b44c, #e2604a); }
</style></head><body>
<header>Acme settings</header>
<button id="save" onclick="window.saveClicks += 1">Save</button>
<a id="docs" href="/second" onclick="window.linkClicks += 1">Docs</a>
<p id="long"></p>
<ul class="menu css-9q8w7e"><li><a class="item" href="/second">One</a></li><li><a class="item" href="/second">Two</a></li></ul>
<iframe id="embed" src="http://localhost:${port}/frame" title="Embedded widget"></iframe>
<div class="card"></div>
<script>
  window.saveClicks = 0; window.linkClicks = 0;
  document.getElementById("long").textContent = "Long\\u202e text\\u0007 " + "word ".repeat(800);
  window.box = selector => { const r = document.querySelector(selector).getBoundingClientRect();
    return { x: r.x, y: r.y, width: r.width, height: r.height }; };
  window.pointLayer = () => { const layer = document.querySelector("[data-interface-point]");
    return layer === null ? null : { mode: layer.getAttribute("data-interface-point"), highlight: layer.getAttribute("data-highlight") }; };
</script></body></html>`;

const FRAME = `<!doctype html><html><body style="margin:0;background:#cfe3ff;font:13px sans-serif">
<button onclick="parent.postMessage('frame-click', '*')">Inside the frame</button></body></html>`;
const SECOND = `<!doctype html><html><head><title>Point fixture</title></head><body><h1>Second page</h1>
<script>window.box = () => null; window.pointLayer = () => { const layer = document.querySelector("[data-interface-point]");
  return layer === null ? null : { mode: layer.getAttribute("data-interface-point") }; };</script></body></html>`;

async function serve() {
  const server = http.createServer((request, response) => {
    response.setHeader("content-type", "text/html; charset=utf-8");
    const port = server.address().port;
    response.end(request.url === "/frame" ? FRAME : request.url === "/second" ? SECOND : PAGE(port));
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  return server;
}

exports.exerciseBrowserPoint = async (window, output) => {
  const { installBrowserPaneGuards, BROWSER_PANE_PARTITION } = await import(
    pathToFileURL(path.join(__dirname, "../dist-main/main/browser-pane.js")).href);
  installBrowserPaneGuards(window.webContents, session.fromPartition(BROWSER_PANE_PARTITION));
  const server = await serve();
  const url = `http://127.0.0.1:${server.address().port}/`;
  const js = code => window.webContents.executeJavaScript(code);
  const call = code => js(`Promise.resolve(${code}).catch(error => { throw new Error(error?.stack ?? String(error)); })`);
  const guest = code => call(`window.browserPointGuest(${JSON.stringify(code)})`);
  const state = () => call("window.browserPointState()");
  const press = name => call(`window.browserPointPress(${JSON.stringify(name)})`);
  const shot = async name => {
    window.webContents.invalidate();
    const file = path.join(output, `${name}.png`);
    await fs.writeFile(file, (await window.webContents.capturePage()).toPNG());
    return file;
  };
  const input = (method, params) => window.webContents.debugger.sendCommand(method, params);
  const mouse = (type, x, y, pressed = false) => input("Input.dispatchMouseEvent", {
    type, x: Math.round(x), y: Math.round(y), button: pressed || type !== "mouseMoved" ? "left" : "none",
    buttons: pressed && type !== "mouseReleased" ? 1 : 0, clickCount: type === "mouseMoved" ? 0 : 1 });
  const click = async (x, y) => {
    await mouse("mouseMoved", x, y);
    await mouse("mousePressed", x, y, true);
    await mouse("mouseReleased", x, y, true);
  };
  const escape = async () => {
    await input("Input.dispatchKeyEvent", { type: "rawKeyDown", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
    await input("Input.dispatchKeyEvent", { type: "keyUp", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
  };
  const until = async (read, ready, message, timeout = 5000) => {
    const deadline = Date.now() + timeout;
    for (;;) {
      const value = await read();
      if (ready(value)) return value;
      assert.ok(Date.now() < deadline, `${message}: ${JSON.stringify(value)}`);
      await new Promise(resolve => setTimeout(resolve, 40));
    }
  };
  const report = {};
  try {
    window.setContentSize(1100, 760);
    const setup = await call(`window.browserPointFixture(${JSON.stringify(url)})`);
    report.selectors = setup.selectors;
    // The card moves the page down while it is open: read where it is each time.
    const at = async selector => {
      const [{ view }, box] = await Promise.all([state(), guest(`window.box(${JSON.stringify(selector)})`)]);
      return { x: view.x + box.x + box.width / 2, y: view.y + box.y + box.height / 2 };
    };

    // The page's own handler works when nothing is picking.
    let save = await at("#save");
    await click(save.x, save.y);
    await until(() => guest("window.saveClicks"), clicks => clicks === 1, "A plain click did not reach the page's button");

    // Pick: hovering highlights the element under the pointer.
    await press("Pick");
    await until(() => guest("window.pointLayer()"), layer => layer?.mode === "element", "Pick put no layer over the page");
    assert.deepEqual((await state()).pressed, { Pick: true, Region: false });
    // Picking moves nothing: the page is where it was before Pick.
    assert.deepEqual(await at("#save"), save, "Pick moved the page");
    assert.equal((await state()).hint.length, 0, "Pick added a line over the page, moving it");
    await mouse("mouseMoved", save.x - 20, save.y);
    await mouse("mouseMoved", save.x, save.y);
    await until(() => guest("window.pointLayer()"), layer => layer?.highlight === "button", "Hovering the button did not highlight it");
    report.highlight = await shot("browser-point-highlight");

    // A click captures it and never reaches the page's button.
    await click(save.x, save.y);
    const picked = await until(state, value => value.card !== null, "The click captured nothing");
    assert.equal(await guest("window.saveClicks"), 1, "The picking click reached the page's own click handler");
    assert.equal(await guest("window.pointLayer()"), null, "The picker's layer stayed on the page after the pick");
    assert.equal(picked.card.summary, "A button “Save”, 120×32");
    assert.equal(picked.card.selector, "#save");
    assert.ok(picked.card.thumb && picked.card.thumb.width >= 136 && picked.card.thumb.height >= 48,
      `The card has no picture of the button: ${JSON.stringify(picked.card.thumb)}`);
    assert.deepEqual(picked.pressed, { Pick: false, Region: false });
    assert.equal(picked.card.addEnabled, false, "Add to draft was offered before a thread was chosen");

    await call("window.browserPointComment('Make this button blue and say Save changes')");
    await call("window.browserPointChoose('Thread for the note', 'Fix the settings header')");
    report.card = await shot("browser-point-card");
    await press("Add to draft");
    const added = await until(state, value => value.card === null, "The card stayed after Add to draft");
    const draft = added.drafts["claude:fixture-header"];
    assert.equal(draft, [
      `In the browser at ${url} ("Point fixture"):`,
      "Element button \"Save\" (selector `#save`, 120×32 at 40,72)",
      "> Make this button blue and say Save changes", "", ""].join("\n"));
    assert.deepEqual(added.attachments, { "claude:fixture-header": ["browser-1.png"] });
    assert.equal(added.images.length, 1);
    assert.ok(added.hint.some(line => line.startsWith("Added to the draft in “Fix the settings header”, with its picture")),
      `No word of where the note went: ${JSON.stringify(added.hint)}`);
    report.draft = draft;

    const picking = () => until(() => guest("window.pointLayer()"), layer => layer?.mode === "element",
      "Pick put no layer over the page");

    // Hostile page text is bounded and cleaned before the card shows it.
    await press("Pick");
    await picking();
    const long = await at("#long");
    await mouse("mouseMoved", long.x, long.y);
    await until(() => guest("window.pointLayer()"), layer => layer?.highlight === "p", "The paragraph was not highlighted");
    await click(long.x, long.y);
    const longCard = (await until(state, value => value.card !== null, "The paragraph was not captured")).card;
    const shown = longCard.summary.slice(longCard.summary.indexOf("“") + 1, longCard.summary.lastIndexOf("”"));
    assert.ok(Array.from(shown).length <= 200 && shown.endsWith("…"), `The page's text was not bounded: ${shown.length}`);
    assert.ok(!/[\u0000-\u001f‮]/u.test(longCard.summary), "Control or reordering characters reached the card");
    assert.equal(longCard.selector, "#long");
    assert.deepEqual(longCard.elements, [], `The card drew page markup: ${JSON.stringify(longCard.elements)}`);
    report.longText = shown.length;
    await press("Dismiss");

    // An element with no id gets a positional selector that names it alone.
    await press("Pick");
    await picking();
    const two = await at(".menu li:nth-of-type(2) a");
    await mouse("mouseMoved", two.x, two.y);
    await until(() => guest("window.pointLayer()"), layer => layer?.highlight === "a", "The link was not highlighted");
    await click(two.x, two.y);
    const linkCard = (await until(state, value => value.card !== null, "The link was not captured")).card;
    assert.equal(await guest(`document.querySelectorAll(${JSON.stringify(linkCard.selector)}).length === 1 && document.querySelector(${JSON.stringify(linkCard.selector)}).textContent`), "Two",
      `The link's selector does not name it alone: ${linkCard.selector}`);
    assert.ok(!linkCard.selector.includes("css-9q8w7e"), `A generated class went into the selector: ${linkCard.selector}`);
    assert.equal(await guest("location.pathname"), "/", "Picking a link followed it");
    report.linkSelector = linkCard.selector;
    await press("Dismiss");

    // A cross-origin frame is out of reach: the card says so and offers Region.
    await press("Pick");
    await picking();
    const frame = await at("#embed");
    await mouse("mouseMoved", frame.x, frame.y);
    await until(() => guest("window.pointLayer()"), layer => layer?.highlight === "iframe", "The frame was not highlighted");
    await click(frame.x, frame.y);
    const frameCard = (await until(state, value => value.card !== null, "The frame was not captured")).card;
    assert.ok(frameCard.summary.includes("can't look inside this embedded frame"), frameCard.summary);
    assert.deepEqual(frameCard.choices, ["Mark a region instead"]);
    await press("Dismiss");

    // Escape cancels and leaves nothing behind in the page.
    await press("Pick");
    await until(() => guest("window.pointLayer()"), layer => layer !== null, "Pick put no layer over the page");
    await escape();
    await until(() => guest("window.pointLayer()"), layer => layer === null, "Escape left the picker's layer on the page");
    const cancelled = await until(state, value => !value.pressed.Pick, "Escape left the pane picking");
    assert.equal(cancelled.card, null, "Escape captured something");

    // Region: drag a rectangle; its picture and box go to the draft.
    await press("Region");
    await until(() => guest("window.pointLayer()"), layer => layer?.mode === "region", "Region put no layer over the page");
    const { view: page } = await state();
    const from = { x: page.x + 40, y: page.y + 320 };
    await mouse("mouseMoved", from.x, from.y);
    await mouse("mousePressed", from.x, from.y, true);
    await mouse("mouseMoved", from.x + 150, from.y + 45, true);
    await mouse("mouseMoved", from.x + 300, from.y + 90, true);
    await until(() => guest("window.pointLayer()"), layer => layer?.highlight === "region", "Dragging drew no region");
    await mouse("mouseReleased", from.x + 300, from.y + 90, true);
    const region = (await until(state, value => value.card !== null, "The region was not captured")).card;
    assert.equal(region.summary, "A region of the page, 300×90");
    assert.ok(region.thumb && region.thumb.width >= 300, `The region has no picture: ${JSON.stringify(region.thumb)}`);
    await call("window.browserPointComment('This card is too loud')");
    await press("Add to draft");
    const second = await until(state, value => value.card === null, "The region card stayed after Add to draft");
    assert.match(second.drafts["claude:fixture-header"],
      /\n\nIn the browser at [^\n]+\nRegion 300×90 at 40,320 of a \d+×\d+ view \(picture attached\)\n> This card is too loud\n\n$/u,
      "The region's note is not after the first one in the draft");
    assert.deepEqual(second.attachments, { "claude:fixture-header": ["browser-1.png", "browser-2.png"] });
    const scale = second.images[1].width / 300;
    assert.ok(scale >= 1 && Math.abs(second.images[1].height - 90 * scale) <= 2,
      `The region's picture is not the dragged rectangle: ${JSON.stringify(second.images[1])}`);
    report.images = second.images;

    // A navigation ends a pick; the new document has no layer.
    await press("Pick");
    await until(() => guest("window.pointLayer()"), layer => layer !== null, "Pick put no layer over the page");
    await guest("location.assign('/second'); true");
    await until(state, value => !value.pressed.Pick, "Navigating left the pane picking");
    await until(() => guest("location.pathname === '/second' && document.readyState === 'complete' && window.pointLayer()"),
      layer => layer === null, "The new page has a picker layer");

    // Viewport presets: Phone lays the page out 390 wide, Fit fills the pane again.
    await call("window.browserPointChoose('Page width', 'Phone 390')");
    await until(() => guest("innerWidth"), width => width === 390, "Phone did not lay the page out 390 wide");
    assert.equal((await state()).viewport, true);
    await guest("history.back(); true");
    await until(() => guest("location.pathname"), pathname => pathname === "/", "Back did not return to the page");
    await until(() => guest("document.readyState === 'complete' && typeof window.box"), type => type === "function", "The page did not load again");
    report.phone = await shot("browser-point-phone");
    await call("window.browserPointChoose('Page width', 'Fit')");
    const { view: fitted } = await state();
    await until(() => guest("innerWidth"), width => width === Math.round(fitted.width), "Fit did not fill the pane again");
    report.final = await shot("browser-point-final");
    return report;
  } finally {
    server.close();
    await js("window.clearBrowserPointFixture()").catch(() => undefined);
  }
};
