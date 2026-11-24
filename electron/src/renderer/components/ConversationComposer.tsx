import {
  memo, useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState,
  useSyncExternalStore,
} from "react";
import type { MutableRefObject, ReactNode } from "react";
import type {
  ContextUsage, EngineId, ProviderCommand, ProviderCommandCatalogue, ProviderCommandScope,
} from "../../shared/engine";
import { formatContextTokens } from "../../shared/engine";
import {
  isFastModeProviderCommand, parseProviderCommandLike,
} from "../../shared/provider-commands";
import type { PromptTurnImage } from "../../shared/prompt-images";
import {
  APPROVAL_ALLOW_KEYSHORTCUTS, APPROVAL_DENY_KEYSHORTCUTS, approvalKeyVerdict,
  type ApprovalKeyVerdict,
} from "../approval-keys";
import { contextMeterDetails, contextMeterLabel } from "../conversation-reducer";
import type { TranscriptDensity } from "../transcript-density";
import {
  conversationEscapeAction, conversationSteerKeyAction, conversationSubmitAction,
  draftAfterFailedSend, queueSubmissionAccepted,
} from "../conversation-pane-rules";
import type { SessionConfigView } from "../conversation-types";
import { composerProviderCommandResponseMatches, preferredEffort } from "../composer-model";
import { pickerModelsForRunning } from "../model-catalog";
import {
  DEFAULT_PROMPT_CONTEXT_LIMIT, normalizePromptContextItems, promptContextOptions,
  promptTurnAttachments, promptTurnImages, type PromptContextCapabilities, type PromptContextItem,
  type PromptContextKind, type PromptTurnAttachment,
} from "../prompt-context";
import { pasteImagesIntoDraft } from "../prompt-paste";
import { appendReplyQuote } from "../composer-quote";
import { draftHasContent, stashChord } from "../draft-stash-view";
import { promptSubmitKeyAction } from "../prompt-submit";
import { accessSelectOptions } from "../access-chip";
import { accessModeToneClass } from "../access-modes";
import {
  canonicalProviderCommandText, contextCompactCommand, providerCommandIntent,
} from "../provider-commands";
import { productLabel } from "../product";
import { withoutIpcErrorShape } from "../ipc-message";
import { useProviderCommandMenu } from "../use-provider-command-menu";
import {
  sessionLine2, sessionMeterText, sessionStatusBar, sessionStatusBarLabel, sessionStatusModel,
  sessionStatusTokens,
} from "../session-status";
import {
  threadComposerAnswersApproval, threadComposerPlaceholder, threadContextReadout,
} from "../thread-composer";
import { AppSelect, type AppSelectOption } from "./AppSelect";
import type { ConversationProps } from "./Conversation";
import { useDocumentChrome } from "./ConversationRows";
import { Icon } from "./glyphs";
import { EffortBar } from "./EffortBar";
import { FastModeToggle } from "./FastModeToggle";
import { KIND_ICON, PromptContextChips, PromptContextPicker } from "./PromptContextPicker";
import { ProviderCommandMenu, providerCommandOptionId } from "./ProviderCommandMenu";
import { ProviderLogo, providerLogoKind } from "./ProviderLogo";
import type { UsageHeadroom } from "../usage-view";
import {
  ThreadComposerMenu, type ThreadMenuSection, type ThreadMenuView,
} from "./ThreadComposerMenu";
import { ThreadGlyph } from "./ThreadGlyph";
import { StashedDrafts } from "./StashedDrafts";
import { sideQuestionCommand } from "../side-question";
// Last, so the Threads composer's rules land after Conversation.css and
// theme.css, which the imports above bring in.
import "./ThreadComposer.css";

/**
 * THE COMPOSER HALF OF THE PANE, memoized: the draft, the slash-command menu
 * and pre-launch discovery, send/queue/steer, paste, the context picker and
 * meter, the density dial's button and the config pills.
 *
 * The draft is read from the caller's DraftStore when there is one, so a
 * keystroke re-renders this component and nothing above or beside it; without
 * one it is component-local, as it always was. Every other prop is stable
 * across a streamed token, so the transcript can grow without redrawing it.
 * The caller's callbacks arrive as the shell's stable forwarders, and `latest`
 * reads the ones a send must hold on to past its own event.
 */

// Module-level so their identity is stable: a fresh `{}` or `[]` per render
// would make the memos below recompute forever.
const NO_CONTEXT: readonly PromptContextItem[] = [];
const NO_CONTEXT_CAPABILITIES: PromptContextCapabilities = {};
const NO_COMMANDS: readonly ProviderCommand[] = [];
const NO_QUEUE_ID = (_id: string): void => undefined;
const NO_CONTEXT_OPEN = (_open: boolean): void => undefined;
const NO_CONTEXT_REQUEST = (_kind: PromptContextKind): void => undefined;

/**
 * WHAT THE SLASH MENU OFFERS. The live session wins outright, and that includes
 * the empty list: `commands.invalidated` reports [] to say this session
 * advertises nothing, so treating [] as "nothing heard yet" would resurrect
 * commands the provider just withdrew. Only never having heard — undefined —
 * reaches the pre-launch answer.
 */
export function effectiveProviderCommands(live: readonly ProviderCommand[] | undefined,
    prelaunch: readonly ProviderCommand[] | null | undefined): readonly ProviderCommand[] {
  return live ?? prelaunch ?? NO_COMMANDS;
}

/**
 * The scope an unlaunched session would launch with. `settingSources` is absent
 * rather than empty on purpose: omitted means "whatever this provider loads by
 * default", while [] would ask Claude to load no settings at all and answer for
 * a session nobody is going to start. An unset model is likewise a real scope —
 * the provider's own default — but a session with no directory is not askable.
 */
export function prelaunchCommandScope(engine: string | undefined, cwd: string | undefined,
    model: string | undefined): ProviderCommandScope | null {
  const askedEngine = engine?.trim() ?? "";
  const askedCwd = cwd?.trim() ?? "";
  if (askedEngine === "" || askedCwd === "") return null;
  const askedModel = model?.trim() ?? "";
  return {
    engine: askedEngine as EngineId,
    cwd: askedCwd,
    ...(askedModel === "" || askedModel === "default" ? {} : { model: askedModel }),
  };
}

/**
 * One discovery per scope for the life of this window. Claude answers by
 * spawning a control query, and re-opening the same chat five times is not five
 * questions.
 *
 * ponytail: unbounded for the run, keyed by scope — a personal app opens tens
 * of directory/model pairs and each entry is a handful of command names. Swap in
 * an LRU only if that ever stops being true.
 */
const prelaunchCommandCache = new Map<string, Promise<readonly ProviderCommand[] | null>>();
/** Settled answers, so a re-opened pane has its list on the FIRST render. */
const prelaunchCommandAnswers = new Map<string, readonly ProviderCommand[] | null>();

/** Same shape the bridge coalesces on, so both sides agree what "same" means. */
function prelaunchCommandKey(scope: ProviderCommandScope): string {
  return JSON.stringify([scope.engine, scope.cwd, scope.model ?? null,
    scope.settingSources ?? null]);
}

/** The answer, or undefined while nobody has asked and while an ask is out. */
function settledPrelaunchCommands(scope: ProviderCommandScope):
    readonly ProviderCommand[] | null | undefined {
  return prelaunchCommandAnswers.get(prelaunchCommandKey(scope));
}

/**
 * Ask once, share the answer. Null covers every way a scope can fail to produce
 * a usable catalogue — a provider that cannot list commands before a chat
 * starts (Grok, Qwen), an answer for some other configuration, or a refusal —
 * and all of them leave the pane exactly as it behaves today.
 */
