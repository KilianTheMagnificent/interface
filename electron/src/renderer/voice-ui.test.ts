import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { VoiceEvent } from "../shared/ipc.js";
import { AcpProfilesEditor } from "./components/AcpProfilesEditor.js";
import { RemoteAccessPanel } from "./components/RemoteAccessPanel.js";
import { ThreadSecretCard } from "./components/ThreadSecretCard.js";
import {
  ACT_OK, GUARDED_MESSAGE, GUARD_ATTR, REDACT_ATTR, STALE_ID_MESSAGE, act, focusPane, mergeVoiceEvent,
  nextVoiceState, voiceMeterBars,
  openProject, runUiCall, snapshot,
} from "./voice-ui.js";

// The renderer suite has no live DOM and this project adds no jsdom, so the
// fixture is the subset of the element interface voice-ui actually touches.
// That subset is the point: anything wider would be a second DOM to maintain.
class FakeElement {
  readonly tagName: string;
  readonly attributes = new Map<string, string>();
  readonly children: FakeElement[] = [];
  readonly events: string[] = [];
  own = "";
  focused = 0;
  clicks = 0;

  constructor(tagName: string, attributes: Record<string, string> = {},
      children: FakeElement[] = [], own = "") {
    this.tagName = tagName;
    for (const [name, value] of Object.entries(attributes)) this.attributes.set(name, value);
    this.children.push(...children);
    this.own = own;
  }

  get textContent(): string {
    return [this.own, ...this.children.map(child => child.textContent)].join(" ");
  }

  getAttribute(name: string): string | null { return this.attributes.get(name) ?? null; }
  setAttribute(name: string, value: string): void { this.attributes.set(name, value); }
  hasAttribute(name: string): boolean { return this.attributes.has(name); }
  focus(): void { this.focused += 1; }
  click(): void { this.clicks += 1; }
  dispatchEvent(event: Event): boolean { this.events.push(event.type); return true; }
}

