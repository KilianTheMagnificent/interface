import { WorkflowControls, type WorkflowApi } from "./WorkflowControls";
import "./Composer.css";
import { useEffect, useId, useMemo, useRef, useState } from "react";
import type { KeyboardEvent, ReactNode } from "react";
import type {
  ProviderCommand, ProviderCommandCatalogue, ProviderCommandScope, SessionKind,
} from "../../shared/engine";
import { parseProviderCommandLike } from "../../shared/provider-commands";
import { accessModeOption, accessModeOptions, accessModeToneClass } from "../access-modes";
import { findCatalogModel } from "../model-catalog";
import { buildLaunchOptions } from "../launch";
import type { BranchList, WorktreeEntry } from "../../shared/threads";
import type { ProviderAccountsView } from "../../shared/provider-accounts";
import { accountChoiceFor, chosenAccount } from "../provider-accounts-view";
import { homeRelativePath, lastSegment } from "../display-paths";
import {
  threadCardAllowsSeveralModels, threadCardKind, threadCardWording, threadFanOutForms,
  threadFormsOnAccount,
  threadWorkspaceLine, type ThreadCardProject, type ThreadWorkspaceChoice,
} from "../thread-card";
import { modelPresetFor, type ModelPreset } from "../model-presets";
import {
  normalizePromptContextItems,
  type PromptContextCapabilities,
  type PromptContextItem,
  type PromptContextKind,
  type PromptContextOption,
} from "../prompt-context";
import { promptDropZone, type PromptDropPath } from "../prompt-drop";
import { pasteImagesIntoDraft, type PastedFile } from "../prompt-paste";
import { promptSubmitKeyAction } from "../prompt-submit";
import { AppSelect, type AppSelectOption } from "./AppSelect";
import { EffortBar } from "./EffortBar";
import {
  dismissedPromptContextMention, IDLE_PROMPT_CONTEXT_MENTION_STATE,
  movedPromptContextMention, promptContextMentionChoice, promptContextMentionKeyAction,
  promptContextMentionOptionId, promptContextMentionView, PromptContextChips,
  PromptContextMentionMenu, PromptContextPicker, releasedPromptContextMention,
  trackedPromptContextMention, type PromptContextMentionState,
} from "./PromptContextPicker";
import { ProviderLogo, providerLogoKind } from "./ProviderLogo";
import {
  ProviderCommandMenu, providerCommandOptionId,
} from "./ProviderCommandMenu";
import { providerLabel } from "../provider-labels";
import { ProductName } from "./ProductName";
import { productLabel } from "../product";
import { projectHueClass } from "../project-hue";
import { ThinkingMark } from "./ThinkingMark";
import { ThreadGlyph } from "./ThreadGlyph";
import { useProviderCommandMenu } from "../use-provider-command-menu";
import { DropIndicator } from "./DropIndicator";
import {
  advertisedModels, composerAccessModeAction, composerAccessModes,
  composerChatPermissionMode, composerCustomModelChoice, composerErrorField,
  composerFormForKind, composerFormForModelChoice, composerFormOnReadyModel, composerKindEngines,
  composerModelLabel,
  composerModelOptions,
  composerProviderCommandResponseMatches, composerProviderCommandScope,
  composerProviderCommandScopesEqual, composerProviderForModelValue,
  composerRememberedPermissionMode, composerRepositoryName, composerScopeOptions,
  decodeComposerModelChoice, DEFAULT_DANGEROUS_MODES, effortsForModel,
  encodeComposerModelChoice, resolveComposerCommandPrompt, seedLaunchForm,
  sessionOptionsSummary,
  type ComposerDefaults, type ComposerLaunchForm, type ComposerModelChoice,
  type EngineChoices, type EngineOption, type FieldError, type ScopeOptions,
} from "../composer-model";
import "../codicons.css";
import "../theme.css";

/**
 * The Threads surface's New thread card. With it the card picks a project
 * first: "No project" is the chat card exactly as it was (kind `chat`, chat
 * engines only), and a project makes it a coding session (kind `code`) with
 * every engine, the access mode and a workspace control. The caller owns the
 * project list, what is remembered, Rust's branch and worktree lists, and the
 * prepare → launch → bind sequence; this card only draws them.
 */
export interface ComposerThreadCard {
  workflow?: WorkflowApi;
  projects: readonly ThreadCardProject[];
  /** The chosen project root; null is "No project". */
  project: string | null;
  onProjectChange: (project: string | null) => void;
  /** The folder picker; resolves with the chosen folder, or null. */
  onChooseFolder: () => Promise<string | null>;
  workspace: ThreadWorkspaceChoice;
  onWorkspaceChange: (workspace: ThreadWorkspaceChoice) => void;
  /** Rust's branches for the project; null while not known. */
  branches: BranchList | null;
  /** The project's existing worktrees; null while not known. */
  worktrees: readonly WorktreeEntry[] | null;
  /** Why this project cannot have a new worktree (not a git repository), or null. */
  worktreeUnavailable: string | null;
  /** Rust's refusal of the last workspace, shown in the card. The draft stays. */
  error: string | null;
  /**
   * What happened before this card opened that the owner should see while it
   * covers the pane (Edit from the first message restored the files), or null.
   */
  note?: string | null;
  /** A prepare → launch → bind sequence is running. */
  busy: boolean;
  /**
   * Protocol 60: each provider's accounts. With more than one for the chosen
   * provider, an account control sits beside the model control.
   */
  accounts?: ProviderAccountsView | null;
  /**
   * Start one thread per form (several only with a new worktree each).
   * `background` (⌘Enter) starts it without switching to it. Resolves true
   * when every thread started; the card then keeps its project and workspace
   * and clears only the prompt.
   */
  onLaunch: (forms: readonly ComposerLaunchForm[], scope: ScopeOptions,
    context: readonly PromptContextItem[], background: boolean) => Promise<boolean>;
}

const THREAD_PROJECT_NONE = "\u0000no-project";
const THREAD_PROJECT_CHOOSE = "\u0000choose-folder";
const THREAD_EXISTING_PREFIX = "existing:";

// Presentational: every side effect (directory picker, session creation,
// persistence of the last cwd) is the caller's, delivered through callbacks.
// `choicesFor` is a pure lookup the caller owns — it is called during render
// with the engine currently selected in this form, so it must not have effects.
export interface ComposerProps {
  open: boolean;
  /** Which surface is launching. Absent means "code". */
  kind?: SessionKind;
  /**
   * Seed values, re-read on every closed -> open transition. The scoping fields
   * are optional: omit them and the card opens at COMPOSER_SCOPE_DEFAULTS, i.e.
   * exactly the behaviour this app had before scoping existed.
   */
  defaults: ComposerDefaults;
  engines: EngineOption[];
  choicesFor: (engine: string) => EngineChoices;
  /**
   * Provider-owned command discovery for the exact draft launch scope. Composer
   * owns request generations because model/project/scope selection is local to
   * this component; late answers can therefore never repopulate another model.
   */
  loadProviderCommands?: (scope: ProviderCommandScope) => Promise<ProviderCommandCatalogue>;
  /** Newest-first settings remembered for exact provider/model pairs. */
  modelPresets?: readonly ModelPreset[];
  /**
   * Each provider's plan headroom, by engine id (review E): said quietly on
   * the model chip for the chosen provider.
   */
  headroom?: Readonly<Record<string, { readonly text: string; readonly title: string }>>;
  /**
   * The teammate a task is for (its name). Its own permission mode, folders
   * and settings decide how the task runs (protocol 38), so the card offers
   * no access mode and no session options.
   */
  teammate?: string;
  /** Persists a changed model/effort/access combination outside this component. */
  onModelPresetChange?: (preset: ModelPreset) => void;
  /**
   * `form` is the whole form (a ComposerForm, so buildLaunchOptions takes it
   * unchanged); `scope` is the resolved LaunchOptions subset to spread beside
   * it. A caller that ignores the second argument silently drops every scoping
   * control the user set, so take it.
   */
  onLaunch: (form: ComposerLaunchForm, scope: ScopeOptions,
    contextItems?: readonly PromptContextItem[]) => void;
  onBrowse: (preferredPath?: string) => Promise<string | null>;
  onClose: () => void;
  /** Launch or directory errors rendered above the dock, not below the canvas. */
  notice?: ReactNode;
  /** One createSession request is in flight; freezes this exact launch snapshot. */
  launching?: boolean;
  /** Optional controlled prompt-context integration. Omission preserves the old launch contract. */
  contextItems?: readonly PromptContextItem[];
  onRecipeContext?: (items: PromptContextItem[]) => void;
  contextCapabilities?: PromptContextCapabilities;
  contextOpen?: boolean;
  /** Context picker/read failure shown inside the visible composer overlay. */
  contextError?: string | null;
  onContextOpenChange?: (open: boolean) => void;
  onContextRequest?: (kind: PromptContextKind, cwd: string) => void;
  onContextRemove?: (id: string) => void;
  onContextPreview?: (item: PromptContextItem) => void;
  /** Paths dropped onto the launch card; the caller grants and attaches them. */
  onDroppedPaths?: (paths: readonly PromptDropPath[]) => void;
  /** Images pasted into the prompt; the caller stores and attaches the bytes.
   *  Saved paths come back so a non-empty draft can insert them at the end. */
  onPastedImages?: (files: readonly PastedFile[]) =>
    void | Promise<readonly string[] | void>;
  pathForFile?: (file: { name: string }) => string;
  /** Reports the repository actually held by the form after browse or reseed. */
  onRepositoryChange?: (cwd: string) => void;
  /** The Threads surface's project-first card. Absent keeps the card as it was. */
  thread?: ComposerThreadCard;
}

