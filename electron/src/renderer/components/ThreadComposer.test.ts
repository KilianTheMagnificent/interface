import { readFileSync } from "node:fs";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { Conversation, type ConversationProps } from "./Conversation";
import { createDraftStore } from "../draft-store";
import type { ConvItem } from "../conversation-reducer";

/**
 * THE THREADS COMPOSER (owner-approved redesign, 2026-10-06), as drawn. The
 * thread-composer-fixture DOM lane presses it: the "+" menu's keyboard, the
 * access chip's choice and the verbs' dispatch.
 */

const config: ConversationProps["config"] = {
  engine: "claude",
  engineLabel: "Claude",
  model: "claude-sonnet",
  effort: "high",
  models: [{ value: "claude-sonnet", displayName: "Sonnet 5",
    supportsEffort: true, effortLevels: ["low", "high"] }],
  canSetModel: true,
  effortLevels: ["low", "high"],
  showEffort: true,
  canSetEffort: true,
  pendingModel: false,
  pendingEffort: false,
  permissionMode: "acceptEdits",
  permissionModes: ["default", "acceptEdits", "plan", "bypassPermissions"],
  canSetPermission: true,
  error: null,
};

const diffs: ConvItem[] = [
  { kind: "diff", id: "d1", callId: "c-d1", path: "/tmp/a.ts", added: 12, removed: 4 },
];

const base: ConversationProps = {
  items: [],
  thinking: false,
  live: true,
  sessionKey: "claude#a",
  config,
  threadLayout: true,
  onSetPermissionMode: () => undefined,
  onSend: () => undefined,
  onStop: () => undefined,
  onSetModel: () => undefined,
  onSetEffort: () => undefined,
  onApprove: () => undefined,
  onDeny: () => undefined,
  approvals: {},
  activeApprovalIds: new Set<string>(),
};

const paint = (over: Partial<ConversationProps> = {}): string =>
  renderToStaticMarkup(createElement(Conversation, { ...base, ...over }));
const drafted = (text: string) => createDraftStore({ initial: { "claude#a": text } });
const busy = (over: Partial<ConversationProps> = {}): string => paint({
  thinking: true, onQueue: () => true, canSteer: true, drafts: drafted("Also lint"), ...over,
});
/** The composer's own markup, past the transcript and the dock. */
const composer = (html: string): string =>
  html.slice(html.search(/class="thread-composer(?: is-(?:working|waiting))?"/u));

