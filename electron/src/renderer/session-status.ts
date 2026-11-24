import type { ContextUsage, ModelOption } from "../shared/engine";
import { formatContextTokens } from "../shared/engine";
import { contextPercentText } from "./conversation-reducer";
import {
  accessModeOption, accessModeTone, providerAccessStatus, type AccessModeTone,
} from "./access-modes";
import { effortDisplayName } from "./effort-levels";
import { homeRelativePath } from "./display-paths";
import { findCatalogModel } from "./model-catalog";
import { formatTurnElapsed } from "./session-activity";

/**
 * Claude Code's footer: two coloured lines under the prompt — model · tokens
 * on the first, mode · effort · a real shortcut on the second. That copy
 * belongs on the composer control row, at Extra-high size, not in the 22px
 * app status bar.
 */
export interface SessionStatusBarView {
  model: string;
  tokens: string;
  mode: string;
  modeTone: AccessModeTone;
  modeMark: string;
  effort: string;
}

const MODE_MARK: Record<AccessModeTone, string> = {
  auto: "▶▶",
  accept: "››",
  plan: "○",
  danger: "!",
  neutral: "›",
};

const MODE_STATUS: Record<string, string> = {
  auto: "auto mode on",
  acceptEdits: "accept edits on",
  plan: "plan mode on",
  bypassPermissions: "bypass on",
  dontAsk: "don't ask",
  never: "never ask",
  default: "default mode",
  untrusted: "ask untrusted",
  "on-request": "ask when requested",
};

/** The real binding this footer may advertise. Shift+tab is Claude Code's, not ours. */
export const SESSION_STATUS_HINT = "⌃⇥ agents";

/** Million-scale the way Claude Code writes a 1M window: `1.0M`, not `1000k`. */
export function formatStatusBarTokens(n: number): string {
  if (!Number.isFinite(n) || n < 0) return "";
  if (n >= 1_000_000) {
    const millions = n / 1_000_000;
    return `${millions >= 10 ? String(Math.round(millions)) : millions.toFixed(1)}M`;
  }
  return formatContextTokens(n);
}

export function sessionStatusTokens(usage: ContextUsage | undefined): string {
  if (usage === undefined) return "";
  if (usage.compacting === true) return "compacting";
  const used = usage.used;
  const size = usage.size;
  const usedLabel = used === undefined ? "" : formatStatusBarTokens(used);
  const sizeLabel = size === undefined ? "" : formatStatusBarTokens(size);
  const span = usedLabel !== "" && sizeLabel !== ""
    ? `${usedLabel} / ${sizeLabel} tokens`
    : usedLabel !== "" ? `${usedLabel} tokens`
      : sizeLabel !== "" ? `${sizeLabel} window`
        : "";
  const pct = contextPercentText(usage);
  if (span !== "" && pct !== "") return `${span} · ${pct}`;
  return span !== "" ? span : pct;
}

export function sessionStatusMode(permissionMode: string | undefined, engine?: string): {
  label: string;
  tone: AccessModeTone;
  mark: string;
} {
  const value = permissionMode?.trim() ?? "";
  if (value === "") {
    return { label: "", tone: "neutral", mark: MODE_MARK.neutral };
  }
  const tone = accessModeTone(value);
  // The provider's own status-line words first ("manual mode on" for Claude
  // Code, "always-approve on" for Grok), then the generic ones.
  const known = providerAccessStatus(value, engine) ?? MODE_STATUS[value];
  const label = known ?? `${accessModeOption(value, engine).label.toLocaleLowerCase()}`;
  return { label, tone, mark: MODE_MARK[tone] };
}

export function sessionStatusModel(
    modelId: string | undefined,
    models: readonly ModelOption[] | undefined,
    engineLabel?: string,
    engineId?: string): string {
  const id = modelId?.trim() ?? "";
  if (id !== "" && id !== "default") {
    // The engine when the caller has it: the `[1m]` alias strip is Claude's, and
    // reading `gpt-5.6-luna[anything]` as a catalogued row is not this row's job.
    const match = findCatalogModel(id, models ?? [], engineId);
    const name = match?.displayName.trim() || id;
    if (name !== "") return name;
  }
  return engineLabel?.trim() ?? "";
}

export function sessionStatusBar(input: {
  modelId?: string;
  models?: readonly ModelOption[];
  engineLabel?: string;
  /** The session's engine id, so the mode reads in that provider's own words. */
  engine?: string;
  effort?: string | null;
  permissionMode?: string;
  usage?: ContextUsage;
}): SessionStatusBarView {
  const effortRaw = input.effort?.trim() ?? "";
  const effort = effortRaw !== "" && effortRaw !== "default"
    ? effortDisplayName(effortRaw) : "";
  const mode = sessionStatusMode(input.permissionMode, input.engine);
  return {
    model: sessionStatusModel(input.modelId, input.models, input.engineLabel),
    tokens: sessionStatusTokens(input.usage),
    mode: mode.label,
    modeTone: mode.tone,
    modeMark: mode.mark,
    effort,
  };
}