type ComposerCommandLoadState =
  | { status: "idle" }
  | { status: "loading"; requested: ProviderCommandScope }
  | { status: "ready"; requested: ProviderCommandScope; commands: readonly ProviderCommand[] }
  | { status: "unavailable"; requested: ProviderCommandScope; reason: string };

const IDLE_COMMAND_LOAD: ComposerCommandLoadState = { status: "idle" };
const NO_PROVIDER_COMMANDS: readonly ProviderCommand[] = [];

const EMPTY_PROMPT_CONTEXT_ITEMS: readonly PromptContextItem[] = [];
const EMPTY_PROMPT_CONTEXT_CAPABILITIES: PromptContextCapabilities = {};
const EMPTY_MODEL_PRESETS: readonly ModelPreset[] = [];

/** `focusVisible` is standard FocusOptions; some DOM lib versions omit it. */
interface QuietFocusOptions extends FocusOptions { focusVisible?: boolean }

/* Focus WITHOUT the focus ring. Measured in this app's own Chromium (Electron
   43.3.0, a real BrowserWindow): script focus on a <textarea> matches
   :focus-visible — `autoFocus` included, since React focuses on mount — while
   `focus({ focusVisible: false })` gives :focus and NOT :focus-visible. That is
   the whole difference between "the caret is here so start typing" and "this
   surface is permanently outlined in the accent". Exported so the option that
   keeps the opening focus quiet is a fact a test can pin. */
export const COMPOSER_QUIET_FOCUS: QuietFocusOptions = { focusVisible: false };
const ignoreContextRemove = (): void => undefined;


