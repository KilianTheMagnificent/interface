import { flushSync } from "react-dom";
import { useState } from "react";
import type { Root } from "react-dom/client";
import type { SessionSummary } from "../../shared/engine";
import { approvalVerdictKey, type ApprovalVerdicts } from "../approval-keys";
import { AgentTile } from "../components/AgentTile";
import { APP_SELECT_CSS } from "../components/AppSelect";
import { Conversation, type ConversationProps } from "../components/Conversation";
import { RECAP_CSS } from "../components/SessionTile";
import { ThreadDiffToggle } from "../components/ThreadPaneHead";
import type { ConvItem } from "../conversation-reducer";
import { createDraftStore } from "../draft-store";
import { assert, frames, noop, until } from "./fixture-support";

/**
 * The Threads composer (owner-approved redesign, 2026-10-06; the Code mode
 * look, 2026-10-23) in a real Electron DOM: one card on the transcript's lane,
 * the field first, the "+" menu holding the old row's leading controls (driven
 * by keyboard and pointer), the access chip beside it and the model and effort
 * chips at the row's right end with the provider's mark once, the access
 * chip's choice reaching the caller, and the verbs — the accent Send at rest,
 * Queue / Steer / Stop mid-turn — dispatching through the composer's own send
 * path. `pose` is what the lane's screenshot shows when the script is done.
 *
 * It opens as a plain chat (No project), whose pane must stay neutral, then
 * becomes a project's thread: the transcript takes its hue ("hued glow",
 * owner, 2026-10-06: the aurora, the bubble, inline code) while the composer
 * stays the theme's own card, with no gradient edge, glow or hued Send.
 *
 * Then the live turn (owner, 2026-10-07): the still head line, a request
 * docked above the card with the card in amber and "Needs you" in the head,
 * the empty field's ⌘↵ / ⌘⌫ answering it (and a field with text queueing
 * instead), the pane's own chord answering a docked card at once, and the
 * compact queue. Motion is checked on whichever side of reduced motion the
 * lane emulates; the threads lane runs it once each way.
 */
const PROJECT = "/Users/owner/Developer/Projects/website";

const SESSION = "claude#thread-composer";
const MODES = ["default", "acceptEdits", "plan", "auto", "bypassPermissions"];

export type ThreadComposerPose = "idle" | "busy" | "menu" | "waiting";

const items: ConvItem[] = [
  { kind: "user", id: "u0", text: "Add login and logout to src/auth.ts" },
  { kind: "tool", id: "t1", tool: "Read", summary: "src/auth.ts", input: {},
    result: { ok: true, summary: "40 lines" } },
  { kind: "tool", id: "t2", tool: "Edit", summary: "src/auth.ts", input: {},
    result: { ok: true, summary: "Edited" } },
  { kind: "diff", id: "d2", callId: "t2", path: "/repo/src/auth.ts", added: 10, removed: 1 },
  { kind: "text", id: "a3", streaming: false,
    text: "Added `login()`, which trims the name, and `logout()`, which clears the session." },
];

function setLook(theme: string, chrome: string): void {
  document.documentElement.dataset.theme = theme;
  document.documentElement.dataset.chrome = chrome;
}

const keydown = (target: Element, key: string, init: KeyboardEventInit = {}): void => {
  flushSync(() => {
    target.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...init }));
  });
};
const press = (target: HTMLElement): void => { flushSync(() => { target.click(); }); };
const rowLabel = (element: Element | null): string =>
  element?.querySelector(".thread-composer-menu-label")?.textContent ?? "";
/** Every finite animation on the page has finished (the running ones never do). */
async function settled(): Promise<void> {
  await Promise.all(document.getAnimations()
    .filter(animation => animation.effect?.getComputedTiming().iterations !== Infinity)
    .map(animation => animation.finished.catch(() => undefined)));
}
/** What a CSS colour expression computes to, for comparing with a used value. */
function resolved(expression: string): string {
  const probe = document.createElement("span");
  probe.style.color = expression;
  document.body.append(probe);
  const value = getComputedStyle(probe).color;
  probe.remove();
  return value;
}

