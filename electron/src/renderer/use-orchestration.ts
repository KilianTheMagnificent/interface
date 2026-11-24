import { useEffect, useRef, useState } from "react";
import type { IfaceApi } from "../shared/ipc";
import type { CoordinationStatusV1, OrchestratorModelsV1 } from "../shared/orchestration-control";
import { ipcMessage } from "./ipc-message";
import { equalValue } from "./stable-values";
import { OrchestrationController, type OrchestrationApi } from "./orchestration-controller";
import { useStoreState } from "./use-store-state";

/** One coordination-status and model-catalogue read per burst of snapshot reads. */
export const COORDINATION_STATUS_DEBOUNCE_MS = 500;

/**
 * App's orchestration snapshot and the canvas's intents. `api` is read once;
 * App passes `window.iface`.
 */
export function useOrchestration(api: OrchestrationApi) {
  const [controller] = useState(() => new OrchestrationController(api, ipcMessage));
  const state = useStoreState(controller);
  // refreshAll's mount call does the initial read; this only follows changes.
  useEffect(controller.connect, [controller]);
  return { ...state, controller };
}

/**
 * The canvas's coordination strip and the policy form's model catalogue for
 * `projectRoot`, re-read after every accepted snapshot read (`reads`) while
 * the canvas is shown (`enabled`), and once each time it is shown again. A
 * hidden canvas read both on every window focus for nobody.
 */
export function useCanvasReads(
  api: Pick<IfaceApi, "getCoordinationStatus" | "getOrchestratorModels">,
  projectRoot: string, reads: number, enabled = true,
) {
  // Coordination is optional: an unregistered project answers with a refusal,
  // and null is the honest rendering of that, not an error the canvas should
  // shout about.
  const [coordination, setCoordination] = useState<CoordinationStatusV1 | null>(null);
  const coordinationRequest = useRef(0);
  const [models, setModels] = useState<OrchestratorModelsV1 | null>(null);
  // Coordination state is a fact about the project on disk, so it is re-read
  // on the same host invalidation the snapshot rides in on — never on a clock
  // of our own.
  useEffect(() => {
    if (!enabled) return;
    if (projectRoot === "") {
      setCoordination(null);
      return;
    }
    // Every accepted snapshot read re-runs this, so a burst of agent state
    // changes collapses into a single read.
    const timer = setTimeout(() => {
      const request = coordinationRequest.current + 1;
      coordinationRequest.current = request;
      void api
        .getCoordinationStatus({ canonicalProjectRoot: projectRoot })
        .then(status => {
          // An unchanged read keeps the held object, so a window focus does
          // not re-render App and the hidden canvas.
          if (coordinationRequest.current === request) {
            setCoordination(current => equalValue(current, status) ? current : status);
          }
        })
        .catch(() => {
          if (coordinationRequest.current === request) setCoordination(null);
        });
    }, COORDINATION_STATUS_DEBOUNCE_MS);
    return () => { clearTimeout(timer); };
  }, [api, projectRoot, reads, enabled]);
  // The catalogue is refined by a later probe or a live `models.available`, so
  // the policy form's "Known ids" hint follows the snapshot rather than being
  // read once at launch and going stale.
  useEffect(() => {
    // Same debounce as the coordination strip, for the same reason: every
    // accepted read re-runs this and each run replaces the catalogue object,
    // so a burst of agent state changes collapses into one read.
    if (!enabled) return;
    const timer = setTimeout(() => {
      void api.getOrchestratorModels()
        .then(next => { setModels(current => equalValue(current, next) ? current : next); })
        .catch(() => { setModels(null); });
    }, COORDINATION_STATUS_DEBOUNCE_MS);
    return () => { clearTimeout(timer); };
  }, [api, reads, enabled]);
  return { coordination, models };
}
