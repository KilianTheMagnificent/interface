import type {
  EngineId, LaunchOptions, ModelOption, ProviderCommand, ProviderCommandCatalogue,
  ProviderCommandScope, SessionKind,
} from "../shared/engine";
import { autoPermissionMode } from "../shared/engine";
import type { AppSelectOption } from "./components/AppSelect";
import { findCatalogModel } from "./model-catalog";
import type { ComposerForm } from "./launch";
import {
  latestModelPresetFor, modelPresetFor, type ModelPreset, type ModelPresetSettings,
} from "./model-presets";
import { providerLabel } from "./provider-labels";
import { lastSegment } from "./display-paths";
import { productLabel } from "./product";
import { canonicalProviderCommandText, providerCommandIntent } from "./provider-commands";

/**
 * The launch card's pure model: engine choices, scope and team fields, the
 * access-mode and effort vocabularies, the combined provider+model choice
 * codec, command discovery scope, and form seeding. `components/Composer.tsx`
 * renders it; nothing here touches React or the DOM.
 */
// One engine this build can run, as the main process reported it. `unavailable`
// is the probe's own reason when that engine is down — the option stays
// selectable (the CLI may come back before the user hits Launch) but says so.
export interface EngineOption {
  id: string;
  /** Provider-owned display name; the stable engine id still routes every operation. */
  label?: string;
  unavailable?: string;
}

// What one engine offers the launch form. Model and permission-mode vocabularies
// are per engine, and effort is not even a universal axis, so all three arrive
// as a lookup rather than as three flat lists: the composer asks for the engine
// the user has selected right now.
export interface EngineChoices {
  /** Concrete provider model values. Provider-default sentinels are ignored. */
  models: string[];
  /** Rich provider rows when the probe reports them. Values remain in `models`
   *  too so older integrations and value-only engines keep working. */
  modelOptions?: ModelOption[];
  /** Concrete selectable effort levels. An empty list means this engine has
   *  no effort axis, and the control is hidden rather than shown empty. */
  efforts: string[];
  permissionModes: string[];
  /**
   * The on-disk settings vocabulary this engine understands — for Claude,
   * ["user", "project", "local"]. ABSENT OR EMPTY HIDES THE CONTROL, which is
   * the safe default in both directions: an engine that ignores
   * LaunchOptions.settingSources never shows a picker that does nothing, and an
   * integrator who has not wired this gets today's behaviour (everything loads)
   * rather than a silently narrowed session.
   *
   * Pass ["user", "project", "local"] for claude or the scoping control will
   * not appear at all.
   */
  settingSources?: string[];
  /**
   * True only for an engine that HONOURS `LaunchOptions.disallowedTools` by
   * removing those tools from the agent's toolset. ABSENT OR FALSE HIDES THE
   * TOOLS CONTROL, and that is the safe default in both directions: Codex
   * ignores the field outright (its adapter emits a "dropped" notice after the
   * agent is already running), so offering the box there would be a restriction
   * that never applies — the exact class of lie this control has already told
   * once.
   *
   * Pass `toolDenyList: true` for claude or the tools control will not appear.
   */
  toolDenyList?: boolean;
  /**
   * Modes that hand the agent unsupervised execution and must never be one
   * click away. Defaults to ["bypassPermissions"], the only mode this project
   * has verified ground truth for (it also requires the adapter to pass
   * allowDangerouslySkipPermissions — see composerScopeOptions). Modes named
   * here remain visible in the picker, but selecting one only opens the
   * two-step confirmation in the scope panel; it never changes the form directly.
   */
  dangerousModes?: string[];
  /** True for an engine that can run a tool-less chat (Claude). Absent hides
   *  the engine from the Chat composer. */
  chat?: boolean;
}

/** Build the exact pre-launch catalogue identity, including Claude isolation. */
export function composerProviderCommandScope(form: ComposerLaunchForm,
    choices: EngineChoices): ProviderCommandScope | null {
  const engine = form.engine.trim();
  const cwd = form.cwd.trim();
  const model = form.model.trim();
  if (engine === "" || cwd === "" || model === "" || model === "default") return null;
  const supportsSettingSources = (choices.settingSources ?? []).length > 0;
  const settingSources = supportsSettingSources
    ? composerScopeOptions(form).settingSources : undefined;
  return {
    engine: engine as EngineId,
    cwd,
    model,
    ...(settingSources === undefined ? {} : { settingSources: [...settingSources] }),
  };
}

