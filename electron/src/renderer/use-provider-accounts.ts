import { useCallback, useEffect, useRef, useState } from "react";
import type { IfaceApi } from "../shared/ipc";
import type {
  AccountProvider, ProviderAccountsCommand, ProviderAccountsView,
} from "../shared/provider-accounts";
import { ipcMessage } from "./ipc-message";

/** Whether an answer may replace the view: never one older than the last shown. */
export function newestAnswer(sequence: number, shown: number): boolean {
  return sequence >= shown;
}

/** The commands after which Rust is asked again whether each account is signed in. */
export function refreshesAfter(command: ProviderAccountsCommand): AccountProvider | null {
  switch (command.command) {
    case "add": case "setDefault": case "remove": return command.provider;
    default: return null;
  }
}

export interface ProviderAccountsState {
  /** Null until the first read answers. */
  view: ProviderAccountsView | null;
  /** A change (add, rename, set default, remove) is on its way. */
  pending: boolean;
  /** A sign-in check (`refresh`) is running; it takes seconds. */
  checking: boolean;
  error: string | null;
  /** What the last change did ("Added Work."), from Rust. */
  notice: string | null;
}

/**
 * Several accounts per provider (protocol 60): the view Rust answers, read on
 * mount and when Settings → Providers opens (`read` paints at once, `refresh`
 * asks each CLI in the background). Every command answers the whole view; an
 * answer older than the one shown is dropped, so a slow refresh cannot undo a
 * change that landed after it began.
 */
export function useProviderAccounts(api: Pick<IfaceApi, "providerAccounts">) {
  const [view, setView] = useState<ProviderAccountsView | null>(null);
  const [pending, setPending] = useState(false);
  const [checks, setChecks] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const sequence = useRef(0);
  const shown = useRef(0);
  const viewRef = useRef<ProviderAccountsView | null>(null);

  const ask = useCallback(async (command: ProviderAccountsCommand): Promise<ProviderAccountsView> => {
    sequence.current += 1;
    const mine = sequence.current;
    const answer = await api.providerAccounts(command);
    if (newestAnswer(mine, shown.current)) {
      shown.current = mine;
      viewRef.current = answer;
      setView(answer);
    }
    return answer;
  }, [api]);

  const read = useCallback(() => {
    void ask({ command: "read" }).catch((err: unknown) => { setError(ipcMessage(err)); });
  }, [ask]);

  const refresh = useCallback((provider?: AccountProvider) => {
    setChecks(count => count + 1);
    void ask(provider === undefined ? { command: "refresh" } : { command: "refresh", provider })
      .then(answer => { if (answer.notice !== undefined) setNotice(answer.notice); })
      .catch((err: unknown) => { setError(ipcMessage(err)); })
      .finally(() => { setChecks(count => count - 1); });
  }, [ask]);

  /** Settings → Providers opened: paint what Rust has, then check each sign-in. */
  const open = useCallback(() => {
    read();
    refresh();
  }, [read, refresh]);

  const change = useCallback((command: ProviderAccountsCommand) => {
    setPending(true);
    setError(null);
    setNotice(null);
    void ask(command).then(answer => {
      setNotice(answer.notice ?? null);
      const again = refreshesAfter(command);
      if (again !== null) refresh(again);
    }).catch((err: unknown) => { setError(ipcMessage(err)); })
      .finally(() => { setPending(false); });
  }, [ask, refresh]);

  useEffect(() => { read(); }, [read]);

  const state: ProviderAccountsState = { view, pending, checking: checks > 0, error, notice };
  return { state, viewRef, read, refresh, open, change };
}
