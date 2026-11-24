import { ipcMessage } from "./ipc-message";
import { deliveryOutcomeUnknown } from "./outbound-lane";
import type { PendingEcho } from "./transcript-merge";

/** Output feedback and recovery need the same visible turn boundary as a composer send. */
export async function sendWithVisibleTurn(createEcho: () => PendingEcho, port: {
  send: (beforeSend: () => void, afterAccepted: () => void) => Promise<void>;
  append: (echo: PendingEcho) => void;
  checkpoint: (turnKey: string) => void;
  delivered: () => void;
  fail: (id: string, message: string) => void;
}): Promise<void> {
  let shown: PendingEcho | undefined;
  try {
    await port.send(() => {
      const echo = createEcho();
      shown = echo;
      port.append(echo);
      try { port.checkpoint(`e:${echo.id}`); }
      catch (error) { console.warn("Could not checkpoint this turn", error); }
    }, port.delivered);
  } catch (error) {
    if (shown && !deliveryOutcomeUnknown(error)) port.fail(shown.id, ipcMessage(error));
    throw error;
  }
}
