import { useCallback, useMemo, useRef, useState, type Dispatch, type RefObject, type SetStateAction } from "react";
import type { EngineId, SessionKey, SessionKind, SessionSummary } from "../shared/engine";
import type { CollaborationEngine } from "../shared/ipc";
import type { AgentControlResponse, AgentRequest } from "../shared/agents";
import type { ThreadCommand, ThreadControlResponse, ThreadsView } from "../shared/threads";
import { agentTaskCommand } from "./agent-profiles";
import type { IntegratedTerminalCenterHandle } from "./components/IntegratedTerminalCenter";
import type { ThreadOpinionAction } from "./components/ThreadOpinionCards";
import {
  COMPOSER_SCOPE_DEFAULTS, COMPOSER_TEAM_DEFAULTS, composerScopeOptions, startingPermissionMode,
  type ComposerLaunchForm, type EngineChoices, type ScopeOptions,
} from "./composer-model";
import { ENGINE_CHOICES, UNKNOWN_ENGINE_CHOICES } from "./engine-choices";
import { latestModelPresetFor, modelPresetFor, type ModelPreset, type ModelPresetSettings } from "./model-presets";
import type { PromptContextItem } from "./prompt-context";
import { providerLabel } from "./provider-labels";
import { accountDisplayLabel } from "./provider-accounts-view";
import type { ProviderAccountsView } from "../shared/provider-accounts";
import type { AppState } from "./store";
import { threadHandoffTask, threadProjectRoot, type ThreadTeammate } from "./thread-inbox";
import {
  continuationName, threadOpinionCards, threadProviderOffers, type ThreadOpinionCard,
  type ThreadProviderFacts, type ThreadProviderOffers,
} from "./thread-opinion";
import { switchThreadTo } from "./thread-pane";
import type { useCollaborations } from "./use-collaborations";

type Collaborations = ReturnType<typeof useCollaborations>;

/**
 * The new thread's launch when a thread continues on another account of its
 * own provider (protocol 60): Rust only prepared the folder and the handoff
 * prompt, so the session itself must name the account (`LaunchOptions.account`).
 */
export function accountContinuationForm(input: {
  engine: string; account: string; cwd: string; prompt: string; model: string; name: string;
  preset?: ModelPresetSettings;
}): ComposerLaunchForm {
  const { engine, preset } = input;
  return {
    engine, cwd: input.cwd, prompt: input.prompt, model: input.model, kind: "code",
    account: input.account, name: input.name,
    effort: preset?.effort ?? "",
    permissionMode: preset?.permissionMode
      ?? startingPermissionMode(engine, ENGINE_CHOICES[engine] ?? UNKNOWN_ENGINE_CHOICES),
    ...COMPOSER_SCOPE_DEFAULTS, ...COMPOSER_TEAM_DEFAULTS,
  };
}

/** What the Threads bridges read from App: its state, its refs and its own verbs. */
export interface ThreadBridgesOptions extends Pick<Collaborations, "collaborations" | "noteCollaboration"
  | "approveCollaboration" | "denyCollaboration" | "acceptCollaborationPeer"
  | "discardCollaborationPeer" | "cancelCollaboration"> {
  state: Pick<AppState, "engineHealth">;
  threads: { readonly view: ThreadsView | null };
  threadRequest: (command: ThreadCommand) => Promise<ThreadControlResponse>;
  agentStore: AgentControlResponse;
  launchingRef: RefObject<boolean>;
  sessionByKeyRef: RefObject<ReadonlyMap<SessionKey, SessionSummary>>;
  closedKeys: RefObject<Set<SessionKey>>;
  terminalCenter: RefObject<IntegratedTerminalCenterHandle | null>;
  setActionError: Dispatch<SetStateAction<string | null>>;
  failAction: (err: unknown) => void;
  refreshAll: (kind: "explicit") => Promise<void>;
  showTerminals: () => void;
  showSurface: (kind: SessionKind) => void;
  selectSession: (key: SessionKey) => void;
  selectAgent: (agentId: string) => void;
  markLive: (key: SessionKey) => void;
  /**
   * A switched thread runs on `to` now instead of `from`: its pane, composer
   * draft and attachments follow it.
   */
  carryThread: (from: SessionKey, to: SessionKey, launched: string | undefined,
    switchedAt: number) => void;
  startAgentTask: (command: Extract<AgentRequest, { kind: "startAgentTask" }>) => Promise<{ key: SessionKey }>;
  /* Continue with another account of the thread's provider (protocol 60):
     a new thread on that account, started like a New thread card's. */
  engineLabels: Readonly<Record<string, string>>;
  modelPresets: readonly ModelPreset[];
  choicesFor: (engine: string) => EngineChoices;
  /** The provider accounts, for naming another account's continuation. */
  accountsRef: RefObject<ProviderAccountsView | null>;
  launch: (form: ComposerLaunchForm, scope: ScopeOptions, contextItems?: readonly PromptContextItem[],
    launchOptions?: { background?: boolean; threadCard?: boolean; extra?: boolean },
  ) => Promise<{ key: SessionKey; prompt: string } | null>;
}

