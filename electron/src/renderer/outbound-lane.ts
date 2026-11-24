import type { PromptRecoveryInput, RecoverySource } from "../shared/recovery";
import type { EngineEvent, SessionKey } from "../shared/engine";
import { parseProviderCommandLike } from "../shared/provider-commands";
import {
  EFFECT_DEFINITELY_REJECTED_CODE, EFFECT_OUTCOME_UNKNOWN_CODE,
} from "../shared/ipc-error";
import { withoutKey } from "./helpers";
import { ipcErrorShape, ipcMessage } from "./ipc-message";
import { productLabel } from "./product";
import { serializePromptWithContext, type PromptContextItem } from "./prompt-context";
import {
  captureOutboundSendEvent, enqueueSteeringPrompt,
  markSteeringPromptInflight, pauseSteeringQueue,
  releaseSteeringPrompt, removeSteeringPrompt, serializeSteeringQueueMap,
  outboundSettledDuringSend, steeringQueueCanAutoDeliver, steeringQueueEventEndsTurn,
  steeringQueueEventStartsTurn, steeringQueueTerminalFailure,
  type SteeringQueueItem,
} from "./steering-queue";

/** The adapters' own words for an effect they cannot account for. */
function uncertaintyPhrase(err: unknown): boolean {
  const message = ipcMessage(err).toLowerCase();
  return message.includes("did not confirm whether")
    || message.includes("delivery outcome unknown");
}

/**
 * Whether a rejected transmission (`iface.send`) may have reached the provider.
 *
 * Adapters own this distinction because only they know whether a provider
 * frame was written, and the host carries their answer as its typed error
 * (shared/ipc-error.ts). Only a proof counts as definite: a replay-safe error
 * (nothing left) or `effect-definitely-rejected` (the provider refused it).
 * Every other host error is unknown — `effect-outcome-unknown`, a registry
 * update lost after a confirmed send, `host-exited`, `host-stopped`,
 * `host-protocol` — and so is a rejection without the typed shape, since an
 * older or unrecognized error cannot show the prompt stayed here.
 */
export function transmissionOutcomeUnknown(err: unknown): boolean {
  const shape = ipcErrorShape(err);
  if (shape.kind !== "typed") return true;
  if (shape.code === EFFECT_OUTCOME_UNKNOWN_CODE || uncertaintyPhrase(err)) return true;
  return !shape.replaySafe && shape.code !== EFFECT_DEFINITELY_REJECTED_CODE;
}

/**
 * A rejection of the transmission call itself, the one step after which the
 * provider may hold the prompt. Its message is the rejection's own, so
 * `ipcMessage` reads it exactly as it would the original.
 */
class TransmissionFailure extends Error {
  readonly outcomeUnknown: boolean;

  constructor(cause: unknown) {
    super(cause instanceof Error ? cause.message : String(cause), { cause });
    this.name = "TransmissionFailure";
    this.outcomeUnknown = transmissionOutcomeUnknown(cause);
  }
}

/**
 * Whether a failed outbound attempt may have reached the provider.
 *
 * Only the transmission can: a failure before it (settings pending, a
 * cancelled or superseded attempt, context validation, the access-mode
 * restore, the queue's durable in-flight mark) never let the prompt leave, so
 * it stays definite unless its words are an adapter's uncertainty phrase. A
 * renderer-wide "timeout means maybe sent" rule would turn a failed
 * connect/initialize into false acceptance and consume the user's context.
 */
export function deliveryOutcomeUnknown(err: unknown): boolean {
  if (err instanceof TransmissionFailure) return err.outcomeUnknown;
  return uncertaintyPhrase(err);
}

/**
 * Everything the lane needs from outside it. App is the only implementation:
 * provider effects go through `window.iface`, the rest reads App's refs.
 */
