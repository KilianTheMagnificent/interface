import { useEffect, useMemo, useSyncExternalStore } from "react";
import { CollaborationController, type CollaborationApi } from "./collaboration-controller";

export function useCollaborations(api: CollaborationApi, errorMessage: (error: unknown) => string) {
  const controller = useMemo(() => new CollaborationController(api, errorMessage), [api, errorMessage]);
  const state = useSyncExternalStore(controller.subscribe, controller.snapshot, controller.snapshot);
  useEffect(controller.connect, [controller]);
  return { ...state, loadCollaborations: controller.loadCollaborations,
    noteCollaboration: controller.noteCollaboration,
    submitCollaboration: controller.submitCollaboration,
    approveCollaboration: controller.approveCollaboration,
    denyCollaboration: controller.denyCollaboration,
    acceptCollaborationPeer: controller.acceptCollaborationPeer,
    discardCollaborationPeer: controller.discardCollaborationPeer,
    cancelCollaboration: controller.cancelCollaboration };
}
