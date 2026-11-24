import type { EngineId, EngineEvent, SessionKey } from "../shared/engine";
import type { EngineProbe } from "../shared/ipc";
import { availableAccessModes } from "./access-modes";
import { fallbackEffortLevels } from "./app-session-projection";
import { concreteEffortLevels, type EngineChoices } from "./composer-model";
import { knownModelOptions, mergeModelOptions, modelOptionsFromValues } from "./model-catalog";
import { builtinModelFallback } from "./model-fallbacks";

/**
 * The renderer's engine table and the choices each engine's pickers offer:
 * cold-start rows for the built-in engines, then whatever the probe and the
 * engine's own sessions report. App owns when these are asked for (choicesFor
 * caches one answer per engine per catalogue revision); this owns the answer.
 */

// Claude's complete string-valued PermissionMode union in the installed Agent
// SDK. Keep the cold-start fallback complete; once probe succeeds the
// provider-reported list below is authoritative.
const PERMISSION_MODES = [
  "default", "acceptEdits", "plan", "auto", "dontAsk", "bypassPermissions",
];
export const BUILT_IN_ENGINES: readonly EngineId[] = [
  "claude", "codex", "acp:grok", "acp:qwen",
];
const GROK_FALLBACK_MODELS = builtinModelFallback("acp:grok");
const QWEN_FALLBACK_MODELS = builtinModelFallback("acp:qwen");

// THE RENDERER'S ENGINE TABLE — cold-start launch-form choices, per engine.
// Which engines EXIST is never decided here: that comes from probeEngines,
// which reports one entry per adapter the main process registered. This only
// answers "what should the pickers offer for engine X before X's own sessions
// have said anything", and an engine with no entry falls through to the empty
// set below rather than borrowing another engine's model names.
export const ENGINE_CHOICES: Record<string, EngineChoices> = {
  claude: {
    models: [], efforts: fallbackEffortLevels("claude"), permissionModes: PERMISSION_MODES,
    chat: true,
    // The SDK's own three sources, in the SDK's own words — 'user' is
    // ~/.claude/settings.json, 'project' is .claude/settings.json (and the only
    // one that makes CLAUDE.md load), 'local' is .claude/settings.local.json.
    // Naming them here is what makes the composer's scoping control appear at
    // all: an engine that has no such axis (Codex) passes nothing and the
    // control stays hidden rather than offering a setting it would ignore.
    settingSources: ["user", "project", "local"],
    // CLAUDE ONLY, and the reason is the same one that hides the scoping
    // control from Codex: this flag says the engine HONOURS
    // LaunchOptions.disallowedTools. The Claude adapter passes it straight to
    // the SDK, which removes those tools from the model's context entirely —
    // it is the one control in this app that actually restricts an agent, as
    // opposed to describing what it may ask for. Codex ignores both tool lists,
    // so it never gets this flag; offering a fence that engine cannot build
    // would be the same lie the removed allow-list told.
    toolDenyList: true,
  },
  // CODEX. AskForApproval has three string policies. `never` means never ask
  // for approval and return failures to the model; it does NOT disable the
  // separate Codex sandbox, so it is intentionally not classified as the
  // full-access/bypass mode below. Codex also supports a structured granular
  // policy, but the current provider-neutral permissionMode contract is a
  // string and must not smuggle that object through JSON text.
  //
  // Models and efforts are deliberately absent: this renderer has no verified
  // list for either, and `knownModelValues` fills the model picker from what
  // this engine's own sessions have reported. Inventing names would put
  // suggestions in front of the user that the CLI can only reject.
  codex: {
    models: [], efforts: [],
    permissionModes: ["untrusted", "on-request", "never"],
  },
  "acp:grok": {
    models: GROK_FALLBACK_MODELS.map(model => model.value),
    modelOptions: GROK_FALLBACK_MODELS,
    // Shown when the probe catalogue has not arrived yet. Grok's live menu is
    // low/medium/high/xhigh; hiding the picker behind a successful probe made
    // the control disappear for a healthy install.
    efforts: ["low", "medium", "high", "xhigh"],
    // The Grok CLI parser names more compatibility policies, but its ACP
    // `agent stdio` transport currently enforces only these three at spawn.
    // Never advertise a mode this adapter cannot actually apply.
    permissionModes: ["default", "auto", "bypassPermissions"],
    dangerousModes: ["bypassPermissions"],
  },
  "acp:qwen": {
    models: QWEN_FALLBACK_MODELS.map(model => model.value),
    modelOptions: QWEN_FALLBACK_MODELS,
    // The built-in local profile fixes its model through process settings. Qwen
    // Code ACP exposes neither a portable effort setter nor non-default access.
    efforts: [],
    permissionModes: ["default"],
  },
};

