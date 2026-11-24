import {
  accountGroupFor, DEFAULT_ACCOUNT_ID, defaultAccountOf, type AccountProvider,
  type ProviderAccountGroup, type ProviderAccountsView, type ProviderAccountView,
} from "../shared/provider-accounts";
import { providerLabel } from "./provider-labels";

/**
 * Several accounts per provider (protocol 60), as the renderer words them:
 * Settings → Providers' accounts list, the New thread card's account control,
 * a thread pane head's account label and the sign-in pane's title. Rust owns
 * the accounts and every rule; this only decides what to draw.
 */

/** Shown wherever another account is offered (owner, 2026-11-21): under
 *  Settings' Add … account and under a thread's limit line that offers one. */
export const ACCOUNT_TERMS_NOTE = "Use only accounts you pay for. Each provider's terms still apply.";

/** "Claude (Work)": a provider and one of its accounts, wherever both are named. */
export function accountDisplayLabel(provider: string, accountLabel: string): string {
  return `${provider} (${accountLabel})`;
}

/** The New thread card's account control: absent (null) with one account. */
export interface AccountChoice {
  options: readonly { value: string; label: string }[];
  /** The provider's default account, the control's starting value. */
  defaultAccount: string;
}

export function accountChoiceFor(view: ProviderAccountsView | null | undefined,
    engine: string): AccountChoice | null {
  const group = accountGroupFor(view, engine);
  if (group === undefined || group.accounts.length <= 1) return null;
  return {
    // "Default" alone on the card reads as a mode or an effort: say it is an account.
    options: group.accounts.map(account => ({ value: account.id, label: `${account.label} account` })),
    defaultAccount: defaultAccountOf(group),
  };
}

/**
 * The account the card shows as chosen: the owner's pick while it is one of
 * the provider's accounts, else the provider's default. Null without a control.
 */
export function chosenAccount(choice: AccountChoice | null, picked: string | null): string | null {
  if (choice === null) return null;
  return picked !== null && choice.options.some(option => option.value === picked)
    ? picked : choice.defaultAccount;
}

/**
 * A thread pane head's account words: only when its provider has more than
 * one account, or when the thread is bound to an account that is gone.
 * "Default" for the inherited account, "account removed" for an id the view
 * no longer lists. Null before the view is read.
 */
export function threadAccountLabel(view: ProviderAccountsView | null | undefined, engine: string,
    account: string | undefined): string | null {
  const group = accountGroupFor(view, engine);
  if (group === undefined) return null;
  const id = account ?? DEFAULT_ACCOUNT_ID;
  const found = group.accounts.find(candidate => candidate.id === id);
  if (found === undefined) return "account removed";
  return group.accounts.length > 1 ? found.label : null;
}

/** One account row of Settings → Providers. */
export interface AccountRowView {
  id: string;
  label: string;
  isDefault: boolean;
  /** "Signed in as …", "Signed in", "Not signed in" or "Not checked". */
  status: "signedIn" | "signedOut" | "unknown";
  statusText: string;
  /** The identity the CLI named, unmasked; the page masks it. Null when none. */
  identity: string | null;
  directory: string | null;
  canRename: boolean;
  canSetDefault: boolean;
  canSignIn: boolean;
  canRemove: boolean;
}

export interface AccountGroupView {
  provider: AccountProvider;
  engine: string;
  /** "Claude", "Codex", "Grok". */
  label: string;
  rows: readonly AccountRowView[];
}

function accountRow(account: ProviderAccountView): AccountRowView {
  const identity = account.identity?.trim() || null;
  const isDefaultAccount = account.id === DEFAULT_ACCOUNT_ID;
  return {
    id: account.id,
    label: account.label,
    isDefault: account.isDefault,
    status: account.signIn,
    statusText: account.signIn === "signedIn" ? (identity === null ? "Signed in" : "Signed in as")
      : account.signIn === "signedOut" ? "Not signed in" : "Not checked",
    identity: account.signIn === "signedIn" ? identity : null,
    directory: account.directory ?? null,
    // Default is the inherited environment: it keeps its name and stays.
    canRename: !isDefaultAccount,
    canSetDefault: !account.isDefault,
    // Signed in already: nothing to sign in to (owner, 2026-11-15).
    canSignIn: account.signIn !== "signedIn",
    canRemove: !isDefaultAccount,
  };
}

/** Settings → Providers' accounts, one group per provider, Default first. */
export function accountGroupsView(view: ProviderAccountsView | null | undefined,
    labels: Readonly<Record<string, string>> = {}): readonly AccountGroupView[] {
  if (view === null || view === undefined) return [];
  return view.providers.map((group: ProviderAccountGroup) => ({
    provider: group.provider,
    engine: group.engine,
    label: providerLabel(group.engine, labels[group.engine]),
    rows: group.accounts.map(accountRow),
  }));
}

/** The first confirmation of Remove: the record goes, the folder stays. */
export function accountRemoveWords(provider: string, row: Pick<AccountRowView, "label" | "directory">):
    { title: string; description: string; confirmLabel: string; cancelLabel: string } {
  return {
    title: `Remove ${accountDisplayLabel(provider, row.label)}?`,
    description: `Threads started on it stay listed but can no longer continue.${row.directory === null
      ? "" : ` Its folder stays on disk unless you choose to delete it next: ${row.directory}`}`,
    confirmLabel: "Remove account",
    cancelLabel: "Cancel",
  };
}

/**
 * The second, separate choice: also delete the account's folder (its sign-in
 * and history). Only its own confirm sends `deleteDirectory: true`.
 */
export function accountFolderWords(directory: string):
    { title: string; description: string; confirmLabel: string; cancelLabel: string } {
  return {
    title: "Also delete its folder?",
    description: `Also delete ${directory}? This removes the account's sign-in and its session history, and cannot be undone.`,
    confirmLabel: "Delete folder",
    cancelLabel: "Keep folder",
  };
}

/** The sign-in pane's title: "Sign in to Claude", or "Sign in to Claude (Work)". */
export function accountSignInTitle(provider: string, accountLabel: string | null): string {
  return accountLabel === null ? `Sign in to ${provider}` : `Sign in to ${accountDisplayLabel(provider, accountLabel)}`;
}

/** A new account's name as typed, trimmed; null when there is nothing to send. */
export function accountNameDraft(text: string): string | null {
  const name = text.trim();
  return name === "" ? null : name;
}