describe("the Threads composer", () => {
  it("draws one box with the field first, and no dirty pill", () => {
    const html = paint({ items: diffs });
    expect(html).toContain('class="thread-composer"');
    expect(html).not.toContain('class="conv-composer"');
    expect(html).not.toContain("conv-dirty");
    expect(html).not.toContain("1 file changed");
    const box = composer(html);
    expect(box.indexOf("<textarea")).toBeLessThan(box.indexOf("thread-composer-foot"));
    expect(box).toContain('class="thread-composer-input"');
  });

  it("leaves every other pane exactly as it was", () => {
    const html = paint({ items: diffs, threadLayout: false });
    expect(html).toContain('class="conv-composer"');
    expect(html).toContain('class="conv-dirty"');
    expect(html).not.toContain("thread-composer");
    expect(html).toContain("Message this session…   (Enter to send)");
  });

  it("says what the field is for at rest and what Enter does mid-turn", () => {
    expect(paint()).toContain('placeholder="Ask for a change or a follow-up"');
    expect(busy({ drafts: drafted("") }))
      .toContain('placeholder="Steer now, or queue a follow-up"');
    expect(busy({ drafts: drafted(""), canSteer: false }))
      .toContain('placeholder="Queue a follow-up"');
    expect(paint({ sendLocked: true }))
      .toContain('placeholder="Ask for a change or a follow-up"');
  });

  it("leads the row with one + button that holds the controls the old row led with", () => {
    const box = composer(paint({ commands: [{ name: "review", description: "Review" }],
      onRunCommand: () => undefined, context: { used: 24, size: 200, percent: 12 },
      items: [{ kind: "tool", id: "t1", tool: "Read", summary: "a.ts", input: {},
        result: { ok: true, summary: "1 line" } }] }));
    expect(box).toContain('aria-label="Add context, commands and options"');
    expect(box).toContain('aria-haspopup="menu"');
    // The old row's leading controls are not drawn on the row any more.
    expect(box).not.toContain("conv-slash-glyph");
    expect(box).not.toContain("conv-detail");
    expect(box).not.toContain("conv-context-meter");
    expect(box).not.toContain("prompt-context-trigger");
    expect(box.indexOf("Add context, commands and options"))
      .toBeLessThan(box.indexOf('aria-label="Model: Sonnet 5"'));
  });

  it("draws no + at all when there is nothing to put behind it", () => {
    // No context picker, no commands, nothing foldable, no context reading.
    expect(composer(paint())).not.toContain("Add context, commands and options");
  });

  it("names the model by its display name, beside the provider's only mark", () => {
    const box = composer(paint());
    expect(box).toContain('aria-label="Model: Sonnet 5"');
    expect(box).toContain('<span class="app-select-value">Sonnet 5</span>');
    expect(box).not.toContain(">claude-sonnet<");
    expect(box.match(/data-provider-logo=/gu)).toHaveLength(1);
    // No provider label anywhere else in the composer.
    expect(box).not.toContain("Claude commands");
    expect(box).not.toMatch(/>Claude</u);
  });

  it("still names a model it cannot change, with the mark", () => {
    const box = composer(paint({ config: { ...config, canSetModel: false } }));
    expect(box).toContain('class="thread-composer-chip thread-composer-model is-static"');
    expect(box).toContain(">Sonnet 5<");
    expect(box.match(/data-provider-logo=/gu)).toHaveLength(1);
  });

  /* The Code mode canvas (owner, 2026-10-23): access beside the "+", then the
     model and effort at the row's right end before the verbs, with no
     hairlines between them; the shield is the Threads access glyph. */
  it("puts access beside the +, then model and effort at the row's end", () => {
    const box = composer(paint({ commands: [{ name: "review", description: "Review" }],
      onRunCommand: () => undefined }));
    expect(box).toContain('aria-label="Effort: High"');
    expect(box).toContain('aria-label="Permission mode: Accept edits"');
    expect(box).not.toContain("thread-composer-sep");
    expect(box.indexOf("Add context, commands and options"))
      .toBeLessThan(box.indexOf("Permission mode:"));
    expect(box.indexOf("Permission mode:")).toBeLessThan(box.indexOf("Model: Sonnet 5"));
    expect(box.indexOf("Model: Sonnet 5")).toBeLessThan(box.indexOf("Effort: High"));
    expect(box.indexOf("Effort: High")).toBeLessThan(box.indexOf('aria-label="Send"'));
    // Model and effort share the right-hand group; access leads the row.
    const run = box.slice(box.indexOf('class="thread-composer-run"'));
    expect(run).toContain("Model: Sonnet 5");
    expect(run).not.toContain("Permission mode:");
    const access = box.slice(box.indexOf("Permission mode:"), box.indexOf('class="thread-composer-run"'));
    expect(access).toContain('class="thread-glyph"');
    expect(box).not.toContain("codicon-shield");
  });

  it("draws the access chip only where the transport can change the policy", () => {
    expect(composer(paint({ config: { ...config, canSetPermission: false } })))
      .not.toContain("Permission mode");
    expect(composer(paint({ onSetPermissionMode: undefined }))).not.toContain("Permission mode");
    expect(composer(paint({ config: { ...config, permissionModes: [] } })))
      .not.toContain("Permission mode");
  });

  it("waits on a policy change in flight, and tones the modes that skip approval", () => {
    const pending = composer(paint({ config: { ...config, pendingPermission: true } }));
    expect(pending).toMatch(/app-select-pending thread-composer-chip thread-composer-access/u);
    expect(pending).toContain('aria-label="Permission mode: Accept edits"');
    expect(composer(paint({ config: { ...config, permissionMode: "bypassPermissions" } })))
      .toContain("thread-composer-access access-tone-danger");
    // An inherited policy is named and cannot be re-picked from the chip.
    expect(composer(paint({ config: { ...config, permissionMode: "dontAsk",
      permissionUnknown: true } }))).toContain('aria-label="Permission mode: Don&#x27;t Ask"');
  });

  it("offers Send at rest, with the Threads arrow, and no Stop", () => {
    const box = composer(paint({ drafts: drafted("Add a test") }));
    expect(box).toContain('aria-label="Send"');
    expect(box).toContain("thread-composer-round thread-composer-send");
    expect(box).toMatch(/aria-label="Send"[^>]*><svg class="thread-glyph thread-composer-glyph"[^>]*>(?:(?!<\/svg>)[\s\S])*M8 12\.75v-9\.5/u);
    expect(box).not.toContain('aria-label="Stop"');
    expect(box).not.toContain("Queue after current turn");
  });

  it("offers Queue, Steer and Stop while a turn runs, each with its key", () => {
    const box = composer(busy());
    expect(box).toContain('aria-label="Queue after current turn" aria-keyshortcuts="Enter"');
    expect(box).toContain('aria-label="Steer" aria-keyshortcuts="Alt+Enter"');
    expect(box).toContain(">⌥↵<");
    expect(box).not.toContain("⌘↵");
    expect(box).toContain('aria-label="Stop" aria-keyshortcuts="Escape"');
    expect(box).toMatch(/aria-label="Stop"[^>]*><svg class="thread-glyph thread-composer-glyph" width="12"/u);
    expect(box).not.toContain('aria-label="Send"');
    expect(box.indexOf("Queue after current turn")).toBeLessThan(box.indexOf('aria-label="Steer"'));
    expect(box.indexOf('aria-label="Steer"')).toBeLessThan(box.indexOf('aria-label="Stop"'));
  });

  it("offers Steer only where the engine proved it, and blocks it like the row below", () => {
    expect(composer(busy({ canSteer: false }))).not.toContain('aria-label="Steer"');
    expect(composer(busy({ queueSendingId: "queued-1" })))
      .toContain('aria-label="Steer unavailable. Another prompt is already being delivered."');
  });

  it("keeps Send beside Stop where a busy pane cannot queue", () => {
    const box = composer(busy({ onQueue: undefined, canSteer: false }));
    expect(box).toContain('aria-label="Send"');
    expect(box).toContain('aria-label="Stop"');
    expect(box).not.toContain("Queue after current turn");
  });

  /* THE LIVE TURN (owner, 2026-10-07; the Code mode look, 2026-10-23): the
     card draws nothing while a turn works (the arc that circled it was cut
     for less motion, and the hued edge went with the hue glow), turns amber
     while a request docked above it waits on the owner, and is the card it
     always was at rest. */
  const approval: ConvItem = { kind: "approval", id: "ap-1", tool: "Bash",
    summary: "npm test", input: { command: "npm test -- auth" } };
  const waiting = (over: Partial<ConversationProps> = {}): string => busy({
    items: [approval], activeApprovalIds: new Set(["ap-1"]), focused: true,
    drafts: drafted(""), ...over,
  });

  it("marks a working box, with nothing travelling its edge", () => {
    const running = composer(busy());
    expect(running).toMatch(/^class="thread-composer is-working"/u);
    expect(running).not.toContain("thread-composer-comet");
    const rest = composer(paint());
    expect(rest).toMatch(/^class="thread-composer"/u);
  });

  it("turns amber and points at the docked request while it waits on the owner", () => {
    const html = waiting();
    const box = composer(html);
    expect(box).toMatch(/^class="thread-composer is-waiting"/u);
    expect(box).not.toContain("thread-composer-comet");
    expect(box).toContain('placeholder="Answer above, or tell it what to do instead"');
    // The request is docked directly above the box, on the composer's strip.
    expect(html.indexOf('class="thread-dock"')).toBeLessThan(html.indexOf('class="thread-composer is-waiting"'));
    expect(html.indexOf('class="conv-strip"')).toBeLessThan(html.indexOf('class="thread-dock"'));
    // Queue, Steer and Stop are still offered: the owner may answer in words.
    expect(box).toContain('aria-label="Queue after current turn"');
    expect(box).toContain('aria-label="Steer"');
    expect(box).toContain('aria-label="Stop"');
  });

  it("is not amber for a request the approval policy is still checking", () => {
    const box = composer(waiting({ approvalGate: () => "checking" }));
    expect(box).toMatch(/^class="thread-composer is-working"/u);
    // An answered request no longer holds the box either.
    const answered = composer(waiting({
      approvals: { [`claude#a\u0000ap-1`]: { verdict: "allow" } },
    }));
    expect(answered).toMatch(/^class="thread-composer is-working"/u);
  });

  it("advertises the docked request's keys from an empty field, and only there", () => {
    const keys = 'aria-keyshortcuts="Escape Meta+Enter Control+Enter Meta+Backspace Control+Backspace"';
    expect(composer(waiting())).toContain(keys);
    // With text in the field, ⌘↵ is Enter again: nothing is advertised.
    const typed = composer(waiting({ drafts: drafted("Use the staging config instead") }));
    expect(typed).not.toContain("Meta+Enter");
    expect(typed).toContain('aria-label="Prompt to queue after current turn" aria-keyshortcuts="Escape"');
    // A pane that does not own the chord advertises nothing.
    expect(composer(waiting({ focused: false }))).not.toContain("Meta+Enter");
  });

  it("draws Queue and Steer as chips, left of the round Stop", () => {
    const box = composer(busy());
    expect(box).toContain('class="conv-iconbtn thread-composer-verb is-queue"');
    expect(box).toContain('class="conv-iconbtn thread-composer-verb is-steer"');
    expect(box.indexOf("is-queue")).toBeLessThan(box.indexOf("is-steer"));
    expect(box.indexOf("is-steer")).toBeLessThan(box.indexOf("thread-composer-stop"));
  });

  it("says a refused setting change where the old row said it", () => {
    const box = composer(paint({ config: { ...config, error: "The model is unavailable." } }));
    expect(box).toContain('role="alert" title="The model is unavailable."');
  });
});

