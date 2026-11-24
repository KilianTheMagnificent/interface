/**
 * `iface:providerAccounts` (protocol 60): several accounts per provider, each
 * a named profile directory for the Claude, Codex or Grok CLI. Rust owns the
 * stores and every rule (`interface_settings::provider_accounts`); the
 * renderer only asks and draws. Mirrors
 * `crates/interface-contracts/src/provider_accounts.rs`; both read
 * `fixtures/provider-accounts-v60.json`.
 */

import type { EngineId } from "./engine.js";

/** The account Interface has always used: the host's inherited environment. */
export const DEFAULT_ACCOUNT_ID = "default";

export type AccountProvider = "claude" | "codex" | "grok";

export const ACCOUNT_PROVIDERS: readonly AccountProvider[] = ["claude", "codex", "grok"];

export type ProviderAccountsCommand =
  | { command: "read" }
  | { command: "refresh"; provider?: AccountProvider }
  | { command: "add"; provider: AccountProvider; label: string }
  | { command: "rename"; provider: AccountProvider; account: string; label: string }
  | { command: "setDefault"; provider: AccountProvider; account: string }
  | { command: "remove"; provider: AccountProvider; account: string; deleteDirectory?: boolean };

/** Whether an account's CLI says it is signed in; `unknown` = not asked yet. */
export type AccountSignIn = "signedIn" | "signedOut" | "unknown";

export interface ProviderAccountView {
  /** `default` for the inherited account. */
  id: string;
  label: string;
  /** New sessions and terminal panes use it. */
  isDefault: boolean;
  signIn: AccountSignIn;
  /** The signed-in identity the CLI named (an email or account type). */
  identity?: string;
  /** The account's own directory; absent for Default. */
  directory?: string;
  /** Unix milliseconds; absent for Default. */
  createdAt?: number;
}

/** One provider's accounts, Default first. */
export interface ProviderAccountGroup {
  provider: AccountProvider;
  /** The engine id its sessions run under. */
  engine: EngineId;
  accounts: ProviderAccountView[];
}

/** What every `iface:providerAccounts` command answers. */
export interface ProviderAccountsView {
  version: 1;
  /** Claude, Codex, Grok, in that order. */
  providers: ProviderAccountGroup[];
  /** What the last command did, for a notice; absent on `read`. */
  notice?: string;
}

/** The provider whose structured sessions run under `engine`, or null. */
export function accountProviderOf(engine: string): AccountProvider | null {
  switch (engine) {
    case "claude": return "claude";
    case "codex": return "codex";
    case "acp:grok": return "grok";
    default: return null;
  }
}

/** The group for `engine` in `view`, or undefined. */
export function accountGroupFor(view: ProviderAccountsView | null | undefined,
    engine: string): ProviderAccountGroup | undefined {
  return view?.providers.find(group => group.engine === engine);
}

/** The account new sessions of `engine` use: the one marked default, else Default. */
export function defaultAccountOf(group: ProviderAccountGroup | undefined): string {
  return group?.accounts.find(account => account.isDefault)?.id ?? DEFAULT_ACCOUNT_ID;
}
