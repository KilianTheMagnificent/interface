import { useCallback, useMemo, useState } from "react";
import type { ThreadSecretCommand, ThreadSecretsView } from "../shared/thread-secrets";
import { ipcMessage } from "./ipc-message";

/**
 * The private secret card's channel (protocol 67, T3 section 12) for App:
 * Give and Decline from a pane's card, and Settings' list with Forget. A
 * value is only ever an argument to `give`; nothing here keeps it.
 */
export function useThreadSecrets(send: (command: ThreadSecretCommand) => Promise<ThreadSecretsView>) {
  const [view, setView] = useState<ThreadSecretsView | null>(null);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const refresh = useCallback(() => {
    send({ kind: "list" }).then(setView, (err: unknown) => { setError(ipcMessage(err)); });
  }, [send]);
  /** Answers why the value was not taken, or null. */
  const give = useCallback(async (key: string, id: string, value: string): Promise<string | null> => {
    try {
      setView(await send({ kind: "give", key, id, value }));
      return null;
    } catch (err: unknown) {
      return ipcMessage(err);
    }
  }, [send]);
  const decline = useCallback((key: string, id: string) => {
    send({ kind: "decline", key, id }).then(setView, (err: unknown) => { setError(ipcMessage(err)); });
  }, [send]);
  const forget = useCallback((key: string, name: string) => {
    setPending(true);
    setError(null);
    send({ kind: "forget", key, name }).then(setView, (err: unknown) => { setError(ipcMessage(err)); })
      .finally(() => { setPending(false); });
  }, [send]);
  return useMemo(() => ({ view, pending, error, refresh, give, decline, forget }),
    [view, pending, error, refresh, give, decline, forget]);
}