/**
 * Modes that hand an agent unsupervised execution. Named here as well as in the
 * composer because the MID-SESSION control has a stricter rule than the launch
 * one: `bypassPermissions` additionally needs `allowDangerouslySkipPermissions`
 * at launch, a flag that cannot be added to a running query, and the Claude
 * adapter therefore REFUSES it mid-session by design. Offering it in the
 * conversation picker would be offering a rejection.
 *
 * Only "bypassPermissions" is listed, and deliberately not Codex's "never":
 * Codex `never` suppresses approval prompts but leaves its independent sandbox
 * in force. Calling it full access would be materially false.
 */
export const DANGEROUS_MODES = ["bypassPermissions"];
// The graceful fallback: offer nothing until the engine names a vocabulary.
// An empty efforts list hides the effort control — an engine we know nothing
// about is assumed to have no effort axis until it says otherwise.
export const UNKNOWN_ENGINE_CHOICES: EngineChoices = {
  models: [], efforts: [], permissionModes: ["default"],
};

/**
 * Pure lookup: provider probe catalogue first, then any catalogue a live
 * session reported, then value-only capabilities. A non-empty observed
 * catalogue is authoritative; built-in rows are a cold/down-probe fallback,
 * not stale additions to a newer provider list.
 */
export function engineChoices(engine: string, health: readonly EngineProbe[],
    events: Record<SessionKey, EngineEvent[]>): EngineChoices {
  const configured = ENGINE_CHOICES[engine];
  const base = configured ?? UNKNOWN_ENGINE_CHOICES;
  const info = health.find(h => h.engine === engine)?.info;
  const capabilities = info?.capabilities;
  const capabilityModels = Array.isArray(capabilities?.models)
    ? modelOptionsFromValues(capabilities.models) : [];
  const observedModels = mergeModelOptions(
    info?.modelCatalog,
    knownModelOptions(events, engine),
    capabilityModels,
  );
  const fallbackModels = mergeModelOptions(
    base.modelOptions,
    modelOptionsFromValues(base.models),
  );
  const models = observedModels.length > 0 ? observedModels : fallbackModels;
  const catalogueEfforts: string[] = [];
  for (const model of models) {
    for (const effort of model.effortLevels) {
      if (!catalogueEfforts.includes(effort)) catalogueEfforts.push(effort);
    }
  }
  const engineEfforts = capabilities?.effortLevels ?? [];
  const reportedEfforts = concreteEffortLevels(
    engineEfforts.length > 0 ? engineEfforts
      : catalogueEfforts.length > 0 ? catalogueEfforts : base.efforts);
  // A successful provider probe is authoritative even for built-in engines.
  // The curated table exists only for cold start and carries renderer-only
  // safety metadata; it must never hide a newly reported provider mode.
  const reportedModes = availableAccessModes(
    base.permissionModes, capabilities?.permissionModes);
  return {
    ...base,
    models: models.map(model => model.value),
    modelOptions: models,
    efforts: reportedEfforts,
    permissionModes: reportedModes,
  };
}