function sameSettingSources(a: readonly string[] | undefined,
    b: readonly string[] | undefined): boolean {
  if (a === undefined || b === undefined) return a === b;
  return a.length === b.length && a.every((value, index) => value === b[index]);
}

/** Raw cwd is compared only between the current form and its own request. */
export function composerProviderCommandScopesEqual(a: ProviderCommandScope | null,
    b: ProviderCommandScope | null): boolean {
  return a !== null && b !== null && a.engine === b.engine && a.cwd === b.cwd
    && a.model === b.model && sameSettingSources(a.settingSources, b.settingSources);
}

/**
 * The bridge canonicalizes cwd, so request generation—not raw path equality—is
 * the cwd stale-response guard. Provider, model and settings must still echo
 * exactly or the answer cannot populate this launch draft.
 */
export function composerProviderCommandResponseMatches(
    result: ProviderCommandCatalogue, requested: ProviderCommandScope): boolean {
  return result.scope.engine === requested.engine && result.scope.model === requested.model
    && sameSettingSources(result.scope.settingSources, requested.settingSources);
}

export type ComposerCommandPromptResolution =
  | { kind: "prompt"; prompt: string }
  | { kind: "command"; prompt: string; command: ProviderCommand }
  | { kind: "blocked"; message: string };

/** Apply the same fail-closed slash policy used by a live Conversation. */
export function resolveComposerCommandPrompt(commands: readonly ProviderCommand[],
    catalogueKnown: boolean, prompt: string, providerLabel: string):
    ComposerCommandPromptResolution {
  const intent = providerCommandIntent(commands, catalogueKnown, prompt.trim());
  if (intent === null) return { kind: "prompt", prompt };
  if (intent.kind === "command") {
    return {
      kind: "command",
      command: intent.command,
      prompt: canonicalProviderCommandText(intent.command, prompt),
    };
  }
  const message = intent.reason === "reserved"
    ? `${productLabel()} did not send /${intent.name}. Use the visible model, effort, access, or session controls instead.`
    : intent.reason === "multiline"
    ? `${productLabel()} did not send /${intent.name}. Provider commands must stay on one line.`
    : `${providerLabel} does not advertise /${intent.name} for this model and project. Nothing was sent.`;
  return { kind: "blocked", message };
}

/* ---- per-session scoping (LaunchOptions Phase 4 fields) -------------------
   These four are not on ComposerForm, which is `src/renderer/launch.ts`'s type
   and is shaped for buildLaunchOptions. They ride alongside it, and this file
   resolves them into the LaunchOptions fields the adapters take. */

/**
 * Which on-disk settings a session loads, as the three cases worth offering:
 *
 *  - "all"      omit settingSources entirely → the CLI default, everything on
 *               disk. TODAY'S BEHAVIOUR, and the honest framing matters: that
 *               is the user's whole global ~/.claude, every hook, skill and MCP
 *               server, on every session — a three-word prompt measured 58 KB.
 *  - "project"  ["project"] → the project's own .claude AND its CLAUDE.md
 *    (including files CLAUDE.md imports, such as AGENTS.md),
 *               without the global setup.
 *  - "isolated" [] → the SDK's isolation mode. Nothing, INCLUDING CLAUDE.md
 *    or files it imports.
 *
 * Three named presets rather than a checkbox per source, deliberately: 'project'
 * is what makes CLAUDE.md load, so a raw checkbox matrix hands the user a click
 * that silently turns off every standing instruction in their repo without ever
 * saying the word CLAUDE.md. Each preset states its consequence in the panel.
 */
export type SettingScope = "all" | "project" | "isolated";