export interface OutboundLanePort {
  /** Hand a serialized prompt or provider command to the session. */
  send(key: SessionKey, prompt: string, recovery?: PromptRecoveryInput): Promise<unknown>;
  interrupt(key: SessionKey): Promise<unknown>;
  /** Reauthorize captured context at the main-process boundary. */
  validateContext(key: SessionKey, items: readonly PromptContextItem[]):
    Promise<PromptContextItem[]>;
  /**
   * After a restart the picker can show a remembered access mode before a
   * handle exists; apply it before the first prompt. Undefined when there is
   * nothing to apply.
   */
  restoreAccessMode(key: SessionKey): Promise<void> | undefined;
  isLive(key: SessionKey): boolean;
  /** Bumped each time the session's provider handle goes away. */
  lifecycle(key: SessionKey): number;
  /** A model/effort/fast/access change is still waiting for its acknowledgement. */
  configPending(key: SessionKey): boolean;
  /** Write the serialized queue map durably; false when storage refused it. */
  persist(serialized: string): boolean;
  /** Reassert the session's current draft to collaboration after a send boundary. */
  publishDraft(key: SessionKey): void;
  /** Take the turn's safety-net checkpoint for a queued prompt. */
  checkpoint(key: SessionKey): void;
  reportError(message: string): void;
  /** A paused chat may have reattached inside send; publish its liveness. */
  markDelivered(key: SessionKey): void;
  /** Give a definitely unsent prompt's captured context back to its composer. */
  restoreContext(key: SessionKey, items: readonly PromptContextItem[]): void;
  queueItemId(): string;
  now(): number;
}

export interface OutboundLaneSnapshot {
  /** Captured attachments remain pinned while an asynchronous send/launch owns them. */
  readonly retainedContext: readonly PromptContextItem[];
  readonly queues: Readonly<Record<SessionKey, SteeringQueueItem[]>>;
  /** The queued row each session is delivering right now. */
  readonly sending: Readonly<Record<SessionKey, string>>;
  /**
   * Two banner channels, one slot. `errors` is delivery-BLOCKING (a paused or
   * failed queue) and feeds the auto-delivery `hasError` gate. `notices` is
   * ADVISORY (a rejected add, a kept draft) and must NOT block the prompts
   * already accepted — refusing to enqueue a 21st prompt cannot be allowed to
   * strand the 20 that are waiting. The tile renders the blocking error first.
   */
  readonly errors: Readonly<Record<SessionKey, string>>;
  readonly notices: Readonly<Record<SessionKey, string>>;
  /** Moves whenever a send or queue gate opens or closes. */
  readonly wake: number;
}

type QueueMap = Record<SessionKey, SteeringQueueItem[]>;

/**
 * The single outbound-prompt lane. Every composer send, provider command and
 * queued steering prompt goes through `sendWithContext`, which keeps a
 * definitely-unsent failure apart from an unknown outcome: an unknown never
 * resends, never restores captured context and never marks an echo
 * undelivered, and a queued row is marked in flight durably BEFORE its bytes
 * leave.
 *
 * Plain class, no React: App wraps it in `useOutboundLane`, forwards engine
 * events to `observe`, and runs `autoDeliver` after each render.
 */
export class OutboundLane {
  private readonly contextCaptures = new Map<symbol, readonly PromptContextItem[]>();
  private retainedContext: readonly PromptContextItem[] = [];
  private queues: QueueMap;
  private sending: Record<SessionKey, string> = {};
  private errors: Record<SessionKey, string> = {};
  private notices: Record<SessionKey, string> = {};
  private wake = 0;
  /** The queue map in memory is ahead of storage: a write failed on a path that had to move on. */
  private unsaved = false;
  private published: OutboundLaneSnapshot;
  private readonly listeners = new Set<() => void>();

  private readonly queueDispatching = new Set<SessionKey>();
  private readonly queueWaitingForTurn = new Set<SessionKey>();
  private readonly queueTurnStarted = new Set<SessionKey>();
  /** Queued rows open in a pane's editor; automatic delivery holds at them. */
  private readonly queueEditing = new Map<SessionKey, Set<string>>();
  // Covers validation + IPC acceptance for every outbound prompt, whether it
  // came from the composer or the queue. The second two maps bridge the narrow
  // gap between IPC acceptance and the provider's first working event, so a
  // double submit cannot overtake validation or slip in before status catches up.
  // Attempt ids, rather than booleans: an older async continuation must never
  // delete the locks of a newer send admitted after Stop.
  private readonly outboundSending = new Map<SessionKey, number>();
  private readonly outboundWaitingForTurn = new Map<SessionKey, number>();
  private readonly outboundTurnStarted = new Map<SessionKey, number>();
  private readonly outboundGeneration = new Map<SessionKey, number>();
  // Some adapters truthfully emit working + terminal from inside send() before
  // its promise resolves. Record that sequence by attempt so acceptance cannot
  // arm a latch after the terminal has already passed.
  private readonly outboundEventsDuringSend = new Map<SessionKey, {
    attempt: number; events: EngineEvent[];
  }>();

