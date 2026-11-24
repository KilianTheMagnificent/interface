import { useCallback, useEffect, useRef, useState } from "react";
import type { IfaceApi } from "../shared/ipc";
import { failedUpdateState, isUpdateState, type UpdateState } from "../shared/updater";
import { productLabel } from "./product";

type UpdaterApi = Pick<IfaceApi,
  "onUpdateState" | "getUpdateState" | "checkForUpdates" | "restartToUpdate">;

/**
 * The updater as the toolbar and the restart dialog see it. The Rust host owns
 * the policy and the state machine; this holds the last state it reported and
 * the one confirmation the user has to give before a restart.
 */
export function useUpdater(api: UpdaterApi) {
  const [updateState, setUpdateState] = useState<UpdateState | null>(null);
  const [updateRestartConfirmOpen, setUpdateRestartConfirmOpen] = useState(false);
  const [updateRestartPending, setUpdateRestartPending] = useState(false);
  const updateRestartOpenerRef = useRef<HTMLElement | null>(null);

  useEffect(() => {
    let active = true;
    const accept = (next: unknown): void => {
      if (!active) return;
      if (isUpdateState(next)) setUpdateState(next);
      else setUpdateState(current => failedUpdateState(
        current, `${productLabel()} received an invalid update status.`));
    };
    const off = api.onUpdateState(accept);
    void api.getUpdateState().then(accept).catch((err: unknown) => {
      console.warn("[app] updater state could not be read:", err);
      if (!active) return;
      setUpdateState(current => failedUpdateState(
        current, `${productLabel()} could not read update status.`));
    });
    return () => { active = false; off(); };
  }, [api]);

  useEffect(() => {
    if (updateRestartPending && updateState?.phase === "error") {
      setUpdateRestartPending(false);
    }
  }, [updateRestartPending, updateState?.phase]);

  const checkForUpdates = useCallback(() => {
    void api.checkForUpdates().then(next => {
      if (isUpdateState(next)) setUpdateState(next);
      else setUpdateState(current => failedUpdateState(
        current, `${productLabel()} received an invalid update status.`));
    }).catch((err: unknown) => {
      console.warn("[app] update check IPC failed:", err);
      setUpdateState(current => failedUpdateState(
        current, `${productLabel()} could not check for updates.`));
    });
  }, [api]);

  const requestRestartToUpdate = useCallback(() => {
    if (updateRestartPending) return;
    updateRestartOpenerRef.current = document.activeElement instanceof HTMLElement
      ? document.activeElement : null;
    setUpdateRestartConfirmOpen(true);
  }, [updateRestartPending]);

  const cancelRestartToUpdate = useCallback(() => {
    setUpdateRestartConfirmOpen(false);
  }, []);

  const confirmRestartToUpdate = useCallback(() => {
    setUpdateRestartConfirmOpen(false);
    setUpdateRestartPending(true);
    void api.restartToUpdate().then(accepted => {
      if (!accepted) {
        setUpdateRestartPending(false);
        setUpdateState(current => failedUpdateState(
          current, "The downloaded update is no longer ready. Check again."));
      }
    }).catch((err: unknown) => {
      console.warn("[app] update restart IPC failed:", err);
      setUpdateRestartPending(false);
      setUpdateState(current => failedUpdateState(
        current, `${productLabel()} could not start the update.`));
    });
  }, [api]);

  return {
    updateState, updateRestartConfirmOpen, updateRestartPending, updateRestartOpenerRef,
    checkForUpdates, requestRestartToUpdate, cancelRestartToUpdate, confirmRestartToUpdate,
  };
}