/**
 * The composer's own fields for the scoping controls, as the form holds them.
 *
 * THERE IS NO ALLOW-LIST FIELD, and its absence is the point. This card used to
 * carry one labelled "allow only", wired to `LaunchOptions.allowedTools`, which
 * the Agent SDK documents as the OPPOSITE of a restriction (sdk.d.ts, the
 * `allowedTools` option): *"List of tool names that are auto-allowed without
 * prompting for permission. These tools will execute automatically without
 * asking the user for approval. To restrict which tools are available, use the
 * `tools` option instead."* Typing `Bash` there restricted nothing and removed
 * every approval prompt for Bash — per-tool bypassPermissions, one word deep,
 * in the same panel where that same power is guarded by a two-step red control.
 *
 * `disallowedTools` is the one of the pair that genuinely restricts (*"removed
 * from the model's context and cannot be used"*), so it is what the panel
 * offers. A real "only these tools exist" control needs `LaunchOptions.tools`,
 * which this app's engine contract does not carry; until it does, an allow-only
 * box here can only be a lie and is therefore absent rather than relabelled
 * into a second unguarded danger control.
 */
export interface ComposerScope {
  settingScope: SettingScope;
  /** Raw text from the field: comma-separated tool names. "" = engine default. */
  disallowedTools: string;
  worktree: boolean;
}

/**
 * A multimodel team, chosen at launch. The chat being started IS the lead — its
 * engine and model are the ones already picked in the toolbar above, never a
 * second copy of that choice here. What this adds is which OTHER providers the
 * lead may spawn workers from; they are its `modelAllowlist`, so no worker
 * exists until the lead decides it needs one.
 */
export interface ComposerTeam {
  team: boolean;
  /** Engine ids the lead may spawn. Empty with `team` on means "any engine". */
  teamEngines: string[];
  /** Refuse a read-only worker: this team's workers do the work themselves. */
  teamWorkersWrite: boolean;
  /** Routing guidance in the human's own words, handed to the lead verbatim. */
  teamNotes: string;
}

export const COMPOSER_TEAM_DEFAULTS: ComposerTeam = {
  team: false, teamEngines: [],
  // On by default: a team whose workers only ever read is a team that does not
  // divide the work, which is the thing a team is for.
  teamWorkersWrite: true,
  teamNotes: "",
};

/** Everything the launch form carries: the existing fields plus the scoping ones. */
export type ComposerLaunchForm = ComposerForm & ComposerScope & ComposerTeam;

/** What a caller seeds the launch card with; the scoping and team fields are optional. */
export type ComposerDefaults = ComposerForm & Partial<ComposerScope> & Partial<ComposerTeam>;

/** The LaunchOptions subset this file resolves. `allowedTools` is deliberately
 *  not in it — see ComposerScope. */
export type ScopeOptions =
  Pick<LaunchOptions, "settingSources" | "disallowedTools" | "worktree">;

/** Scoping at its most permissive: exactly what the app did before it existed. */
export const COMPOSER_SCOPE_DEFAULTS: ComposerScope = {
  settingScope: "all", disallowedTools: "", worktree: false,
};

export const DEFAULT_DANGEROUS_MODES = ["bypassPermissions"];

/** Cursor's agent composer identifies the current repository by its final path
 * segment, while keeping the complete path in the control's title. Accept both
 * separators because remembered sessions can come from a remote/Windows host. */
export function composerRepositoryName(cwd: string): string {
  return lastSegment(cwd.trim()) || "No Repo";
}

// "Read, Grep,, Read " -> ["Read", "Grep"]. undefined for an empty field, which
// is the difference between "restrict to nothing" and "do not restrict".
function parseToolList(raw: string): string[] | undefined {
  const names: string[] = [];
  for (const part of raw.split(",")) {
    const name = part.trim();
    if (name !== "" && !names.includes(name)) names.push(name);
  }
  return names.length > 0 ? names : undefined;
}

/**
 * The scoping fields, resolved into LaunchOptions. Handed to `onLaunch` as its
 * second argument so the caller spreads it and cannot forget it:
 *
 *   onLaunch={(form, scope) => {
 *     const opts = buildLaunchOptions(form);          // validates, returns {error}|LaunchRequest
 *     if ("error" in opts) return;
 *     window.iface.createSession({ ...opts, ...scope });
 *   }}
 *
 * A key omitted here means "say nothing to the engine", which is not the same
 * as an empty value: `settingSources: []` is SDK isolation mode, while omitting
 * it loads everything. Only "all" omits.
 *
 * NOTE FOR THE LAUNCH SIDE: `permissionMode: "bypassPermissions"` additionally
 * requires the Claude adapter to pass allowDangerouslySkipPermissions: true.
 * That flag is not a LaunchOptions field and cannot be set from here.
 */