export function loadPrelaunchProviderCommands(scope: ProviderCommandScope,
    load: (scope: ProviderCommandScope) => Promise<ProviderCommandCatalogue>):
    Promise<readonly ProviderCommand[] | null> {
  const key = prelaunchCommandKey(scope);
  const cached = prelaunchCommandCache.get(key);
  if (cached !== undefined) return cached;
  const asked = (async () => {
    try {
      const result = await load(scope);
      // A cancelled sibling discovery is not an answer. Caching it as null
      // made restored tiles after restart never retry `/` until a live turn
      // published commands.available.
      if (/newer model or command scope/i.test(result.reason ?? "")) {
        prelaunchCommandCache.delete(key);
        return null;
      }
      const commands = composerProviderCommandResponseMatches(result, scope)
        && result.available ? result.commands : null;
      prelaunchCommandAnswers.set(key, commands);
      return commands;
    } catch {
      // A throw is not an answer. Forget it so the next visit may ask again.
      prelaunchCommandCache.delete(key);
      return null;
    }
  })();
  prelaunchCommandCache.set(key, asked);
  return asked;
}

/**
 * The dial's three steps, in the order they are drawn — least on screen to
 * most. Each `hint` says what the step DOES rather than restating its name; the
 * word on the button is already the label, and saying it twice is the pattern
 * this pane spent a release removing.
 *
 * THREE STEPS, THREE SENTENCES. Compact and balanced used to share one hint
 * word for word, which was honest reporting of a dial whose first two steps
 * painted the same screen. Both halves of that are fixed together: compact
 * folds every run (toolRunMinimum), and each hint now names the one thing that
 * separates its step from the step beside it.
 */
const DENSITY_STEPS: readonly {
  value: TranscriptDensity; label: string; hint: string;
}[] = [
  { value: "compact", label: "Compact",
    hint: "Finished runs fold · summaries only" },
  { value: "balanced", label: "Balanced",
    hint: "Repeats fold · single calls show" },
  { value: "detailed", label: "Detailed",
    hint: "Everything open" },
];

// The live-session model/effort row. Two pills, each opening a small picker;
// changing one is fire-and-forget through the caller, which owns the optimistic
// value, the confirmation and the error. The row renders the CURRENT setting
// even while a change is in flight — dimmed and italic, so "asked for" reads
// differently from "running".
function ContextMeter({ usage, previous, variant = "chip", onCompact }: {
  usage?: ContextUsage;
  previous?: ContextUsage;
  variant?: "chip" | "status";
  onCompact?: () => void;
}) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const panelId = `${useId()}-context-meter`;
  const chipLabel = usage === undefined ? "" : contextMeterLabel(usage);
  const statusLabel = sessionMeterText(usage);
  // What is READ is not what is shown. sessionMeterText is deliberately
  // unit-less — `12k / 200k` scans at 11px — but an accessible name that says
  // "12k / 200k" and nothing else has lost the noun that made it a measurement,
  // so anything spoken falls back to the sentence instead.
  const spokenLabel = sessionStatusTokens(usage);
  const label = variant === "status" ? statusLabel : chipLabel;

  useEffect(() => {
    if (!open) return;
    const onPointer = (event: PointerEvent): void => {
      if (root.current?.contains(event.target as Node) === true) return;
      setOpen(false);
    };
    const onKey = (event: KeyboardEvent): void => {
      if (event.key === "Escape") setOpen(false);
    };
    window.addEventListener("pointerdown", onPointer);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("pointerdown", onPointer);
      window.removeEventListener("keydown", onKey);
    };
  }, [open]);

  if (usage === undefined || label === "") return null;
  const details = contextMeterDetails(usage, previous);
  const compacting = usage.compacting === true;
  const high = !compacting && (usage.percent ?? 0) >= 80;
  const usedLabel = usage.used === undefined ? "" : formatContextTokens(usage.used);
  const sizeLabel = usage.size === undefined ? "" : formatContextTokens(usage.size);
  const head = compacting
    ? "Compacting the session window"
    // No percent to lead with: the sentence already carries both figures, and
    // appending them again would title the control `12k / 200k tokens · 12k / 200k`.
    : chipLabel === "" ? spokenLabel
      : usedLabel !== "" && sizeLabel !== ""
        ? `${chipLabel} · ${usedLabel} / ${sizeLabel}`
        : chipLabel;
  const aria = compacting
    ? "Compacting session context"
    : `Session context ${chipLabel !== "" ? chipLabel : spokenLabel}`;
  const tokenClass = compacting ? "conv-session-tokens is-compacting"
    : high ? "conv-session-tokens is-high" : "conv-session-tokens";

  return (
    <div className="conv-context-meter" ref={root}>
      <button type="button"
        className={variant === "status" ? tokenClass
          : high ? "conv-iconbtn conv-context-meter-btn is-high"
            : "conv-iconbtn conv-context-meter-btn"}
        aria-label={aria}
        aria-expanded={open}
        aria-controls={open ? panelId : undefined}
        title={head}
        onClick={() => { setOpen(next => !next); }}>
        {label}
      </button>
      {open && (
        <div id={panelId} className="conv-context-meter-panel is-floating" role="group"
          aria-label="Session context details">
          <p className="conv-context-meter-title">Session context</p>
          <p className="conv-context-meter-head">{head}</p>
          {details.windowRemaining !== undefined && (
            <div className="conv-context-meter-row">
              <span>Window remaining</span><span>{details.windowRemaining}</span>
            </div>
          )}
          {details.reportChange !== undefined && (
            <div className="conv-context-meter-row">
              <span>Since prior report</span><span>{details.reportChange}</span>
            </div>
          )}
          {usage.categories?.map(row => (
            <div key={row.name} className="conv-context-meter-row">
              <span>{row.name}</span>
              <span>{formatContextTokens(row.tokens)}</span>
            </div>
          ))}
          {details.autoCompact !== undefined && (
            <div className="conv-context-meter-row">
              <span>Auto-compact</span><span>{details.autoCompact}</span>
            </div>
          )}
          {details.autoCompactThreshold !== undefined && (
            <div className="conv-context-meter-row">
              <span>Auto-compact threshold</span>
              <span>{details.autoCompactThreshold}</span>
            </div>
          )}
          <p className="conv-context-meter-note">Provider reported</p>
          {onCompact !== undefined && (
            <button type="button" className="conv-context-meter-action"
              onClick={() => { setOpen(false); onCompact(); }}>
              Compact now
            </button>
          )}
        </div>
      )}
    </div>
  );
}

/**
 * The model and effort rows both control rows offer: the running model first
 * under its catalogue name (an alias of a catalogued model reuses that row, so
 * the trigger never shows a raw id a display name exists for), and the
 * selected model's real effort levels.
 */
function useRunChoices(config: SessionConfigView) {
  const selectableModels = useMemo(
    () => pickerModelsForRunning(config.models, config.model ?? "", config.engine),
    [config.models, config.model, config.engine],
  );
  const modelOptions = useMemo((): AppSelectOption[] => selectableModels.map(model => ({
    value: model.value,
    label: model.displayName,
    description: model.description,
    meta: model.value !== model.displayName ? model.value : undefined,
  })), [selectableModels]);
  const effortLevels = useMemo(
    () => config.effortLevels.filter(level => level !== "" && level !== "default"),
    [config.effortLevels],
  );
  const effortValue = preferredEffort(effortLevels, config.effort);
  return { selectableModels, modelOptions, effortLevels, effortValue };
}

