import type { AppSelectOption } from "./components/AppSelect";

interface AccessModePresentation {
  label: string;
  description: string;
  meta?: string;
}

/**
 * Provider protocols use terse identifiers because they travel on the wire.
 * The UI should explain the decision instead of making people decode those
 * identifiers. Values remain untouched, so choosing a friendly row still sends
 * the provider's exact token.
 */
const ACCESS_MODE_PRESENTATIONS: Record<string, AccessModePresentation> = {
  default: {
    label: "Provider default",
    description: "Use the provider's configured approval policy.",
  },
  acceptEdits: {
    label: "Accept edits",
    description: "Apply file edits without asking; request approval for other tools.",
  },
  plan: {
    label: "Plan only",
    description: "Read files and run read-only commands to research, without editing source files.",
  },
  bypassPermissions: {
    label: "Skip prompts",
    description: "Skip interactive approval prompts; other configured restrictions can still block actions.",
    meta: "Confirm",
  },
  dontAsk: {
    label: "Deny unapproved",
    description: "Show no approval prompts; deny actions that are not already allowed.",
  },
  untrusted: {
    label: "Ask for untrusted actions",
    description: "Ask before actions the provider classifies as untrusted.",
  },
  "on-request": {
    // "Approve for me" read as "the agent approves on my behalf"; the
    // description below says the opposite. This is the phrasing the CLI status
    // line already used ("ask when requested").
    label: "Ask when needed",
    description: "Let the agent decide when it needs to request additional access.",
  },
  never: {
    label: "Fail unapproved",
    description: "Show no approval prompts; actions blocked by the sandbox fail instead of escalating.",
  },
  auto: {
    label: "Auto mode",
    description: "Use the provider's classifier to approve or deny permission requests.",
  },
};

/**
 * EACH PROVIDER'S OWN NAMES (owner, 2026-10-06: "match what they are in the
 * CLI and app for each agent"). A provider's row wins over the generic one
 * above, so a Claude Code user meets "Manual" / "Accept edits" / "Plan" as in
 * Claude Code's own picker, and a Grok user "Always approve". Read from the
 * installed CLIs on 2026-10-06 (claude 2.1.292, codex 0.160.1, grok 1.0.30,
 * agy 1.3.1, opencode 1.18.34, qwen 0.22.3, omp 18.1.16). `status` is the
 * CLI's own status-line wording, without its glyph.
 *
 * Codex has no CLI name for the bare approval policies Interface sends: its
 * /permissions presets pair a policy with a sandbox, and Interface leaves the
 * sandbox to the owner's Codex config. So `on-request` takes the preset name
 * the CLI shows for it with the default sandbox ("Ask for approval"), and
 * `never` keeps an honest "Never ask" rather than "Full Access", which would
 * claim a sandbox Interface does not set. Qwen, OMP, Muse Code and Kimi Code start in
 * whatever their own settings say (Interface sends them no mode), so "default"
 * says so. Muse Code's names are from dev.meta.ai/docs/muse-code/permissions.
 * Kimi Code likewise gets no mode; its names are from kimi 2.3.1's own picker.
 */
interface ProviderAccessMode extends AccessModePresentation {
  /** The CLI's status-line words for this mode, e.g. "accept edits on". */
  status: string;
}

const PROVIDER_ACCESS_MODES: Readonly<Record<string, Readonly<Record<string, ProviderAccessMode>>>> = {
  claude: {
    default: { label: "Manual", status: "manual mode on",
      description: "Asks before editing files or running commands." },
    acceptEdits: { label: "Accept edits", status: "accept edits on",
      description: "Approves file edits and common file commands without asking; asks for the rest." },
    plan: { label: "Plan", status: "plan mode on",
      description: "Researches and proposes changes without making them." },
    auto: { label: "Auto", status: "auto mode on",
      description: "No routine prompts; a reviewer model screens each action before it runs." },
    dontAsk: { label: "Don't Ask", status: "don't ask on",
      description: "Never prompts; denies anything that is not already allowed." },
    bypassPermissions: { label: "Bypass Permissions", status: "bypass permissions on", meta: "Confirm",
      description: "Skips every permission prompt; configured deny rules still apply." },
  },
  codex: {
    "on-request": { label: "Ask for approval", status: "ask for approval",
      description: "Works in the project and asks before going outside it or onto the network. "
        + "Your Codex reviewer setting decides who answers." },
    never: { label: "Never ask", status: "never ask",
      description: "Never asks; anything your Codex sandbox blocks fails instead of asking." },
    // Codex 0.160's CLI dropped `-a untrusted`, but its app server — the only
    // way Interface talks to Codex — still takes and echoes it (probed
    // 2026-10-07), and a gated agent refuses to start if it ever stops.
    untrusted: { label: "Untrusted", status: "untrusted",
      description: "Asks before running anything Codex does not consider safe. Codex's own CLI no "
        + "longer offers this mode; Interface still sets it through Codex's app server." },
  },
  "acp:grok": {
    default: { label: "Default", status: "default",
      description: "Grok's default behavior: asks before running tools." },
    auto: { label: "Auto", status: "auto",
      description: "A classifier approves safe tools; risky actions may still ask or be denied." },
    bypassPermissions: { label: "Always approve", status: "always-approve on", meta: "Confirm",
      description: "Approves every tool action and skips all permission prompts." },
  },
  "acp:gemini": {
    default: { label: "Default", status: "default",
      description: "Antigravity's default: pauses for review before changing files." },
  },
  "acp:opencode": {
    default: { label: "Build", status: "build",
      description: "OpenCode's default agent; tools run under your OpenCode permission settings." },
  },
  "acp:qwen": {
    default: { label: "As set in Qwen", status: "qwen settings",
      description: "Qwen's own configured mode. Out of the box Qwen starts in Auto." },
  },
  "acp:omp": {
    default: { label: "As set in OMP", status: "omp settings",
      description: "OMP's own Tool Approval setting. Out of the box that is Yolo, which asks for nothing." },
  },
  "acp:muse": {
    default: { label: "As set in Muse Code", status: "muse settings",
      description: "Muse Code's own permission profile. Out of the box that is Auto-review: a reviewer "
        + "answers eligible approvals and asks you when it is unavailable." },
  },
  "acp:kimi": {
    default: { label: "As set in Kimi Code", status: "kimi settings",
      description: "Kimi Code's own default permission mode. Out of the box that is Always Ask." },
  },
};

