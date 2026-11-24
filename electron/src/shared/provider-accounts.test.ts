import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { encodeRendererPayload } from "../main/ipc-payload";
import { handsfreeRefusal } from "../main/handsfree";
import type { EngineUsage, LaunchOptions } from "./engine";
import { CHANNELS, type CreateTerminalRequest } from "./ipc";
import type { ThreadCommand, ThreadView } from "./threads";
import {
  accountGroupFor, accountProviderOf, defaultAccountOf, type ProviderAccountsCommand,
  type ProviderAccountsView,
} from "./provider-accounts";

const fixture = JSON.parse(readFileSync(
  new URL("../fixtures/provider-accounts-v60.json", import.meta.url), "utf8",
)) as {
  channel: string;
  protocol: number;
  requests: ProviderAccountsCommand[];
  refusedRequests: unknown[];
  view: ProviderAccountsView;
  launch: LaunchOptions;
  usage: EngineUsage;
  signInTerminal: CreateTerminalRequest;
  thread: ThreadView;
  continueWith: Extract<ThreadCommand, { kind: "continueWith" }>;
};

const roundTrip = <T>(value: T): T => JSON.parse(JSON.stringify(value)) as T;

describe("provider accounts v60", () => {
  it("sends each command to Rust unchanged on its own channel", () => {
    expect(fixture.channel).toBe(CHANNELS.providerAccounts);
    expect(fixture.protocol).toBe(60);
    expect(fixture.requests).toHaveLength(9);
    for (const request of fixture.requests) {
      expect(encodeRendererPayload(CHANNELS.providerAccounts, [request])).toEqual(request);
    }
    expect(fixture.refusedRequests).toHaveLength(6);
  });

  it("round-trips the view, launch, usage, sign-in terminal, thread and continueWith shapes", () => {
    for (const shape of [fixture.view, fixture.launch, fixture.usage, fixture.signInTerminal,
      fixture.thread]) {
      expect(roundTrip(shape)).toEqual(shape);
    }
    expect(fixture.launch.account).toBe("acct-0123456789ab");
    expect(fixture.usage).toMatchObject({ account: "acct-0123456789ab", accountLabel: "Work" });
    expect(fixture.signInTerminal).toMatchObject({ signIn: true, account: "acct-0123456789ab",
      title: "Sign in to Claude (Work)" });
    expect(fixture.thread.account).toBe("acct-0123456789ab");
    // Continue with another account of the thread's own provider.
    expect(roundTrip(fixture.continueWith)).toEqual(fixture.continueWith);
    expect(fixture.continueWith).toMatchObject({ kind: "continueWith", engine: "claude",
      account: "acct-0123456789ab" });
    expect(encodeRendererPayload(CHANNELS.threadControl, [fixture.continueWith])).toEqual(fixture.continueWith);
    expect(encodeRendererPayload(CHANNELS.createSession, [{ engine: "claude", ...fixture.launch }]))
      .toEqual({ engine: "claude", options: fixture.launch });
  });

  it("reads each provider's group and default account from the view", () => {
    const view = fixture.view;
    expect(view.providers.map(group => group.provider)).toEqual(["claude", "codex", "grok"]);
    expect(view.providers.every(group => group.accounts[0]?.id === "default")).toBe(true);
    expect(defaultAccountOf(accountGroupFor(view, "claude"))).toBe("acct-0123456789ab");
    expect(defaultAccountOf(accountGroupFor(view, "acp:grok"))).toBe("default");
    expect(defaultAccountOf(accountGroupFor(view, "acp:qwen"))).toBe("default");
    expect(accountGroupFor(null, "claude")).toBeUndefined();
    expect(["claude", "codex", "acp:grok", "acp:qwen"].map(accountProviderOf))
      .toEqual(["claude", "codex", "grok", null]);
  });

  it("hands-free sessions may manage accounts", () => {
    expect(handsfreeRefusal(CHANNELS.providerAccounts, { command: "add", provider: "claude",
      label: "Work" })).toBeUndefined();
  });
});