export function composerScopeOptions(form: ComposerScope): ScopeOptions {
  const out: ScopeOptions = {};
  if (form.settingScope === "project") out.settingSources = ["project"];
  else if (form.settingScope === "isolated") out.settingSources = [];
  const denied = parseToolList(form.disallowedTools);
  if (denied !== undefined) out.disallowedTools = denied;
  if (form.worktree) out.worktree = true;
  return out;
}

const SCOPE_LABEL: Record<SettingScope, string> = {
  all: "full setup", project: "project only", isolated: "isolated",
};

/**
 * The non-default scope details shown by the collapsed Session options pill.
 * The ordinary `all` preset is deliberately omitted: it is still explained in
 * the open panel, but repeating "full setup" in the everyday launch toolbar
 * made the primary path read like a configuration form. Anything the user has
 * actually changed remains visible without reopening the panel.
 *
 * `show` is the set of controls the CURRENT engine actually offers, and the
 * summary is built from what will be SENT, never from what is typed: the pill
 * used to print "tools" for a field holding nothing but a stray comma, which
 * parseToolList drops, so it announced a restriction that never left the
 * window. Every branch here goes through the same resolver the launch does.
 */
export function scopeSummary(form: ComposerScope, dangerous: boolean,
    show: { sources: boolean; tools: boolean }): string {
  const parts: string[] = [];
  if (dangerous) parts.push("approval bypass");
  if (show.sources && form.settingScope !== "all") {
    parts.push(SCOPE_LABEL[form.settingScope]);
  }
  if (form.worktree) parts.push("worktree");
  const denied = show.tools ? parseToolList(form.disallowedTools) : undefined;
  if (denied !== undefined) {
    parts.push(`${denied.length} tool${denied.length === 1 ? "" : "s"} blocked`);
  }
  return parts.join(" · ");
}

/**
 * The compact Session options summary covers the advanced launch axes that
 * stay behind the disclosure. Effort is a first-class toolbar control, so it
 * is not repeated here. An empty string means every remaining option is at
 * its ordinary default, so the trigger can simply say "Session options".
 */
export function sessionOptionsSummary(
    form: ComposerScope & Partial<ComposerTeam>,
    dangerous: boolean,
    show: { sources: boolean; tools: boolean }): string {
  const scope = scopeSummary(form, dangerous, show);
  if (form.team !== true) return scope;
  const engines = form.teamEngines ?? [];
  const team = engines.length === 0
    ? "team, any engine" : `team of ${engines.length + 1}`;
  return scope === "" ? team : `${team} · ${scope}`;
}

/** Keep every provider-advertised access mode visible, dangerous ones included. */
/** The engine's non-blocking mode when it advertises one; its default otherwise. */
export function startingPermissionMode(engineId: string, choices: EngineChoices): string {
  const auto = autoPermissionMode(engineId);
  return auto !== undefined && choices.permissionModes.includes(auto) ? auto : "default";
}

export function composerAccessModes(current: string, offered: readonly string[]): string[] {
  const values = [...offered];
  return current !== "" && !values.includes(current) ? [current, ...values] : values;
}

/** Dangerous rows are visible but request confirmation instead of changing the form. */
export function composerAccessModeAction(value: string,
    dangerous: readonly string[]): "apply" | "confirm" {
  return dangerous.includes(value) ? "confirm" : "apply";
}

/** Drop the leave-it-alone sentinel. Effort menus only offer real levels. */
export function concreteEffortLevels(levels: readonly string[]): string[] {
  const out: string[] = [];
  for (const level of levels) {
    if (level !== "" && level !== "default" && !out.includes(level)) out.push(level);
  }
  return out;
}

/** First still-valid remembered or current level; otherwise the model's first advertised level. */
export function preferredEffort(
  levels: readonly string[],
  ...candidates: Array<string | null | undefined>
): string {
  const concrete = concreteEffortLevels(levels);
  for (const candidate of candidates) {
    if (candidate !== undefined && candidate !== null
        && candidate !== "" && candidate !== "default"
        && concrete.includes(candidate)) {
      return candidate;
    }
  }
  return concrete[0] ?? "";
}