function ConfigPills({ config, locked, usage, previousContext,
    planName, headroom, running, onSetModel, onSetEffort, onSetFastMode, onCompact }: {
  config: SessionConfigView;
  locked: boolean;
  usage?: ContextUsage;
  previousContext?: ContextUsage;
  /** The account's plan, for line one. Absent until the usage read has run. */
  planName?: string;
  /** The plan's headroom ("38% left"), beside it (review E). */
  headroom?: UsageHeadroom;
  /** Drives `esc interrupt` on line two: a hint only while there is a turn. */
  running: boolean;
  onSetModel: ConversationProps["onSetModel"];
  onSetEffort: ConversationProps["onSetEffort"];
  onSetFastMode?: ConversationProps["onSetFastMode"];
  onCompact?: () => void;
}) {
  const cli = useDocumentChrome() === "cli";
  const { selectableModels, modelOptions, effortLevels, effortValue } = useRunChoices(config);
  const showModel = config.canSetModel;
  const showEffortBar = config.canSetEffort && config.showEffort
    && effortLevels.length > 0;
  const showFast = config.supportsFastMode === true && onSetFastMode !== undefined;
  const status = sessionStatusBar({
    modelId: config.model,
    models: config.models,
    engineLabel: config.engineLabel,
    engine: config.engine,
    effort: config.effort,
    permissionMode: config.permissionMode,
    usage,
  });

  const modelPicker = showModel
    ? (
      <AppSelect
        key="model"
        label="Model"
        value={config.model ?? ""}
        options={modelOptions}
        placeholder="Choose model"
        leading={<ProviderLogo engine={config.engine ?? ""}
          label={config.engineLabel ?? config.engine ?? "Provider"} />}
        allowCustom={selectableModels.length === 0}
        customLabel="Enter model id"
        customPlaceholder="provider model id"
        onChange={onSetModel}
        onCustom={onSetModel}
        searchable={modelOptions.length > 8}
        searchPlaceholder="Search models"
        disabled={locked || config.pendingModel}
        pending={config.pendingModel}
        compact
        className="conv-config-select conv-model-select"
      />
    )
    : null;
  const effortPicker = showEffortBar
    ? (
      <EffortBar
        key="effort"
        className="conv-config-select conv-effort-select"
        levels={effortLevels}
        value={effortValue}
        notes={effortLevels.includes("max")
          ? { max: "Applies to this session only." } : undefined}
        onChange={onSetEffort}
        disabled={locked || config.pendingEffort}
        pending={config.pendingEffort}
      />
    )
    : null;
  const fastToggle = showFast
    ? (
      <FastModeToggle
        key="fast"
        className="conv-fastmode-toggle"
        state={config.fastMode ?? "off"}
        reason={config.fastModeReason}
        pending={config.pendingFastMode === true}
        disabled={locked}
        onToggle={onSetFastMode}
      />
    )
    : null;

  const parts: ReactNode[] = [];
  const addPart = (node: ReactNode): void => {
    if (parts.length > 0) {
      parts.push(<span key={`sep-${parts.length}`} className="conv-pill-sep"
        aria-hidden="true">·</span>);
    }
    parts.push(node);
  };
  if (modelPicker !== null) addPart(modelPicker);
  if (effortPicker !== null) addPart(effortPicker);
  if (fastToggle !== null) addPart(fastToggle);

  const error = config.error !== null
    ? (
      <span className="conv-pill-err" role="alert" title={config.error}>
        {config.error}
      </span>
    )
    : null;

  if (!cli) {
    return (
      <div
        className="conv-pills"
        aria-busy={config.pendingModel || config.pendingEffort
          || config.pendingFastMode || undefined}
      >
        {parts}
        {error}
      </div>
    );
  }

  const modelControl = modelPicker ?? (status.model !== ""
    ? <span key="model-label">{status.model}</span> : null);
  const effortControl = effortPicker !== null
    ? <span key="effort-wrap" className="conv-session-effort">{effortPicker}</span>
    : status.effort !== ""
      ? <span key="effort-label" className="conv-session-effort">{status.effort}</span>
      : null;
  const meter = sessionMeterText(usage);

  return (
    <div
      className="conv-pills"
      /* A bare div is role=generic, which takes no accessible name — the whole
         composer status line was inaudible. The group is what makes the label
         reach the reader. */
      role="group"
      aria-label={sessionStatusBarLabel(status)}
      aria-busy={config.pendingModel || config.pendingEffort
        || config.pendingFastMode || undefined}
    >
      <div className="conv-session-line">
        {modelControl !== null && (
          <span className="conv-session-model">
            <span className="conv-session-star" aria-hidden="true">*</span>
            {modelControl}
          </span>
        )}
        {planName !== undefined && planName !== "" && (
          <span className="conv-session-plan">{planName}</span>
        )}
        {headroom !== undefined && (
          <span className="conv-session-plan conv-session-headroom" title={headroom.title}>
            {headroom.text}
          </span>
        )}
        {effortControl}
        {fastToggle}
        {meter !== "" && (
          <ContextMeter usage={usage} previous={previousContext}
            variant="status" onCompact={onCompact} />
        )}
      </div>
      <div className="conv-session-line conv-session-hints">{sessionLine2(running)}</div>
      {error}
    </div>
  );
}

const NO_ACCESS_MODES: readonly string[] = [];

/**
 * THE THREADS LAYOUT'S RUN SETTINGS (owner, 2026-10-06; laid out as the Code
 * mode canvas, 2026-10-23): the access chip first, beside the "+", then model
 * and effort as quiet 28px chips at the row's right end, then Fast where the
 * model has it. The provider's mark is drawn ONCE, on the model chip —
 * nowhere else in this composer names the provider. The same pickers and the
 * same callbacks as the row above; only the presentation differs.
 *
 * The access chip is the pane header's selector over the same rows
 * (accessSelectOptions), worded as the row's own short label beside the
 * access shield. It is drawn only where the transport can change the policy
 * mid-session, and it waits, dimmed and italic, while a change is in flight.
 */
function ThreadRunSettings({ config, locked, headroom, onSetModel, onSetEffort, onSetFastMode,
    onSetPermissionMode }: {
  config: SessionConfigView;
  locked: boolean;
  /** The plan's headroom, quiet on the model chip (review E). */
  headroom?: UsageHeadroom;
  onSetModel: ConversationProps["onSetModel"];
  onSetEffort: ConversationProps["onSetEffort"];
  onSetFastMode?: ConversationProps["onSetFastMode"];
  onSetPermissionMode?: (mode: string) => void;
}) {
  const { selectableModels, modelOptions, effortLevels, effortValue } = useRunChoices(config);
  const accessModes = config.permissionModes ?? NO_ACCESS_MODES;
  const accessMode = config.permissionMode ?? "";
  const accessUnknown = config.permissionUnknown === true;
  const accessOptions = useMemo(
    () => accessSelectOptions(accessModes, accessMode, accessUnknown, "label", config.engine),
    [accessModes, accessMode, accessUnknown, config.engine],
  );
  const engineName = config.engineLabel ?? config.engine ?? "Provider";
  const logo = <ProviderLogo engine={config.engine ?? ""} label={engineName} size={14} />;
  const modelName = sessionStatusModel(config.model, config.models, config.engineLabel,
    config.engine);
  const showEffort = config.canSetEffort && config.showEffort && effortLevels.length > 0;
  const showAccess = onSetPermissionMode !== undefined && config.canSetPermission === true
    && accessModes.length > 0;
  const showFast = config.supportsFastMode === true && onSetFastMode !== undefined;
  const accessPending = config.pendingPermission === true;

  // The row reads left to right as the canvas does: access beside the "+",
  // then model, effort and Fast at the right end, before the verbs.
  const run: ReactNode[] = [];
  if (config.canSetModel) {
    run.push((
      <AppSelect
        key="model"
        label="Model"
        value={config.model ?? ""}
        options={modelOptions}
        placeholder="Choose model"
        leading={logo}
        allowCustom={selectableModels.length === 0}
        customLabel="Enter model id"
        customPlaceholder="provider model id"
        onChange={onSetModel}
        onCustom={onSetModel}
        searchable={modelOptions.length > 8}
        searchPlaceholder="Search models"
        disabled={locked || config.pendingModel}
        pending={config.pendingModel}
        compact
        className="thread-composer-chip thread-composer-model"
        trailing={headroom?.text}
        trailingTitle={headroom?.title}
      />
    ));
  } else if (modelName !== "") {
    // A transport that cannot change the model mid-session still says which
    // one is running, and this is still where the provider's mark lives.
    run.push((
      <span key="model" className="thread-composer-chip thread-composer-model is-static"
        title={headroom === undefined ? `Model: ${modelName}` : `Model: ${modelName} — ${headroom.title}`}>
        <span className="thread-composer-chip-leading" aria-hidden="true">{logo}</span>
        <span className="thread-composer-chip-value">{modelName}</span>
        {headroom !== undefined && <span className="app-select-trailing">{headroom.text}</span>}
      </span>
    ));
  }
  if (showEffort) {
    run.push((
      <EffortBar
        key="effort"
        className="thread-composer-chip thread-composer-effort"
        levels={effortLevels}
        value={effortValue}
        notes={effortLevels.includes("max")
          ? { max: "Applies to this session only." } : undefined}
        onChange={onSetEffort}
        disabled={locked || config.pendingEffort}
        pending={config.pendingEffort}
      />
    ));
  }

  return (
    // data-provider gives the model chip its provider's colour (--provider-accent).
    <div className="thread-composer-settings" role="group" aria-label="Run settings"
      data-provider={providerLogoKind(config.engine ?? "")}
      aria-busy={config.pendingModel || config.pendingEffort || accessPending
        || config.pendingFastMode || undefined}>
      {showAccess && (
        <AppSelect
          label="Permission mode"
          value={accessMode}
          options={accessOptions}
          leading={<ThreadGlyph name="access" size={14} />}
          onChange={onSetPermissionMode}
          disabled={accessPending}
          pending={accessPending}
          compact
          className={["thread-composer-chip thread-composer-access",
            accessModeToneClass(accessMode)].filter(Boolean).join(" ")}
        />
      )}
      <span className="thread-composer-run">
        {run}
        {showFast && (
          <FastModeToggle
            className="thread-composer-fast"
            state={config.fastMode ?? "off"}
            reason={config.fastModeReason}
            pending={config.pendingFastMode === true}
            disabled={locked}
            onToggle={onSetFastMode}
          />
        )}
      </span>
    </div>
  );
}