describe("the Threads composer's stylesheet", () => {
  const css = readFileSync(new URL("./ThreadComposer.css", import.meta.url), "utf8");
  const source = readFileSync(new URL("./ConversationComposer.tsx", import.meta.url), "utf8");

  /** The rule whose whole selector is `selector`, from the start of its line. */
  const rule = (selector: string): string => {
    const at = css.indexOf(`\n${selector} {`);
    expect(at, selector).toBeGreaterThan(-1);
    return css.slice(at + 1, css.indexOf("}", at) + 1);
  };
  const reduced = [...css.matchAll(/@media \(prefers-reduced-motion: reduce\) \{([\s\S]*?)\n\}/gu)]
    .map(([, body]) => body).join("\n");

  /* The TN Code look (owner, 2026-11-16; the Code mode look of 2026-10-23
     before it): the theme's surface under a hairline ring, a 22px corner and
     a soft two-layer lift whose second layer Bare-CLI drops; 12px under the
     transcript and 24px over the pane's foot. */
  it("sits on the transcript's reading lane as the canvas's card", () => {
    const card = rule(".thread-composer");
    expect(card).toContain("width:min(calc(100% - 2 * var(--conv-gutter)), var(--conv-measure));");
    expect(card).toContain("margin:12px auto 24px;");
    // Bare CLI's card: the surface under a bordered edge.
    expect(card).toContain("background:var(--surface);");
    expect(card).toContain("border:var(--border-w) solid var(--border);");
    // 22px, on the radius system, so the Bare-CLI form axis squares it.
    expect(card).toContain("border-radius:calc(var(--radius-composer) * 11 / 7);");
    expect(card).toContain("box-shadow:var(--composer-lift);");
    // Refined chrome: a hairline ring for the edge (the border kept,
    // transparent, for the waiting amber) over a soft second layer of lift.
    const refined = rule(':where(:root:not([data-chrome="cli"])) .thread-composer');
    expect(refined).toContain("background:light-dark(color-mix(in srgb, var(--surface) 92%, transparent),");
    expect(refined).toContain("border-color:transparent;");
    expect(refined).toContain("box-shadow:0 0 0 .5px var(--tn-separator), var(--composer-lift),");
    expect(refined).toContain("0 8px 24px -10px color-mix(in srgb, var(--composer-lift-color) 90%, transparent)");
    // The field: 14px text, 14px over its first line and 16px in.
    const field = rule(".thread-composer .thread-composer-input");
    expect(field).toContain("padding:14px 16px 0;");
    expect(field).toContain("font-size:var(--text-label);");
    expect(rule(".thread-composer-foot")).toContain("padding:8px 10px 10px;");
    // "+" is a filled circle at rest.
    expect(rule(".thread-composer .thread-composer-plus .thread-composer-icon"))
      .toContain("background:var(--tn-fill);");
  });

  it("fills Send with the accent and Stop with the strongest ink, 32px squares", () => {
    expect(rule(".thread-composer .thread-composer-round"))
      .toContain("width:32px; height:32px;");
    // Circles since the TN Code look (owner, 2026-11-16).
    expect(rule(".thread-composer .thread-composer-round"))
      .toContain("border-radius:50%;");
    expect(rule(".thread-composer .thread-composer-foot .thread-composer-send"))
      .toContain("color:var(--accent-fg); background:var(--accent);");
    expect(rule(".thread-composer .thread-composer-foot .thread-composer-stop"))
      .toContain("color:var(--surface); background:var(--fg-strong);");
    expect(css).toMatch(/\.thread-composer-verb \{[^}]*height:28px/u);
  });

  it("draws nothing that moves on the box's edge (owner: less motion)", () => {
    expect(css).not.toContain("thread-composer-comet");
    expect(css).not.toContain("@keyframes thread-comet");
    expect(css).not.toContain("@property");
  });

  it("never takes a project's hue: no gradient edge, glow, Send or Steer", () => {
    expect(css).not.toContain("--project-h");
    expect(css).not.toContain("project-hue-");
    expect(css).not.toContain("linear-gradient");
    expect(css).not.toContain("--thread-live-");
  });

  it("waits in amber: the amber edge and a soft, still 3px ring, the ring off in Bare-CLI", () => {
    expect(rule(".thread-composer.is-waiting")).toContain("border-color:var(--thread-amber);");
    const ring = rule(':where(:root:not([data-chrome="cli"])) .thread-composer.is-waiting');
    expect(ring).toContain("0 0 0 3px color-mix(in srgb, var(--thread-amber) 22%, transparent)");
    expect(ring).not.toContain("animation");
    expect(rule(".thread-composer, .thread-dock")).toContain("--thread-amber:var(--status-waiting);");
  });

  it("docks a waiting request on the card's lane, in amber, and lets it arrive once", () => {
    expect(rule(".thread-dock")).toContain("width:min(calc(100% - 2 * var(--conv-gutter)), var(--conv-measure));");
    expect(rule(".thread-dock")).toContain("margin:12px auto 16px;");
    const card = rule(".thread-dock-card");
    expect(card).toContain("padding:16px 18px 12px;");
    expect(card).toContain("background:var(--surface);");
    expect(card).toContain("border-radius:var(--radius-composer);");
    const pending = rule(".thread-dock-card.is-pending");
    expect(pending).toContain("border-color:var(--thread-amber);");
    expect(pending).toContain("thread-dock-in 260ms cubic-bezier(.2, .8, .2, 1) both");
    expect(pending).toContain("thread-dock-ping 1.1s ease-out 260ms 2");
    expect(css).toContain("@keyframes thread-dock-in { from { opacity:0; transform:translateY(18px); } }");
    // Reduced motion: in place, with a still ring.
    expect(reduced).toMatch(/\.thread-dock-card\.is-pending \{ animation:none;\s*box-shadow:0 0 0 3px/u);
    // The glyph on a 30px tile, amber while the request waits.
    expect(rule(".thread-dock-head > .thread-dock-tile")).toContain("width:30px; height:30px;");
    expect(rule(".thread-dock-head > .thread-dock-tile.thread-dock-glyph"))
      .toContain("background:color-mix(in srgb, var(--thread-amber) 14%, transparent);");
    // The command and the diff on the pane's own ground.
    expect(rule(".thread-dock-card .thread-dock-command")).toContain("background:var(--editor-bg);");
    expect(rule(".thread-dock-diff")).toContain("background:var(--editor-bg);");
    // Deny secondary, Allow once the one filled button: 28px capsules (the
    // TN Code look, owner 2026-11-16).
    expect(rule(".thread-dock-card .thread-dock-acts .btn")).toContain("height:28px;");
    expect(rule(".thread-dock-card .thread-dock-acts .btn")).toContain("border-radius:var(--radius-pill);");
    expect(rule(".thread-dock-card .thread-dock-acts .thread-dock-ghost"))
      .toContain("border:var(--border-w) solid var(--border-strong);");
    expect(rule(".thread-dock-card .thread-dock-acts .btn-primary:not(:disabled)"))
      .toContain("background:var(--fg-strong);");
  });

  it("fills Queue and Steer as neutral washes", () => {
    expect(rule(".thread-composer .thread-composer-verb.is-queue"))
      .toContain("background:color-mix(in srgb, var(--fg) 5%, transparent);");
    expect(rule(".thread-composer .thread-composer-verb.is-steer"))
      .toContain("background:color-mix(in srgb, var(--fg-dim) 10%, transparent);");
  });

  it("loads after the sheets it overrides", () => {
    const imports = [...source.matchAll(/^import .*$/gmu)].map(([line]) => line);
    expect(imports.at(-1)).toBe('import "./ThreadComposer.css";');
  });
});