/** The effort menu follows the selected provider model when it advertises a
 * model-specific vocabulary. A listed model that explicitly has no effort axis
 * hides the control; custom ids retain the engine-wide fallback. No model yet
 * hides the control so a launch cannot sit on an unremembered sentinel. */
export function effortsForModel(choices: EngineChoices, selectedModel: string,
    engineId?: string): string[] {
  const engineLevels = concreteEffortLevels(choices.efforts);
  if (selectedModel === "" || selectedModel === "default") return [];
  const selected = findCatalogModel(selectedModel, choices.modelOptions ?? [], engineId);
  if (selected === undefined) return engineLevels;
  if (!selected.supportsEffort) return [];
  return selected.effortLevels.length > 0
    ? concreteEffortLevels(selected.effortLevels)
    : engineLevels;
}

/** One globally-selectable launch model. The provider is part of the value so
 * identical model ids from two adapters can never route to the wrong one. */
export interface ComposerModelChoice { engine: string; model: string }

const COMPOSER_MODEL_CHOICE_PREFIX = "interface-model:";
const COMPOSER_CUSTOM_PROVIDER_PREFIX = "interface-custom-provider:";

export function encodeComposerModelChoice(engine: string, model: string): string {
  return `${COMPOSER_MODEL_CHOICE_PREFIX}${JSON.stringify([engine, model])}`;
}

export function decodeComposerModelChoice(value: string): ComposerModelChoice | null {
  if (!value.startsWith(COMPOSER_MODEL_CHOICE_PREFIX)) return null;
  try {
    const decoded: unknown = JSON.parse(value.slice(COMPOSER_MODEL_CHOICE_PREFIX.length));
    if (!Array.isArray(decoded) || decoded.length !== 2
        || typeof decoded[0] !== "string" || decoded[0].trim() === ""
        || typeof decoded[1] !== "string" || decoded[1].trim() === "") return null;
    return { engine: decoded[0], model: decoded[1] };
  } catch {
    return null;
  }
}

export function encodeComposerCustomProvider(engine: string): string {
  return `${COMPOSER_CUSTOM_PROVIDER_PREFIX}${JSON.stringify(engine)}`;
}

/** Recover the stable provider identity from either kind of combined-picker row. */
export function composerProviderForModelValue(value: string): string | null {
  const modelChoice = decodeComposerModelChoice(value);
  if (modelChoice !== null) return modelChoice.engine;
  if (!value.startsWith(COMPOSER_CUSTOM_PROVIDER_PREFIX)) return null;
  try {
    const engine: unknown = JSON.parse(value.slice(COMPOSER_CUSTOM_PROVIDER_PREFIX.length));
    return typeof engine === "string" && engine.trim() !== "" ? engine : null;
  } catch {
    return null;
  }
}

export function composerCustomModelChoice(
  customProviderValue: string | undefined,
  model: string,
  fallbackEngine: string,
): ComposerModelChoice {
  if (customProviderValue?.startsWith(COMPOSER_CUSTOM_PROVIDER_PREFIX)) {
    const engine = composerProviderForModelValue(customProviderValue);
    if (engine !== null) return { engine, model };
  }
  return { engine: fallbackEngine, model };
}

/** The name the model picker shows for `model`: its catalog display name, else the id. */
export function composerModelLabel(choices: EngineChoices, engine: string, model: string): string {
  return findCatalogModel(model, advertisedModels(choices), engine)?.displayName ?? model;
}

export const advertisedModels = (choices: EngineChoices): ModelOption[] => {
  const models = choices.modelOptions !== undefined && choices.modelOptions.length > 0
    ? [...choices.modelOptions]
    : choices.models.map(value => ({
        value,
        displayName: value,
        supportsEffort: false,
        effortLevels: [],
      } satisfies ModelOption));
  return models.filter(model => model.value.trim() !== ""
    && model.value !== "default");
};

/**
 * The access mode a launch remembers for its model, on a card that HAS an Access
 * mode control. An empty candidate is a field this engine does not offer rather
 * than a choice of none, so the mode the form already carries stays instead.
 *
 * A chat card has no such control at all and must not reach this function; see
 * composerChatPermissionMode for the rule that covers it.
 */
export function composerRememberedPermissionMode(
  candidate: string, carried: string,
): string {
  return candidate.trim() || carried.trim() || "default";
}