/** Send's glyph: the Threads arrow, in the accent's own ink. */
function SendArrow() {
  return <ThreadGlyph name="send" className="thread-composer-glyph" />;
}

/** Stop's glyph: the Threads square, in the surface's colour. */
function StopSquare() {
  return <ThreadGlyph name="stop" size={12} className="thread-composer-glyph" />;
}

export type ConversationComposerProps = Pick<ConversationProps,
  | "drafts" | "sessionKey" | "thinking" | "live" | "config" | "commands"
  | "loadProviderCommands" | "cwd" | "planName" | "headroom" | "context" | "previousContext"
  | "contextItems" | "contextLocked" | "contextCapabilities" | "contextOpen"
  | "onContextOpenChange" | "onContextRequest" | "onContextRemove" | "onContextPreview" | "onPastedImages"
  | "queueSendingId" | "queueControlsLocked" | "sendLocked" | "sendLockReason"
  | "queueWhileBusy" | "canSteer" | "onQueue" | "onRunCommand" | "onStop" | "onSetModel"
  | "onSetEffort" | "onSetFastMode" | "threadLayout" | "onSetPermissionMode" | "stash" | "onSideQuestion"
  | "composerTop"> & {
  /** The caller's props as the pane last committed them. A send reads its
   * callbacks here once and keeps them for the answer, as a closure would. */
  latest: () => ConversationProps;
  /** What this session has done to the working tree, or "" for nothing. */
  dirtyLabel: string;
  /** A brand-new session on a mutable model with none chosen yet. */
  modelRequired: boolean;
  /** The transcript has something the density dial would change. */
  foldable: boolean;
  density: TranscriptDensity;
  onChooseDensity: (next: TranscriptDensity) => void;
  /** Mint this send's echo and pull the transcript to its newest line. The
   * function returned marks that echo undelivered, on the session it was sent
   * from, when the send is refused. */
  postEcho: (
    text: string,
    images: readonly PromptTurnImage[],
    attachments?: readonly PromptTurnAttachment[],
  ) => (why: string) => void;
  /** Called after every commit of this composer: it can resize the transcript. */
  afterCommit: () => void;
  /**
   * THREADS (owner, 2026-10-07, "a live turn"): a request docked above waits
   * on the owner. The box turns amber and the field points at the request;
   * while it is false and a turn runs, the box's edge carries the running comet.
   */
  approvalPending?: boolean;
  /**
   * Press the docked request's keyed button for this verdict: an empty
   * field's ⌘↵ / ⌘⌫ answer it, since an empty draft has nothing to queue.
   * Returns false when no button took the key, which then does what it did.
   * Absent where the pane does not own the chord.
   */
  onAnswerApproval?: (verdict: ApprovalKeyVerdict) => boolean;
  /**
   * Quote (T3 section 9): the shell's slot, which this composer fills with
   * "put this reply selection in the draft as a quote, caret after it".
   */
  quoteInto?: MutableRefObject<((text: string) => void) | null>;
};