  constructor(private readonly port: OutboundLanePort, initialQueues: QueueMap) {
    this.queues = initialQueues;
    this.published = this.build();
  }

  snapshot = (): OutboundLaneSnapshot => this.published;

  captureContext(items: readonly PromptContextItem[]): () => void {
    const token = Symbol("outbound context");
    this.contextCaptures.set(token, items.map(item => ({ ...item })));
    this.retainedContext = [...this.contextCaptures.values()].flat();
    this.bump(); this.publish();
    return () => {
      if (!this.contextCaptures.delete(token)) return;
      this.retainedContext = [...this.contextCaptures.values()].flat();
      this.bump(); this.publish();
    };
  }

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  };

  /** Validation or IPC acceptance of a prompt is in progress. */
  isSending(key: SessionKey): boolean {
    return this.outboundSending.has(key);
  }

  /** In progress, or accepted and waiting for the provider's turn to end. */
  outboundPending(key: SessionKey): boolean {
    return this.outboundSending.has(key) || this.outboundWaitingForTurn.has(key);
  }

  private build(): OutboundLaneSnapshot {
    return { retainedContext: this.retainedContext, queues: this.queues, sending: this.sending, errors: this.errors,
      notices: this.notices, wake: this.wake };
  }

  private publish(): void {
    const current = this.published;
    if (current.queues === this.queues && current.sending === this.sending
        && current.errors === this.errors && current.notices === this.notices
        && current.wake === this.wake) return;
    this.published = this.build();
    for (const listener of this.listeners) listener();
  }

  private bump(): void {
    this.wake += 1;
  }

  private setError(key: SessionKey, message: string): void {
    this.errors = { ...this.errors, [key]: message };
  }

  private setNotice(key: SessionKey, message: string): void {
    this.notices = { ...this.notices, [key]: message };
  }

  private nextGeneration(key: SessionKey): number {
    const next = (this.outboundGeneration.get(key) ?? 0) + 1;
    this.outboundGeneration.set(key, next);
    return next;
  }

  /* ---- durable queue storage --------------------------------------------- */

  /** Write the whole map, then adopt it. A refused write changes nothing. */
  private commit(next: QueueMap): boolean {
    if (!this.port.persist(serializeSteeringQueueMap(next))) return false;
    this.queues = next;
    this.unsaved = false;
    return true;
  }

  /**
   * Adopt a map storage refused. Storage keeps its previous copy until the
   * retry after the render that shows this one (`retryUnsavedQueues`) or the
   * next accepted change, both of which write the whole map.
   */
  private adoptUnsaved(next: QueueMap): void {
    this.queues = next;
    this.unsaved = true;
  }

  /** One more write of a map storage refused, run after the render that shows it. */
  retryUnsavedQueues(): void {
    if (!this.unsaved) return;
    if (this.port.persist(serializeSteeringQueueMap(this.queues))) this.unsaved = false;
  }

  private updateQueue(key: SessionKey,
      update: (items: readonly SteeringQueueItem[]) => SteeringQueueItem[]): boolean {
    const nextItems = update(this.queues[key] ?? []);
    const next = nextItems.length === 0
      ? withoutKey(this.queues, key)
      : { ...this.queues, [key]: nextItems };
    return this.commit(next);
  }

  /** A queue edit from the pane: move, edit, remove or clear. */
  applyQueueUpdate(key: SessionKey,
      update: (items: readonly SteeringQueueItem[]) => SteeringQueueItem[]): boolean {
    const saved = this.updateQueue(key, update);
    this.errors = saved
      ? withoutKey(this.errors, key)
      : { ...this.errors, [key]: "This queue change could not be saved. No queued prompts were changed." };
    // A successful move/edit/remove/clear can make an advisory (e.g. "queue
    // full") stale — dropping a row frees a slot — so clear it alongside.
    if (saved) this.notices = withoutKey(this.notices, key);
    this.publish();
    return saved;
  }

  /**
   * A pane opened (`editing`) or closed a queued row's editor. While it is
   * open, automatic delivery holds at that row; closing it wakes the queue so
   * delivery goes on with whatever the owner saved.
   */
  setQueueEditing(key: SessionKey, id: string, editing: boolean): void {
    const ids = this.queueEditing.get(key) ?? new Set<string>();
    if (ids.has(id) === editing) return;
    if (editing) ids.add(id); else ids.delete(id);
    if (ids.size === 0) this.queueEditing.delete(key); else this.queueEditing.set(key, ids);
    this.bump();
    this.publish();
  }

  /* ---- lifecycle ---------------------------------------------------------- */

  /**
   * Durably pause every armed row of a session's queue and explain why. The
   * `session.closed` / failed-turn path arms it too: a provider that dies on its
   * own must leave the queue as guarded as a user's Stop does, never armed to
   * auto-deliver stale prompts into the session that gets reattached later.
   * `pauseSteeringQueue` leaves in-flight rows alone.
   */
  pause(key: SessionKey, message: string): void {
    this.nextGeneration(key);
    // Keep an in-flight validation/send attempt's own lock until its finally
    // settles, but prevent the current or queued turn from starting again.
    this.outboundWaitingForTurn.delete(key);
    this.outboundTurnStarted.delete(key);
    this.queueWaitingForTurn.delete(key);
    this.queueTurnStarted.delete(key);
    const queue = this.queues[key] ?? [];
    if (queue.length > 0) {
      const next = { ...this.queues, [key]: pauseSteeringQueue(queue) };
      const persisted = this.commit(next);
      if (!persisted) this.adoptUnsaved(next);
      this.setError(key, persisted
        ? message
        : `${message} The pause could not be saved; review it before restarting ${productLabel()}.`);
    }
    this.bump();
    this.publish();
  }

  /** The lane's view of one engine event, in the order App receives them. */
  observe(key: SessionKey, evt: EngineEvent): void {
    const startsOutboundTurn = steeringQueueEventStartsTurn(evt);
    const bufferedOutbound = this.outboundEventsDuringSend.get(key);
    captureOutboundSendEvent(bufferedOutbound, this.outboundSending.get(key), evt);
    if (startsOutboundTurn && this.queueWaitingForTurn.has(key)) {
      this.queueTurnStarted.add(key);
    }
    if (startsOutboundTurn) {
      const attempt = this.outboundWaitingForTurn.get(key);
      if (attempt !== undefined) this.outboundTurnStarted.set(key, attempt);
    }
    if (steeringQueueEventEndsTurn(evt)) {
      const maySettle = evt.kind === "session.closed" || this.queueTurnStarted.has(key);
      if (maySettle && this.queueWaitingForTurn.delete(key)) {
        this.queueTurnStarted.delete(key);
        this.bump();
      }
      const maySettleOutbound = evt.kind === "session.closed"
        || this.outboundTurnStarted.get(key) === this.outboundWaitingForTurn.get(key);
      if (maySettleOutbound && this.outboundWaitingForTurn.has(key)) {
        const attempt = this.outboundWaitingForTurn.get(key);
        this.outboundWaitingForTurn.delete(key);
        if (this.outboundTurnStarted.get(key) === attempt) {
          this.outboundTurnStarted.delete(key);
        }
        // `outboundSending` belongs to the async IPC call and is released by
        // that attempt's finally block. A terminal provider event can arrive
        // before the IPC response; clearing it here would admit an overlap.
        this.bump();
      }
    }
    // A turn that ends BADLY — the session dying on its own, a failed turn, or
    // a non-retryable error — must leave the queue as durably paused as Stop
    // does, or the next reattached idle would auto-deliver prompts written for
    // a state the session no longer has, with no chip or banner saying why. A
    // clean idle is left untouched so the queue keeps draining. User-initiated
    // Stop/Close already paused the rows before this event, so only ARMED rows
    // are paused here; that also keeps the deliberate close's own message.
    if (steeringQueueTerminalFailure(evt)
        && (this.queues[key] ?? []).some(item => item.deliveryState === undefined)) {
      this.pause(key, evt.kind === "session.closed"
        ? "Queue paused because the provider session ended. Use Send now on each prompt to resume it."
        : "Queue paused because the turn failed. Resolve the error, then use Send now on each prompt to resume it.");
    }
    this.publish();
  }

  /** Forget a deleted session: its queue, banners and every lock it held. */
  drop(key: SessionKey): void {
    const next = withoutKey(this.queues, key);
    if (next !== this.queues && !this.commit(next)) this.adoptUnsaved(next);
    this.errors = withoutKey(this.errors, key);
    this.notices = withoutKey(this.notices, key);
    this.sending = withoutKey(this.sending, key);
    this.queueDispatching.delete(key);
    this.queueWaitingForTurn.delete(key);
    this.queueTurnStarted.delete(key);
    this.queueEditing.delete(key);
    this.outboundSending.delete(key);
    this.outboundWaitingForTurn.delete(key);
    this.outboundTurnStarted.delete(key);
    this.nextGeneration(key);
    this.publish();
  }

  /* ---- sending ------------------------------------------------------------ */

  /**
   * The one outbound path. `beforeSend` is the caller's last cancellation
   * check, run after validation and immediately before transmission;
   * `afterAccepted` runs once the provider has the prompt.
   */
  async sendWithContext(key: SessionKey, text: string,
      context: readonly PromptContextItem[], beforeSend?: () => void,
      afterAccepted?: (alreadySettled: boolean) => void, recoverySource?: RecoverySource): Promise<void> {
    if (this.port.configPending(key)) {
      throw new Error("Wait for the session settings change to finish before sending.");
    }
    if (this.outboundSending.has(key) || this.outboundWaitingForTurn.has(key)) {
      throw new Error("Another prompt is already being delivered. Queue this follow-up instead.");
    }
    const attempt = this.nextGeneration(key);
    const releaseContext = this.captureContext(context);
    let releaseValidated = (): void => {};
    this.outboundSending.set(key, attempt);
    this.outboundTurnStarted.delete(key);
    const generation = attempt;
    // A session key survives Resume, but a provider handle does not. Validation
    // started under one handle must never send into a replacement with the same
    // key after session.closed.
    const lifecycle = this.port.lifecycle(key);
    this.bump();
    this.publish();
    try {
      const validatedContext = await this.port.validateContext(key, context);
      releaseValidated = this.captureContext(validatedContext);
      if ((this.outboundGeneration.get(key) ?? 0) !== generation) {
        throw new Error("Prompt delivery was cancelled before it started.");
      }
      if (this.port.lifecycle(key) !== lifecycle) {
        throw new Error("That session closed while prompt context was being checked. "
          + "Nothing was sent; review the prompt and send it again after Resume.");
      }
      // A setting can be changed while context validation is in flight. Check
      // again at the last pre-transmission boundary; provider policy and the
      // optimistic UI must agree before any prompt bytes leave Interface.
      if (this.port.configPending(key)) {
        throw new Error("Wait for the session settings change to finish before sending.");
      }
      // The caller's final cancellation/reorder check belongs after the async
      // validation and immediately before transmission. A cleared queue item
      // therefore cannot be resurrected by an older validation promise.
      beforeSend?.();
      const serializedPrompt = serializePromptWithContext(text, validatedContext);
      // Arm event capture only at the transmission boundary. While context is
      // being validated, any working→idle pair can still belong to the old
      // provider turn and must not settle this attempt.
      this.outboundEventsDuringSend.set(key, { attempt, events: [] });
      const restoring = this.port.restoreAccessMode(key);
      if (restoring !== undefined) await restoring;
      try {
        await this.port.send(key, serializedPrompt, parseProviderCommandLike(text) !== null ? undefined : {
          text, context: validatedContext.map(({ previewUrl: _preview, ...item }) => item),
          ...(recoverySource === undefined ? {} : { source: recoverySource }),
        });
      } catch (err: unknown) {
        throw new TransmissionFailure(err);
      }
      // `send` acceptance can itself be asynchronous (Codex may still be
      // connecting/starting a turn). If Stop raced it, interrupt again now that
      // the provider has a turn id instead of letting work begin after Stop.
      if ((this.outboundGeneration.get(key) ?? 0) !== generation) {
        try {
          await this.port.interrupt(key);
          this.port.reportError(`The provider accepted the prompt as Stop was pressed, so ${productLabel()} sent a second interrupt after the turn became available. The prompt will not be sent again.`);
        } catch (err: unknown) {
          this.port.reportError(`The provider accepted the prompt as Stop was pressed, but the follow-up interrupt failed: ${ipcMessage(err)}. The prompt will not be sent again.`);
        }
        return;
      }
      const buffered = this.outboundEventsDuringSend.get(key);
      const alreadySettled = buffered?.attempt === attempt
        ? outboundSettledDuringSend(buffered.events) : false;
      // Do not expose a new attempt to unrelated old-turn terminals while its
      // IPC is pending, but do replay its own buffered working→terminal pair.
      if (!alreadySettled) this.outboundWaitingForTurn.set(key, attempt);
      // Acceptance is the first event that cannot belong to the previous turn.
      // Mark it started so a Claude result with no deltas (usage + terminal)
      // can release the latch on its terminal edge.
      if (!alreadySettled && this.outboundWaitingForTurn.get(key) === attempt) {
        this.outboundTurnStarted.set(key, attempt);
      }
      if (!alreadySettled && this.queueWaitingForTurn.has(key)) {
        this.queueTurnStarted.add(key);
      }
      afterAccepted?.(alreadySettled);
    } catch (err) {
      if (this.outboundWaitingForTurn.get(key) === attempt) {
        this.outboundWaitingForTurn.delete(key);
      }
      if (this.outboundTurnStarted.get(key) === attempt) {
        this.outboundTurnStarted.delete(key);
      }
      throw err;
    } finally {
      releaseValidated(); releaseContext();
      if (this.outboundEventsDuringSend.get(key)?.attempt === attempt) {
        this.outboundEventsDuringSend.delete(key);
      }
      if (this.outboundSending.get(key) === attempt) this.outboundSending.delete(key);
      // Main invalidates the last empty-draft assertion before every ordinary
      // send. Reassert the current renderer-owned draft after that boundary;
      // provider working/queue state still independently blocks collaboration
      // until the session is genuinely idle.
      this.port.publishDraft(key);
      this.bump();
      this.publish();
    }
  }

  /** The composer's send. `context` is the captured "next prompt" context. */
  submitPrompt(key: SessionKey, text: string,
      context: readonly PromptContextItem[]): Promise<void> {
    const releaseContext = this.captureContext(context);
    return this.sendWithContext(key, text, context).then(() => {
      // A paused chat may have reattached inside iface.send. Successful
      // delivery is enough to publish that fact without waiting for the
      // provider's first event.
      this.port.markDelivered(key);
    }).catch((err: unknown) => {
      if (deliveryOutcomeUnknown(err)) {
        this.port.reportError(`${ipcMessage(err)} Delivery outcome unknown; `
          + `${productLabel()} will not resend this prompt automatically.`);
        // Resolving is deliberate: the provider may have accepted the bytes,
        // so the echo must not say "Not delivered" and the captured context
        // must not invite a duplicate retry.
        return;
      }
      this.port.restoreContext(key, context);
      throw new Error(ipcMessage(err));
    }).finally(releaseContext);
  }

  /**
   * Provider commands are control input, not a prompt with files/diffs/
   * prior-chat context appended to it. The session key still routes through
   * the same serialized lane, so a command can never jump providers or
   * overtake another outbound request.
   */
  runCommand(key: SessionKey, text: string): Promise<void> {
    return this.sendWithContext(key, text, []).then(() => {
      this.port.markDelivered(key);
    }).catch((err: unknown) => {
      if (deliveryOutcomeUnknown(err)) {
        this.port.reportError(`${ipcMessage(err)} Command outcome unknown; `
          + `${productLabel()} will not run it again automatically.`);
        return;
      }
      throw new Error(ipcMessage(err));
    });
  }

  /* ---- the steering queue ------------------------------------------------- */

  enqueue(key: SessionKey, text: string, context: readonly PromptContextItem[]): boolean {
    // Every refusal here leaves the EXISTING queue untouched, so it is advisory,
    // not delivery-blocking: it goes to `notices` so the prompts already
    // accepted keep draining. Routing it through `errors` would latch the
    // `hasError` gate and silently strand a full, healthy queue.
    if (parseProviderCommandLike(text) !== null) {
      this.setNotice(key, "Provider commands cannot enter the steering queue. Wait for the active turn, then run the command directly.");
      this.publish();
      return false;
    }
    const current = this.queues[key] ?? [];
    const result = enqueueSteeringPrompt(current, {
      id: this.port.queueItemId(),
      text,
      ...(context.length === 0 ? {} : { context: [...context] }),
      createdAt: this.port.now(),
      ...(current.some(item => item.deliveryState === "paused")
        ? { deliveryState: "paused" as const } : {}),
    });
    if (result.outcome !== "added") {
      this.setNotice(key, result.outcome === "full"
        ? "The steering queue is full. Send or remove a queued prompt before adding another."
        : "This prompt could not be added to the steering queue.");
      this.publish();
      return false;
    }
    if (!this.commit({ ...this.queues, [key]: result.items })) {
      this.setNotice(key, "This prompt could not be saved to the durable queue. Your draft was kept.");
      this.publish();
      return false;
    }
    this.notices = withoutKey(this.notices, key);
    this.publish();
    return true;
  }

  deliverQueued(key: SessionKey, id: string): void {
    if (this.queueDispatching.has(key)) return;
    const item = this.queues[key]?.find(candidate => candidate.id === id);
    if (item === undefined) return;
    if (parseProviderCommandLike(item.text) !== null) {
      this.setError(key, "This queued row looks like a provider command. It was not sent; remove it and run the command directly.");
      this.publish();
      return;
    }
    if (!this.port.isLive(key)) {
      // Refuse WITHOUT touching the durable queue: the row stays exactly as it
      // was (paused rows stay paused), so nothing is lost and Send now can be
      // retried once the session is live again.
      this.setError(key, "Resume this session before sending its queued prompts. They stay queued until then.");
      this.publish();
      return;
    }
    this.queueDispatching.add(key);
    // A queued prompt is a user send like any other and gets the same net,
    // taken before delivery. It has no transcript echo by design, so there is
    // no row to hang an offer on; the snapshot is kept regardless.
    this.port.checkpoint(key);
    this.sending = { ...this.sending, [key]: id };
    this.errors = withoutKey(this.errors, key);
    // A row is leaving, so a stale "queue full" advisory no longer applies.
    this.notices = withoutKey(this.notices, key);
    this.publish();
    let markedInflight = false;
    this.sendWithContext(key, item.text, item.context ?? [], () => {
      // Validation may take long enough for the user to remove or clear this
      // row. Re-read authoritative queue state at the last pre-send moment.
      const currentQueue = this.queues[key] ?? [];
      if (!currentQueue.some(candidate => candidate.id === id)) {
        throw new Error("That queued prompt was removed before it was sent.");
      }
      const guarded = markSteeringPromptInflight(currentQueue, id);
      if (!this.commit({ ...this.queues, [key]: guarded })) {
        throw new Error("The queued prompt could not be marked durable before sending. Nothing was sent.");
      }
      markedInflight = true;
      this.queueTurnStarted.delete(key);
      this.publish();
    }, alreadySettled => {
      // Acceptance, not the pre-send status snapshot, owns this latch. This
      // cannot be cleared by the old turn's terminal while the IPC is pending.
      if (!alreadySettled) {
        this.queueWaitingForTurn.add(key);
        this.queueTurnStarted.add(key);
      }
    }).then(
      () => {
        const remaining = removeSteeringPrompt(this.queues[key] ?? [], id);
        const next = remaining.length === 0
          ? withoutKey(this.queues, key)
          : { ...this.queues, [key]: remaining };
        if (!this.commit(next)) {
          // Storage keeps the `inflight` marker until a retry lands; keeping
          // that durable marker is safer than ever resending a
          // provider-accepted prompt. Remove the row from the live UI and
          // surface the storage failure explicitly.
          this.adoptUnsaved(next);
          this.setError(key, "The provider accepted this prompt, but durable queue storage could not be updated. It will not be sent again; review any Delivery outcome unknown row after restart.");
        }
        this.publish();
      },
      (err: unknown) => {
        this.queueWaitingForTurn.delete(key);
        this.queueTurnStarted.delete(key);
        // Clear/remove is a cancellation, not an error on an item that no
        // longer exists. A genuine validation/provider refusal leaves the row
        // parked with retryable copy and blocks automatic spin.
        const currentQueue = this.queues[key] ?? [];
        if (currentQueue.some(candidate => candidate.id === id)) {
          if (!deliveryOutcomeUnknown(err)) {
            const released = markedInflight
              ? releaseSteeringPrompt(currentQueue, id)
              : currentQueue;
            // Every definite refusal is persistently parked. A volatile error
            // banner is not enough: validation can fail before the in-flight
            // marker too, and restart must not turn either case into auto-send.
            const retryable = released.map(candidate => candidate.id === id
              ? { ...candidate, deliveryState: "paused" as const } : candidate);
            const next = { ...this.queues, [key]: retryable };
            if (!this.commit(next)) {
              // Storage keeps its previous copy (`inflight` once marked, the
              // row as it was otherwise) until a retry lands; in memory the row
              // becomes retryable only with an explicit visible warning.
              this.adoptUnsaved(next);
              this.setError(key, `${ipcMessage(err)} The retry state could not be saved; review this queue before restarting.`);
              this.publish();
              return;
            }
          }
          this.setError(key, deliveryOutcomeUnknown(err)
            ? `${ipcMessage(err)} Delivery outcome unknown; ${productLabel()} will not retry this prompt automatically.`
            : `${ipcMessage(err)} The prompt is paused; use Send now to retry it.`);
        }
        this.publish();
      },
    ).finally(() => {
      this.queueDispatching.delete(key);
      this.sending = withoutKey(this.sending, key);
      this.bump();
      this.publish();
    });
  }

  /**
   * Do NOT release the durable pause before delivery is known possible. The
   * old code committed a resume (un-pause) to localStorage FIRST, then handed
   * off to deliverQueued, which refuses when the session is not live — the
   * common case right after a restart, exactly when the user follows the
   * "Use Send now to resume" banner. That left the row un-paused-but-
   * undelivered: the Stop guard was destroyed, nothing was sent, and the next
   * reattached idle would auto-deliver the prompt unattended. deliverQueued
   * marks the row in-flight only at the transmission boundary (past its own
   * liveness check) and re-pauses it on failure, so routing straight through
   * it keeps every refused row paused, visible, and recoverable across a
   * restart instead of silently released.
   */
  sendQueuedNow(key: SessionKey, id: string): void {
    this.deliverQueued(key, id);
  }

  /**
   * Default Cursor-style queue policy: after the provider reports the current
   * turn idle, submit exactly one item and wait for that turn's terminal edge
   * before considering the next. The latch prevents a still-idle registry row
   * from draining the whole queue in one render. `queues` and `errors` are the
   * rendered snapshot's.
   */
  autoDeliver(queues: Readonly<QueueMap>, errors: Readonly<Record<SessionKey, string>>,
      statusOf: (key: SessionKey) => string | undefined): void {
    for (const [key, queue] of Object.entries(queues)) {
      if (this.port.configPending(key)) continue;
      if (!steeringQueueCanAutoDeliver(queue.length, {
        dispatching: this.queueDispatching.has(key) || this.outboundSending.has(key),
        waitingForTurn: this.queueWaitingForTurn.has(key)
          || this.outboundWaitingForTurn.has(key),
        hasError: errors[key] !== undefined,
        live: this.port.isLive(key),
        status: statusOf(key),
        firstDeliveryState: queue[0]?.deliveryState,
        firstBeingEdited: queue[0] !== undefined
          && this.queueEditing.get(key)?.has(queue[0].id) === true,
      })) continue;
      this.deliverQueued(key, queue[0].id);
    }
  }
}