/**
 * The access mode a CHAT card may write for a model. There is no Access mode
 * control on that card, so nothing it carries is a choice about one — and the
 * preset store is shared with the Code card, so writing the chat form's seeded
 * "default" would silently reset a mode a Code card saved for the same model.
 * An existing preset therefore keeps its own mode untouched, and a model with no
 * preset yet gets the engine's neutral starting point, exactly as a fresh Code
 * card would have opened on it.
 */
export function composerChatPermissionMode(
  engineId: string, choices: EngineChoices, preset?: ModelPresetSettings,
): string {
  return preset?.permissionMode.trim() || startingPermissionMode(engineId, choices);
}

/**
 * The engines one surface may launch. A chat runs tool-less and with no folder,
 * so it offers only the engines that say they can do that (EngineChoices.chat).
 * An empty list is intentional: unsupported adapters refuse Chat before launch.
 */
export function composerKindEngines(
  kind: SessionKind,
  engines: readonly EngineOption[],
  choicesFor: (engine: string) => EngineChoices,
): readonly EngineOption[] {
  if (kind !== "chat") return engines;
  return engines.filter(engine => choicesFor(engine.id).chat === true);
}

/**
 * The form a card holds once its kind changes underneath it — the Threads card
 * moving between "No project" (a chat) and a project (a coding session).
 *
 * To a chat: an engine that cannot run one gives way to the first engine that
 * can, on its most recent model (or its first advertised one), so the card
 * never sits on a provider the chat picker does not even list. The per-launch
 * worktree toggle is off the chat card, so it is cleared as well.
 *
 * To a folder kind: a chat card carries "default" (or nothing) as its access
 * mode because it never offered the control. That is not a choice, so the
 * model's remembered mode, or the engine's own non-blocking mode, replaces it.
 * A mode the owner actually picked is left alone.
 */
export function composerFormForKind(
  form: ComposerLaunchForm,
  kind: SessionKind,
  engines: readonly EngineOption[],
  choicesFor: (engine: string) => EngineChoices,
  presets: readonly ModelPreset[],
): ComposerLaunchForm {
  if (kind === "chat") {
    const chatEngines = composerKindEngines("chat", engines, choicesFor);
    if (chatEngines.some(engine => engine.id === form.engine)) {
      return form.worktree ? { ...form, worktree: false } : form;
    }
    const engine = chatEngines.find(option => option.unavailable === undefined)?.id
      ?? chatEngines[0]?.id;
    if (engine === undefined) return { ...form, worktree: false };
    const choices = choicesFor(engine);
    const model = latestModelPresetFor(presets, engine)?.model
      ?? advertisedModels(choices)[0]?.value ?? "";
    if (model === "") return { ...form, engine, model: "", effort: "", worktree: false };
    return { ...composerFormForModelChoice(form, { engine, model }, choices,
      modelPresetFor(presets, engine, model)), worktree: false };
  }
  if (form.permissionMode !== "" && form.permissionMode !== "default") return form;
  const choices = choicesFor(form.engine);
  const remembered = modelPresetFor(presets, form.engine, form.model)?.permissionMode ?? "";
  const permissionMode = remembered !== "" && remembered !== "default"
      && choices.permissionModes.includes(remembered)
    ? remembered : startingPermissionMode(form.engine, choices);
  return permissionMode === form.permissionMode ? form : { ...form, permissionMode };
}

/**
 * Flatten every registered provider catalogue into one polished picker. Each
 * row carries its provider in both the visible label and encoded value. The
 * currently selected custom model is retained as a real row so the trigger
 * never loses the provider name after custom entry.
 */