export const ConversationComposer = memo(function ConversationComposer(
  props: ConversationComposerProps,
): ReactNode {
  const { thinking, sessionKey } = props;
  const lockHelpId = useId();
  const commandMenuId = useId();
  const commandErrorId = useId();
  // THE DRAFT. With a caller's store it lives outside React state, keyed by
  // session, so a keystroke re-renders this composer and nothing else, and a
  // pane switch, workspace switch or remount finds it where it was. Every pane
  // showing one session reads and writes the same entry. Without a store it is
  // component-local, as it always was, and a session switch clears it.
  const drafts = props.drafts;
  const draftKey = sessionKey ?? "";
  const subscribeDraft = useCallback((listener: () => void) => (drafts === undefined
    ? () => undefined : drafts.subscribe(draftKey, listener)), [drafts, draftKey]);
  const readDraft = (): string => (drafts === undefined ? "" : drafts.get(draftKey));
  const storedDraft = useSyncExternalStore(subscribeDraft, readDraft, readDraft);
  const [localDraft, setLocalDraft] = useState("");
  const draft = drafts === undefined ? localDraft : storedDraft;
  // The live draft, for the one caller that has to read it from a closure born
  // an arbitrary number of keystrokes ago — see the failed-send restore in
  // send(). A controlled parent owns the value, so React's own functional
  // setState cannot answer that question.
  const draftNow = useRef(draft);
  draftNow.current = draft;
  // The session this pane is showing RIGHT NOW, read from the same kind of
  // closure. Routes 2 and 3 of the contract above keep one instance across a
  // switch, so an answer that arrives after the pane moved on must not write
  // the previous session's text into the new session's composer.
  const sessionKeyNow = useRef(sessionKey);
  sessionKeyNow.current = sessionKey;
  const setDraft = (next: string | ((prev: string) => string)): void => {
    const value = typeof next === "function"
      ? next(drafts === undefined ? draftNow.current : drafts.get(draftKey)) : next;
    draftNow.current = value;
    if (drafts !== undefined) drafts.set(draftKey, value);
    else setLocalDraft(value);
  };
  // A SESSION SWITCH WITHOUT A REMOUNT, adjusted during render for the reason
  // the transcript's own reset gives (see ConversationTranscript): a store
  // already answers with B's draft, and a pane without one clears its own so
  // A's half-typed words are never sent to B.
  const [seenKey, setSeenKey] = useState(sessionKey);
  if (seenKey !== sessionKey) {
    setSeenKey(sessionKey);
    if (drafts === undefined) setLocalDraft("");
  }
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  // THE SEND GATE READS THE LIVE SESSION AND NOTHING ELSE. A pre-launch answer
  // is a good enough basis to OFFER a command, but it was given before this
  // session existed and may not match what the running provider ends up
  // advertising — so it must never be the thing that turns slash text this pane
  // would have sent into a refusal. `commandsKnown` therefore stays false until
  // a live session speaks, which keeps providerCommandIntent permissive.
  const commandsKnown = props.commands !== undefined;
  const liveCommands = props.commands ?? NO_COMMANDS;
  // The cache below holds the value; this only asks React to look again once an
  // in-flight answer lands. Reading through the current scope every render is
  // what makes a model change unable to show the previous model's list.
  const [, setPrelaunchAnswered] = useState(0);
  const commandScope = useMemo(
    () => prelaunchCommandScope(props.config.engine, props.cwd, props.config.model),
    [props.config.engine, props.cwd, props.config.model]);
  const loadCommands = props.loadProviderCommands;
  const hasCommandLoader = loadCommands !== undefined;
  const prelaunchCommands = commandScope === null || !hasCommandLoader
    ? null : settledPrelaunchCommands(commandScope) ?? null;
  // Fast mode is model-gated, and its toggle already follows that gate. Drop
  // the provider's own /fast row on models that cannot serve it so the command
  // palette never offers it.
  const commands = useMemo(() => {
    const listed = effectiveProviderCommands(props.commands, prelaunchCommands);
    return props.config.supportsFastMode === true ? listed
      : listed.filter(command => !isFastModeProviderCommand(command));
  }, [props.commands, prelaunchCommands, props.config.supportsFastMode]);
  const commandScopeKey = commandScope === null ? null : prelaunchCommandKey(commandScope);
  useEffect(() => {
    if (commandsKnown || loadCommands === undefined || commandScope === null) return;
    if (settledPrelaunchCommands(commandScope) !== undefined) return;
    let cancelled = false;
    let retryTimer: number | undefined;
    const ask = (attempt: number): void => {
      void loadPrelaunchProviderCommands(commandScope, loadCommands).then(() => {
        if (cancelled) return;
        if (settledPrelaunchCommands(commandScope) !== undefined) {
          setPrelaunchAnswered(seen => seen + 1);
          return;
        }
        // A cancelled sibling is not an answer. The previous effect fired once
        // per scope and left `/` empty until a live turn published the list.
        if (attempt >= 4) {
          setPrelaunchAnswered(seen => seen + 1);
          return;
        }
        retryTimer = window.setTimeout(() => { ask(attempt + 1); }, 400 * (attempt + 1));
      });
    };
    ask(0);
    return () => {
      cancelled = true;
      if (retryTimer !== undefined) window.clearTimeout(retryTimer);
    };
  // The scope key IS the request identity, so the scope object's own identity
  // is not a dependency; sessionKey is here so switching chats re-reads the
  // cache even when two sessions happen to share one scope.
  }, [commandsKnown, hasCommandLoader, commandScopeKey, sessionKey]);
  const [commandError, setCommandError] = useState<string | null>(null);
  // The slash menu this composer shares with the launch card. A chosen row
  // clears any refusal, and so does running one before it is re-checked.
  const commandMenu = useProviderCommandMenu({
    commands, draft, available: commands.length > 0,
    setDraft: text => { setDraft(text); setCommandError(null); },
    submit: text => { setCommandError(null); send(text); },
    focus: () => { textareaRef.current?.focus(); },
  });
  const commandMatches = commandMenu.matches;
  const commandMenuOpen = commandMenu.open;
  const activeCommandIndex = commandMenu.activeIndex;
  const draftCommandIntent = providerCommandIntent(liveCommands, commandsKnown, draft.trim());
  const draftCommandLike = parseProviderCommandLike(draft.trim());
  useEffect(() => {
    commandMenu.reset();
    setCommandError(null);
  }, [commands, sessionKey]);
  const contextItems = useMemo(
    () => normalizePromptContextItems(props.contextItems ?? NO_CONTEXT), [props.contextItems]);
  const queueWhileBusy = props.queueWhileBusy ?? thinking;
  const queueDraft = queueWhileBusy && props.onQueue !== undefined
    && draftCommandLike === null
    && !(draftCommandIntent !== null && props.onRunCommand !== undefined);
  // Steer is offered exactly where Enter would otherwise queue AND the engine
  // proved it can take mid-turn input. It is a second verb beside the primary
  // control, never a replacement for it: Enter still queues.
  const steerOffered = queueDraft && props.canSteer === true;
  // The same in-flight guard the queue's own Send now wears. Two prompts racing
  // into one turn is the failure this exists to prevent, and the outbound lane
  // is serialized for exactly that reason — so this covers a queued row being
  // sent AND a direct send that has not settled yet.
  const steerBlockedReason = props.queueSendingId != null
    || props.queueControlsLocked === true
    ? "Another prompt is already being delivered." : null;
  const contextPickerReady = props.onContextOpenChange !== undefined
    && props.onContextRequest !== undefined && props.onContextRemove !== undefined;
  const modelRequired = props.modelRequired;
  const composerLocked = props.sendLocked === true || modelRequired;
  // ONE predicate for the verb, worn by the button and by Alt+Enter alike. The
  // chord preventDefaults before it dispatches, so a chord that is offered on
  // terms the dispatch rule then refuses does not steer AND does not type a
  // newline — it is a dead keystroke on a busy tile with an empty or locked
  // composer, which is precisely where the user is most likely to try it.
  const steerReady = steerOffered && steerBlockedReason === null
    && !composerLocked && draft.trim() !== "";
  const composerLockReason = modelRequired
    ? "Choose a model before sending…"
    : props.sendLockReason ?? "Applying session settings…";
  const commandLauncherReason = composerLocked
    ? composerLockReason
    : queueWhileBusy
    ? "Provider commands are available after the current turn finishes."
    : draft.trim().length > 0
    ? "Clear the current draft before choosing a provider command."
    : null;
  const compactText = contextCompactCommand({
    commands: props.commands,
    live: props.live,
    locked: queueWhileBusy || composerLocked || draft.trim().length > 0,
    hasPromptContext: contextItems.length > 0,
    canRun: props.onRunCommand !== undefined,
  });
  const compactNow = compactText === null ? undefined : () => { send(compactText); };
  const dirtyLabel = props.dirtyLabel;
  const foldable = props.foldable;
  const density = props.density;
  const chooseDensity = props.onChooseDensity;
  const densityIndex = DENSITY_STEPS.findIndex(step => step.value === density);
  const densityStep = DENSITY_STEPS[densityIndex] ?? DENSITY_STEPS[1];
  const nextDensityStep = DENSITY_STEPS[(densityIndex + 1 + DENSITY_STEPS.length)
    % DENSITY_STEPS.length] ?? DENSITY_STEPS[0];

  function send(promptOverride?: string, steer = false) {
    if (typeof promptOverride === "string" && promptOverride !== draft) {
      setDraft(promptOverride);
    }
    const text = (typeof promptOverride === "string" ? promptOverride : draft).trim();
    // Protocol 67: `/btw <question>` asks a side question instead of sending.
    const sideQuestion = props.onSideQuestion === undefined ? null : sideQuestionCommand(text);
    if (sideQuestion !== null && props.onSideQuestion !== undefined) {
      if (sideQuestion === "") {
        setCommandError("Write the question after /btw, as in /btw which test covers this?");
        return;
      }
      if (contextItems.length > 0) {
        setCommandError("A side question takes words only. Remove the attachments, then ask again.");
        return;
      }
      const asked = sessionKey ?? "";
      setCommandError(null);
      setDraft("");
      void props.onSideQuestion(sideQuestion).then(refusal => {
        if (refusal === null || (sessionKeyNow.current ?? "") !== asked) return;
        setDraft(text);
        setCommandError(refusal);
      });
      return;
    }
    const commandLike = parseProviderCommandLike(text);
    const commandIntent = providerCommandIntent(liveCommands, commandsKnown, text);
    if (commandIntent?.kind === "blocked") {
      const provider = props.config.engineLabel ?? "This provider";
      setCommandError(commandIntent.reason === "reserved"
        ? `${productLabel()} did not send /${commandIntent.name}. Use the visible model, effort, `
          + "access, or session controls instead."
        : commandIntent.reason === "multiline"
        ? `${productLabel()} did not send /${commandIntent.name}. Provider commands must stay on one line.`
        : `${provider} no longer advertises /${commandIntent.name} for this session. `
          + "Nothing was sent; choose a currently available command.");
      return;
    }
    const command = commandIntent?.kind === "command" ? commandIntent.command : null;
    const runsAsCommand = command !== null && props.onRunCommand !== undefined;
    if (commandLike !== null && contextItems.length > 0) {
      setCommandError(`Remove attached context before running /${commandLike.name}. `
        + "Provider commands cannot consume files, diffs, or prior-chat attachments.");
      return;
    }
    if (queueWhileBusy && commandLike !== null && !runsAsCommand) {
      setCommandError(`Wait for ${props.config.engineLabel ?? "the provider"} to finish. `
        + "Slash commands are never queued or sent with prompt context.");
      return;
    }
    if (runsAsCommand && queueWhileBusy) {
      setCommandError(`Wait for ${props.config.engineLabel ?? "the provider"} to finish `
        + `before running /${command.name}. Provider commands are never queued.`);
      return;
    }
    const outboundText = runsAsCommand && command !== null
      ? canonicalProviderCommandText(command, text) : text;
    const action = conversationSubmitAction(outboundText, runsAsCommand ? false : queueWhileBusy,
      props.onQueue !== undefined, composerLocked, steer);
    if (action === "ignore") return;
    if (action === "queue") {
      // The caller owns the bounded queue and may refuse at its cap. Calling it
      // before clearing means a refusal (or thrown bug) cannot eat the draft or
      // its typed attachments. Queued prompts deliberately have no transcript echo.
      const pane = props.latest();
      const accepted = queueSubmissionAccepted(pane.onQueue?.(outboundText));
      if (!accepted) return;
      setDraft("");
      if (contextItems.length > 0) pane.onContextConsumed?.(contextItems);
      return;
    }
    setDraft("");
    // Show it. The engine will not: a live session never emits `user.message`,
    // so this echo is the only record of the message until the app is reloaded
    // and the same words come back through history. The shell mints it (see
    // Conversation's postEcho) and your own message pulls the view back down.
    // Attached pictures ride with the echo, so the message looks on screen the
    // way it looked in the composer the instant it was sent.
    const refuseEcho = props.postEcho(
      outboundText,
      runsAsCommand ? [] : promptTurnImages(contextItems),
      runsAsCommand ? [] : promptTurnAttachments(contextItems),
    );

    // The send is fire-and-forget only in the sense that nothing WAITS for it.
    // A rejection is not dropped: it marks this echo undelivered, on the same
    // session's list, so the transcript stops claiming a message that never
    // left the app. The session key is captured deliberately — the answer may
    // land after the user has switched sessions, and it belongs to the one it
    // was sent from. So are the caller's callbacks as they stand at this send.
    const key = sessionKey ?? "";
    const pane = props.latest();
    const sending = runsAsCommand
      ? pane.onRunCommand?.(outboundText) : pane.onSend(outboundText);
    if (sending === undefined) {
      if (!runsAsCommand && contextItems.length > 0) pane.onContextConsumed?.(contextItems);
      return;
    }
    void Promise.resolve(sending).then(
      () => {
        if (!runsAsCommand && contextItems.length > 0) pane.onContextConsumed?.(contextItems);
      },
      (err: unknown) => {
        const said = err instanceof Error ? withoutIpcErrorShape(err.message) : "";
        const why = said !== "" ? said : "the engine did not accept it";
        // Marking the echo undelivered stops the transcript lying; it does not
        // give the user their sentence back. The composer was emptied at
        // dispatch for a message that never left, so hand the text back too —
        // unless newer typing already claimed the draft. The attachments were
        // never consumed on this branch, so the chips are still there.
        //
        // ONLY into the session it was typed for. `setDraft` writes to this
        // render's session, so on the routes that keep one instance across a
        // switch (2 and 3 of ConversationProps' contract) an answer arriving
        // after the pane moved on would put session A's sentence in session B's
        // composer. The echo still gets its refusal below either way — that one
        // is addressed by key and cannot land in the wrong transcript.
        if ((sessionKeyNow.current ?? "") === key) {
          setDraft(prev => draftAfterFailedSend(prev, outboundText));
        }
        refuseEcho(why);
      },
    );
  }

  const afterCommit = props.afterCommit;
  useLayoutEffect(() => { afterCommit(); });

  // Quote (T3 section 9): the selection goes after the typed draft, one
  // blank line apart, and the caret waits on the empty line below it.
  const quoteInto = props.quoteInto;
  useLayoutEffect(() => {
    if (quoteInto === undefined) return undefined;
    quoteInto.current = (text: string): void => {
      const write = appendReplyQuote(drafts === undefined ? draftNow.current : drafts.get(draftKey), text);
      if (write === null) return;
      setDraft(write.text);
      window.requestAnimationFrame(() => {
        const field = textareaRef.current;
        if (field === null) return;
        field.focus();
        field.setSelectionRange(write.cursor, write.cursor);
      });
    };
    return () => { quoteInto.current = null; };
  });
  const stash = props.stash;
  const stashable = stash !== undefined && !stash.busy && draftHasContent(draft, contextItems);

  const threadLayout = props.threadLayout === true;
  // The Threads card's three states: at rest, running and blocked on the
  // owner (amber). Waiting wins: a turn held on an approval is not working,
  // whatever `thinking` says.
  const approvalPending = threadLayout && props.approvalPending === true;
  const running = threadLayout && thinking && !approvalPending;
  // The docked request's keys, from an empty field only: with text in it,
  // ⌘↵ is Enter and queues or sends exactly as it always did.
  const answersApproval = props.onAnswerApproval !== undefined
    && threadComposerAnswersApproval({ waiting: approvalPending, draft, locked: composerLocked });

  /** The slash launcher: an empty draft becomes "/" and the menu opens on it. */
  function startProviderCommand(): void {
    if (commandLauncherReason !== null) return;
    setDraft("/");
    commandMenu.reset();
    setCommandError(null);
    window.requestAnimationFrame(() => { textareaRef.current?.focus(); });
  }

  // THE THREADS "+" MENU: every control that led the row above, in the order
  // it led — context, the slash launcher, the detail dial, the session meter.
  // Nothing is dropped; each keeps its own refusal, now said as the row's
  // detail line, and the meter's figures are the same ones its panel shows.
  const threadMenu: { sections: ThreadMenuSection[]; views: Record<string, ThreadMenuView> } = {
    sections: [], views: {},
  };
  if (threadLayout) {
    const lead: ThreadMenuSection = { key: "lead", rows: [] };
    if (contextPickerReady) {
      const limit = DEFAULT_PROMPT_CONTEXT_LIMIT;
      const blocked = props.contextLocked === true || composerLocked;
      const count = `${contextItems.length} of ${limit}`;
      const request = props.onContextRequest ?? NO_CONTEXT_REQUEST;
      lead.rows = [...lead.rows, {
        key: "context", label: "Add context", meta: count,
        ariaLabel: `Add prompt context, ${contextItems.length} of ${limit} selected`,
        glyph: <ThreadGlyph name="add" size={14} />, opens: "context", disabled: blocked,
        detail: !blocked ? undefined : composerLocked ? composerLockReason
          : "Available once the current send or settings change has landed.",
      }];
      threadMenu.views.context = {
        title: "Add context", meta: count,
        sections: [{ key: "kinds", rows: promptContextOptions(
          props.contextCapabilities ?? NO_CONTEXT_CAPABILITIES, contextItems.length, limit,
        ).map(option => ({
          key: option.kind, label: option.label,
          detail: option.disabledReason ?? option.description,
          disabled: option.disabled,
          glyph: <Icon name={KIND_ICON[option.kind]} sm />,
          onChoose: () => { request(option.kind); },
        })) }],
      };
    }
    if (stash !== undefined) {
      // T3 section 9: park this draft and its chips; ⌘S does the same.
      lead.rows = [...lead.rows, {
        key: "stash", label: "Stash draft", meta: "⌘S",
        glyph: <ThreadGlyph name="down" size={14} />,
        disabled: !stashable,
        detail: stashable ? "Park it here; Stashed brings it back."
          : stash.busy ? "Stashing…" : "Type something or attach a file first.",
        onChoose: stash.onStash,
      }];
    }
    if (commands.length > 0 || props.onRunCommand !== undefined) {
      lead.rows = [...lead.rows, {
        key: "commands", label: "Slash commands",
        glyph: <span className="conv-slash-glyph">/</span>,
        disabled: commandLauncherReason !== null,
        detail: commandLauncherReason ?? undefined,
        onChoose: startProviderCommand,
      }];
    }
    if (lead.rows.length > 0) threadMenu.sections.push(lead);
    if (foldable) {
      threadMenu.sections.push({ key: "detail", heading: "Transcript detail",
        rows: DENSITY_STEPS.map(step => ({
          key: step.value, label: step.label, detail: step.hint,
          checked: step.value === density,
          onChoose: () => { chooseDensity(step.value); },
        })) });
    }
    const readout = threadContextReadout(props.context, props.previousContext);
    if (readout !== null) {
      threadMenu.sections.push({ key: "session", heading: "Session context", rows: [
        { key: "readout", label: "Context window", meta: readout.meta, metaHigh: readout.high,
          figures: readout.lines, disabled: true },
        ...(compactNow === undefined ? [] : [{
          key: "compact", label: "Compact now", onChoose: compactNow,
        }]),
      ] });
    }
  }

  // Above the field in either layout: the slash menu, why the composer is
  // locked, a refused command, and the attached context.
  const banners = (
    <>
      {commandMenuOpen && commandMatches !== null && (
        <ProviderCommandMenu
          id={commandMenuId}
          providerLabel={props.config.engineLabel ?? "Provider"}
          commands={commandMatches}
          activeIndex={activeCommandIndex}
          onActiveIndexChange={commandMenu.setActiveIndex}
          onChoose={commandMenu.insert}
          onSubmit={commandMenu.run}
        />
      )}
      {composerLocked && (
        <div id={lockHelpId} className="conv-lock-reason" role="status">
          <Icon name="info" sm />
          <span>{composerLockReason}</span>
        </div>
      )}
      {commandError !== null && (
        <div id={commandErrorId} className="conv-command-error" role="alert">
          <Icon name="warning" sm />
          <span>{commandError}</span>
        </div>
      )}
      {threadLayout && props.config.error !== null && (
        <div className="conv-command-error" role="alert" title={props.config.error}>
          <Icon name="warning" sm />
          <span>{props.config.error}</span>
        </div>
      )}
      {contextItems.length > 0 && props.onContextRemove !== undefined && (
        <div className="conv-context-row">
          <PromptContextChips items={contextItems}
            onRemove={props.onContextRemove}
            onPreview={props.onContextPreview}
            disabled={props.contextLocked}
            injectStyles={threadLayout || !contextPickerReady} />
        </div>
      )}
    </>
  );
  const field = (
    <textarea
      ref={textareaRef}
      className={threadLayout ? "thread-composer-input" : "conv-ta"}
      rows={1}
      value={draft}
      disabled={composerLocked}
      aria-busy={props.sendLocked || undefined}
      aria-describedby={[
        composerLocked ? lockHelpId : null,
        commandError !== null ? commandErrorId : null,
      ].filter(Boolean).join(" ") || undefined}
      aria-controls={commandMenuOpen ? commandMenuId : undefined}
      aria-expanded={commands.length > 0 ? commandMenuOpen : undefined}
      aria-haspopup={commands.length > 0 ? "listbox" : undefined}
      aria-activedescendant={commandMenuOpen && commandMatches !== null
        && commandMatches.length > 0
        ? providerCommandOptionId(commandMenuId, activeCommandIndex) : undefined}
      aria-autocomplete={commands.length > 0 ? "list" : undefined}
      aria-label={queueDraft ? "Prompt to queue after current turn" : "Message"}
      // Announced only while it does something: a disabled textarea
      // fires no keydown, and there is nothing to interrupt when no
      // turn is running. An empty field also answers a docked request.
      aria-keyshortcuts={answersApproval
        ? [thinking ? "Escape" : null, APPROVAL_ALLOW_KEYSHORTCUTS, APPROVAL_DENY_KEYSHORTCUTS]
          .filter(Boolean).join(" ")
        : thinking && !composerLocked ? "Escape" : undefined}
      // The lock reason is said ONCE, by the banner above that this
      // textarea's aria-describedby already points at (and again on the
      // send button, which is the control that is refusing). Repeating
      // it here stacked two identical sentences 40px apart in the
      // primary control — nine copies across a three-pane split — and
      // read as a rendering bug. What is left is what a placeholder is
      // for: what this field is, with no promise about Enter, which is
      // not available while the composer is locked.
      placeholder={threadLayout
        ? threadComposerPlaceholder({
          locked: composerLocked, queueing: queueDraft, steer: steerOffered,
          waiting: approvalPending,
        })
        : composerLocked
        ? "Message this session…"
        : queueDraft
        ? "Queue after current turn…   (Enter to queue)"
        : "Message this session…   (Enter to send)"}
      onChange={e => {
        setDraft(e.target.value);
        commandMenu.reset();
        setCommandError(null);
      }}
      onBlur={() => { commandMenu.close(); }}
      // A pasted screenshot has no path, so it cannot take the drop
      // route this composer already offers; see pasteImagesIntoDraft.
      // Paths that land after this pane moved to another session are
      // dropped rather than written into that session's draft.
      onPaste={event => {
        const sessionAtPaste = sessionKey;
        pasteImagesIntoDraft(event, {
          enabled: !composerLocked && props.contextLocked !== true,
          onPastedImages: props.onPastedImages,
          updateDraft: setDraft,
          textarea: textareaRef,
          isCurrent: () => sessionKeyNow.current === sessionAtPaste,
        });
      }}
      onKeyDown={e => {
        if (conversationEscapeAction(e,
            { thinking, commandMenuOpen }) === "stop") {
          e.preventDefault();
          e.stopPropagation();
          props.onStop();
          return;
        }
        if (commandMenu.onKeyDown(e)) return;
        // ⌘S stashes a non-empty draft (T3 section 9; free in a composer:
        // docs/APP_MENU.md). An empty draft takes it and does nothing.
        if (stash !== undefined && stashChord(e)) {
          e.preventDefault();
          if (stashable) stash.onStash();
          return;
        }
        // The docked request's own keys, pressed from an empty field: the
        // pane's chord refuses a text field, so the field answers for it,
        // through the card's button. With a draft this never runs.
        if (answersApproval) {
          const verdict = approvalKeyVerdict(e, false);
          if (verdict !== null && props.onAnswerApproval?.(verdict) === true) {
            e.preventDefault();
            e.stopPropagation();
            return;
          }
        }
        if (conversationSteerKeyAction(e, steerReady) === "steer") {
          e.preventDefault();
          send(undefined, true);
          return;
        }
        if (promptSubmitKeyAction(e) === "submit") {
          e.preventDefault();
          send();
        }
      }}
    />
  );

  if (threadLayout) {
    return (
      // THE THREADS LAYOUT. No dirty pill: in a Threads pane each turn's
      // changes card and the head's Changes button say what the session did
      // to the tree, so the pill would be the third copy. The box is the
      // transcript's reading lane; the field comes first, then one quiet row.
      <div className={`thread-composer${running ? " is-working" : ""}${
        approvalPending ? " is-waiting" : ""}`}>
        {/* The kept prompt, on the card's top edge (owner, 2026-11-15). */}
        {props.composerTop}
        {banners}
        {field}
        <div className="thread-composer-foot">
          {threadMenu.sections.length > 0 && (
            <ThreadComposerMenu
              key={sessionKey}
              label="Add context, commands and options"
              sections={threadMenu.sections}
              views={threadMenu.views}
            />
          )}
          {stash !== undefined && (
            <StashedDrafts key={`stash-${sessionKey ?? ""}`} entries={stash.entries}
              busy={stash.busy} onRestore={stash.onRestore} onRemove={stash.onRemove} />
          )}
          <ThreadRunSettings
            key={`settings-${sessionKey ?? ""}`}
            config={props.config}
            locked={props.sendLocked === true}
            headroom={props.headroom}
            onSetModel={props.onSetModel}
            onSetEffort={props.onSetEffort}
            onSetFastMode={props.onSetFastMode}
            onSetPermissionMode={props.onSetPermissionMode}
          />
          <div className="thread-composer-actions">
            {/* Enter's verb while a turn runs, said with its key. */}
            {queueDraft && (
              <button type="button"
                className="conv-iconbtn thread-composer-verb is-queue"
                onClick={() => { send(); }} disabled={!draft.trim() || composerLocked}
                aria-label="Queue after current turn" aria-keyshortcuts="Enter"
                title={composerLocked ? composerLockReason : "Queue after current turn (↵)"}>
                <span>Queue</span>
                <kbd className="thread-composer-kbd" aria-hidden="true">↵</kbd>
              </button>
            )}
            {/* Steer is offered on the same terms as the row below, and its
                chord is Option+Enter (conversationSteerKeyAction), not ⌘↵:
                ⌘Enter is Enter, which queues. */}
            {steerOffered && (
              <button type="button"
                className="conv-iconbtn thread-composer-verb is-steer"
                onClick={() => { send(undefined, true); }}
                disabled={!steerReady}
                aria-label={steerBlockedReason === null
                  ? "Steer" : `Steer unavailable. ${steerBlockedReason}`}
                aria-keyshortcuts="Alt+Enter"
                title={steerBlockedReason ?? "Delivered into the running turn (⌥↵)"}>
                <span>Steer</span>
                <kbd className="thread-composer-kbd" aria-hidden="true">⌥↵</kbd>
              </button>
            )}
            {(thinking || queueWhileBusy) && (
              <button type="button"
                className="conv-iconbtn thread-composer-round thread-composer-stop"
                onClick={props.onStop}
                aria-label="Stop" aria-keyshortcuts={thinking ? "Escape" : undefined}
                title={thinking ? "Stop (Esc)" : "Stop"}>
                <StopSquare />
              </button>
            )}
            {!queueDraft && (
              <button type="button"
                className="conv-iconbtn thread-composer-round thread-composer-send"
                onClick={() => { send(); }} disabled={!draft.trim() || composerLocked}
                aria-label="Send" aria-keyshortcuts="Enter"
                title={composerLocked ? composerLockReason : "Send (↵)"}>
                <SendArrow />
              </button>
            )}
          </div>
        </div>
      </div>
    );
  }

  return (
    <>
      {/* A statement of what the session has done to the working tree, and
          nothing more: the pane it used to open is gone, so the line stays
          and the click does not. */}
      {dirtyLabel !== "" && (
        <div className="conv-dirty">
          {dirtyLabel}
        </div>
      )}
      {/* One box: textarea on top, controls beneath, inside a single border. */}
      <div className="conv-composer">
        {props.composerTop}
        {banners}
        <div className="conv-ta-row">
        <span className="conv-prompt-mark" aria-hidden="true">❯</span>
        {field}
        </div>
        <div className="conv-controls">
          {(commands.length > 0 || props.onRunCommand !== undefined) && (
            <button type="button" className="conv-iconbtn"
              aria-disabled={commandLauncherReason !== null || undefined}
              aria-label={commandLauncherReason === null
                ? `${props.config.engineLabel ?? "Provider"} commands`
                : `${props.config.engineLabel ?? "Provider"} commands unavailable. ${commandLauncherReason}`}
              title={commandLauncherReason
                ?? `${props.config.engineLabel ?? "Provider"} commands`}
              onClick={startProviderCommand}>
              <span aria-hidden="true" className="conv-slash-glyph">/</span>
            </button>
          )}
          {contextPickerReady && (
            <PromptContextPicker
              open={props.contextOpen ?? false}
              items={contextItems}
              capabilities={props.contextCapabilities ?? NO_CONTEXT_CAPABILITIES}
              onOpenChange={props.onContextOpenChange ?? NO_CONTEXT_OPEN}
              onRequest={props.onContextRequest ?? NO_CONTEXT_REQUEST}
              onRemove={props.onContextRemove ?? NO_QUEUE_ID}
              onPreview={props.onContextPreview}
              placement="above"
              showChips={false}
              disabled={props.contextLocked || composerLocked}
            />
          )}
          <ContextMeter usage={props.context} previous={props.previousContext}
            onCompact={compactNow} />
          {/* A GLYPH PER STEP. The dial cycles three states and drew one
              icon in all three, so the only place its position was stated
              was the tooltip — a cycling control whose current value is
              invisible is the one shape that cannot teach itself. Flat for
              compact, a tree for detailed, and the middle mark between
              them; data-density states the same fact for CLI chrome and
              for anything measuring the surface. */}
          {foldable && (
            <button type="button" className="conv-iconbtn conv-detail"
              data-density={densityStep.value}
              aria-label={`Transcript detail: ${densityStep.label}. Change to ${nextDensityStep.label}: ${nextDensityStep.hint}`}
              title={`${densityStep.label} detail. Next: ${nextDensityStep.label} — ${nextDensityStep.hint}`}
              onClick={() => { chooseDensity(nextDensityStep.value); }}>
              <Icon name={densityStep.value === "compact" ? "list-flat"
                : densityStep.value === "detailed" ? "list-tree" : "list-selection"} />
            </button>
          )}
          {/* Keyed on the session so a switch drops an open picker, the same
              reason the textarea draft is cleared above. */}
          <ConfigPills
            key={sessionKey}
            config={props.config}
            usage={props.context}
            previousContext={props.previousContext}
            onCompact={compactNow}
            locked={props.sendLocked === true}
            planName={props.planName}
            headroom={props.headroom}
            running={thinking && !composerLocked}
            onSetModel={props.onSetModel}
            onSetEffort={props.onSetEffort}
            onSetFastMode={props.onSetFastMode}
          />
          <div className="conv-actions">
            {queueWhileBusy && !thinking && (
              <button type="button" className="conv-iconbtn" onClick={props.onStop}
                aria-label="Stop" title="Stop">
                <Icon name="debug-stop" />
              </button>
            )}
            {/* The second verb, and deliberately the quieter one: Enter
                still queues, and this is the way to say "into the turn that
                is already running" out loud. Only engines whose adapter
                proved the path get it — see EngineCapabilities.steer. */}
            {steerOffered && (
              <button type="button" className="conv-iconbtn conv-steer"
                onClick={() => { send(undefined, true); }}
                disabled={!steerReady}
                aria-label={steerBlockedReason === null
                  ? "Steer" : `Steer unavailable. ${steerBlockedReason}`}
                title={steerBlockedReason ?? "Delivered into the running turn"}>
                {/* The queue's own Send now mark: the same act, said the
                    same way, so the two controls do not look like two
                    different things. */}
                <Icon name="play" sm />
                <span>Steer</span>
              </button>
            )}
            <button type="button"
              className={`conv-iconbtn conv-send${queueDraft ? " conv-send-queue" : ""}`}
              onClick={() => { send(); }} disabled={!draft.trim() || composerLocked}
              aria-label={queueDraft ? "Queue after current turn" : "Send"}
              title={composerLocked
                ? composerLockReason
                : queueDraft
                ? "Queue after current turn (Enter)" : "Send (Enter)"}>
              <Icon name="send" />
              {queueDraft && <span>Queue</span>}
            </button>
          </div>
        </div>
      </div>
    </>
  );
});
