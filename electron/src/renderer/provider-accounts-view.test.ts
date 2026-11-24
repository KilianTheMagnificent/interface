import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import type { ProviderAccountsView } from "../shared/provider-accounts";
import {
  accountChoiceFor, accountFolderWords, accountGroupsView, accountNameDraft, accountRemoveWords,
  accountSignInTitle, chosenAccount, threadAccountLabel,
} from "./provider-accounts-view";
import { threadFormsOnAccount } from "./thread-card";
import { buildLaunchOptions } from "./launch";
import { COMPOSER_SCOPE_DEFAULTS, COMPOSER_TEAM_DEFAULTS, type ComposerLaunchForm } from "./composer-model";
import { newestAnswer, refreshesAfter } from "./use-provider-accounts";
import { accountContinuationForm } from "./use-thread-bridges";

const fixture = JSON.parse(readFileSync(
  new URL("../fixtures/provider-accounts-v60.json", import.meta.url), "utf8",
)) as { view: ProviderAccountsView };
const VIEW = fixture.view;
const WORK = "acct-0123456789ab";
const DIRECTORY = "/Users/me/Library/Application Support/Interface/accounts/claude/acct-0123456789ab";

describe("the New thread card's account control", () => {
  it("offers the provider's accounts, starting on its default, and is absent with one account", () => {
    expect(accountChoiceFor(VIEW, "claude")).toEqual({
      options: [{ value: "default", label: "Default account" }, { value: WORK, label: "Work account" }],
      defaultAccount: WORK,
    });
    expect(accountChoiceFor(VIEW, "codex"), "one account: no control").toBeNull();
    expect(accountChoiceFor(VIEW, "acp:grok"), "one account: no control").toBeNull();
    expect(accountChoiceFor(VIEW, "acp:qwen"), "no accounts at all").toBeNull();
    expect(accountChoiceFor(null, "claude"), "before the first read").toBeNull();
  });

  it("keeps the owner's pick while it is one of the accounts, else the default", () => {
    const choice = accountChoiceFor(VIEW, "claude");
    expect(chosenAccount(choice, null)).toBe(WORK);
    expect(chosenAccount(choice, "default")).toBe("default");
    expect(chosenAccount(choice, "acct-gone"), "a removed pick").toBe(WORK);
    expect(chosenAccount(null, "default"), "no control, no account").toBeNull();
  });

  it("sends the chosen account only on the chosen provider's forms", () => {
    const form = (engine: string, account?: string): ComposerLaunchForm => ({
      engine, cwd: "/p", prompt: "Hello", model: "m", effort: "default", permissionMode: "",
      ...COMPOSER_SCOPE_DEFAULTS, ...COMPOSER_TEAM_DEFAULTS, ...(account === undefined ? {} : { account }),
    });
    const forms = threadFormsOnAccount([form("claude"), form("codex", WORK)], "claude", "default");
    expect(forms.map(each => each.account)).toEqual(["default", undefined]);
    expect(threadFormsOnAccount([form("claude", WORK)], "claude", null)[0]).not.toHaveProperty("account");
    expect(buildLaunchOptions(forms[0])).toMatchObject({ engine: "claude", account: "default" });
    expect(buildLaunchOptions(form("claude", WORK))).toMatchObject({ account: WORK });
    expect(buildLaunchOptions(form("claude"))).not.toHaveProperty("account");
  });
});