export function composerModelOptions(
  engines: readonly EngineOption[],
  choicesFor: (engine: string) => EngineChoices,
  selectedEngine: string,
  selectedModel: string,
): AppSelectOption[] {
  const listed = engines.some(engine => engine.id === selectedEngine)
    ? engines
    : [{ id: selectedEngine }, ...engines];
  // Ready providers first, unavailable ones last, each in registration order:
  // two down providers above the one that works read as the menu's top.
  const down = (engine: EngineOption): boolean => (engine.unavailable?.trim() ?? "") !== "";
  const providers = [...listed.filter(engine => !down(engine)), ...listed.filter(down)];
  const seenProviders = new Set<string>();
  const rows: AppSelectOption[] = [];

  for (const provider of providers) {
    if (seenProviders.has(provider.id)) continue;
    seenProviders.add(provider.id);
    const label = providerLabel(provider.id, provider.label);
    const unavailable = provider.unavailable?.trim();
    const group = `${label}${
      unavailable ? " · Unavailable" : ""}`;
    const models = [...advertisedModels(choicesFor(provider.id))];
    if (provider.id === selectedEngine && selectedModel.trim() !== ""
        && selectedModel !== "default"
        && findCatalogModel(selectedModel, models, provider.id) === undefined) {
      models.push({
        value: selectedModel,
        displayName: selectedModel,
        description: `Custom ${label} model id.`,
        supportsEffort: false,
        effortLevels: [],
      });
    }

    for (const [index, model] of models.entries()) {
      const displayName = model.displayName;
      rows.push({
        value: encodeComposerModelChoice(provider.id, model.value),
        label: displayName,
        group,
        triggerLabel: `${label} · ${displayName}`,
        description: unavailable && index === 0
          ? `${label} is unavailable: ${unavailable}` : model.description,
        meta: model.value !== model.displayName
          ? model.value : undefined,
      });
    }
    rows.push({
      value: encodeComposerCustomProvider(provider.id),
      label: "Use custom model ID…",
      group,
      description: `Enter a ${label} model that is not listed above.`,
      custom: true,
      customLabel: `Custom ${label} model ID`,
      customPlaceholder: `${label} model id`,
    });
  }
  return rows;
}

/**
 * A fresh Threads card's model. The seeded one stays when its provider can
 * run; otherwise the most recently used model of a provider that is ready and
 * offered for this kind, else that kind's first ready provider's first
 * advertised model (the seeded provider's own first, when it is ready). A
 * card used to open on "Choose model" whenever Claude had no remembered
 * model, and on a down provider whenever the seed named one. Nothing ready:
 * the form is left as it is.
 */
export function composerFormOnReadyModel(
  form: ComposerLaunchForm,
  kind: SessionKind,
  engines: readonly EngineOption[],
  choicesFor: (engine: string) => EngineChoices,
  presets: readonly ModelPreset[],
): ComposerLaunchForm {
  const ready = composerKindEngines(kind, engines, choicesFor)
    .filter(engine => (engine.unavailable?.trim() ?? "") === "");
  const isReady = (id: string): boolean => ready.some(engine => engine.id === id);
  const model = form.model.trim();
  if (isReady(form.engine) && model !== "" && model !== "default") return form;
  const recent = presets.find(preset => preset.model !== "default" && isReady(preset.engine));
  let choice: ComposerModelChoice | undefined = recent === undefined
    ? undefined : { engine: recent.engine, model: recent.model };
  if (choice === undefined) {
    const engine = isReady(form.engine) ? form.engine : ready[0]?.id;
    const first = engine === undefined ? undefined : advertisedModels(choicesFor(engine))[0]?.value;
    if (engine !== undefined && first !== undefined) choice = { engine, model: first };
  }
  if (choice === undefined) return form;
  return composerFormForModelChoice(form, choice, choicesFor(choice.engine),
    modelPresetFor(presets, choice.engine, choice.model));
}

/**
 * Restore a remembered model preset only when its values still belong to the
 * target model/provider vocabularies. Provider upgrades can remove an effort
 * or access mode, and stale localStorage must never resurrect one.
 */
