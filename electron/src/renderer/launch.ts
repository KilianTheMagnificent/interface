import type { EngineId, LaunchOptions, SessionKind } from "../shared/engine";

// The engine a session starts on when the composer offers no choice yet (the
// probe has not answered) or the field was left blank. Deliberately the same
// value the bridge falls back to — see DEFAULT_ENGINE in src/main/bridge.ts.
export const DEFAULT_ENGINE: EngineId = "claude";

// Raw composer field values. `model` must be a concrete provider model;
// Interface never silently delegates that choice. `effort` still carries the
// literal "default" for "let the engine pick". `engine` is a plain string
// because the provider-aware model picker is populated from
// whatever the main process reported as registered; validating it here would only
// duplicate the adapter-map lookup that the bridge already does properly.
export interface ComposerForm {
  engine: string;
  cwd: string; prompt: string; model: string; effort: string; permissionMode: string;
  /** Which surface is launching. Absent means "code". */
  kind?: SessionKind;
  /** The session's title instead of its prompt's start (a multi-model fan-out). */
  name?: string;
  /** Protocol 60: the provider account to run on; absent = the provider's default. */
  account?: string;
}

// What createSession is invoked with: the shared LaunchOptions plus the engine
// to route to. The engine travels ALONGSIDE the options rather than inside
// them because LaunchOptions is the contract each adapter's createSession()
// takes, and an adapter has no use for a field naming itself. A LaunchRequest
// is a LaunchOptions everywhere one is expected, so the IPC surface is
// unchanged and the extra property rides through the structured clone.
export type LaunchRequest = LaunchOptions & { engine: EngineId };

// Validates and normalizes the composer fields into a launch request.
// Returns `{ error }` for the first invalid field, in composer field order.
export function buildLaunchOptions(form: ComposerForm): LaunchRequest | { error: string } {
  // No validation branch of its own: an engine the app cannot run is not
  // something this function can know about, and the bridge rejects it by name
  // ("no adapter for engine ...") rather than the composer guessing.
  const engine = (form.engine.trim() || DEFAULT_ENGINE) as EngineId;
  const kind = form.kind ?? "code";
  const cwd = form.cwd.trim();
  // A chat has no folder of its own; the bridge pins it to the chat home.
  if (!cwd && kind !== "chat") return { error: "Working directory is required." };
  const prompt = form.prompt.trim();
  if (!prompt) return { error: "Prompt is required." };
  const model = form.model.trim();
  if (!model || model === "default") return { error: "Choose a model." };

  // Effort's "default" sentinel stays absent on the wire; the SDK's
  // EffortLevel union does not accept the word itself.
  const effort = form.effort.trim();
  const permissionMode = form.permissionMode.trim();
  const name = form.name?.trim() ?? "";
  const account = form.account?.trim() ?? "";
  return {
    engine, cwd, prompt,
    model,
    ...(kind === "code" ? {} : { kind }),
    ...(effort && effort !== "default" ? { effort } : {}),
    ...(permissionMode ? { permissionMode } : {}),
    ...(name ? { name } : {}),
    ...(account ? { account } : {}),
  };
}