/**
 * THE MODE IS NOT IN THIS NAME. The access mode is a pane-header control now,
 * with its own accessible name, and a screen reader that meets it again here
 * reads one setting twice for one pane.
 */
export function sessionStatusBarLabel(view: SessionStatusBarView): string {
  return [view.model, view.tokens, view.effort].filter(part => part !== "").join(", ");
}

export interface WelcomeLines {
  /** `<engine> <version>` — the version half is dropped when unknown. */
  title: string;
  /** `<model> · <effort> effort · <plan>` — each part dropped when unknown. */
  detail: string;
  /** The project directory, `~`-relative. Empty when the caller has none. */
  path: string;
}

/**
 * The three lines Claude Code opens a session with.
 *
 * EVERY FIELD IS OPTIONAL AND EVERY ABSENCE IS SILENT. The engine's version
 * comes from a probe that may not have answered (EngineProbe.version is
 * optional); the plan comes from a usage read that is lazy and often has not
 * run; the model may be the engine's own default with no id to print. A welcome
 * block that guesses at any of them is worse than one that says less.
 */
export function welcomeLines(input: {
  engineLabel?: string;
  version?: string;
  model?: string;
  effort?: string | null;
  plan?: string;
  path?: string;
}): WelcomeLines {
  const engine = input.engineLabel?.trim() ?? "";
  // THE NUMBER, NOT THE LINE. EngineProbe.version is raw `--version` stdout, and
  // no two engines write it the same way: claude prints "2.1.251 (Claude Code)"
  // and codex prints "codex-cli 0.150.1". Both would stutter against the engine
  // name already beside them, so only the dotted number survives — and a line
  // with no number in it contributes nothing rather than a guess.
  const version = /\d+(?:\.\d+)+/.exec(input.version ?? "")?.[0] ?? "";
  const model = input.model?.trim() ?? "";
  const effortRaw = input.effort?.trim() ?? "";
  const effort = effortRaw !== "" && effortRaw !== "default"
    ? `${effortDisplayName(effortRaw)} effort` : "";
  const plan = input.plan?.trim() ?? "";
  return {
    title: [engine, version].filter(part => part !== "").join(" "),
    detail: [model, effort, plan].filter(part => part !== "").join(" · "),
    path: homeRelativePath(input.path?.trim() ?? ""),
  };
}

/**
 * `Working… (2m 14s · ↓ 1.5k)`.
 *
 * `tokens` IS ALWAYS UNDEFINED TODAY and the parameter is not speculative
 * padding: the spec makes the figure conditional ("shown only when the turn's
 * usage is known"), and this is where that condition lives. What the app has is
 * `usage.updated`, which is a session-cumulative total with zero sentinels — see
 * conversationItems, which drops it. Differencing it per turn would print a
 * wrong number, so nothing calls this with one.
 *
 * `label` is the word the busy strip is saying — "Thinking", a live tool name,
 * or the default. Both chromes read the same one so the CLI line cannot report
 * a different activity from the refined strip beside it.
 */
export function workingLineText(elapsedMs: number, tokens?: number,
    label = "Working"): string {
  const parts = [formatTurnElapsed(elapsedMs)];
  if (tokens !== undefined) parts.push(`↓ ${formatStatusBarTokens(tokens)}`);
  return `${label.trim() === "" ? "Working" : label.trim()}… (${parts.join(" · ")})`;
}

/** `Turn finished · 2m 14s`. Same token rule as workingLineText. */
export function doneLineText(elapsedMs: number, tokens?: number): string {
  const parts = ["Turn finished", formatTurnElapsed(elapsedMs)];
  if (tokens !== undefined) parts.push(`↓ ${formatStatusBarTokens(tokens)}`);
  return parts.join(" · ");
}

/**
 * `1.2k / 200k · 41%`. sessionStatusTokens says the same thing with the word
 * "tokens" in it, which is right for an accessible name and wrong for a status
 * line whose whole job is to be scannable at 11px.
 */
export function sessionMeterText(usage: ContextUsage | undefined): string {
  if (usage === undefined) return "";
  if (usage.compacting === true) return "compacting";
  const used = usage.used === undefined ? "" : formatStatusBarTokens(usage.used);
  const size = usage.size === undefined ? "" : formatStatusBarTokens(usage.size);
  // A LONE WINDOW SIZE IS NOT A MEASUREMENT. `200k · 41%` reads as 200k used,
  // so the denominator only prints when there is a numerator in front of it.
  const span = used === "" ? "" : size === "" ? used : `${used} / ${size}`;
  const pct = contextPercentText(usage);
  return [span, pct].filter(part => part !== "").join(" · ");
}

/**
 * The hint line. `esc interrupt` appears only while a turn is running, because
 * a hint for a key that does nothing is the failure SESSION_STATUS_HINT already
 * exists to prevent.
 */
export function sessionLine2(running: boolean): string {
  return [running ? "esc interrupt" : "", SESSION_STATUS_HINT, "/ commands"]
    .filter(part => part !== "").join(" · ");
}