export function composerFormForModelChoice(
  current: ComposerLaunchForm,
  choice: ComposerModelChoice,
  choices: EngineChoices,
  preset?: ModelPresetSettings,
): ComposerLaunchForm {
  const providerChanged = choice.engine !== current.engine;
  const nextLevels = effortsForModel(choices, choice.model, choice.engine);
  const restoredEffort = preferredEffort(nextLevels, preset?.effort,
    providerChanged ? undefined : current.effort);
  const restoredPermission = preset !== undefined
      && (preset.permissionMode === "default"
        || choices.permissionModes.includes(preset.permissionMode))
    ? preset.permissionMode
    // A remembered choice wins; otherwise a new provider starts on its own
    // non-blocking mode rather than on a policy that stops to ask.
    : providerChanged ? startingPermissionMode(choice.engine, choices)
      : current.permissionMode;
  return {
    ...current,
    ...(providerChanged ? COMPOSER_SCOPE_DEFAULTS : {}),
    // Worktree isolation belongs to this launch, not to a provider. An
    // implicit provider switch through the model menu must not turn it off.
    worktree: current.worktree,
    // Nor does the team: changing the lead's own model is not a decision about
    // which other providers it may spawn.
    team: current.team,
    teamEngines: current.teamEngines,
    teamWorkersWrite: current.teamWorkersWrite,
    teamNotes: current.teamNotes,
    engine: choice.engine,
    model: choice.model,
    effort: restoredEffort,
    permissionMode: restoredPermission,
  };
}

// "form" carries any validation error that belongs to no single field.
export type FieldError = { field: "cwd" | "prompt" | "model" | "form"; message: string };

/**
 * Which field a rejected launch belongs to, in buildLaunchOptions' own order:
 * cwd, then prompt, then model, and a form-level line for any rule that is not
 * about an empty field. A CHAT HAS NO FOLDER — launch.ts skips the cwd rule for
 * it and this card renders no repository button — so a chat's error must never
 * be attributed to "cwd", which would print it above the dock beside a control
 * that is not on screen while leaving the field that IS wrong unmarked.
 */
export function composerErrorField(form: ComposerForm): FieldError["field"] {
  const kind = form.kind ?? "code";
  return kind !== "chat" && !form.cwd.trim() ? "cwd"
    : !form.prompt.trim() ? "prompt"
    : !form.model.trim() || form.model === "default" ? "model"
    : "form";
}

// The launch card's own CSS. It is injected rather than added to theme.css for
// the same reason the sidebar's archive control is: this component owns one
// file, and hover, focus-within and ::placeholder cannot be said in an inline
// style object. Every value below is a token from theme.css's :root — no colour
// literal, no geometry literal outside the space ramp — so a retheme is still a
// single-file edit there.
//
// The shape is Cursor's composer: one rounded box holding the prompt with a
// control row beneath it, inside the same border. The pickers in that row are
// pills — borderless for model and effort, one bordered round pill for the
// permission mode — and the submit sits at the right of the row with the
// keyboard hint beside it.

// `defaults` carries the scoping fields only if the caller has wired them. A
// key present with an explicit `undefined` is treated as absent rather than
// spread over the default — otherwise `{...form, worktree: maybeUndefined}`
// from a caller would put `undefined` into a field the pill prints and the
// checkbox binds to, which React reads as an uncontrolled input.
function seed(defaults: ComposerDefaults): ComposerLaunchForm {
  const scope: ComposerScope = { ...COMPOSER_SCOPE_DEFAULTS };
  if (defaults.settingScope !== undefined) scope.settingScope = defaults.settingScope;
  if (defaults.disallowedTools !== undefined) scope.disallowedTools = defaults.disallowedTools;
  if (defaults.worktree !== undefined) scope.worktree = defaults.worktree;
  const team: ComposerTeam = { ...COMPOSER_TEAM_DEFAULTS };
  if (defaults.team !== undefined) team.team = defaults.team;
  if (defaults.teamEngines !== undefined) team.teamEngines = [...defaults.teamEngines];
  if (defaults.teamWorkersWrite !== undefined) team.teamWorkersWrite = defaults.teamWorkersWrite;
  if (defaults.teamNotes !== undefined) team.teamNotes = defaults.teamNotes;
  return { ...defaults, ...scope, ...team };
}

/** Seed the launch form, then fill a blank effort from the preset or model ladder. */
export function seedLaunchForm(
  defaults: ComposerDefaults,
  choices: EngineChoices,
  preset?: ModelPresetSettings,
): ComposerLaunchForm {
  const base = seed(defaults);
  if (base.model === "" || base.model === "default") return base;
  const permission = preset?.permissionMode;
  return {
    ...base,
    effort: preferredEffort(effortsForModel(choices, base.model, base.engine),
      preset?.effort, base.effort),
    permissionMode: permission !== undefined && permission !== ""
      && (permission === "default" || choices.permissionModes.includes(permission))
      ? permission
      : base.permissionMode,
  };
}