export function Composer(props: ComposerProps) {
  const {
    open, kind: kindProp = "code", defaults, engines, choicesFor, onLaunch, onBrowse, onClose,
    loadProviderCommands,
    modelPresets = EMPTY_MODEL_PRESETS,
    onModelPresetChange,
    headroom,
    teammate,
    contextItems = EMPTY_PROMPT_CONTEXT_ITEMS,
    contextCapabilities = EMPTY_PROMPT_CONTEXT_CAPABILITIES,
    contextOpen,
    onContextOpenChange,
    onContextRequest,
    onRecipeContext,
    onContextRemove = ignoreContextRemove,
    onDroppedPaths,
    onPastedImages,
    pathForFile,
    onRepositoryChange,
    launching: launchingProp = false,
    notice,
    thread,
  } = props;
  const uid = useId();
  // The Threads card's kind follows its project: none is a chat, a project is
  // a coding session. Everything below reads `kind`, so the chat card it falls
  // back to is the chat card exactly.
  const kind = thread === undefined ? kindProp : threadCardKind(thread.project);
  const launching = launchingProp || thread?.busy === true;
  /** A Threads card's seeded form: fitted to its kind and held at its project. */
  function threadSeed(seeded: ComposerLaunchForm): ComposerLaunchForm {
    if (thread === undefined) return seeded;
    return { ...composerFormOnReadyModel(
      composerFormForKind(seeded, kind, engines, choicesFor, modelPresets),
      kind, engines, choicesFor, modelPresets), cwd: thread.project ?? "" };
  }
  // A chat has no folder of its own, so every control that only means something
  // against one is ABSENT here rather than disabled: the repository button, the
  // access mode that governs what may happen inside that folder, the session
  // options behind them, and attached prompt context of any kind.
  const folderScoped = kind !== "chat";
  const [wasOpen, setWasOpen] = useState(open);
  const [form, setForm] = useState<ComposerLaunchForm>(() =>
    threadSeed(seedLaunchForm(defaults, choicesFor(defaults.engine),
      modelPresetFor(modelPresets, defaults.engine, defaults.model))));
  // The kind the form was last fitted to. A Threads card switching between a
  // chat and a project refits the form once, in the render that sees it.
  const [formKind, setFormKind] = useState(kind);
  // Models Shift-chosen beside the primary one (a project with a new worktree
  // only): each starts a thread of its own.
  const [extraModels, setExtraModels] = useState<readonly ComposerModelChoice[]>([]);
  // The account the owner picked on the New thread card; the provider's
  // default account until then, and again when the provider changes.
  const [accountPick, setAccountPick] = useState<{ engine: string; account: string } | null>(null);
  const accountChoice = thread === undefined ? null : accountChoiceFor(thread.accounts, form.engine);
  const threadAccount = chosenAccount(accountChoice,
    accountPick?.engine === form.engine ? accountPick.account : null);
  const [error, setError] = useState<FieldError | null>(null);
  const [browsing, setBrowsing] = useState(false);
  const [localContextOpen, setLocalContextOpen] = useState(false);
  // Disclosure state for the scope panel, and the second step of the full-access
  // confirmation. Neither is part of the form: closing the panel changes nothing
  // about the session, and an unconfirmed arming attempt has changed nothing yet.
  const [scopeOpen, setScopeOpen] = useState(false);
  const [confirmingFullAccess, setConfirmingFullAccess] = useState(false);
  const [dropHover, setDropHover] = useState(0);
  const [commandLoad, setCommandLoad] = useState<ComposerCommandLoadState>(IDLE_COMMAND_LOAD);
  const [commandError, setCommandError] = useState<string | null>(null);
  // Typed "@" completion. Every rule lives in PromptContextPicker's controller
  // so the per-session composer can wire the same decisions instead of growing
  // its own; this card holds only the state the controller transitions.
  const [mentionState, setMentionState] =
    useState<PromptContextMentionState>(IDLE_PROMPT_CONTEXT_MENTION_STATE);
  const commandRequestGeneration = useRef(0);
  const commandLoaderRef = useRef(loadProviderCommands);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const repositoryChangeRef = useRef(onRepositoryChange);
  const defaultRepositoryRef = useRef(defaults.cwd);
  commandLoaderRef.current = loadProviderCommands;
  repositoryChangeRef.current = onRepositoryChange;
  defaultRepositoryRef.current = defaults.cwd;

  // Scope support is provider-owned. In particular, an old Claude "isolated"
  // selection must not travel to Codex or Grok merely because the hidden form
  // field still contains it while the provider switch is committing.
  const commandChoices = useMemo(
    () => choicesFor(form.engine),
    [choicesFor, form.engine],
  );
  const commandScope = composerProviderCommandScope(form, commandChoices);
  const commandSupportsSettingSources = (commandChoices.settingSources ?? []).length > 0;
  const hasCommandLoader = loadProviderCommands !== undefined;
  // A selection change invalidates the previous catalogue synchronously, one
  // render before the next effect starts loading. The prompt itself is never
  // touched by catalogue state or a late response.
  const activeCommandLoad = commandLoad.status !== "idle"
      && composerProviderCommandScopesEqual(commandLoad.requested, commandScope)
    ? commandLoad : IDLE_COMMAND_LOAD;
  const providerCommands = activeCommandLoad.status === "ready"
    ? activeCommandLoad.commands : NO_PROVIDER_COMMANDS;
  // The slash menu this card shares with a session's composer. Writing the
  // prompt through `set` clears a command refusal, as typing does.
  const commandMenu = useProviderCommandMenu({
    commands: providerCommands, draft: form.prompt,
    available: activeCommandLoad.status !== "idle",
    setDraft: text => { set("prompt", text); },
    submit: text => { submit(text); },
    focus: () => { textareaRef.current?.focus(); },
  });

  useEffect(() => {
    const generation = ++commandRequestGeneration.current;
    const loader = commandLoaderRef.current;
    const requested = composerProviderCommandScope(form, choicesFor(form.engine));
    if (!open || loader === undefined || requested === null) {
      setCommandLoad(IDLE_COMMAND_LOAD);
      return;
    }
    let cancelled = false;
    setCommandLoad({ status: "loading", requested });
    commandMenu.reset();
    setCommandError(null);
    void (async () => {
      try {
        const result = await loader(requested);
        if (cancelled || generation !== commandRequestGeneration.current) return;
        if (!composerProviderCommandResponseMatches(result, requested)) {
          setCommandLoad({
            status: "unavailable", requested,
            reason: "The provider returned commands for a different launch configuration.",
          });
          return;
        }
        setCommandLoad(result.available
          ? { status: "ready", requested, commands: result.commands }
          : {
              status: "unavailable", requested,
              reason: result.reason?.trim() || "Commands are unavailable for this model.",
            });
      } catch {
        if (cancelled || generation !== commandRequestGeneration.current) return;
        setCommandLoad({
          status: "unavailable", requested,
          reason: "Could not load commands for this model.",
        });
      }
    })();
    return () => { cancelled = true; };
  // `choicesFor` is a pure lookup but may return a fresh object. These scalar
  // dependencies are the complete request identity and avoid reload loops.
  }, [open, form.engine, form.model, form.cwd, form.settingScope,
    commandSupportsSettingSources, hasCommandLoader]);

  // Notify only after commit. Composer historically reseeds during the
  // closed -> open render, so calling the parent in that block would update a
  // different component while this one is rendering. Refs retain the latest
  // optional callback/default without re-notifying merely because either prop
  // identity changes while the card stays open. Mounting open is also a seed.
  useEffect(() => {
    if (open) repositoryChangeRef.current?.(defaultRepositoryRef.current);
  }, [open]);
  // A Threads card's repository is its project (none for a chat), reported
  // after the seed above so the caller's context root follows the picker.
  const threadProject = thread === undefined ? undefined : thread.project ?? "";
  useEffect(() => {
    if (open && threadProject !== undefined) repositoryChangeRef.current?.(threadProject);
  }, [open, threadProject]);

  // Opening the card puts the caret in the prompt, which is what `autoFocus`
  // used to do — but as a quiet focus, so the box's accent edge stays a focus
  // INDICATOR instead of the card's default border. See COMPOSER_QUIET_FOCUS.
  useEffect(() => {
    if (!open) return;
    textareaRef.current?.focus(COMPOSER_QUIET_FOCUS);
  }, [open]);

  const selectedModel = form.model.trim();
  const kindEngines = useMemo(() => composerKindEngines(kind, engines, choicesFor),
    [kind, engines, choicesFor]);
  const selectedKindEngine = kindEngines.find(engine => engine.id === form.engine);
  const chatLaunchUnavailable = kind !== "chat" ? null
    : !kindEngines.some(engine => engine.unavailable === undefined)
    ? "Chat requires an available Claude provider. Check providers in Settings."
    : selectedKindEngine === undefined || selectedKindEngine.unavailable !== undefined
    ? "Choose an available Chat-compatible model." : null;
  const { modelRows, selectedCatalogModel } = useMemo(() => {
    if (!open) {
      return { modelRows: [] as AppSelectOption[], selectedCatalogModel: undefined };
    }
    return {
      modelRows: kind !== "chat"
        ? composerModelOptions(engines, choicesFor, form.engine, selectedModel)
        : kindEngines.length === 0 ? [] : composerModelOptions(
          kindEngines, choicesFor,
          selectedKindEngine?.id ?? kindEngines[0].id,
          selectedKindEngine === undefined ? "" : selectedModel),
      selectedCatalogModel: findCatalogModel(
        selectedModel, advertisedModels(commandChoices), form.engine),
    };
  }, [open, kind, engines, form.engine, kindEngines, selectedKindEngine,
    choicesFor, selectedModel, commandChoices]);

  // The component keeps its state while closed, so the closed -> open
  // transition is the reset point: re-seed the fields from `defaults`.
  if (open !== wasOpen) {
    setWasOpen(open);
    if (open) {
      setForm(threadSeed(seedLaunchForm(defaults, choicesFor(defaults.engine),
        modelPresetFor(modelPresets, defaults.engine, defaults.model))));
      setFormKind(kind);
      setExtraModels([]);
      setError(null);
      setBrowsing(false);
      setLocalContextOpen(false);
      // A card that reopens with the panel shut and the danger step disarmed is
      // the whole point of making it two clicks: an abandoned attempt from the
      // last launch must never be one click from arming this one.
      setScopeOpen(false);
      setConfirmingFullAccess(false);
      setCommandLoad(IDLE_COMMAND_LOAD);
      commandMenu.reset();
      setCommandError(null);
      setMentionState(IDLE_PROMPT_CONTEXT_MENTION_STATE);
    }
  }

  if (thread !== undefined && open) {
    const projectCwd = thread.project ?? "";
    if (kind !== formKind) {
      setFormKind(kind);
      setForm(current => ({
        ...composerFormForKind(current, kind, engines, choicesFor, modelPresets), cwd: projectCwd,
      }));
    } else if (form.cwd !== projectCwd) {
      setForm(current => ({ ...current, cwd: projectCwd }));
    }
  }
  const severalModels = thread !== undefined
    && threadCardAllowsSeveralModels(thread.project, thread.workspace);
  if (!severalModels && extraModels.length > 0) setExtraModels([]);

  if (!open) return null;

  const selectedContextItems = normalizePromptContextItems(contextItems);
  const commandQueryMatches = commandMenu.matches;
  const commandQueryActive = commandQueryMatches !== null;
  const commandMenuOpen = commandMenu.open;
  const activeCommandIndex = commandMenu.activeIndex;
  const promptContextOpen = contextOpen ?? localContextOpen;
  const changePromptContextOpen = (next: boolean): void => {
    if (contextOpen === undefined) setLocalContextOpen(next);
    onContextOpenChange?.(next);
  };

  // TYPING "@" IS THE SAME ATTACHMENT FLOW AS PRESSING "+".
  // It offers the kinds this engine actually supports, because both read the
  // one capability map; a caller with no `onContextRequest` has no surface to
  // resolve a chosen kind, so "@" stays inert text there rather than opening a
  // menu that cannot finish. The slash menu wins a tie: only one completion is
  // ever open under this caret.
  const mentionView = promptContextMentionView({
    draft: form.prompt,
    state: mentionState,
    capabilities: contextCapabilities,
    selectedCount: selectedContextItems.length,
    enabled: folderScoped && onContextRequest !== undefined && !launching,
    suppressed: commandMenuOpen,
  });
  const mentionCapable = mentionView.capable;
  const mentionMenuOpen = mentionView.open;
  const mentionMenuId = `${uid}-prompt-context-mention`;
  // Both completions are the same combobox on the same textarea.
  const commandCombobox = commandScope !== null && hasCommandLoader;
  const promptCombobox = commandCombobox || mentionCapable;
  const promptPopupOpen = commandMenuOpen || mentionMenuOpen;

  /** Follow the caret; `retarget` is the typing case, which re-points at row 0. */
  const trackPromptCaret = (node: HTMLTextAreaElement, retarget = false): void => {
    setMentionState(state => trackedPromptContextMention(state, node, retarget));
  };

  const chooseMention = (option: PromptContextOption): void => {
    // One decision for the pointer and the keyboard, and the one that refuses a
    // capped row: nothing is removed from the draft when the row cannot be taken.
    const choice = promptContextMentionChoice(mentionView, option, form.prompt);
    if (choice === null) return;
    set("prompt", choice.edit.prompt);
    setMentionState(IDLE_PROMPT_CONTEXT_MENTION_STATE);
    // The requested surface is usually another modal (Quick Open, the URL
    // dialog). Restore the caret only if the draft still owns focus after that
    // mounts — `setSelectionRange` never moves focus, so this cannot pull the
    // user out of an aria-modal the way a deferred `focus()` would.
    const caret = choice.edit.caret;
    window.requestAnimationFrame(() => {
      const node = textareaRef.current;
      if (node === null || document.activeElement !== node) return;
      node.setSelectionRange(caret, caret);
    });
    onContextRequest?.(choice.option.kind, form.cwd);
  };

  const set = <K extends keyof ComposerLaunchForm>(key: K, value: ComposerLaunchForm[K]) => {
    setForm(f => ({ ...f, [key]: value }));
    setError(e => (e && (e.field === key || e.field === "form") ? null : e));
    setCommandError(null);
  };

  async function browse() {
    if (browsing) return;
    setBrowsing(true);
    try {
      const picked = await onBrowse(form.cwd || undefined);
      if (picked) {
        set("cwd", picked);
        onRepositoryChange?.(picked);
      }
    } catch {
      setError({ field: "cwd", message: "Could not open the directory picker." });
    } finally {
      setBrowsing(false);
    }
  }

  function submit(promptOverride?: string, background = false) {
    if (launching) return;
    if (chatLaunchUnavailable !== null) return;
    const prompt = typeof promptOverride === "string" ? promptOverride : form.prompt;
    if (typeof promptOverride === "string" && promptOverride !== form.prompt) {
      set("prompt", promptOverride);
    }
    const pendingCommandLike = parseProviderCommandLike(prompt.trim());
    if (prompt.trim() === "/" && commandQueryActive) {
      setCommandError(activeCommandLoad.status === "unavailable"
        ? activeCommandLoad.reason : "Choose a provider command before starting the chat.");
      return;
    }
    const commandResolution = resolveComposerCommandPrompt(
      providerCommands, activeCommandLoad.status === "ready", prompt,
      providerLabel(form.engine, engines.find(option => option.id === form.engine)?.label),
    );
    if (commandResolution.kind === "blocked") {
      setCommandError(commandResolution.message);
      return;
    }
    if (pendingCommandLike !== null && selectedContextItems.length > 0) {
      setCommandError(`Remove attached context before running /${pendingCommandLike.name}. `
        + "Provider commands must stay on one line and cannot consume prompt attachments.");
      return;
    }
    // The Access mode control is hidden on a chat, so the mode this form was
    // seeded with must not travel to the engine from behind it either — nor may
    // stale per-launch worktree state, because that toggle is off the card too.
    // A chat has no repository to branch.
    const launchForm: ComposerLaunchForm = {
      ...form, prompt: commandResolution.prompt, kind,
      ...(kind === "chat" ? { permissionMode: "", worktree: false } : {}),
    };
    const result = buildLaunchOptions(launchForm);
    if ("error" in result) {
      // Attributed by the same rule launch.ts validates by — cwd only where
      // this card has a folder at all, then prompt, then model.
      setError({ field: composerErrorField(launchForm), message: result.error });
      return;
    }
    setError(null);
    setCommandError(null);
    // A control this engine does not offer must not SEND a value either — an
    // engine that hides the settings preset (or the tool list) is one that
    // ignores the field, and a leftover from a caller-supplied default would
    // otherwise travel to it and be silently dropped on the far side.
    // `choicesFor` is contractually a pure lookup, so asking again is free.
    const engineChoices = choicesFor(form.engine);
    const effective: ComposerLaunchForm = {
      ...launchForm,
      ...((engineChoices.settingSources ?? []).length > 0 ? {} : { settingScope: "all" as const }),
      ...(engineChoices.toolDenyList === true ? {} : { disallowedTools: "" }),
    };
    rememberModelSettings(effective);
    if (thread !== undefined) {
      // One thread per model. The extras take the primary's prompt and
      // scoping, each on its own model's remembered effort and access mode,
      // and each is named for its model so the rows read apart.
      const forms = threadFanOutForms([effective, ...extraModels.map(choice => {
        const fitted = composerFormForModelChoice(effective, choice, choicesFor(choice.engine),
          modelPresetFor(modelPresets, choice.engine, choice.model));
        const choicesOf = choicesFor(choice.engine);
        return {
          ...fitted, prompt: effective.prompt, kind,
          ...((choicesOf.settingSources ?? []).length > 0 ? {} : { settingScope: "all" as const }),
          ...(choicesOf.toolDenyList === true ? {} : { disallowedTools: "" }),
        };
      })], each => composerModelLabel(choicesFor(each.engine), each.engine, each.model));
      void thread.onLaunch(threadFormsOnAccount(forms, effective.engine, threadAccount), composerScopeOptions(effective), selectedContextItems, background)
        .then(started => {
          if (!started || !background) return;
          // ⌘Enter: the thread runs on its own and a fresh card stays, on the
          // same project, workspace and model.
          setForm(current => ({ ...current, prompt: "" }));
          setExtraModels([]);
          window.requestAnimationFrame(() => { textareaRef.current?.focus(COMPOSER_QUIET_FOCUS); });
        });
      return;
    }
    // The resolved scoping options travel as the second argument so the caller
    // spreads them beside buildLaunchOptions' result and cannot forget them.
    onLaunch(effective, composerScopeOptions(effective), selectedContextItems);
  }

  const errorFor = (name: FieldError["field"]) =>
    error?.field === name
      ? <div className="composer-error" role="alert">{error.message}</div>
      : null;

  // Keep a value the caller did not advertise selectable rather than letting
  // the select silently render blank.
  const keep = (value: string, offered: string[]): string[] =>
    value && value !== "default" && !offered.includes(value)
      ? [value, ...offered] : offered;

  // The combined model picker owns provider routing. Everything after it is
  // still derived from the provider embedded in the selected model row.
  const choices = commandChoices;
  const dangerousModes = choices.dangerousModes ?? DEFAULT_DANGEROUS_MODES;
  // Every provider-advertised row stays visible. A dangerous row does not write
  // the form from the menu: its selection opens the existing confirmation below,
  // and only that confirmation arms it. This keeps the provider vocabulary
  // complete without turning bypass into a one-click escalation.
  const modes = composerAccessModes(form.permissionMode, choices.permissionModes);
  const armed = dangerousModes.includes(form.permissionMode);
  // The arming control exists only for an engine that actually offers such a
  // mode; for one that does not (Codex today reports untrusted/on-request/never
  // and none of them is on the dangerous list) there is nothing to arm.
  const dangerousMode = choices.permissionModes.find(m => dangerousModes.includes(m));
  const sources = choices.settingSources ?? [];
  const showSources = sources.length > 0;
  const offersProject = sources.includes("project");
  // Hidden unless the engine says it honours a deny list — see EngineChoices.
  const showTools = choices.toolDenyList === true;
  const modeNote = accessModeOptions([form.permissionMode], form.engine)[0]?.description;
  const levels = keep(form.effort, effortsForModel(choices, form.model, form.engine));
  const modelMissing = selectedModel === "" || selectedModel === "default";
  const selectedModelChoice = modelMissing
    ? "" : encodeComposerModelChoice(form.engine,
      selectedCatalogModel?.value ?? selectedModel);
  const providerLabelFor = (engine: string): string =>
    providerLabel(engine, engines.find(option => option.id === engine)?.label);
  const selectedProviderLabel = providerLabelFor(form.engine);
  const commandMenuId = `${uid}-provider-commands`;
  const commandErrorId = `${uid}-provider-command-error`;
  const commandMenuEmptyLabel = activeCommandLoad.status === "loading"
    ? `Loading ${selectedProviderLabel} commands for ${selectedModel}…`
    : activeCommandLoad.status === "unavailable"
    ? activeCommandLoad.reason
    : providerCommands.length === 0
    ? `No ${selectedProviderLabel} commands are available for this model.`
    : "No matching commands";
  const commandLauncherReason = launching
    ? "Wait for the new chat to start."
    : form.prompt.trim().length > 0
    ? "Clear the current prompt before choosing a provider command."
    : null;
  const providerGroupLogo = (option: AppSelectOption) => {
    const engine = composerProviderForModelValue(option.value);
    return engine === null ? null
      : <ProviderLogo engine={engine} label={providerLabelFor(engine)} />;
  };
  const rememberModelSettings = (candidate: ComposerLaunchForm): void => {
    const engine = candidate.engine.trim();
    const model = candidate.model.trim();
    if (engine === "" || model === "" || model === "default") return;
    onModelPresetChange?.({
      engine,
      model,
      effort: candidate.effort.trim() || "default",
      // A chat card carries a mode it never offered a control for, so its value
      // is not a choice and must never be the one written here.
      permissionMode: kind === "chat"
        ? composerChatPermissionMode(engine, choicesFor(engine),
          modelPresetFor(modelPresets, engine, model))
        : composerRememberedPermissionMode(
          candidate.permissionMode, form.permissionMode),
    });
  };
  const setRememberedModelSetting = <K extends "effort" | "permissionMode">(
      key: K, value: ComposerLaunchForm[K]): void => {
    const next = { ...form, [key]: value };
    set(key, value);
    rememberModelSettings(next);
  };
  const applyModelChoice = (choice: ComposerModelChoice): void => {
    const providerChanged = choice.engine !== form.engine;
    const next = composerFormForModelChoice(
      form, choice, choicesFor(choice.engine),
      modelPresetFor(modelPresets, choice.engine, choice.model));
    // Preserve the settings being left even when the user never launches that
    // draft, then promote the selected model and its effective settings.
    rememberModelSettings(form);
    setForm(next);
    rememberModelSettings(next);
    if (providerChanged) {
      setScopeOpen(false);
      setConfirmingFullAccess(false);
    }
    commandMenu.reset();
    setCommandError(null);
    setError(current => current?.field === "form" ? null : current);
  };
  const selectModel = (value: string): void => {
    const choice = decodeComposerModelChoice(value);
    if (choice !== null) applyModelChoice(choice);
  };
  // The New thread card's own rows. Recent projects first, then the two
  // actions; "No project" is the plain chat.
  const projectRows: AppSelectOption[] = thread === undefined ? [] : [
    ...thread.projects.map(project => ({
      value: project.root, label: project.label, group: "Projects",
      description: homeRelativePath(project.root),
    })),
    { value: THREAD_PROJECT_CHOOSE, label: "Choose folder…",
      description: "Start a thread in another folder." },
    { value: THREAD_PROJECT_NONE, label: "No project",
      description: "A plain conversation, without tools or files." },
  ];
  // The workspace drawer's rows: the worktrees that already exist (its third
  // segment) and the branches a new worktree may start from.
  const existingWorktreeRows: AppSelectOption[] = [];
  const baseBranchRows: AppSelectOption[] = [];
  if (thread !== undefined && thread.project !== null) {
    const project = thread.project;
    const existing = (thread.worktrees ?? []).filter(entry => entry.path !== project);
    const chosen = thread.workspace.mode === "existing" ? thread.workspace.worktreePath : null;
    if (chosen !== null && !existing.some(entry => entry.path === chosen)) {
      existing.unshift({ path: chosen });
    }
    for (const entry of existing) {
      existingWorktreeRows.push({
        value: `${THREAD_EXISTING_PREFIX}${entry.path}`,
        label: entry.branch ?? lastSegment(entry.path),
        description: homeRelativePath(entry.path),
        meta: entry.ownerKey !== undefined ? "Thread" : undefined,
      });
    }
    const branches = thread.branches;
    baseBranchRows.push({
      value: "",
      label: branches?.current !== undefined ? `Current branch (${branches.current})` : "Current branch",
      triggerLabel: branches?.current ?? "Current branch",
      description: "Branch from what the project has checked out now.",
    });
    const base = thread.workspace.mode === "worktree" ? thread.workspace.baseRef : undefined;
    const names = [...(branches?.branches ?? [])];
    if (base !== undefined && !names.includes(base)) names.unshift(base);
    for (const name of names) {
      baseBranchRows.push({ value: name, label: name,
        meta: name === branches?.defaultBranch ? "default" : undefined });
    }
  }
  // The New thread card's words, and the project's name for the drawer's line.
  const threadWording = threadCardWording(thread?.project ?? null);
  const threadProjectLabel = thread === undefined || thread.project === null ? ""
    : thread.projects.find(project => project.root === thread.project)?.label
      ?? lastSegment(thread.project);
  // The thread card's model chip names the model alone: its provider is the
  // logo beside it, and both stay in the menu's rows.
  const modelChipRows = thread === undefined ? modelRows
    : modelRows.map(row => row.custom === true ? row : { ...row, triggerLabel: row.label });
  const extraModelValues = new Set(extraModels.map(choice =>
    encodeComposerModelChoice(choice.engine, choice.model)));
  const toggleExtraModel = (value: string): void => {
    const choice = decodeComposerModelChoice(value);
    if (choice === null) return;
    setExtraModels(current => current.some(entry =>
        entry.engine === choice.engine && entry.model === choice.model)
      ? current.filter(entry => entry.engine !== choice.engine || entry.model !== choice.model)
      : [...current, choice]);
  };
  const engineDown = engines.find(e => e.id === form.engine)?.unavailable;
  const modeRows = accessModeOptions(modes, form.engine);
  const repositoryName = composerRepositoryName(form.cwd);
  const showEffort = levels.length > 0;
  const optionDetails = sessionOptionsSummary(form, armed, {
    sources: showSources, tools: showTools,
  });
  const optionLabel = optionDetails === ""
    ? "Session options" : `Session options · ${optionDetails}`;
  // What this surface asks for, and what it starts. Both name the one thing the
  // card in front of the user actually does.
  const promptPlaceholder = thread !== undefined ? threadWording.placeholder
    : kind === "chat" ? "Send a message"
    : kind === "agent" ? "What should the agent do in this folder?"
    : `Give ${productLabel()} a task`;
  const submitLabel = kind === "agent" ? "Start task"
    : thread !== undefined && kind === "code" ? "Start thread" : "Start chat";
  // The hero under the wordmark, same rule: Code's line offers a file path and a
  // traceback, which is a promise a tool-less chat cannot keep and an
  // understatement of what a task in a folder does. The New thread card has
  // no hero: its headline asks the question around the project picker.
  const introTitle = kind === "chat" ? "Start a conversation"
    : kind === "agent" ? "Give your agent a task" : "Start a coding session";
  const introBody = kind === "chat" ? "Explore an idea, ask a question, or work through a problem."
    : kind === "agent"
      ? "Describe the job. The agent works in the folder you choose "
        + "and shows its progress here."
    : "Drop a file path, a traceback, or a rough idea. I’ll investigate, "
      + "suggest next steps, and keep things reversible.";
  // The drop zone this card shares with a session's composer; see promptDropZone.
  const dropZone = promptDropZone({
    enabled: !launching, pathForFile, onDroppedPaths, onPastedImages, setHover: setDropHover,
  });
  // The provider-command affordance. The New thread card's footer opens on
  // "+" (prompt context), so there it comes second.
  const commandButton = hasCommandLoader && commandScope !== null && (
    <button type="button" className="composer-icon-btn composer-command-button"
      aria-disabled={commandLauncherReason !== null || undefined}
      aria-label={commandLauncherReason === null
        ? `${selectedProviderLabel} commands for ${selectedModel}`
        : `${selectedProviderLabel} commands for ${selectedModel} unavailable. ${commandLauncherReason}`}
      title={commandLauncherReason
        ?? `${selectedProviderLabel} commands for ${selectedModel}`}
      onClick={() => {
        if (commandLauncherReason !== null) return;
        set("prompt", "/");
        commandMenu.reset();
        window.requestAnimationFrame(() => { textareaRef.current?.focus(); });
      }}
    >/</button>
  );
  // The footer's model chips: the model, the models Shift added, and effort.
  const modelControls = (<>
    {/* Provider routing is encoded in each model row. This is one list
        of everything the registered adapters can actually launch, so
        choosing Claude, Codex, Grok or Qwen never requires a preliminary
        provider selection. */}
    <AppSelect
      label="Model"
      value={selectedModelChoice}
      options={modelChipRows}
      placeholder="Choose model"
      leading={<ProviderLogo engine={form.engine} label={selectedProviderLabel} />}
      renderGroupLeading={providerGroupLogo}
      onChange={selectModel}
      onCustom={(model, option) => {
        applyModelChoice(composerCustomModelChoice(
          option?.value, model, form.engine));
      }}
      searchable={modelRows.length > 8}
      searchPlaceholder="Search models and providers"
      compact
      popoverClassName="composer-model-popover"
      className="composer-app-picker composer-model-picker"
      trailing={headroom?.[form.engine]?.text}
      trailingTitle={headroom?.[form.engine]?.title}
      disabled={kind === "chat" && kindEngines.length === 0}
      onShiftChoose={severalModels ? toggleExtraModel : undefined}
      checkedValues={severalModels ? extraModelValues : undefined}
    />
    {accountChoice !== null && threadAccount !== null && (
      <AppSelect
        label="Account"
        value={threadAccount}
        options={accountChoice.options}
        onChange={value => { setAccountPick({ engine: form.engine, account: value }); }}
        compact
        popoverClassName="composer-model-popover"
        className="composer-app-picker composer-account-picker"
      />
    )}
    {severalModels && extraModels.length > 0 && (
      <button type="button" className="composer-pill composer-pill-btn composer-thread-models"
        title={`Also starting: ${extraModels.map(choice =>
          `${providerLabelFor(choice.engine)} · ${choice.model}`).join(", ")}. Click to clear.`}
        aria-label={`Also starting ${extraModels.length} more ${extraModels.length === 1
          ? "thread" : "threads"}: ${extraModels.map(choice => choice.model).join(", ")}. Clear`}
        onClick={() => { setExtraModels([]); }}>
        <span className="composer-pill-v">{`+${extraModels.length} ${
          extraModels.length === 1 ? "model" : "models"}`}</span>
        <span className="codicon codicon-sm codicon-close" aria-hidden="true" />
      </button>
    )}
    {showEffort && (
      <EffortBar
        className="composer-app-picker composer-effort-picker"
        levels={levels}
        value={form.effort}
        notes={levels.includes("max")
          ? { max: "Applies to this session only." } : undefined}
        onChange={value => { setRememberedModelSetting("effort", value); }}
      />
    )}
  </>);
  // The footer's access mode and the Session options behind it, for a card
  // that works in a folder.
  const accessControls = folderScoped && teammate === undefined && (<>
    {/* How much this agent may do on its own. The most important
        control on the card once several sessions run at once — it is
        the difference between one agent you supervise and three you let
        run — which is why it keeps the one bordered round pill in the
        app. Dangerous modes remain visible, but selecting one opens the
        separately-confirmed control below instead of arming it. */}
    <AppSelect
      label="Access mode"
      value={form.permissionMode}
      options={modeRows}
      leading={thread === undefined ? undefined : <ThreadGlyph name="access" size={14} />}
      onChange={value => {
        if (composerAccessModeAction(value, dangerousModes) === "confirm") {
          setScopeOpen(true);
          setConfirmingFullAccess(true);
          return;
        }
        setConfirmingFullAccess(false);
        setRememberedModelSetting("permissionMode", value);
      }}
      compact
      popoverClassName="composer-model-popover"
      className={["composer-app-picker composer-access-picker",
        accessModeToneClass(form.permissionMode),
        armed ? "composer-danger-picker" : ""].filter(Boolean).join(" ")}
    />

    {/* Advanced session setup stays behind one quiet disclosure. At
        defaults it says only "Session options"; once changed, its
        compact summary keeps those non-default choices visible. */}
    <button
      type="button"
      className={armed
        ? "composer-pill composer-pill-btn composer-danger"
        : "composer-pill composer-pill-btn"}
      aria-expanded={scopeOpen}
      aria-controls={`${uid}-scope`}
      // The New thread card's footer keeps it to a gear, naming only
      // what was changed; the full name is its accessible one.
      aria-label={thread === undefined ? undefined : optionLabel}
      title={optionDetails === ""
        ? "Session options: setup, worktree, and tool restrictions"
        : `Session options — ${optionDetails}`}
      onClick={() => {
        const next = !scopeOpen;
        setScopeOpen(next);
        if (!next) setConfirmingFullAccess(false);
      }}
    >
      {thread !== undefined && (
        <span className="codicon codicon-sm codicon-settings-gear" aria-hidden="true" />
      )}
      <span className="composer-pill-v">{thread === undefined ? optionLabel : optionDetails}</span>
      <span
        className={scopeOpen
          ? "codicon codicon-sm codicon-chevron-down composer-chevron"
          : "codicon codicon-sm codicon-chevron-down composer-chevron is-collapsed"}
        aria-hidden="true"
      />
    </button>
  </>);
  // The New thread card's workspace, a project's only (a chat has no folder
  // to work in): where its edits land — the project's own checkout, a new
  // worktree from a base branch, or a worktree that already exists — as the
  // chips beside the project, above the prompt. Its one line is the group's
  // description, said in words the segments' titles also carry.
  const threadWorkspace = thread === undefined || thread.project === null ? null : (
    <div className="composer-thread-drawer">
      <div className="composer-thread-segments" role="group" aria-label="Workspace"
        aria-describedby={`${uid}-workspace-line`}>
        <button type="button" className="composer-thread-segment"
          aria-pressed={thread.workspace.mode === "local"}
          title="Work in the project’s own checkout."
          onClick={() => { thread.onWorkspaceChange({ mode: "local" }); }}>
          Local checkout
        </button>
        <button type="button" className="composer-thread-segment"
          aria-pressed={thread.workspace.mode === "worktree"}
          aria-disabled={thread.worktreeUnavailable !== null || undefined}
          title={thread.worktreeUnavailable
            ?? "A new git worktree and branch, so this thread’s edits stay apart."}
          onClick={() => {
            // Pressing it again keeps the base branch already chosen.
            if (thread.worktreeUnavailable !== null || thread.workspace.mode === "worktree") return;
            thread.onWorkspaceChange({ mode: "worktree" });
          }}>
          New worktree
        </button>
        {existingWorktreeRows.length > 0 && (
          <AppSelect
            label="Existing worktree"
            value={thread.workspace.mode === "existing"
              ? `${THREAD_EXISTING_PREFIX}${thread.workspace.worktreePath}` : ""}
            options={existingWorktreeRows}
            placeholder="Existing…"
            onChange={value => {
              if (!value.startsWith(THREAD_EXISTING_PREFIX)) return;
              thread.onWorkspaceChange({ mode: "existing",
                worktreePath: value.slice(THREAD_EXISTING_PREFIX.length) });
            }}
            popoverClassName="composer-model-popover"
            className={thread.workspace.mode === "existing"
              ? "composer-thread-existing is-on" : "composer-thread-existing"}
          />
        )}
      </div>
      <span id={`${uid}-workspace-line`} className="composer-thread-line">
        {threadWorkspaceLine(thread.workspace, threadProjectLabel)}
      </span>
      {thread.workspace.mode === "worktree" ? (
        <AppSelect
          label="Base branch"
          value={thread.workspace.baseRef ?? ""}
          options={baseBranchRows}
          leading={<ThreadGlyph name="branch" size={12} />}
          searchable={baseBranchRows.length > 8}
          searchPlaceholder="Search branches"
          onChange={value => {
            thread.onWorkspaceChange(value === ""
              ? { mode: "worktree" } : { mode: "worktree", baseRef: value });
          }}
          popoverClassName="composer-model-popover"
          className="composer-app-picker composer-branch-picker"
        />
      ) : thread.workspace.mode === "local" && thread.branches?.current !== undefined ? (
        // Local works on whatever the project has checked out: said, not offered.
        <span className="composer-thread-branch" title="The branch the project has checked out now">
          <ThreadGlyph name="branch" size={12} />
          <span className="composer-sr">Current branch </span>
          <span className="composer-thread-branch-name">{thread.branches.current}</span>
        </span>
      ) : null}
    </div>
  );

  return (
    <section
      // The Threads card's project keeps its hue only as the square in its
      // picker (project-hue.ts); the card itself is the theme's own.
      className={(dropHover > 0 ? "composer-card is-drop-target" : "composer-card")
        + (thread !== undefined ? " composer-thread-card" : "")}
      aria-labelledby={`${uid}-title`}
      aria-busy={launching || undefined}
      {...dropZone}
      onKeyDown={(e: KeyboardEvent<HTMLElement>) => {
        if (e.key !== "Escape" || launching) return;
        e.stopPropagation();
        if (scopeOpen) {
          setScopeOpen(false);
          setConfirmingFullAccess(false);
        } else onClose();
      }}
    >
      {dropHover > 0 && (
        <DropIndicator label="Drop files to attach" />
      )}
      <button
        type="button"
        className="composer-icon-btn composer-dismiss"
        onClick={onClose}
        aria-label="Close composer"
        title="Close composer"
      >
        <span className="codicon codicon-close" aria-hidden="true" />
      </button>

      <div className="composer-body" inert={launching ? true : undefined}>
        {thread === undefined ? (
          <div className="composer-intro">
            <span className="composer-eyebrow"><ProductName /></span>
            <h1 id={`${uid}-title`}>{introTitle}</h1>
            <p>{introBody}</p>
          </div>
        ) : (<>
          {/* The New thread card's one column opens on its question beside
              the still thinking mark — "What should we build in atlas?" —
              then the chips that say where it runs: the project (its hue a
              square, No project a ring) and, for a project, its workspace. */}
          <h1 id={`${uid}-title`} className="composer-thread-headline">
            <ThinkingMark still size={36} className="composer-thread-mark" />
            <span className="composer-thread-question">
              {threadWording.lead}
              {thread.project !== null && <>{" "}
                <span className="composer-thread-name">{threadProjectLabel}</span></>}
              {threadWording.trail !== "" && (
                <span className="composer-thread-trail">{threadWording.trail}</span>
              )}
            </span>
          </h1>
          <div className="composer-thread-chips">
            <AppSelect
              label="Project"
              value={thread.project ?? THREAD_PROJECT_NONE}
              options={projectRows}
              leading={<span className={`composer-thread-hue ${projectHueClass(thread.project)}`}
                aria-hidden="true" />}
              onChange={value => {
                if (value === THREAD_PROJECT_NONE) thread.onProjectChange(null);
                else if (value === THREAD_PROJECT_CHOOSE) {
                  void thread.onChooseFolder().then(path => {
                    if (path !== null && path !== "") thread.onProjectChange(path);
                  }, () => {
                    setError({ field: "form", message: "Could not open the folder picker." });
                  });
                } else thread.onProjectChange(value);
              }}
              popoverClassName="composer-model-popover"
              className="composer-thread-project"
            />
            {threadWorkspace}
          </div>
        </>)}
        {notice !== undefined && notice !== null && (
          <div className="composer-notice">{notice}</div>
        )}
        {errorFor("cwd")}
        {thread?.note != null && (
          <div className="composer-notice composer-thread-note" role="status">{thread.note}</div>
        )}
        {thread?.error != null && (
          <div className="composer-error" role="alert">{thread.error}</div>
        )}

        <div className="composer-dock">
        <div className="composer-box" data-provider={providerLogoKind(form.engine)}>
          <label className="composer-sr" htmlFor={`${uid}-prompt`}>Prompt</label>
          <textarea
            ref={textareaRef}
            id={`${uid}-prompt`}
            className="composer-ta"
            rows={thread === undefined ? 1 : 3}
            value={form.prompt}
            placeholder={promptPlaceholder}
            aria-invalid={error?.field === "prompt" || undefined}
            aria-describedby={commandError !== null ? commandErrorId : undefined}
            aria-controls={commandMenuOpen ? commandMenuId
              : mentionMenuOpen ? mentionMenuId : undefined}
            aria-expanded={promptCombobox ? promptPopupOpen : undefined}
            aria-haspopup={promptCombobox ? "listbox" : undefined}
            aria-activedescendant={commandMenuOpen && commandQueryMatches !== null
                && commandQueryMatches.length > 0
              ? providerCommandOptionId(commandMenuId, activeCommandIndex)
              : mentionMenuOpen
              ? promptContextMentionOptionId(mentionMenuId, mentionView.activeIndex) : undefined}
            aria-autocomplete={promptCombobox ? "list" : undefined}
            onChange={e => {
              set("prompt", e.target.value);
              commandMenu.reset();
              // Editing the query re-points at the top row, the same way the
              // command menu behaves one line below.
              trackPromptCaret(e.target, true);
            }}
            // A selection change is the other way the caret reaches an "@", and
            // the controller is what refuses a RANGE selection here: the anchor
            // of a drag-select is not a caret, and treating it as one let Enter
            // delete the token the user had just selected past.
            onSelect={e => { trackPromptCaret(e.currentTarget); }}
            // A pasted screenshot is the one attachment with no path, so it
            // cannot travel the drop route above; see pasteImagesIntoDraft.
            onPaste={event => {
              pasteImagesIntoDraft(event, {
                enabled: !launching,
                onPastedImages,
                updateDraft: update => {
                  setForm(current => ({ ...current, prompt: update(current.prompt) }));
                },
                textarea: textareaRef,
              });
            }}
            onBlur={() => {
              commandMenu.close();
              setMentionState(releasedPromptContextMention);
            }}
            onKeyDown={e => {
              // Null means this key is NOT the menu's — at the item cap that is
              // every Enter and every Tab, which must reach the submit and the
              // focus order exactly as they would with no menu open.
              const mentionKey = promptContextMentionKeyAction(mentionView, form.prompt, e);
              if (mentionKey !== null) {
                e.preventDefault();
                e.stopPropagation();
                if (mentionKey.type === "move") {
                  setMentionState(state => movedPromptContextMention(state, mentionKey.index));
                } else if (mentionKey.type === "dismiss") {
                  setMentionState(state =>
                    dismissedPromptContextMention(state, mentionKey.start));
                } else chooseMention(mentionKey.option);
                return;
              }
              if (commandMenu.onKeyDown(e)) return;
              // ⌘Enter (with or without ⌥) on the New thread card starts the
              // thread in the background and leaves a fresh card here.
              if (thread !== undefined && e.key === "Enter" && (e.metaKey || e.ctrlKey)
                  && !e.nativeEvent.isComposing) {
                e.preventDefault();
                submit(undefined, true);
                return;
              }
              if (promptSubmitKeyAction(e) === "submit") { e.preventDefault(); submit(); }
            }}
          />

          {commandMenuOpen && (
            <ProviderCommandMenu
              id={commandMenuId}
              providerLabel={selectedProviderLabel}
              commands={commandQueryMatches ?? NO_PROVIDER_COMMANDS}
              activeIndex={activeCommandIndex}
              placement="inline"
              emptyLabel={commandMenuEmptyLabel}
              onActiveIndexChange={commandMenu.setActiveIndex}
              onChoose={commandMenu.insert}
              onSubmit={commandMenu.run}
            />
          )}

          {mentionMenuOpen && (
            <PromptContextMentionMenu
              id={mentionMenuId}
              options={mentionView.options}
              activeIndex={mentionView.activeIndex}
              onActiveIndexChange={index => {
                setMentionState(state => movedPromptContextMention(state, index));
              }}
              onChoose={chooseMention}
            />
          )}

          {commandError !== null && (
            <div id={commandErrorId} className="composer-command-error" role="alert">
              <span className="codicon codicon-sm codicon-warning" aria-hidden="true" />
              <span>{commandError}</span>
            </div>
          )}

          {selectedContextItems.length > 0 && (
            <div className="composer-context-items">
              <PromptContextChips
                items={selectedContextItems}
                onRemove={onContextRemove}
                onPreview={props.onContextPreview}
                injectStyles={false}
              />
            </div>
          )}
          {props.contextError && (
            <div className="composer-error" role="alert">{props.contextError}</div>
          )}

          <div className="composer-controls">
            <div className="composer-control-row composer-control-primary">
            {thread === undefined && commandButton}
            {folderScoped && (<>
            <PromptContextPicker
              open={promptContextOpen}
              items={selectedContextItems}
              capabilities={contextCapabilities}
              onOpenChange={changePromptContextOpen}
              onRequest={kind => { onContextRequest?.(kind, form.cwd); }}
              onRemove={onContextRemove}
              onPreview={props.onContextPreview}
              placement="above"
              showChips={false}
              triggerHint={mentionCapable
                ? "Add prompt context — or type @ in the prompt" : undefined}
            />
            {thread === undefined && <button
              type="button"
              className="composer-context-button"
              onClick={browse}
              disabled={browsing}
              aria-label="Choose repository"
              aria-invalid={error?.field === "cwd" || undefined}
              title={form.cwd || "Choose a repository"}
            >
              <span className="composer-context-name">
                {browsing ? "Opening…" : repositoryName}
              </span>
              <span className="codicon codicon-sm codicon-chevron-down" aria-hidden="true" />
            </button>}
            </>)}
            {thread !== undefined && commandButton}
            {/* The New thread card reads as the Code mode canvas: access and
                Session options beside "+" and "/", then the model chips and
                Start at the row's right end. Every other card keeps its row. */}
            {thread === undefined ? modelControls : accessControls}
            </div>

            <div className="composer-control-row composer-control-secondary">
            {thread === undefined ? accessControls : modelControls}

            <span className="composer-actions">
              <button
                type="button"
                className="composer-submit"
                onClick={() => { submit(); }}
                disabled={launching || modelMissing || chatLaunchUnavailable !== null}
                aria-label={launching ? "Starting session" : chatLaunchUnavailable ?? (modelMissing
                  ? "Choose a model before starting the chat" : submitLabel)}
                title={launching ? "Starting session…" : chatLaunchUnavailable ?? (modelMissing
                  ? "Choose a model" : `${submitLabel} (Enter)`)}
              >
                {thread !== undefined && !launching
                  ? <ThreadGlyph name="send" className="composer-submit-glyph" />
                  : <span className={`codicon codicon-${launching
                    ? "loading codicon-modifier-spin" : "send"}`} aria-hidden="true" />}
                <span className="composer-submit-label">
                  {launching ? "Starting…" : submitLabel}
                </span>
              </button>
            </span>
            </div>
          </div>

          {/* A teammate's card has no Session options or access mode to open
              this with (see `teammate`). */}
          {scopeOpen && (
            <div className="composer-scope" id={`${uid}-scope`}>
              {showSources && (
                <fieldset className="composer-group">
                  <legend className="composer-legend">Settings this session loads</legend>

                  <label className="composer-choice">
                    <input
                      type="radio"
                      name={`${uid}-sources`}
                      checked={form.settingScope === "all"}
                      onChange={() => set("settingScope", "all")}
                    />
                    <span className="composer-choice-body">
                      <span className="composer-choice-name">Everything (default)</span>
                      <span className="composer-choice-hint">
                        Your global settings, this project’s, and its CLAUDE.md (including
                        imported instructions such as AGENTS.md) — every hook, skill and
                        MCP server you have installed comes with it.
                      </span>
                    </span>
                  </label>

                  {offersProject && (
                    <label className="composer-choice">
                      <input
                        type="radio"
                        name={`${uid}-sources`}
                        checked={form.settingScope === "project"}
                        onChange={() => set("settingScope", "project")}
                      />
                      <span className="composer-choice-body">
                        <span className="composer-choice-name">Project only</span>
                        <span className="composer-choice-hint">
                          This project’s settings and its CLAUDE.md, including imported
                          instructions such as AGENTS.md. Your global hooks, skills and
                          MCP servers stay out — a much cheaper session.
                        </span>
                      </span>
                    </label>
                  )}

                  <label className="composer-choice">
                    <input
                      type="radio"
                      name={`${uid}-sources`}
                      checked={form.settingScope === "isolated"}
                      onChange={() => set("settingScope", "isolated")}
                    />
                    <span className="composer-choice-body">
                      <span className="composer-choice-name">Isolated</span>
                      {/* Stated outright. A control that silently switches off
                          the repository's standing instructions would be a
                          nasty surprise arriving as strange agent behaviour. */}
                      <span className="composer-choice-hint">
                        No settings files at all — and no CLAUDE.md or imported AGENTS.md,
                        so this agent gets none of the project’s standing instructions.
                      </span>
                    </span>
                  </label>
                </fieldset>
              )}

              {/* SHELVED 2026-08-27: the team switch. Only its form fields
                  remain (composer-model.ts, ComposerTeam): the launch path
                  that read them is gone and no run has an orchestrator host,
                  so restoring it means rebuilding the team, not putting this
                  fieldset back. Its four CSS rules were deleted too. */}
              {thread === undefined && <label className="composer-choice">
                <input
                  type="checkbox"
                  checked={form.worktree}
                  onChange={e => set("worktree", e.target.checked)}
                />
                <span className="composer-choice-body">
                  <span className="composer-choice-name">New worktree</span>
                  <span className="composer-choice-hint">
                    A new git worktree and branch, so concurrent work cannot overwrite each
                    other’s edits. Off, the agent works in the Local checkout.
                  </span>
                </span>
              </label>}

              {/* ONE field, and it is the deny list. The removed "allow only"
                  box sent LaunchOptions.allowedTools, which auto-approves the
                  named tools instead of restricting the agent to them — see
                  ComposerScope for the SDK's own wording. What is left says
                  exactly what it sends. */}
              {showTools && (
                <fieldset className="composer-group">
                  <legend className="composer-legend">Tools</legend>
                  <div className="composer-tools">
                    <span className="composer-tool">
                      <label className="composer-label" htmlFor={`${uid}-deny`}>
                        never use
                      </label>
                      <input
                        id={`${uid}-deny`}
                        className="composer-tool-input"
                        value={form.disallowedTools}
                        placeholder="Bash, WebFetch"
                        onChange={e => set("disallowedTools", e.target.value)}
                      />
                    </span>
                  </div>
                  {/* Honest about both the effect and the failure mode: this app
                      has no list of the engine's tool names to validate against,
                      so a typo is not an error, it is a restriction that never
                      applies. */}
                  <span className="composer-choice-hint">
                    These tools are taken away from the agent — it cannot use them and
                    cannot ask you for them. Empty leaves the engine’s own set. Names
                    must match the engine’s exactly; one it does not recognise is
                    ignored, not reported.
                  </span>
                </fieldset>
              )}

              {dangerousMode !== undefined && (
                <div className={armed ? "composer-fullaccess composer-fullaccess-on" : "composer-fullaccess"}>
                  {/* The picker's own label for this mode, read from the same
                      table the pill reads: one card must not name the same
                      choice twice, and the legend used to say "Bypass
                      permissions" beside a pill that says "Skip prompts". */}
                  <span className="composer-legend composer-danger">
                    {accessModeOption(dangerousMode, form.engine).label}
                  </span>
                  {armed ? (
                    <>
                      <span className="composer-choice-hint composer-danger">
                        Interactive approval prompts are bypassed for this session.
                        Configured restrictions and environment limits can still block actions.
                      </span>
                      <span className="composer-row">
                        <button
                          type="button"
                          className="btn"
                          onClick={() => {
                            setRememberedModelSetting("permissionMode", "default");
                            setConfirmingFullAccess(false);
                          }}
                        >
                          Turn permission bypass off
                        </button>
                      </span>
                    </>
                  ) : confirmingFullAccess ? (
                    // Step two. The consequence is spelled out at the moment of
                    // the decision, not in a tooltip somewhere above it.
                    <>
                      <span className="composer-choice-hint composer-danger">
                        {`Sets ${dangerousMode}: the provider will skip interactive approval
                          prompts for this session. Other configured restrictions may still
                          block actions. Use it only where unattended changes are acceptable.`}
                      </span>
                      <span className="composer-row">
                        <button
                          type="button"
                          className="btn composer-danger"
                          onClick={() => {
                            setRememberedModelSetting("permissionMode", dangerousMode);
                            setConfirmingFullAccess(false);
                          }}
                        >
                          <span className="codicon codicon-sm codicon-warning" aria-hidden="true" />
                          Yes, skip approval prompts
                        </button>
                        <button
                          type="button"
                          className="btn"
                          onClick={() => { setConfirmingFullAccess(false); }}
                        >
                          Cancel
                        </button>
                      </span>
                    </>
                  ) : (
                    <>
                      <span className="composer-choice-hint">
                        Skips interactive approval prompts for this session. Other configured
                        restrictions still apply. Two clicks, on purpose.
                      </span>
                      <span className="composer-row">
                        <button
                          type="button"
                          className="btn composer-danger"
                          onClick={() => { setConfirmingFullAccess(true); }}
                        >
                          Enable permission bypass…
                        </button>
                      </span>
                    </>
                  )}
                </div>
              )}
            </div>
          )}
          {errorFor("model")}
        </div>

        {thread?.project != null && thread.workflow !== undefined && (
          <WorkflowControls key={thread.project} project={thread.project} api={thread.workflow}
            form={form} context={selectedContextItems} workspace={thread.workspace} launching={launching}
            onUse={draft => {
              setForm(current => ({ ...composerFormForModelChoice(current,
                { engine: draft.recipe.engine, model: draft.recipe.model }, choicesFor(draft.recipe.engine)), prompt: draft.prompt }));
              setExtraModels([]);
              thread.onWorkspaceChange(draft.recipe.newWorktree
                ? { mode: "worktree", ...(draft.recipe.baseRef === undefined ? {} : { baseRef: draft.recipe.baseRef }) }
                : { mode: "local" });
              onRecipeContext?.(draft.context.map((item, index) => ({
                id: `recipe-${index}`, kind: item.kind, value: item.value,
                label: item.kind === "url" ? item.value : lastSegment(item.value),
              })));
            }} />
        )}

        {/* Ordinary access policy is already named by the visible selector and
            explained in its app-owned menu. Only bypass keeps a persistent
            warning below the composer because it changes the safety boundary. */}
        {armed && modeNote !== undefined && (
          <div className="composer-note composer-danger">
            <span className="codicon codicon-sm codicon-warning" aria-hidden="true" />
            <span>{modeNote}</span>
          </div>
        )}

        {errorFor("prompt")}

        {engineDown !== undefined && (
          // The engine is registered, its CLI just did not answer the probe.
          // Saying so here beats a silent failure on Launch, and the launch is
          // still allowed in case it has come back — but it reads as the same
          // error the sidebar banner shows for this fact, not as a milder one.
          <div className="composer-note" role="status" title={engineDown}>
            <span className="codicon codicon-sm codicon-error composer-warn" aria-hidden="true" />
            <span>{form.engine} is not responding — {engineDown}</span>
          </div>
        )}

        {chatLaunchUnavailable !== null && (
          <div className="composer-notice" role="status">{chatLaunchUnavailable}</div>
        )}
        {errorFor("form")}

        {thread !== undefined && (
          <div className="composer-thread-foot">
            {thread.project !== null && (
              <button type="button" className="composer-thread-link"
                onClick={() => {
                  thread.onProjectChange(null);
                  // The link goes with the project; the caret goes back to the prompt.
                  window.requestAnimationFrame(() => {
                    textareaRef.current?.focus(COMPOSER_QUIET_FOCUS);
                  });
                }}>
                or chat without a project
              </button>
            )}
            <span className="composer-thread-keys">
              <kbd>↵</kbd> start
              <span aria-hidden="true">·</span>
              <kbd>⌘↵</kbd> start and write another
            </span>
          </div>
        )}
        </div>
      </div>
    </section>
  );
}