describe("Settings → Providers' accounts", () => {
  it("lists each provider's accounts with their status, Default badge and actions", () => {
    const groups = accountGroupsView({ ...VIEW, providers: [{
      ...VIEW.providers[0],
      accounts: [VIEW.providers[0].accounts[0], { ...VIEW.providers[0].accounts[1] },
        { id: "acct-3", label: "Side", isDefault: false, signIn: "unknown", directory: "/a/acct-3" }],
    }, ...VIEW.providers.slice(1)] });
    expect(groups.map(group => [group.provider, group.label]))
      .toEqual([["claude", "Claude Code"], ["codex", "Codex"], ["grok", "Grok"]]);
    const [inherited, work, side] = groups[0].rows;
    expect(inherited).toMatchObject({ id: "default", label: "Default", isDefault: false,
      statusText: "Signed in as", identity: "me@example.test",
      canRename: false, canSetDefault: true, canSignIn: false, canRemove: false });
    expect(work).toMatchObject({ label: "Work", isDefault: true, statusText: "Not signed in", identity: null,
      directory: DIRECTORY, canRename: true, canSetDefault: false, canSignIn: true, canRemove: true });
    expect(side).toMatchObject({ statusText: "Not checked", canSetDefault: true, canSignIn: true, canRemove: true });
    expect(groups[1].rows).toHaveLength(1);
    expect(groups[1].rows[0]).toMatchObject({ isDefault: true, statusText: "Not checked",
      canSetDefault: false, canRemove: false, canRename: false });
    expect(accountGroupsView(null)).toEqual([]);
  });

  it("asks twice before the folder goes, and the second ask names the folder", () => {
    const work = accountGroupsView(VIEW)[0].rows[1];
    const first = accountRemoveWords("Claude Code", work);
    expect(first.title).toBe("Remove Claude Code (Work)?");
    expect(first.confirmLabel).toBe("Remove account");
    expect(first.description).toContain(`Its folder stays on disk unless you choose to delete it next: ${DIRECTORY}`);
    const second = accountFolderWords(DIRECTORY);
    expect(second).toMatchObject({ title: "Also delete its folder?", confirmLabel: "Delete folder",
      cancelLabel: "Keep folder" });
    expect(second.description.startsWith(`Also delete ${DIRECTORY}?`)).toBe(true);
    expect(accountRemoveWords("Claude Code", { label: "X", directory: null }).description)
      .not.toContain("folder");
  });

  it("names a new account only when there is a name, and titles its sign-in pane", () => {
    expect(accountNameDraft("  Work ")).toBe("Work");
    expect(accountNameDraft("   ")).toBeNull();
    expect(accountSignInTitle("Claude", "Work")).toBe("Sign in to Claude (Work)");
    expect(accountSignInTitle("Claude", null)).toBe("Sign in to Claude");
  });

  it("drops an answer older than the one shown and re-checks sign-in after a change", () => {
    expect(newestAnswer(3, 2)).toBe(true);
    expect(newestAnswer(1, 2), "a slow refresh that began before a change").toBe(false);
    expect(refreshesAfter({ command: "add", provider: "claude", label: "Work" })).toBe("claude");
    expect(refreshesAfter({ command: "remove", provider: "grok", account: WORK })).toBe("grok");
    expect(refreshesAfter({ command: "setDefault", provider: "codex", account: "default" })).toBe("codex");
    expect(refreshesAfter({ command: "rename", provider: "claude", account: WORK, label: "W" })).toBeNull();
    expect(refreshesAfter({ command: "read" })).toBeNull();
  });
});

describe("Continue with another account of the thread's provider", () => {
  it("launches the new thread's session on that account", () => {
    const form = accountContinuationForm({ engine: "claude", account: WORK, cwd: "/p",
      prompt: "You are taking over a thread", model: "claude-opus-5", name: "Fix it · Claude Code (Work)" });
    expect(buildLaunchOptions(form)).toMatchObject({ engine: "claude", account: WORK, cwd: "/p",
      model: "claude-opus-5", name: "Fix it · Claude Code (Work)" });
  });
});

describe("a thread pane head's account", () => {
  it("shows the account only when its provider has more than one, and says when it is gone", () => {
    expect(threadAccountLabel(VIEW, "claude", WORK)).toBe("Work");
    expect(threadAccountLabel(VIEW, "claude", undefined)).toBe("Default");
    expect(threadAccountLabel(VIEW, "claude", "acct-gone")).toBe("account removed");
    expect(threadAccountLabel(VIEW, "codex", undefined), "one account").toBeNull();
    expect(threadAccountLabel(VIEW, "codex", "acct-gone"), "its only other account was removed")
      .toBe("account removed");
    expect(threadAccountLabel(VIEW, "acp:qwen", undefined)).toBeNull();
    expect(threadAccountLabel(null, "claude", WORK), "before the first read").toBeNull();
  });
});