/** The provider's own row for a token, when this file knows that provider. */
function providerAccessMode(value: string, engine: string | undefined): ProviderAccessMode | undefined {
  return engine === undefined ? undefined : PROVIDER_ACCESS_MODES[engine]?.[value];
}

/** The CLI's own status-line words for a mode, or undefined when unknown. */
export function providerAccessStatus(value: string, engine: string | undefined): string | undefined {
  return providerAccessMode(value, engine)?.status;
}

export type AccessModeTone = "plan" | "accept" | "danger" | "auto" | "neutral";

/**
 * The HUE a mode earns, which is a different question from the copy above.
 * Claude Code's CLI colours its permission modes instead of spelling them out —
 * plan blue, auto-accept green, bypass red — and this is that vocabulary
 * resolved in one place, so the launch card and the live session pill cannot
 * disagree about what a provider token means.
 *
 * `never` and `dontAsk` join `bypassPermissions` on the danger hue because all
 * three end the conversation about approvals; they differ in what happens to a
 * disallowed action, not in whether anyone is asked first.
 *
 * Everything else is NEUTRAL on purpose, `default`, `untrusted` and
 * `on-request` included. A colour here is a claim about how much rope an agent
 * has, and this function has no basis for making one about a token it has never
 * seen — a provider mints these freely, and a wrong colour on the app's most
 * consequential control is worse than no colour at all.
 */
export function accessModeTone(value: string): AccessModeTone {
  switch (value) {
    case "plan": return "plan";
    case "acceptEdits": return "accept";
    case "bypassPermissions": case "never": case "dontAsk": return "danger";
    case "auto": return "auto";
    default: return "neutral";
  }
}

/**
 * The class both access pickers hang that hue on; theme.css owns the four
 * rules and every theme's values. Neutral returns "" rather than a fifth class
 * with no rule behind it, so "this mode is unremarkable" stays visible in the
 * markup as the ABSENCE of a tone rather than as a tone that paints nothing.
 */
export function accessModeToneClass(value: string): string {
  const tone = accessModeTone(value);
  return tone === "neutral" ? "" : `access-tone-${tone}`;
}

function readableAccessMode(value: string): string {
  const words = value
    .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
    .replace(/[-_]+/g, " ")
    .trim();
  return words === "" ? "Unknown policy"
    : `${words[0].toLocaleUpperCase()}${words.slice(1)}`;
}

/**
 * A picker row for a provider token. With the session's engine the row uses
 * that provider's own name for the mode; without one (an older caller, an
 * unknown provider) the generic wording above.
 */
export function accessModeOption(value: string, engine?: string): AppSelectOption {
  const provider = providerAccessMode(value, engine);
  if (provider !== undefined) {
    const { status: _status, ...presentation } = provider;
    return { value, ...presentation };
  }
  const known = ACCESS_MODE_PRESENTATIONS[value];
  if (known !== undefined) return { value, ...known };
  return {
    value,
    label: readableAccessMode(value),
    description: "Provider-defined access policy.",
    meta: value,
  };
}

export function accessModeOptions(values: readonly string[], engine?: string): AppSelectOption[] {
  return values.map(value => accessModeOption(value, engine));
}

/**
 * A successful provider probe is authoritative for that provider's vocabulary.
 * Keep the curated list only as a cold-start fallback, and never merge stale
 * fallback values into a non-empty provider answer. Exact tokens are preserved;
 * duplicate rows are removed without mutating either input.
 */
export function availableAccessModes(fallback: readonly string[],
    reported: readonly string[] | undefined): string[] {
  return [...new Set(reported !== undefined && reported.length > 0 ? reported : fallback)];
}
