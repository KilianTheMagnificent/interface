import type {
  IfaceApi, OrchestrationAgentStopRequest, OrchestrationSnapshotDto,
} from "../shared/ipc";
import type {
  OrchestrationAgentBindParentRequestV1, OrchestrationAgentCreateRequestV1,
  OrchestrationAgentRegisterSessionRequestV1, OrchestratorPolicyClearRequestV1,
  OrchestratorPolicySetRequestV1, WorkerAbandonRequestV1, WorkerAckRequestV1,
  WorkerIntegrateRequestV1, WorkerSubmitRequestV1,
} from "../shared/orchestration-control";
import { equalValue } from "./stable-values";

export type OrchestrationApi = Pick<IfaceApi, "getOrchestrationSnapshot"
  | "onOrchestrationEvent" | "createOrchestrationAgent" | "stopOrchestrationAgent"
  | "setOrchestratorPolicy" | "clearOrchestratorPolicy" | "bindOrchestrationParent"
  | "registerOrchestrationSession" | "submitCoordinatedWorker" | "integrateCoordinatedWorker"
  | "abandonCoordinatedTask" | "ackCoordinatedObligation">;

export interface OrchestrationState {
  readonly snapshot: OrchestrationSnapshotDto | null;
  /**
   * A count of accepted reads. The canvas re-reads coordination status and
   * the model catalogue on each accepted read, an equal one included (a window
   * focus re-probes engines and then rereads this), as it did when every
   * accepted read was a new object.
   */
  readonly reads: number;
  readonly error: string | null;
}

/**
 * The orchestration snapshot App's canvas, sidebar and worker sessions read,
 * and the canvas's nine intents. Every intent re-reads the snapshot once the
 * host has answered it.
 */
export class OrchestrationController {
  private request = 0;
  private state: OrchestrationState = { snapshot: null, reads: 0, error: null };
  private readonly listeners = new Set<() => void>();

  constructor(private readonly api: OrchestrationApi,
    private readonly errorMessage: (error: unknown) => string) {}

  snapshot = (): OrchestrationState => this.state;

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  };

  private publish(patch: Partial<OrchestrationState>): void {
    // A patch that changes nothing (the error already clear) publishes nothing.
    const keys = Object.keys(patch) as (keyof OrchestrationState)[];
    if (keys.every(key => Object.is(this.state[key], patch[key]))) return;
    this.state = { ...this.state, ...patch };
    for (const listener of this.listeners) listener();
  }

  /** Accept a read unless it is older than the held one for the same host run. */
  private accept(next: OrchestrationSnapshotDto): void {
    const current = this.state.snapshot;
    if (current === null || current.hostId !== next.hostId
        || current.runtimeEpoch !== next.runtimeEpoch || next.revision >= current.revision) {
      // An equal snapshot (every window focus rereads it) keeps the one held,
      // so the worker sessions, sidebar and canvas derived from it hold still.
      const accepted = current !== null && equalValue(current, next) ? current : next;
      this.publish({ snapshot: accepted, reads: this.state.reads + 1 });
    }
    this.publish({ error: null });
  }

  load = async (): Promise<void> => {
    const request = this.request + 1;
    this.request = request;
    try {
      const snapshot = await this.api.getOrchestrationSnapshot();
      if (this.request === request) this.accept(snapshot);
    } catch (error) {
      if (this.request === request) {
        if (this.state.snapshot !== null) {
          this.publish({ snapshot: null, reads: this.state.reads + 1 });
        }
        this.publish({ error: this.errorMessage(error) });
      }
    }
  };

  /** A failure App reports on the canvas itself (a team that could not start). */
  reportError = (message: string): void => {
    this.publish({ error: message });
  };

  /** Follow the host's change pushes; App's refresh makes the initial read. */
  connect = (): (() => void) => this.api.onOrchestrationEvent(event => {
    if (event.version === 1 && event.kind === "changed") void this.load();
  });

  createAgent = async (input: OrchestrationAgentCreateRequestV1): Promise<void> => {
    await this.api.createOrchestrationAgent(input);
    await this.load();
  };

  stopAgent = async (input: OrchestrationAgentStopRequest): Promise<void> => {
    await this.api.stopOrchestrationAgent(input);
    await this.load();
  };

  setPolicy = async (input: OrchestratorPolicySetRequestV1): Promise<void> => {
    await this.api.setOrchestratorPolicy(input);
    await this.load();
  };

  clearPolicy = async (input: OrchestratorPolicyClearRequestV1): Promise<void> => {
    await this.api.clearOrchestratorPolicy(input);
    await this.load();
  };

  bindParent = async (input: OrchestrationAgentBindParentRequestV1): Promise<void> => {
    await this.api.bindOrchestrationParent(input);
    await this.load();
  };

  registerSession = async (input: OrchestrationAgentRegisterSessionRequestV1): Promise<void> => {
    await this.api.registerOrchestrationSession(input);
    await this.load();
  };

  submitWorker = async (input: WorkerSubmitRequestV1): Promise<void> => {
    await this.api.submitCoordinatedWorker(input);
    await this.load();
  };

  integrateWorker = async (input: WorkerIntegrateRequestV1):
      Promise<{ taskId: string; state: "verifying" }> => {
    // The verify runs on after this answers, and its outcome reaches the canvas
    // as a worker notice; re-reading the snapshot here is what turns the card's
    // taskState to "verifying" while it does.
    const claim = await this.api.integrateCoordinatedWorker(input);
    await this.load();
    return { taskId: claim.taskId, state: "verifying" };
  };

  abandonTask = async (input: WorkerAbandonRequestV1): Promise<void> => {
    await this.api.abandonCoordinatedTask(input);
    await this.load();
  };

  ackObligation = async (input: WorkerAckRequestV1): Promise<void> => {
    await this.api.ackCoordinatedObligation(input);
    await this.load();
  };
}
