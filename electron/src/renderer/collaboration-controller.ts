import type { CollaborationStatusDto, IfaceApi } from "../shared/ipc";
import { equalValue } from "./stable-values";

export type CollaborationApi = Pick<IfaceApi, "listCollaborations" | "onCollaborationEvent"
  | "submitCollaboration" | "approveCollaboration" | "denyCollaboration"
  | "acceptCollaborationPeer" | "discardCollaborationPeer" | "cancelCollaboration">;

/**
 * Keep the newest durable receipt across list, mutation and push races. A
 * status equal to the one held keeps the held object, and a merge that
 * changes nothing returns `current` itself, so the relist every window focus
 * makes publishes nothing.
 */
export function mergeCollaborationStatuses(
  current: readonly CollaborationStatusDto[], incoming: readonly CollaborationStatusDto[],
): readonly CollaborationStatusDto[] {
  const merged = new Map(current.map(status => [status.receipt.collaborationId, status]));
  for (const status of incoming) {
    const known = merged.get(status.receipt.collaborationId);
    if (known === undefined || status.receipt.revision >= known.receipt.revision) {
      merged.set(status.receipt.collaborationId,
        known !== undefined && equalValue(known, status) ? known : status);
    }
  }
  const sorted = [...merged.values()].sort((left, right) =>
    right.receipt.updatedAt.localeCompare(left.receipt.updatedAt)
    || left.receipt.collaborationId.localeCompare(right.receipt.collaborationId));
  return sorted.length === current.length
    && sorted.every((status, index) => status === current[index]) ? current : sorted;
}

export class CollaborationController {
  private request = 0;
  private connected = false;
  private state = { collaborations: [] as readonly CollaborationStatusDto[],
    collaborationsLoading: true, collaborationsError: null as string | null };
  private readonly listeners = new Set<() => void>();
  constructor(private readonly api: CollaborationApi,
    private readonly errorMessage: (error: unknown) => string) {}
  snapshot = () => this.state;
  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  };
  private publish(patch: Partial<typeof this.state>): void {
    if (!this.connected) return;
    // A patch that changes nothing (loading already false, an equal relist)
    // would still be a new snapshot, and every subscriber would render.
    const keys = Object.keys(patch) as (keyof typeof this.state)[];
    if (keys.every(key => Object.is(this.state[key], patch[key]))) return;
    this.state = { ...this.state, ...patch };
    for (const listener of this.listeners) listener();
  }
  private upsert = (status: CollaborationStatusDto): void => {
    this.publish({ collaborations: mergeCollaborationStatuses(this.state.collaborations, [status]) });
  };
  connect = (): (() => void) => {
    this.connected = true;
    const unsubscribe = this.api.onCollaborationEvent(event => {
      if (event.version === 1 && event.kind === "changed") this.upsert(event.status);
    });
    return () => {
      this.connected = false;
      this.request++;
      unsubscribe();
    };
  };
  /** `quiet`: a background relist (window focus) that shows no loading state,
   * so an equal answer renders nothing; pushes already keep the list current. */
  loadCollaborations = async (options: { quiet?: boolean } = {}): Promise<void> => {
    const request = ++this.request;
    this.publish(options.quiet === true ? { collaborationsError: null }
      : { collaborationsLoading: true, collaborationsError: null });
    try {
      const listed = await this.api.listCollaborations();
      if (this.request === request) {
        this.publish({ collaborations: mergeCollaborationStatuses(this.state.collaborations, listed) });
      }
    } catch (error) {
      if (this.request === request) this.publish({ collaborationsError: this.errorMessage(error) });
    } finally {
      if (this.request === request) this.publish({ collaborationsLoading: false });
    }
  };
  /** A status another channel answered (a thread's Second opinion submits through
   *  `iface:threadControl`); merged like any other, newest revision winning. */
  noteCollaboration = (status: CollaborationStatusDto): void => { this.upsert(status); };
  // Mutation rejections propagate to the initiating UI; they are never retried.
  submitCollaboration = async (...args: Parameters<CollaborationApi["submitCollaboration"]>) => {
    this.upsert(await this.api.submitCollaboration(...args));
  };
  approveCollaboration = async (...args: Parameters<CollaborationApi["approveCollaboration"]>) => {
    this.upsert(await this.api.approveCollaboration(...args));
  };
  denyCollaboration = async (...args: Parameters<CollaborationApi["denyCollaboration"]>) => {
    this.upsert(await this.api.denyCollaboration(...args));
  };
  acceptCollaborationPeer = async (...args: Parameters<CollaborationApi["acceptCollaborationPeer"]>) => {
    this.upsert(await this.api.acceptCollaborationPeer(...args));
  };
  discardCollaborationPeer = async (...args: Parameters<CollaborationApi["discardCollaborationPeer"]>) => {
    this.upsert(await this.api.discardCollaborationPeer(...args));
  };
  cancelCollaboration = async (...args: Parameters<CollaborationApi["cancelCollaboration"]>) => {
    this.upsert(await this.api.cancelCollaboration(...args));
  };
}