export async function threadComposerFixture(root: Root, theme: string, chrome: string,
    pose: ThreadComposerPose = "idle"): Promise<Record<string, unknown>> {
  flushSync(() => root.render(null));
  setLook(theme, chrome);
  const calls: string[] = [];
  const drafts = createDraftStore({});
  const control = {
    thinking: (_value: boolean): void => undefined,
    project: (_value: string | null): void => undefined,
    approval: (_value: ConvItem | null): void => undefined,
    status: (_value: SessionSummary["status"]): void => undefined,
    queue: (_value: { id: string; text: string }[]): void => undefined,
    totals: (_value: { added: number; removed: number } | null): void => undefined,
  };
  const session: SessionSummary = {
    engine: "claude", sessionId: "thread-composer", title: "Add login and logout to auth",
    cwd: "/repo", status: "idle", cloud: false,
    lastActivity: "2026-10-06T10:00:00Z", firstSeen: "2026-10-06T09:00:00Z", archived: false,
  };
  function Harness() {
    const [thinking, setThinking] = useState(false);
    const [mode, setMode] = useState("acceptEdits");
    const [project, setProject] = useState<string | null>(null);
    const [approval, setApproval] = useState<ConvItem | null>(null);
    const [status, setStatus] = useState<SessionSummary["status"]>("idle");
    const [queue, setQueue] = useState<{ id: string; text: string }[]>([]);
    const [verdicts, setVerdicts] = useState<ApprovalVerdicts>({});
    const [totals, setTotals] = useState<{ added: number; removed: number } | null>(null);
    control.totals = setTotals;
    control.thinking = setThinking;
    control.project = setProject;
    control.approval = setApproval;
    control.status = setStatus;
    control.queue = setQueue;
    const answer = (id: string, verdict: "allow" | "deny"): void => {
      setVerdicts(current => ({ ...current, [approvalVerdictKey(SESSION, id)]: { verdict } }));
    };
    const props: ConversationProps = {
      items: approval === null ? items : [...items, approval], live: true, thinking, focused: true,
      sessionKey: SESSION, drafts,
      threadLayout: true,
      config: {
        engine: "claude", engineLabel: "Claude", model: "claude-opus-5", effort: "high",
        models: [{ value: "claude-opus-5", displayName: "Opus 5", supportsEffort: true,
          effortLevels: ["low", "medium", "high"] }],
        canSetModel: true, effortLevels: ["low", "medium", "high"], showEffort: true,
        canSetEffort: true, pendingModel: false, pendingEffort: false,
        permissionMode: mode, permissionModes: MODES, canSetPermission: true,
        supportsFastMode: true, fastMode: "off", error: null,
      },
      context: { used: 24_000, size: 200_000, percent: 12 },
      commands: [{ name: "review", description: "Review changes" },
        { name: "compact", description: "Compact context" }],
      contextCapabilities: { file: { available: true }, folder: { available: true } },
      contextItems: [],
      onContextOpenChange: noop,
      onContextRequest: kind => { calls.push(`context:${kind}`); },
      onContextRemove: noop,
      queueWhileBusy: thinking,
      canSteer: true,
      onQueue: text => { calls.push(`queue:${text}`); return true; },
      onSend: text => { calls.push(`send:${text}`); },
      onRunCommand: text => { calls.push(`run:${text}`); },
      onStop: () => { calls.push("stop"); },
      onSetModel: model => { calls.push(`model:${model ?? ""}`); },
      onSetEffort: effort => { calls.push(`effort:${effort ?? ""}`); },
      onSetFastMode: enabled => { calls.push(`fast:${String(enabled)}`); },
      onSetPermissionMode: next => { calls.push(`access:${next}`); setMode(next); },
      onApprove: id => { calls.push(`approve:${id}`); answer(id, "allow"); },
      onDeny: (id, note) => { calls.push(`deny:${id}:${note ?? ""}`); answer(id, "deny"); },
      approvals: verdicts,
      activeApprovalIds: new Set(approval === null ? [] : [approval.id]),
      queueItems: queue,
      onQueueMove: (id, move) => { calls.push(`queue-move:${id}:${move}`); },
      onQueueEdit: (id, text) => { calls.push(`queue-edit:${id}:${text}`); },
      onQueueRemove: noop, onQueueClear: noop,
      onQueueSendNow: id => { calls.push(`queue-send:${id}`); },
      onQueueEditing: (id, editing) => { calls.push(`queue-editing:${id}:${String(editing)}`); },
      pathRoot: "/repo",
    };
    return (
      <div style={{ height: "100vh", display: "flex", flexDirection: "column", minWidth: 0 }}>
        <style>{APP_SELECT_CSS}</style>
        {/* The tile stack's own sheet, which App's SessionTile injects. */}
        <style>{RECAP_CSS}</style>
        <div className="workspace-content" style={{ flex: 1, minHeight: 0 }}>
          <div className="workspace-grid workspace-count-1">
            <AgentTile session={{ ...session, status }} selected maximized={false} live engineLabel="Claude"
              projectRoot={project} onFocus={noop} onToggleMaximize={noop} onClose={noop}
              headActions={<ThreadDiffToggle open={false} onToggle={noop} totals={totals} />}>
              <div className="tile-stack"><Conversation {...props} /></div>
            </AgentTile>
          </div>
        </div>
      </div>
    );
  }
  flushSync(() => root.render(<Harness />));
  await document.fonts.ready;
  await frames(3);
  // With motion on, the tile and the pane arrive animated; measure them landed.
  await settled();

  // ---- one box on the lane, the field first, no dirty pill ----------------
  const box = document.querySelector<HTMLElement>(".thread-composer");
  assert(box !== null, "The Threads composer did not mount");
  assert(document.querySelector(".conv-composer") === null, "The old composer box is still drawn");
  assert(document.querySelector(".conv-dirty") === null, "The dirty pill is still drawn");
  const field = box.querySelector<HTMLTextAreaElement>("textarea");
  assert(field !== null && box.firstElementChild === field, "The field is not the box's first child");
  assert(field.placeholder === "Ask for a change or a follow-up",
    `The idle placeholder reads ${field.placeholder}`);
  const pane = box.closest<HTMLElement>(".conv")!.getBoundingClientRect();
  const rect = box.getBoundingClientRect();
  // The transcript's reading lane, measured where the box stands: the same
  // expression its column, working strip and clocks use.
  const probe = document.createElement("div");
  probe.style.width = "min(calc(100% - 2 * var(--conv-gutter)), var(--conv-measure))";
  box.parentElement!.append(probe);
  const lane = probe.getBoundingClientRect().width;
  probe.remove();
  assert(Math.abs(rect.width - lane) <= 0.5, `The box is ${rect.width}px on a ${lane}px lane`);
  // The lane is TN Code's wide chat width, 72rem (owner, 2026-11-16).
  assert(chrome === "cli" || rect.width <= 1152, `The box is ${rect.width}px, past 1152px`);
  assert(Math.abs((rect.left - pane.left) - (pane.right - rect.right)) <= 1,
    `The box is not centred on the transcript's lane: box ${rect.left}–${rect.right}, `
      + `pane ${pane.left}–${pane.right}`);
  const body = box.closest<HTMLElement>(".workspace-tile-body")!.getBoundingClientRect();
  assert(rect.top >= body.top && rect.bottom <= body.bottom + 0.5,
    `The box (${rect.top}–${rect.bottom}) is not inside its pane (${body.top}–${body.bottom})`);
  const radius = getComputedStyle(box).borderTopLeftRadius;
  assert(chrome === "cli" ? radius === "0px" : radius === "22px", `The box's corner is ${radius}`);

  // ---- the row: + · model | effort | access · fast ... verbs --------------
  const foot = box.querySelector<HTMLElement>(".thread-composer-foot")!;
  assert(foot.scrollWidth <= foot.clientWidth + 1, "The control row overflows the box");
  for (const control of foot.querySelectorAll<HTMLElement>("button")) {
    const at = control.getBoundingClientRect();
    assert(at.left >= rect.left - 0.5 && at.right <= rect.right + 0.5,
      `${control.getAttribute("aria-label") ?? control.textContent} sits outside the box`);
    assert(at.height >= 24 && at.width >= 24,
      `${control.getAttribute("aria-label") ?? control.textContent} is under the 24px target`);
  }
  const plus = foot.querySelector<HTMLButtonElement>(
    'button[aria-label="Add context, commands and options"]');
  assert(plus !== null, "The + button is missing");
  const plusRect = plus.getBoundingClientRect();
  assert(plusRect.width === 28 && plusRect.height === 28, "The + button is not 28px");
  assert(box.querySelectorAll("[data-provider-logo]").length === 1,
    "The provider's mark is not drawn exactly once");
  const model = foot.querySelector<HTMLButtonElement>(".thread-composer-model .app-select-trigger");
  assert(model !== null && model.textContent?.includes("Opus 5") === true
    && model.querySelector("[data-provider-logo]") !== null, "The model chip is not mark + name");
  assert(model.getBoundingClientRect().height === 28, "The model chip is not 28px");
  assert(foot.querySelector(".thread-composer-sep") === null, "The row still draws hairlines");
  const effort = foot.querySelector(".thread-composer-effort .app-select-trigger");
  assert(effort?.getAttribute("aria-label") === "Effort: High", "The effort chip is missing");
  const access = foot.querySelector<HTMLButtonElement>(".thread-composer-access .app-select-trigger");
  assert(access !== null && access.getAttribute("aria-label") === "Permission mode: Accept edits",
    "The access chip is missing");
  assert(access.querySelector(".thread-glyph") !== null, "The access chip has no shield");
  // The canvas's row: + and access on the left, the model chips at the right end.
  assert(plusRect.right <= access.getBoundingClientRect().left + 0.5
    && access.getBoundingClientRect().right <= model.getBoundingClientRect().left,
  "Access does not sit between the + and the model chip");
  assert(foot.querySelector(".thread-composer-fast") !== null, "Fast left the row");
  const send = foot.querySelector<HTMLButtonElement>('button[aria-label="Send"]');
  assert(send !== null && foot.querySelector('button[aria-label="Stop"]') === null,
    "At rest the row should offer Send and no Stop");
  const sendRect = send.getBoundingClientRect();
  assert(sendRect.width === 32 && sendRect.height === 32, "Send is not a 32px square");
  assert(model.getBoundingClientRect().right <= sendRect.left, "The model chip is not left of Send");
  const typeInto = (text: string): void => {
    flushSync(() => {
      Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!.call(field, text);
      field.dispatchEvent(new Event("input", { bubbles: true }));
    });
  };
  typeInto("Add a test");
  await frames();
  // The reduced-motion guard leaves every property a 0.01ms transition, and a
  // hidden window can hold one at its first frame (the disabled fill) past a
  // rAF: finish it, so the colour read is the enabled disc's.
  for (const animation of send.getAnimations()) animation.finish();
  assert(getComputedStyle(send).backgroundColor === resolved("var(--accent)"),
    "Send is not filled with the accent");

  // ---- a plain chat stays neutral; a project's thread takes its hue --------
  const conv = box.closest<HTMLElement>(".conv")!;
  const bubble = conv.querySelector<HTMLElement>(".conv-turn-user")!;
  const code = conv.querySelector<HTMLElement>(".conv-text .conv-md-inline-code")!;
  const modelChip = foot.querySelector<HTMLElement>(".thread-composer-model .app-select-trigger")!;
  const look = () => ({
    aurora: getComputedStyle(conv).backgroundImage,
    bubble: getComputedStyle(bubble).backgroundImage,
    code: getComputedStyle(code).color,
    edge: getComputedStyle(box).borderTopColor,
    boxImage: getComputedStyle(box).backgroundImage,
    glow: getComputedStyle(box).boxShadow,
    send: getComputedStyle(send).backgroundImage,
  });
  const neutral = look();
  assert(box.closest(".project-hue-none") !== null, "The plain chat's tile is not .project-hue-none");
  assert(neutral.aurora === "none" && neutral.bubble === "none" && neutral.boxImage === "none"
    && neutral.send === "none", `A plain chat is not neutral: ${JSON.stringify(neutral)}`);
  // The TN Code look (owner, 2026-11-16): in refined chrome the card's edge
  // is a hairline ring in its shadow, the border kept, transparent, for the
  // waiting amber. Bare CLI drops its shadows, so its edge is the border.
  assert(chrome === "cli" ? neutral.edge === resolved("var(--border)")
    : neutral.edge === "rgba(0, 0, 0, 0)" && /0px 0px 0px 0\.5px/u.test(neutral.glow),
  `A plain chat's box edge is ${neutral.edge}, its shadow ${neutral.glow}`);
  assert(neutral.code === resolved("var(--fg)"), `A plain chat's inline code is ${neutral.code}`);
  // The model chip is a borderless pop-up button like the rest: no wash at rest.
  assert(getComputedStyle(modelChip).backgroundColor === "rgba(0, 0, 0, 0)",
    "The model chip still carries a wash at rest");
  flushSync(() => { control.project(PROJECT); });
  await frames(2);
  const hued = look();
  assert(box.closest(".project-hue-none") === null && box.closest("[class*='project-hue-']") !== null,
    "The project's tile does not carry its hue class");
  // The transcript draws no hue since the Threads look (owner, 2026-10-23):
  // no aurora, your words on the raised tier, inline code in the text colour.
  assert(hued.aurora === "none", `The pane still draws an aurora: ${hued.aurora}`);
  assert(hued.bubble === "none", `The bubble is still lit: ${hued.bubble}`);
  assert(hued.code === neutral.code, "Inline code took the hue");
  // The composer is the theme's own card in a project too (owner, 2026-10-23):
  // no gradient edge, no glow of the hue, no hued Send.
  assert(hued.edge === neutral.edge && hued.boxImage === "none" && hued.glow === neutral.glow
    && hued.send === "none", `The composer took the project's hue: ${JSON.stringify(hued)}`);
  typeInto("");
  await frames();
  assert(getComputedStyle(send).backgroundImage === "none",
    "A disabled Send took the hue");

  // ---- the + menu: keyboard ------------------------------------------------
  plus.focus();
  keydown(plus, "ArrowDown");
  await frames();
  const menu = (): HTMLElement | null => box.querySelector<HTMLElement>('[role="menu"]');
  const rows = (): HTMLElement[] => [...(menu()?.querySelectorAll<HTMLElement>(
    '[role="menuitem"], [role="menuitemradio"]') ?? [])];
  assert(menu() !== null, "ArrowDown on + did not open its menu");
  assert(menu()!.classList.contains("is-floating"), "The menu is not a floating surface");
  const labels = rows().map(rowLabel);
  assert(JSON.stringify(labels) === JSON.stringify(["Add context", "Slash commands", "Compact",
    "Balanced", "Detailed", "Context window", "Compact now"]), `The menu holds ${labels.join(", ")}`);
  assert(document.activeElement === rows()[0], "The menu did not focus its first row");
  assert(rows()[0].getAttribute("aria-label") === "Add prompt context, 0 of 8 selected",
    "The context row does not say how much is attached");
  assert(rows()[5].getAttribute("aria-disabled") === "true"
    && rows()[5].textContent?.includes("12%") === true
    && rows()[5].textContent.includes("Provider reported"), "The session readout is missing");
  keydown(document.activeElement!, "ArrowDown");
  await frames();
  assert(rowLabel(document.activeElement) === "Slash commands", "ArrowDown did not move");
  keydown(document.activeElement!, "ArrowUp");
  keydown(document.activeElement!, "ArrowUp");
  await frames();
  assert(rowLabel(document.activeElement) === "Compact now", "ArrowUp did not wrap to the end");
  keydown(document.activeElement!, "Home");
  await frames();
  assert(rowLabel(document.activeElement) === "Add context", "Home did not reach the first row");
  // Drill into the context kinds and back out.
  keydown(document.activeElement!, "ArrowRight");
  await frames();
  assert(menu()?.getAttribute("aria-label") === "Add context", "ArrowRight did not drill in");
  assert(rowLabel(document.activeElement) === "File", "The context view did not focus File");
  const kinds = rows().map(rowLabel);
  assert(kinds.length === 8 && kinds[0] === "Add context",
    `The context view lists ${kinds.join(", ")}`);
  assert(rows().filter(row => row.getAttribute("aria-disabled") === "true").length === 5,
    "Unavailable kinds are not marked with their reasons");
  keydown(document.activeElement!, "Escape");
  await frames();
  assert(menu()?.getAttribute("aria-label") === "Add context, commands and options"
    && rowLabel(document.activeElement) === "Add context", "Escape did not step back out");
  keydown(document.activeElement!, "Escape");
  await frames();
  assert(menu() === null && document.activeElement === plus, "Escape did not close onto +");

  // ---- the + menu: pointer -------------------------------------------------
  press(plus);
  await frames();
  press(rows()[0]);
  await frames();
  press(rows().find(row => rowLabel(row) === "File")!);
  await frames();
  assert(calls.at(-1) === "context:file", `Choosing File asked for ${calls.at(-1)}`);
  assert(menu() === null && document.activeElement === plus, "A chosen kind left the menu open");
  press(plus);
  await frames();
  press(rows().find(row => rowLabel(row) === "Detailed")!);
  await frames();
  press(plus);
  await frames();
  assert(rows().find(row => rowLabel(row) === "Detailed")?.getAttribute("aria-checked") === "true",
    "The detail dial did not take Detailed");
  press(rows().find(row => rowLabel(row) === "Slash commands")!);
  await until(() => field.value === "/" && box.querySelector(".provider-command-menu") !== null,
    "The slash row did not open the command menu");
  typeInto("");
  await frames();
  // Tab leaves for the control after +, which is the access chip beside it.
  plus.focus();
  keydown(plus, "ArrowDown");
  await frames();
  keydown(document.activeElement!, "Tab");
  await frames();
  assert(menu() === null && document.activeElement === access, "Tab did not leave for the access chip");

  // ---- the access chip -----------------------------------------------------
  press(access);
  await frames();
  const plan = [...document.querySelectorAll<HTMLElement>('.app-select-popover [role="option"]')]
    .find(option => option.querySelector(".app-select-option-label, .app-select-label")?.textContent?.trim() === "Plan"
      || option.textContent?.trim().startsWith("Plan") === true);
  assert(plan !== undefined, "The access chip did not open its modes");
  press(plan);
  await until(() => calls.includes("access:plan")
      && foot.querySelector(".thread-composer-access .app-select-trigger")
        ?.getAttribute("aria-label") === "Permission mode: Plan",
  () => `The access chip did not take Plan: ${calls.join(", ")}`);
  assert(foot.querySelector(".thread-composer-access.access-tone-plan") !== null,
    "The access chip lost its tone");

  // ---- mid-turn: Queue, Steer, Stop ------------------------------------------
  flushSync(() => { control.thinking(true); });
  await frames();
  const busyWords: string = field.placeholder;
  assert(busyWords === "Steer now, or queue a follow-up",
    `The busy placeholder reads ${busyWords}`);
  const verb = (label: string): HTMLButtonElement | null =>
    foot.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`);
  assert(verb("Send") === null && verb("Stop") !== null
    && verb("Queue after current turn") !== null && verb("Steer") !== null,
  "Mid-turn the row should offer Queue, Steer and Stop");
  typeInto("Also run the linter");
  await frames();
  press(verb("Steer")!);
  await frames();
  assert(calls.at(-1) === "send:Also run the linter", `Steer did ${calls.at(-1)}`);
  typeInto("Then update the README");
  await frames();
  press(verb("Queue after current turn")!);
  await frames();
  assert(calls.at(-1) === "queue:Then update the README", `Queue did ${calls.at(-1)}`);
  typeInto("Steer by key");
  await frames();
  keydown(field, "Enter", { altKey: true });
  await frames();
  assert(calls.at(-1) === "send:Steer by key", `⌥↵ did ${calls.at(-1)}`);
  press(verb("Stop")!);
  assert(calls.at(-1) === "stop", "Stop did not stop");
  assert(foot.scrollWidth <= foot.clientWidth + 1, "The busy row overflows the box");
  // Queue and Steer are chips, both neutral washes.
  assert(verb("Queue after current turn")!.classList.contains("is-queue")
    && verb("Steer")!.classList.contains("is-steer"), "Queue and Steer are not chips");
  assert(getComputedStyle(verb("Steer")!).backgroundColor !== "rgba(0, 0, 0, 0)",
    "The Steer chip has no fill");

  // ---- the live turn: running ------------------------------------------------
  const reduced = matchMedia("(prefers-reduced-motion: reduce)").matches;
  typeInto("");
  flushSync(() => { control.status("working"); });
  await frames(2);
  const head = document.querySelector<HTMLElement>(".workspace-agent .workspace-tile-head")!;
  assert(box.classList.contains("is-working") && !box.classList.contains("is-waiting"),
    `A running box is ${box.className}`);
  // Less motion (owner, 2026-10-07): nothing travels the box's edge, and the
  // head's working line is a still 2px line across the head, motion on or off.
  assert(box.querySelector(".thread-composer-comet") === null, "A running box still draws a comet");
  const line = getComputedStyle(head, "::after");
  const headWidth = head.getBoundingClientRect().width;
  assert(line.animationName === "none" && line.height === "2px"
    && Math.abs(parseFloat(line.width) - headWidth) <= 1,
  `The head's working line is ${line.animationName} ${line.height} ${line.width}`);
  assert(getComputedStyle(head, "::before").backgroundColor === "rgba(0, 0, 0, 0)",
    "A working Threads head still draws its running dot");
  // The Changes chip: still on its first look, then its totals pop in, then a
  // change flashes it (and the next change replays the flash).
  assert(head.dataset.changesCue === undefined, "The Changes chip moved on its first look");
  flushSync(() => { control.totals({ added: 10, removed: 1 }); });
  await frames(2);
  assert(head.dataset.changesCue === "pop", `Known totals cue ${head.dataset.changesCue}`);
  const statAnimation = getComputedStyle(head.querySelector(".thread-head-changes-stat")!).animationName;
  assert(reduced ? statAnimation === "none" : statAnimation === "tile-changes-pop",
    `The totals arrive with ${statAnimation}`);
  flushSync(() => { control.totals({ added: 12, removed: 1 }); });
  await frames(2);
  assert(head.dataset.changesCue === "flash-a", `Changed totals cue ${head.dataset.changesCue}`);
  const flash = getComputedStyle(head.querySelector(".thread-head-changes")!).animationName;
  assert(reduced ? flash === "none" : flash === "tile-changes-flash-a", `A change flashes ${flash}`);
  flushSync(() => { control.totals({ added: 14, removed: 2 }); });
  await frames(2);
  assert(head.dataset.changesCue === "flash-b", `A second change cues ${head.dataset.changesCue}`);

  // ---- the live turn: a request docked above the box --------------------------
  const strip = conv.querySelector<HTMLElement>(".conv-strip")!;
  const list = conv.querySelector<HTMLElement>(".conv-list")!;
  const dockCard = (): HTMLElement | null => strip.querySelector<HTMLElement>(".thread-dock-card");
  const dockButton = (label: string): HTMLButtonElement | undefined =>
    [...strip.querySelectorAll<HTMLButtonElement>(".thread-dock-acts button")]
      .find(button => button.textContent?.startsWith(label) === true);
  flushSync(() => {
    control.approval({ kind: "approval", id: "ap-1", tool: "Bash", summary: "npm test -- auth",
      input: { command: "npm test -- auth", description: "Run the auth tests" } });
    control.status("waiting");
  });
  await frames(2);
  const card = dockCard();
  assert(card !== null, "The request is not docked");
  assert(card.closest(".thread-dock")?.nextElementSibling === box, "The dock is not directly above the box");
  assert(list.querySelector(".thread-dock-card, .conv-approval") === null,
    "The transcript draws the docked request too");
  assert(card.getAttribute("role") === "alert" && card.getAttribute("aria-label") === "Approval required"
    && card.dataset.voiceGuard === "approval", "The docked card lost its alert semantics");
  assert(card.querySelector(".thread-dock-title")?.textContent === "Run a command?",
    `The docked card asks ${card.querySelector(".thread-dock-title")?.textContent}`);
  assert(card.querySelector(".thread-dock-command")?.textContent === "$ npm test -- auth",
    `The docked command reads ${card.querySelector(".thread-dock-command")?.textContent}`);
  assert(/^waiting \d:\d\d$/u.test(card.querySelector(".thread-dock-meta")?.textContent ?? ""),
    "The docked card has no waiting clock");
  // The dock reads the app's one amber once, into its local; the used colours
  // are measured against that local, and the local against the palette's token.
  const amber = getComputedStyle(card).getPropertyValue("--thread-amber").trim();
  assert(amber !== "" && resolved(amber)
    === resolved(getComputedStyle(document.documentElement).getPropertyValue("--status-waiting")),
  `The dock's amber is ${amber}`);
  assert(getComputedStyle(card).borderTopColor === resolved(amber),
    `The docked card's edge is ${getComputedStyle(card).borderTopColor}`);
  const cardRadius = getComputedStyle(card).borderTopLeftRadius;
  assert(cardRadius === (chrome === "cli" ? "0px" : "14px"), `The docked card's corner is ${cardRadius}`);
  if (reduced) {
    assert(getComputedStyle(card).animationName === "none"
      && getComputedStyle(card).boxShadow.includes("3px"), "Reduced motion did not hold the card still");
  } else {
    assert(getComputedStyle(card).animationName.startsWith("thread-dock-in"),
      `The docked card did not arrive: ${getComputedStyle(card).animationName}`);
  }
  await settled();
  // Its glyph sits on a 30px amber tile (measured landed: the card arrives moving).
  const tile = card.querySelector<HTMLElement>(".thread-dock-head > .thread-dock-glyph")!;
  assert(tile.offsetWidth === 30 && tile.offsetHeight === 30
    && getComputedStyle(tile).backgroundColor === resolved(`color-mix(in srgb, ${amber} 14%, transparent)`),
  `The docked glyph's tile is ${tile.offsetWidth}×${tile.offsetHeight}px on ${getComputedStyle(tile).backgroundColor}`);
  const deny = dockButton("Deny");
  const allow = dockButton("Allow once");
  assert(deny !== undefined && allow !== undefined
    && deny.compareDocumentPosition(allow) === Node.DOCUMENT_POSITION_FOLLOWING,
  "The docked card does not read Deny, then Allow once");
  assert(deny.getBoundingClientRect().height === 28 && allow.getBoundingClientRect().height === 28,
    "The docked buttons are not 28px");
  // Allow once is the one filled, deciding button: the strongest ink.
  assert(getComputedStyle(allow).backgroundColor === resolved("var(--fg-strong)")
    && getComputedStyle(deny).backgroundColor === "rgba(0, 0, 0, 0)",
  `Allow once is ${getComputedStyle(allow).backgroundColor}, Deny ${getComputedStyle(deny).backgroundColor}`);
  assert(deny.dataset.approvalKey === "deny" && allow.dataset.approvalKey === "allow",
    "The focused pane's docked card does not carry the keys");
  // The box waits in amber, without the comet, and points at the request.
  assert(box.classList.contains("is-waiting") && !box.classList.contains("is-working")
    && box.querySelector(".thread-composer-comet") === null, `A waiting box is ${box.className}`);
  const waitingWords: string = field.placeholder;
  assert(waitingWords === "Answer above, or tell it what to do instead",
    `The waiting placeholder reads ${waitingWords}`);
  assert(getComputedStyle(box).borderTopColor === resolved(amber),
    `The waiting box's edge is ${getComputedStyle(box).borderTopColor}`);
  // The head holds a still amber line and says Needs you before Changes.
  const needs = head.querySelector<HTMLElement>(".workspace-tile-needs");
  assert(needs?.textContent === "Needs you", "The waiting head does not say Needs you");
  assert(needs.compareDocumentPosition(head.querySelector(".thread-head-changes")!)
    === Node.DOCUMENT_POSITION_FOLLOWING, "Needs you is not before Changes");
  assert(getComputedStyle(needs).backgroundColor === resolved(amber)
    && getComputedStyle(needs).color === resolved("var(--surface)"),
  `Needs you is ${getComputedStyle(needs).color} on ${getComputedStyle(needs).backgroundColor}`);
  const held = getComputedStyle(head, "::after");
  assert(held.animationName === "none"
    && held.backgroundColor === resolved(`color-mix(in srgb, ${amber} 70%, transparent)`),
  `The waiting head's line is ${held.animationName} ${held.backgroundColor}`);
  assert(foot.scrollWidth <= foot.clientWidth + 1, "The waiting row overflows the box");

  // ---- the empty field answers it; a field with text queues --------------------
  field.focus();
  typeInto("Use the staging config instead");
  keydown(field, "Enter", { metaKey: true });
  await frames();
  assert(calls.at(-1) === "queue:Use the staging config instead" && !calls.includes("approve:ap-1"),
    `⌘↵ with text in the field did ${calls.at(-1)}`);
  assert(field.value === "", "The queued draft stayed in the field");
  keydown(field, "Enter", { metaKey: true });
  await frames(2);
  assert(calls.at(-1) === "approve:ap-1", `⌘↵ from the empty field did ${calls.at(-1)}`);
  assert(document.activeElement === field, "Answering from the field moved the caret out of it");
  assert(dockCard()?.textContent?.includes("Allow sent") === true, "The docked card did not say Allow sent");
  assert(box.classList.contains("is-working"), "An answered request still holds the box in amber");
  // A second request, an edit: its file and diff, then ⌘⌫ opens Deny's reason.
  flushSync(() => {
    control.approval({ kind: "approval", id: "ap-2", tool: "Edit", summary: "/repo/src/auth.ts",
      input: { file_path: "/repo/src/auth.ts", old_string: "return null;",
        new_string: "return session.user;" } });
  });
  await frames(2);
  const edit = dockCard()!;
  assert(edit.querySelector(".thread-dock-title")?.textContent === "Edit a file?", "The edit is not asked");
  assert(edit.querySelector(".thread-dock-path")?.textContent === "src/auth.ts"
    && edit.querySelector(".file-badge") !== null, "The edit's file row is missing");
  assert(edit.querySelector(".thread-dock-stat")?.textContent === "+1−1", "The edit's counts are missing");
  assert(edit.querySelectorAll(".thread-dock-line").length === 2, "The edit's diff is missing");
  keydown(field, "Backspace", { metaKey: true });
  await frames(2);
  const reason = document.activeElement;
  assert(reason instanceof HTMLInputElement && reason.getAttribute("aria-label") === "Reason for denying",
    "⌘⌫ from the empty field did not open Deny's reason");
  keydown(reason, "Enter");
  await frames(2);
  assert(calls.at(-1) === "deny:ap-2:", `Confirming the deny did ${calls.at(-1)}`);
  // The pane's own chord, from outside any field, answers a docked card at
  // once: it is outside the transcript and always on screen.
  flushSync(() => {
    control.approval({ kind: "approval", id: "ap-3", tool: "Bash", summary: "ls",
      input: { command: "ls" } });
  });
  await frames(2);
  conv.focus();
  keydown(conv, "Enter", { metaKey: true });
  await frames(2);
  assert(calls.at(-1) === "approve:ap-3", `The pane's chord did ${calls.at(-1)} for a docked card`);

  // ---- the compact queue -------------------------------------------------------
  flushSync(() => {
    control.queue([{ id: "q1", text: "Then update the README with the new login flow" },
      { id: "q2", text: "Run the linter" }]);
  });
  await frames(2);
  const queueCard = conv.querySelector<HTMLElement>(".steering-queue.is-compact");
  assert(queueCard !== null, "A Threads pane does not draw the compact queue");
  // Bare-CLI chrome draws every queue as bare lines (theme.css); refined
  // draws the compact card's dashed edge.
  assert(chrome === "cli" || getComputedStyle(queueCard).borderTopStyle === "dashed",
    `The queue card's edge is ${getComputedStyle(queueCard).borderTop}`);
  assert(queueCard.querySelector(".steering-queue-title")?.textContent === "QueuedQueued follow-ups",
    "The queue's micro-label lost its words");
  const row = queueCard.querySelector<HTMLElement>(".steering-queue-item")!;
  const earlier = row.querySelector<HTMLButtonElement>('[aria-label="Move queued prompt 1 later"]')!;
  assert(getComputedStyle(earlier).display === "none", "Reordering shows before hover or focus");
  row.focus();
  await frames();
  assert(getComputedStyle(earlier).display !== "none", "Focusing a queued row does not show reordering");
  const copy = row.querySelector<HTMLElement>(".steering-queue-copy")!;
  const oneLine = parseFloat(getComputedStyle(copy).lineHeight);
  assert(copy.getBoundingClientRect().height < 1.5 * oneLine,
    `A queued prompt takes more than one line: ${copy.getBoundingClientRect().height}px at ${oneLine}px`);
  press(row.querySelector<HTMLButtonElement>('[aria-label="Send queued prompt 1 now"]')!);
  assert(calls.at(-1) === "queue-send:q1", `Send now did ${calls.at(-1)}`);

  // An open editor tells the caller, which holds delivery at that row: the
  // owner's words are never lost to a turn ending, and Save hands them over
  // before the editor reports itself closed.
  press(row.querySelector<HTMLButtonElement>('[aria-label="Edit queued prompt 1"]')!);
  await frames();
  assert(calls.at(-1) === "queue-editing:q1:true", `Opening the editor did ${calls.at(-1)}`);
  const editor = queueCard.querySelector<HTMLTextAreaElement>(".steering-queue-textarea")!;
  const rewritten = "Then update the README and the changelog";
  flushSync(() => {
    Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!.call(editor, rewritten);
    editor.dispatchEvent(new Event("input", { bubbles: true }));
  });
  // Another row leaving (delivered ahead of it) keeps the editor and its words.
  flushSync(() => { control.queue([{ id: "q1", text: "Then update the README with the new login flow" }]); });
  await frames();
  assert(queueCard.querySelector<HTMLTextAreaElement>(".steering-queue-textarea")?.value === rewritten,
    "The queue changing under an open editor lost the owner's words");
  assert(calls.at(-1) === "queue-editing:q1:true", `A queue change closed the editor: ${calls.at(-1)}`);
  press([...queueCard.querySelectorAll<HTMLButtonElement>(".steering-queue-edit-actions button")]
    .find(button => button.textContent === "Save")!);
  await frames();
  assert(calls.slice(-2).join(" | ") === `queue-edit:q1:${rewritten} | queue-editing:q1:false`,
    `Save did ${calls.slice(-2).join(" | ")}`);
  press(queueCard.querySelector<HTMLButtonElement>('[aria-label="Edit queued prompt 1"]')!);
  await frames();
  press([...queueCard.querySelectorAll<HTMLButtonElement>(".steering-queue-edit-actions button")]
    .find(button => button.textContent === "Cancel")!);
  await frames();
  assert(calls.slice(-2).join(" | ") === "queue-editing:q1:true | queue-editing:q1:false",
    `Cancel did ${calls.slice(-2).join(" | ")}`);
  flushSync(() => {
    control.queue([{ id: "q1", text: "Then update the README with the new login flow" },
      { id: "q2", text: "Run the linter" }]);
  });
  await frames();
  queueCard.querySelector<HTMLElement>(".steering-queue-item")!.focus();
  await frames();

  // ---- the pose the screenshot keeps ----------------------------------------
  if (pose === "waiting") {
    // An edit waiting on the owner above the box, with two follow-ups queued.
    flushSync(() => {
      control.approval({ kind: "approval", id: "ap-4", tool: "Edit", summary: "/repo/src/auth.ts",
        input: { file_path: "/repo/src/auth.ts",
          old_string: "export function logout() {\n  session = null;\n}",
          new_string: "export function logout() {\n  session?.end();\n  session = null;\n}" } });
    });
    field.blur();
  } else {
    flushSync(() => {
      control.approval(null);
      control.queue([]);
      control.status("idle");
      control.totals(null);
    });
  }
  if (pose !== "busy" && pose !== "waiting") {
    flushSync(() => { control.thinking(false); });
    await frames();
  }
  if (pose === "menu") {
    // A draft, so the capture shows Send as the accent square.
    typeInto("Add a test for logout() when nobody is signed in.");
    await frames();
    plus.focus();
    keydown(plus, "ArrowDown");
  }
  await frames(2);
  return { theme, chrome, pose, width: Math.round(rect.width), radius, calls };
}

export function clearThreadComposerFixture(root: Root): void {
  flushSync(() => root.render(null));
}