/** An input whose `value` accessor lives on the PROTOTYPE, as a real one's does. */
class FakeInput extends FakeElement {
  #value = "";
  get value(): string { return this.#value; }
  set value(next: string) { this.#value = next; }
  /** The field as the element itself holds it, past any instance-level shadow. */
  readValue(): string { return this.#value; }
}

function documentOf(body: FakeElement): Document {
  return { body } as unknown as Document;
}

function element(tagName: string, attributes: Record<string, string> = {},
    children: FakeElement[] = [], own = ""): FakeElement {
  return new FakeElement(tagName, attributes, children, own);
}

function fixture(): { body: FakeElement; composer: FakeInput; tab: FakeElement } {
  const composer = new FakeInput("INPUT", { placeholder: "Message Claude" });
  const tab = element("BUTTON", { role: "tab", "aria-selected": "true" }, [], "Hegemon");
  const body = element("BODY", {}, [
    element("HEADER", { class: "workspace-toolbar" }, [
      element("BUTTON", { "aria-label": "Voice assistant", "aria-pressed": "true" }),
      element("NAV", { role: "tablist", "aria-label": "Workspace" }, [tab]),
    ]),
    element("DIV", { "aria-hidden": "true" }, [
      element("BUTTON", { "aria-label": "Behind the open dialog" }),
    ]),
    composer,
    element("SECTION", {
      class: "terminal-view",
      "aria-label": "Claude Code — Hegemon terminal",
    }, [
      element("DIV", { class: "xterm-rows" }, [
        element("DIV", {}, [], "$ cargo test"),
        element("DIV", {}, [], "ok"),
        element("DIV", {}, [], ""),
      ]),
    ]),
  ]);
  return { body, composer, tab };
}

describe("voice UI snapshot", () => {
  // FIRST snapshot in this file, so the ids are v1.. — the counter only ever
  // goes up, which is what makes a stale id refusable below.
  it("writes one line per reachable element, with the terminal's visible tail", () => {
    const { body } = fixture();
    expect(snapshot(documentOf(body))).toBe([
      '[v1] button "Voice assistant" pressed',
      '[v2] tablist "Workspace"',
      '  [v3] tab "Hegemon" selected',
      '[v4] textbox "Message Claude" value=""',
      '[v5] terminal "Claude Code — Hegemon terminal"',
      "    $ cargo test",
      "    ok",
    ].join("\n"));
  });

  it("stamps every reported element with the id it was given", () => {
    const { body, tab } = fixture();
    snapshot(documentOf(body));
    expect(tab.getAttribute("data-voice-id")).toBe("v8");
  });
});

describe("voice UI act", () => {
  it("refuses an id the newest snapshot did not hand out", () => {
    const { body } = fixture();
    const stale = /\[(v\d+)\] button/u.exec(snapshot(documentOf(body)))?.[1] ?? "";
    expect(stale).not.toBe("");
    snapshot(documentOf(fixture().body));
    expect(act(stale, "click")).toEqual({ ok: false, message: STALE_ID_MESSAGE });
  });

  it("types through the prototype setter React's change tracking listens to", () => {
    const { body, composer } = fixture();
    const lines = snapshot(documentOf(body)).split("\n");
    const id = /\[(v\d+)\] textbox/u.exec(lines.join("\n"))?.[1] ?? "";
    // React shadows `value` on the INSTANCE with its own tracker: assigning
    // through that is exactly the write the element never sees.
    let tracked = "";
    Object.defineProperty(composer, "value", {
      configurable: true,
      get: () => tracked,
      set: (next: string) => { tracked = next; },
    });
    expect(act(id, "type", "run the tests")).toEqual({ ok: true });
    expect(composer.readValue()).toBe("run the tests");
    expect(tracked).toBe("");
    expect(composer.events).toEqual(["input"]);
    expect(composer.focused).toBe(1);
  });

  it("refuses to type into anything that is not a text field", () => {
    const { body } = fixture();
    const id = /\[(v\d+)\] button/u.exec(snapshot(documentOf(body)))?.[1] ?? "";
    expect(act(id, "type", "hello"))
      .toEqual({ ok: false, message: "type only works on an input or textarea" });
  });
});

describe("voice UI approval guard", () => {
  function guardedFixture(): { body: FakeElement; allow: FakeElement; other: FakeElement } {
    const allow = element("BUTTON", {}, [], "Allow");
    const other = element("BUTTON", {}, [], "Open");
    const body = element("BODY", {}, [
      element("DIV", { [GUARD_ATTR]: "approval" }, [
        element("DIV", {}, [allow]),
      ]),
      other,
    ]);
    return { body, allow, other };
  }

  it("is the attribute the approval components actually write", () => {
    expect(GUARD_ATTR).toBe("data-voice-guard");
  });

  it("reports approval controls as guarded and refuses to click them", () => {
    const { body, allow } = guardedFixture();
    const doc = documentOf(body);
    const lines = snapshot(doc);
    // Seen, so the assistant can say what the user is being asked; not usable.
    expect(lines).toContain('button "Allow" guarded');
    const id = /\[(v\d+)\] button "Allow"/u.exec(lines)?.[1] ?? "";
    expect(act(id, "click")).toEqual({ ok: false, message: GUARDED_MESSAGE });
    expect(runUiCall(doc, "ui_act", { id, action: "click" }))
      .toEqual({ ok: false, output: GUARDED_MESSAGE });
    expect(allow.clicks).toBe(0);
    expect(allow.focused).toBe(0);
  });

  it("refuses every action on a guarded target, not only the click", () => {
    const { body } = guardedFixture();
    const id = /\[(v\d+)\] button "Allow"/u.exec(snapshot(documentOf(body)))?.[1] ?? "";
    for (const action of ["focus", "key", "type"] as const) {
      expect(act(id, action, "Enter")).toEqual({ ok: false, message: GUARDED_MESSAGE });
    }
  });

  it("leaves the rest of the document clickable", () => {
    const { body, other } = guardedFixture();
    const doc = documentOf(body);
    const id = /\[(v\d+)\] button "Open"/u.exec(snapshot(doc))?.[1] ?? "";
    expect(runUiCall(doc, "ui_act", { id, action: "click" }))
      .toEqual({ ok: true, output: ACT_OK });
    expect(other.clicks).toBe(1);
  });
});

describe("voice indicator frames", () => {
  const frame = (over: Partial<VoiceEvent>): VoiceEvent => ({
    state: "listening",
    session_key: null,
    cwd: null,
    partial: null,
    level: null,
    message: null,
    microphone: null,
    asset: null,
    ...over,
  });

  it("treats a null field as unchanged while the state holds", () => {
    const heard = mergeVoiceEvent(null, frame({ partial: "run the tests", level: 0.4 }));
    // A level frame between two partials must not blank the words…
    const louder = mergeVoiceEvent(heard, frame({ level: 0.7 }));
    expect(louder.partial).toBe("run the tests");
    expect(louder.level).toBe(0.7);
    // …and a partial must not empty the meter.
    const longer = mergeVoiceEvent(louder, frame({ partial: "run the tests now" }));
    expect(longer.partial).toBe("run the tests now");
    expect(longer.level).toBe(0.7);
  });

  it("keeps a notify message on the frames that follow it", () => {
    const notified = mergeVoiceEvent(frame({ state: "thinking" }),
      frame({ state: "thinking", message: "Build finished" }));
    expect(mergeVoiceEvent(notified, frame({ state: "thinking" })).message)
      .toBe("Build finished");
  });

  it("clears the last utterance when the state changes", () => {
    const listening = mergeVoiceEvent(null,
      frame({ partial: "hello", level: 0.5, message: "Build finished" }));
    expect(mergeVoiceEvent(listening, frame({ state: "thinking" })))
      .toMatchObject({ state: "thinking", partial: null, level: null, message: null });
  });

  it("takes every other field from the frame that just arrived", () => {
    const first = mergeVoiceEvent(null, frame({ session_key: "claude#s1", cwd: "/work" }));
    expect(mergeVoiceEvent(first, frame({ state: "error", message: "Microphone denied" })))
      .toMatchObject({ state: "error", session_key: null, message: "Microphone denied" });
  });

  it("keeps the same state for level frames that light the same bars", () => {
    const heard = nextVoiceState(null, frame({ partial: "run", level: 0.3 }));
    expect(voiceMeterBars(0.3)).toBe(2);
    // 10 Hz level frames inside one bar must not re-render the app.
    expect(nextVoiceState(heard, frame({ level: 0.35 }))).toBe(heard);
    expect(nextVoiceState(heard, frame({}))).toBe(heard);
    const louder = nextVoiceState(heard, frame({ level: 0.9 }));
    expect(louder).not.toBe(heard);
    expect(louder).toMatchObject({ partial: "run", level: 0.9 });
    expect(nextVoiceState(louder, frame({ level: 0.95, partial: "run it" }))).toMatchObject({
      partial: "run it" });
    expect(nextVoiceState(louder, frame({ level: 0.9, state: "thinking" }))).toMatchObject({
      state: "thinking" });
    expect(nextVoiceState(louder, frame({ level: 0.9, microphone: "denied" }))).not.toBe(louder);
    expect(nextVoiceState(louder, frame({ level: 0.9, asset: { status: "downloading", progress: 0.5 } })))
      .not.toBe(louder);
  });
});

describe("voice UI selection tools", () => {
  it("opens a collapsed project and leaves an open one open", () => {
    const collapsed = element("BUTTON",
      { class: "group-header", title: "/work/iface", "aria-expanded": "false" });
    const open = element("BUTTON",
      { class: "group-header", title: "/work/other", "aria-expanded": "true" });
    const doc = documentOf(element("BODY", {}, [collapsed, open]));
    expect(openProject(doc, "/work/iface")).toEqual({ ok: true });
    expect(collapsed.clicks).toBe(1);
    expect(openProject(doc, "/work/other")).toEqual({ ok: true });
    expect(open.clicks).toBe(0);
    expect(openProject(doc, "/work/missing"))
      .toEqual({ ok: false, message: "no project row for /work/missing" });
  });

  it("focuses a pane by clicking its tab, and says so when there is none", () => {
    const tab = element("BUTTON", { class: "terminal-tab", "data-terminal-id": "t1" });
    const doc = documentOf(element("BODY", {}, [tab]));
    expect(focusPane(doc, "t1")).toEqual({ ok: true });
    expect(tab.clicks).toBe(1);
    expect(focusPane(doc, "t9")).toEqual({ ok: false, message: "no pane t9" });
  });
});

describe("voice UI call dispatch", () => {
  it("returns each tool's result text verbatim", () => {
    const doc = documentOf(fixture().body);
    expect(runUiCall(doc, "ui_snapshot", {}).output).toContain('button "Voice assistant"');
    expect(runUiCall(doc, "ui_act", { id: "v0", action: "click" }))
      .toEqual({ ok: false, output: STALE_ID_MESSAGE });
    expect(runUiCall(doc, "focus_pane", { pane_id: "nope" }))
      .toEqual({ ok: false, output: "no pane nope" });
    expect(runUiCall(doc, "ui_screenshot", {}))
      .toEqual({ ok: false, output: "unknown voice UI tool: ui_screenshot" });
  });

  it("answers a real act with the exact body the host wraps as text", () => {
    const { body, tab } = fixture();
    const doc = documentOf(body);
    const id = /\[(v\d+)\] tab "/u.exec(snapshot(doc))?.[1] ?? "";
    expect(runUiCall(doc, "ui_act", { id, action: "click" }))
      .toEqual({ ok: true, output: ACT_OK });
    expect(tab.clicks).toBe(1);
  });
});

/** Minimal HTML-to-FakeElement parser for static React markup in tests. */
function fakeDomFromHtml(html: string): FakeElement {
  const root = element("BODY");
  const stack: FakeElement[] = [root];
  const voidTags = new Set(["AREA", "BASE", "BR", "COL", "EMBED", "HR", "IMG", "INPUT", "LINK", "META", "SOURCE", "TRACK", "WBR"]);
  const tokenPattern = /<!--[\s\S]*?-->|<\/([a-zA-Z0-9:-]+)\s*>|<([a-zA-Z0-9:-]+)((?:\s+[^\s/>=]+(?:="[^"]*")?)*)\s*(\/?)>|([^<]+)/gu;
  for (const match of html.matchAll(tokenPattern)) {
    const [, closing, opening, rawAttrs = "", selfClosing, text] = match;
    if (closing !== undefined) {
      if (stack.length > 1) stack.pop();
      continue;
    }
    if (opening !== undefined) {
      const tag = opening.toUpperCase();
      if (tag === "STYLE" || tag === "SCRIPT") {
        if (selfClosing !== "/" && !voidTags.has(tag)) stack.push(element(tag, { hidden: "" }));
        continue;
      }
      const attrs: Record<string, string> = {};
      for (const attr of rawAttrs.matchAll(/([^\s/>=]+)(?:="([^"]*)")?/gu)) {
        attrs[attr[1]!] = (attr[2] ?? "").replace(/&quot;/gu, '"').replace(/&#x27;/gu, "'").replace(/&amp;/gu, "&");
      }
      const node = tag === "INPUT" || tag === "TEXTAREA" ? new FakeInput(tag, attrs) : element(tag, attrs);
      if (node instanceof FakeInput && attrs.value !== undefined) node.value = attrs.value;
      stack.at(-1)!.children.push(node);
      if (selfClosing !== "/" && !voidTags.has(tag)) stack.push(node);
      continue;
    }
    if (text !== undefined && stack.at(-1)?.tagName !== "STYLE") {
      const decoded = text.replace(/&quot;/gu, '"').replace(/&#x27;/gu, "'").replace(/&rsquo;/gu, "’").replace(/&amp;/gu, "&");
      if (decoded.trim() !== "") stack.at(-1)!.own = [stack.at(-1)!.own, decoded].filter(Boolean).join(" ");
    }
  }
  return root;
}

describe("voice UI secret redaction and guarded settings", () => {
  it("omits password input values and data-voice-redact text and values from snapshots", () => {
    expect(REDACT_ATTR).toBe("data-voice-redact");
    const password = new FakeInput("INPUT", { type: "password", placeholder: "STRIPE_TEST_KEY" });
    password.value = "sk_live_super_secret_999";
    const upperPassword = new FakeInput("INPUT", { type: "PASSWORD", "aria-label": "Database password" });
    upperPassword.value = "hunter2-db-secret";
    const redactedInput = new FakeInput("INPUT", { placeholder: "Pairing token" });
    redactedInput.value = "pair-bearer-777";
    const plainInput = new FakeInput("INPUT", { placeholder: "Search" });
    plainInput.value = "visible query";
    const regionWithNodes = Object.assign(
      element("DIV", { role: "region" }, [
        element("CODE", { class: "remote-pairing-code", [REDACT_ATTR]: "" }, [], "ABCD-EFGH"),
        element("P", {}, [], "Enter this code in Safari within five minutes."),
      ]),
      {
        childNodes: [
          { nodeType: 1, nodeValue: null, hasAttribute: (n: string) => n === REDACT_ATTR, children: [] },
          { nodeType: 3, nodeValue: "Enter this code in Safari within five minutes." },
        ],
      },
    );
    const body = element("BODY", {}, [
      password,
      upperPassword,
      element("DIV", { [REDACT_ATTR]: "" }, [
        redactedInput,
        element("BUTTON", {}, [], "9999-SECRET-CODE"),
      ]),
      element("SECTION", { role: "region" }, [
        element("CODE", { class: "remote-pairing-code", [REDACT_ATTR]: "" }, [], "WXYZ-1234"),
        element("P", {}, [], "It works once."),
      ]),
      regionWithNodes,
      plainInput,
    ]);

    const out = snapshot(documentOf(body));
    expect(out).not.toContain("sk_live_super_secret_999");
    expect(out).not.toContain("hunter2-db-secret");
    expect(out).not.toContain("pair-bearer-777");
    expect(out).not.toContain("9999-SECRET-CODE");
    expect(out).not.toContain("ABCD-EFGH");
    expect(out).not.toContain("WXYZ-1234");
    expect(out).toContain('textbox "STRIPE_TEST_KEY"');
    expect(out).toContain('textbox "Database password"');
    expect(out).toContain('textbox "Pairing token"');
    expect(out).toContain('button ""');
    expect(out).toContain('region "It works once."');
    expect(out).toContain('region "Enter this code in Safari within five minutes."');
    expect(out).toContain('textbox "Search" value="visible query"');
  });

  it("guards ThreadSecretCard, RemoteAccessPanel and AcpProfilesEditor controls from voice actions", () => {
    const secretHtml = renderToStaticMarkup(createElement(ThreadSecretCard, {
      request: { id: "sec-1", name: "OPENAI_API_KEY", reason: "Run integration suite", agent: "Claude" },
      onGive: async () => null,
      onDecline: () => undefined,
    }));
    expect(secretHtml).toContain('data-voice-guard="secret"');

    const remoteHtml = renderToStaticMarkup(createElement(RemoteAccessPanel, {
      id: "remote",
      manage: async () => ({ version: 1 as const, enabled: false, localUrl: "", publicUrl: null, devices: [] }),
    }));
    expect(remoteHtml).toContain('data-voice-guard="settings"');

    const acpHtml = renderToStaticMarkup(createElement(AcpProfilesEditor, {
      profiles: [{ slug: "custom", displayName: "Custom", executablePath: "/usr/local/bin/custom", args: ["stdio"] }],
      onSave: () => undefined,
    }));
    expect(acpHtml).toContain('data-voice-guard="settings"');

    const combined = fakeDomFromHtml(secretHtml + remoteHtml + acpHtml);
    // Seed a live secret into the rendered ThreadSecretCard password field before snapshotting.
    const findPassword = (node: FakeElement): FakeInput | undefined => {
      if (node instanceof FakeInput && node.getAttribute("type") === "password") return node;
      for (const child of node.children) {
        const found = findPassword(child);
        if (found !== undefined) return found;
      }
      return undefined;
    };
    const secretInput = findPassword(combined);
    expect(secretInput).toBeDefined();
    secretInput!.value = "sk-proj-never-leak-this";

    const lines = snapshot(documentOf(combined));
    expect(lines).not.toContain("sk-proj-never-leak-this");
    const ids = [...lines.matchAll(/\[(v\d+)\]/gu)].map(match => match[1]!);
    expect(ids.length).toBeGreaterThanOrEqual(8);
    for (const line of lines.split("\n")) {
      expect(line, line).toContain("guarded");
    }
    for (const id of ids) {
      expect(act(id, "click")).toEqual({ ok: false, message: GUARDED_MESSAGE });
      expect(act(id, "type", "injected")).toEqual({ ok: false, message: GUARDED_MESSAGE });
    }
  });
});