/**
 * The Threads bridges (open in terminal, continue as thread, hand to a
 * teammate) and the second-opinion block (the offers, Switch to, the
 * opinion cards and their buttons), moved out of App as they were. Every
 * returned callback keeps App's dependency array, so it is as stable as it
 * was there: `threadBridges` goes into memo(Sidebar) and `continueAsThread`
 * into memo(IntegratedTerminalCenter).
 */
export function useThreadBridges({
  state, threads, threadRequest, agentStore,
  launchingRef, sessionByKeyRef, closedKeys, terminalCenter,
  setActionError, failAction, refreshAll, showTerminals, showSurface, selectSession, selectAgent,
  markLive, carryThread, startAgentTask, engineLabels, modelPresets, choicesFor, accountsRef, launch,
  collaborations, noteCollaboration, approveCollaboration, denyCollaboration,
  acceptCollaborationPeer, discardCollaborationPeer, cancelCollaboration,
}: ThreadBridgesOptions) {
  /* ---- Threads ↔ Code ↔ Agent (review C1) ----------------------------------
     Rust keeps one writer per provider session and refuses a second; these
     ask it to move a session and then show where it went. */

  /** Open in terminal: Rust closes the thread's session and holds it for a new Code pane. */
  const openThreadInTerminal = useCallback((key: SessionKey) => {
    if (launchingRef.current) return;
    setActionError(null);
    const title = sessionByKeyRef.current.get(key)?.title;
    void threadRequest({ kind: "openInTerminal", key }).then(response => {
      const handoff = response.terminal;
      if (handoff === undefined) throw new Error("Interface did not say which Code pane to open.");
      showTerminals();
      terminalCenter.current?.openResumedSession({
        cwd: handoff.cwd, profileId: handoff.profileId, providerSessionId: handoff.providerSessionId,
        ...(title === undefined || title.trim() === "" ? {} : { title }),
      });
    }).catch(failAction);
  }, [failAction, showTerminals, threadRequest]);

  /**
   * Continue as thread, once the Code surface has ended the pane: list the
   * session, let Rust take it into Threads (it waits for the pane to exit),
   * then open it there.
   */
  const continueAsThread = useCallback((key: SessionKey) => {
    setActionError(null);
    void (async () => {
      await refreshAll("explicit");
      await threadRequest({ kind: "continueFromTerminal", key });
      if (!sessionByKeyRef.current.has(key)) {
        throw new Error("The session is in Threads, but this list has not caught up yet. Refresh and open it there.");
      }
      showSurface("chat");
      selectSession(key);
    })().catch(failAction);
  }, [failAction, refreshAll, selectSession, showSurface, threadRequest]);

  /** Hand to teammate: the teammate's own gated task, about this thread. The thread is left as it is. */
  const handThreadToTeammate = useCallback((key: SessionKey, agentId: string) => {
    setActionError(null);
    const agent = agentStore.agents.find(candidate => candidate.id === agentId);
    const session = sessionByKeyRef.current.get(key);
    if (agent === undefined || session === undefined) {
      failAction(new Error("That teammate or thread is no longer here."));
      return;
    }
    const record = threads.view?.threads.find(thread => thread.key === key);
    const projectRoot = threadProjectRoot(session, record);
    const branch = record?.workspace?.branch?.trim() ?? "";
    const text = threadHandoffTask({
      title: session.title, key, projectRoot, sessionRoot: session.cwd,
      ...(branch === "" ? {} : { branch }),
    });
    void startAgentTask(agentTaskCommand(agent, { engine: agent.engine, cwd: session.cwd },
      session.title, text, crypto.randomUUID()))
      .then(() => { showSurface("agent"); selectAgent(agent.id); })
      .catch(failAction);
  }, [agentStore, failAction, selectAgent, showSurface, startAgentTask, threads.view]);

  /* ---- Second opinion (review B4) and Switch to (protocol 59) -------------
     Rust refuses what it must (the thread's own provider, a running turn, a
     session a pane holds) and words what is sent; these say where it went. */

  /** What the probes say about each provider, for the two offers. */
  const providerFacts = useMemo<readonly ThreadProviderFacts[]>(() => state.engineHealth.map(probe => ({
    engine: probe.engine,
    ...(probe.info?.displayName === undefined ? {} : { label: probe.info.displayName }),
    ready: probe.ok && probe.info?.auth !== "signedOut",
    consult: probe.info?.capabilities.consultationSafety?.available === true,
  })), [state.engineHealth]);
  const threadOffersFor = useCallback((engine: string, project: boolean): ThreadProviderOffers =>
    threadProviderOffers(engine, providerFacts, project), [providerFacts]);

  /** Second opinion: Rust submits the consult; its card waits for the owner's Ask. */
  const askSecondOpinion = useCallback((key: SessionKey, engine: CollaborationEngine) => {
    setActionError(null);
    void threadRequest({ kind: "secondOpinion", key, engine,
      requestId: `opinion-${crypto.randomUUID()}` }).then(response => {
      // Rust opened the session to read it, as a send would have.
      closedKeys.current.delete(key);
      markLive(key);
      if (response.consult !== undefined) noteCollaboration(response.consult);
      // The card waits in the thread's own pane: show it there.
      selectSession(key);
    }).catch(failAction);
  }, [failAction, markLive, noteCollaboration, selectSession, threadRequest]);

  /**
   * Switch to (protocol 59): Rust carries the same thread on to `engine` (a
   * new session in its folder, or that provider's earlier one) and answers
   * the new head; the pane, its draft and its attachments follow the thread
   * there. A handoff that may not have arrived is said, never sent again.
   */
  const switchingRef = useRef(false);
  const switchThreadProvider = useCallback((key: SessionKey, engine: string) => {
    if (switchingRef.current) return;
    switchingRef.current = true;
    setActionError(null);
    void switchThreadTo({
      request: threadRequest,
      carry: carryThread,
      now: Date.now,
      select: next => {
        closedKeys.current.delete(next);
        markLive(next);
        showSurface("chat");
        selectSession(next);
      },
    }, key, engine as EngineId).then(({ notice }) => {
      if (notice !== null) setActionError(notice);
    }).catch(failAction).finally(() => { switchingRef.current = false; });
  }, [carryThread, failAction, markLive, selectSession, showSurface, threadRequest]);

  /**
   * Continue with another account of the thread's own provider (protocol 60):
   * Rust prepares the thread's folder and the first prompt; the new thread
   * starts on that account like a New thread card's and binds with the token.
   */
  const continuingRef = useRef(false);
  const continueThreadOnAccount = useCallback((key: SessionKey, engine: string, account: string) => {
    if (continuingRef.current) return;
    const parent = sessionByKeyRef.current.get(key);
    const providerName = engineLabels[engine] ?? providerLabel(engine);
    // The account is named with its provider: "Claude Code (Work)".
    const accountLabel = accountsRef.current?.providers.find(group => group.engine === engine)
      ?.accounts.find(candidate => candidate.id === account)?.label;
    const label = accountLabel === undefined ? providerName : accountDisplayLabel(providerName, accountLabel);
    const model = latestModelPresetFor(modelPresets, engine)?.model
      ?? choicesFor(engine).models[0];
    if (parent === undefined || model === undefined || model === "") {
      failAction(new Error(parent === undefined ? "That thread is no longer listed."
        : `${label} has no model to start with yet. Pick one in a New thread first.`));
      return;
    }
    continuingRef.current = true;
    setActionError(null);
    void (async () => {
      const response = await threadRequest({ kind: "continueWith", key, engine: engine as EngineId, account });
      const prepared = response.prepared;
      const prompt = response.prompt;
      if (prepared === undefined || prompt === undefined) {
        throw new Error("Interface did not prepare the new thread.");
      }
      const preset = modelPresetFor(modelPresets, engine, model);
      let launched: { key: SessionKey; prompt: string } | null = null;
      try {
        launched = await launch(accountContinuationForm({ engine, account, cwd: prepared.cwd, prompt,
          model, name: continuationName(parent.title, label), preset }),
        composerScopeOptions(COMPOSER_SCOPE_DEFAULTS), [], { extra: true });
      } finally {
        if (launched === null) {
          await threadRequest({ kind: "discardWorkspace", token: prepared.token }).catch(() => undefined);
        }
      }
      if (launched === null) throw new Error(`The ${label} thread did not start.`);
      await threadRequest({ kind: "bind", key: launched.key, token: prepared.token,
        prompt: launched.prompt, titleSource: "prompt" }).catch((err: unknown) => {
        console.warn("[threads] the continued thread's record was not saved:", err);
      });
      showSurface("chat");
      selectSession(launched.key);
    })().catch(failAction).finally(() => { continuingRef.current = false; });
  }, [accountsRef, choicesFor, engineLabels, failAction, launch, modelPresets, selectSession,
    showSurface, threadRequest]);

  /** A second opinion card's buttons: the collaboration channels' own verdicts. */
  const [opinionPending, setOpinionPending] = useState<readonly string[]>([]);
  const [dismissedOpinions, setDismissedOpinions] = useState<ReadonlySet<string>>(() => new Set());
  const opinionAction = useCallback((_key: SessionKey, card: ThreadOpinionCard,
      action: ThreadOpinionAction) => {
    if (action === "dismiss") {
      setDismissedOpinions(current => new Set([...current, card.id]));
      return;
    }
    const { approval, peer } = card.status;
    const run = (): Promise<void> | null => {
      switch (action) {
        case "ask": case "decline": {
          if (approval === undefined) return null;
          const verdict = { collaborationId: approval.collaborationId, approvalId: approval.approvalId,
            expectedRevision: approval.expectedRevision };
          return action === "ask" ? approveCollaboration(verdict) : denyCollaboration(verdict);
        }
        case "send": case "discard": {
          if (peer === undefined) return null;
          const verdict = { collaborationId: peer.collaborationId, resultId: peer.resultId,
            resultDigest: peer.resultDigest, expectedRevision: peer.expectedRevision };
          return action === "send" ? acceptCollaborationPeer(verdict) : discardCollaborationPeer(verdict);
        }
        case "cancel": return cancelCollaboration(card.id);
      }
    };
    const started = run();
    if (started === null) return;
    setOpinionPending(current => [...current, card.id]);
    setActionError(null);
    void started.catch(failAction).finally(() => {
      setOpinionPending(current => current.filter(id => id !== card.id));
    });
  }, [acceptCollaborationPeer, approveCollaboration, cancelCollaboration, denyCollaboration,
    discardCollaborationPeer, failAction]);

  /** Each thread's second opinions, one stable array per thread while nothing changed. */
  const opinionsRef = useRef<Readonly<Record<SessionKey, readonly ThreadOpinionCard[]>>>({});
  const threadOpinions = useMemo(() => {
    const now = Date.now();
    const next: Record<SessionKey, readonly ThreadOpinionCard[]> = {};
    for (const key of new Set(collaborations.map(status => status.sourceSessionKey))) {
      const cards = threadOpinionCards(collaborations, key, dismissedOpinions, now);
      if (cards.length === 0) continue;
      const held = opinionsRef.current[key];
      next[key] = held !== undefined && held.length === cards.length
        && held.every((card, index) => card.status === cards[index]?.status
          && card.stage === cards[index]?.stage) ? held : cards;
    }
    opinionsRef.current = next;
    return next;
  }, [collaborations, dismissedOpinions]);

  const threadTeammates = useMemo<readonly ThreadTeammate[]>(() =>
    agentStore.agents.map(agent => ({ id: agent.id, name: agent.name })), [agentStore]);
  const threadBridges = useMemo(() => ({
    onOpenInTerminal: openThreadInTerminal,
    teammates: threadTeammates,
    onHandToTeammate: handThreadToTeammate,
    offersFor: threadOffersFor,
    onSecondOpinion: askSecondOpinion,
    onSwitchTo: switchThreadProvider,
  }), [askSecondOpinion, switchThreadProvider, handThreadToTeammate, openThreadInTerminal,
    threadOffersFor, threadTeammates]);

  return {
    openThreadInTerminal, continueAsThread, providerFacts, threadOffersFor, askSecondOpinion,
    switchThreadProvider, continueThreadOnAccount, opinionPending, opinionAction, threadOpinions,
    threadBridges,
  };
}
