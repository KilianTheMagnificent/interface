import { describe, expect, it } from "vitest";
import type { CollaborationState, CollaborationStatusDto } from "../shared/ipc";
import type { EngineUsage } from "../shared/engine";
import type { ProviderAccountsView } from "../shared/provider-accounts";
import {
  FAILED_OPINION_SHOWN_MS, otherAccountChoices, threadHandoffLine, threadHandoffNotice, threadLimitOffer,
  threadOpinionCards, threadProviderOffers, threadSwitchedFrom, type ThreadProviderFacts,
} from "./thread-opinion";
import {
  buildUsageView, threadUsageHeadroom, usageAccountKey, usageHeadroom, usageHeadrooms, usageRowLabel,
  usageStripItems,
} from "./usage-view";

const NOW = Date.parse("2026-10-14T12:00:00.000Z");
const KEY = "claude#a";

function status(state: CollaborationState, extra: Partial<CollaborationStatusDto> = {},
    id = "collaboration-1", updatedAt = "2026-10-14T11:59:00.000Z"): CollaborationStatusDto {
  return {
    receipt: {
      version: 1, collaborationId: id, revision: 3, state, mode: "consult",
      sourceEngine: "claude", targetEngine: "codex", approvalRequired: true,
      cancellationSafe: state === "awaiting_approval" || state === "running_target",
      createdAt: "2026-10-14T11:58:00.000Z", updatedAt,
    },
    sourceSessionKey: KEY,
    ...extra,
  };
}

const PROVIDERS: ThreadProviderFacts[] = [
  { engine: "claude", ready: true, consult: true },
  { engine: "codex", ready: true, consult: true },
  { engine: "acp:grok", ready: false, consult: true },
  { engine: "acp:qwen", ready: true, consult: false },
];

describe("second opinion (review B4) and Switch to (protocol 59)", () => {
  it("offers the other ready providers: consult targets for an opinion, any for Switch to", () => {
    const offers = threadProviderOffers("claude", PROVIDERS, true);
    expect(offers.opinion).toEqual([{ engine: "codex", label: "Codex" }]);
    expect(offers.switchTo.map(choice => choice.engine)).toEqual(["codex", "acp:qwen"]);
    // A plain chat has no folder to carry.
    expect(threadProviderOffers("claude", PROVIDERS, false).switchTo).toEqual([]);
    expect(threadProviderOffers("codex", PROVIDERS.slice(0, 2), true).opinion)
      .toEqual([{ engine: "claude", label: "Claude Code" }]);
  });

  it("draws a thread's consults as cards: review, running, the reply, sending, failed", () => {
    const review = status("awaiting_approval", { approval: {
      collaborationId: "collaboration-1", approvalId: "approval-1", expectedRevision: 3,
      expiresAt: "2026-10-14T12:10:00.000Z", mode: "consult", sourceEngine: "claude",
      sourceSessionKey: KEY, canonicalCwd: "/p", canonicalProjectRoot: "/p", targetEngine: "codex",
      objective: "o", packetHash: "h", packetPreview: "packet", packetBytes: 9_626,
      contextBytes: 8_000, omissions: 1, managedWorktree: false,
    } });
    const ready = status("awaiting_peer_review", { targetSessionKey: "codex#t", peer: {
      state: "awaiting-review", collaborationId: "collaboration-2", expectedRevision: 5,
      resultId: "r", resultDigest: "d", resultBytes: 20, sourceSessionKey: KEY,
      sourceEngine: "claude", targetSessionKey: "codex#t", targetEngine: "codex",
      objective: "o", body: "The retry ceiling is never read.",
    } }, "collaboration-2");
    const failed = status("failed", { receipt: { ...status("failed").receipt,
      collaborationId: "collaboration-3",
      error: { code: "COLLABORATION_TARGET_UNHEALTHY",
        safeNextAction: "Restore the target provider, then submit a new request.", retryable: true },
    } });
    const elsewhere = { ...status("running_target", {}, "collaboration-4"), sourceSessionKey: "codex#z" };
    const done = status("completed", {}, "collaboration-5");
    const cards = threadOpinionCards([ready, failed, review, elsewhere, done], KEY, new Set(), NOW);
    expect(cards.map(card => [card.id, card.stage])).toEqual([
      ["collaboration-2", "ready"], ["collaboration-3", "failed"], ["collaboration-1", "review"],
    ]);
    const byId = new Map(cards.map(card => [card.id, card]));
    expect(byId.get("collaboration-1")).toMatchObject({
      label: "Codex", sourceLabel: "Claude Code", cancellable: true,
      packet: { bytes: 9_626, omissions: 1, preview: "packet" },
    });
    expect(byId.get("collaboration-2")?.body).toBe("The retry ceiling is never read.");
    expect(byId.get("collaboration-3")?.error).toBe(
      "Restore the target provider, then submit a new request.");
    expect(threadOpinionCards([ready], KEY, new Set(["collaboration-2"]), NOW)).toEqual([]);
    const old = status("failed", {}, "collaboration-6",
      new Date(NOW - FAILED_OPINION_SHOWN_MS - 1).toISOString());
    expect(threadOpinionCards([old], KEY, new Set(), NOW)).toEqual([]);
  });

  it("says a switched thread's previous provider, folds its handoffs and says a lost one", () => {
    expect(threadSwitchedFrom(undefined)).toBeNull();
    expect(threadSwitchedFrom({ segments: [
      { key: "claude#a", engine: "claude", order: 0, firstAt: 1, lastAt: 2 },
      { key: "codex#b", engine: "codex", order: 1, firstAt: 3, lastAt: 4 },
    ] })).toBe("Claude Code");
    expect(threadHandoffLine("You are taking over a thread from Claude in Interface; the owner "
      + "switched it to Codex.\n\nThread: Fix it\n")).toBe("Handed the thread to Codex");
    expect(threadHandoffLine("The owner switched this thread back to you (Claude) in Interface. "
      + "While you were away, Codex continued this thread:\n")).toBe("Handed the thread back to Claude");
    expect(threadHandoffLine("You are taking over a thread from Claude in Interface; the owner "
      + "asked Codex to continue it.\n"), "a Continue with prompt is an ordinary first prompt").toBeNull();
    expect(threadHandoffLine("Fix the login")).toBeNull();
    expect(threadHandoffNotice("sent", "Codex")).toBeNull();
    expect(threadHandoffNotice("unknown", "Codex"))
      .toBe("The thread is on Codex now, but its handoff may not have arrived. Type to carry on.");
    expect(threadHandoffNotice("unsent", "Codex")).toContain("its handoff was not sent");
  });
});

describe("usage headroom (review E)", () => {
  const views = buildUsageView([
    { engine: "claude", available: true, plan: "max", windows: [
      { label: "5-hour", utilization: 62, resetsAt: "2026-10-14T14:00:00.000Z" },
      { label: "7-day", utilization: 31, resetsAt: null },
    ] },
    { engine: "codex", available: true, windows: [
      { label: "5-hour", utilization: 100, resetsAt: null },
    ] },
    { engine: "acp:grok", available: true, windows: [{ label: "5-hour", utilization: null, resetsAt: null }] },
  ]);

  it("says what is left of the fullest window, quietly, and nothing for an unknown", () => {
    expect(usageHeadroom(views[0], NOW - 60_000, NOW)).toEqual({
      engine: "claude", text: "38% left", tone: "ok",
      title: "Claude Code plan: 62% of the 5-hour window used, resets in 2h (read 1m ago)",
    });
    expect(usageHeadroom(views[0], NOW - 6 * 60_000, NOW)?.text).toBe("~38% left");
    expect(usageHeadroom(views[1], NOW, NOW)).toMatchObject({ text: "At limit", tone: "full" });
    expect(usageHeadroom(views[2], NOW, NOW)).toBeNull();
    expect(Object.keys(usageHeadrooms(views, NOW, NOW))).toEqual(["claude", "codex"]);
  });

  it("offers Switch to the provider with the most headroom when a thread hits its limit", () => {
    const offers = threadProviderOffers("codex", [
      { engine: "codex", ready: true, consult: true },
      { engine: "claude", ready: true, consult: true },
      { engine: "acp:qwen", ready: true, consult: false },
    ], true);
    const headrooms = usageHeadrooms(views, NOW, NOW);
    expect(threadLimitOffer(
      "Rate limit reached (primary); resets 2026-10-14T13:30:00Z", "codex", offers, headrooms, NOW,
    )).toEqual({
      message: "Codex hit its usage limit; it resets in 1h 30m.",
      engine: "claude", label: "Claude Code", headroom: "38% left",
    });
    expect(threadLimitOffer(undefined, "codex", offers, headrooms, NOW)).toBeNull();
    expect(threadLimitOffer("Rate limit reached", "claude",
      threadProviderOffers("claude", [{ engine: "codex", ready: true, consult: true }], true),
      headrooms, NOW), "Codex is at its limit too").toBeNull();
    expect(threadLimitOffer("Rate limit reached", "claude",
      threadProviderOffers("claude", [{ engine: "claude", ready: true, consult: true }], true),
      headrooms, NOW), "nowhere to go").toBeNull();
  });
});

describe("several accounts per provider (protocol 60)", () => {
  const WORK = "acct-0123456789ab";
  const accounts: ProviderAccountsView = { version: 1, providers: [{
    provider: "claude", engine: "claude", accounts: [
      { id: "default", label: "Default", isDefault: true, signIn: "signedIn" },
      { id: WORK, label: "Work", isDefault: false, signIn: "signedIn", directory: "/a/work" },
      { id: "acct-side", label: "Side", isDefault: false, signIn: "signedIn", directory: "/a/side" },
    ],
  }] };
  const rows: EngineUsage[] = [
    { engine: "claude", available: true, windows: [{ label: "5-hour", utilization: 100, resetsAt: null }] },
    { engine: "claude", available: true, account: WORK, accountLabel: "Work",
      windows: [{ label: "5-hour", utilization: 62, resetsAt: null }] },
    { engine: "claude", available: true, account: "acct-side", accountLabel: "Side",
      windows: [{ label: "5-hour", utilization: null, resetsAt: null }] },
    { engine: "codex", available: true, windows: [{ label: "5-hour", utilization: 100, resetsAt: null }] },
  ];
  const views = buildUsageView(rows);
  const headrooms = usageHeadrooms(views, NOW, NOW);

  it("lists each account's windows under its own label; one account reads as before", () => {
    expect(views.map(view => usageRowLabel(view))).toEqual(["Claude Code", "Claude Code (Work)",
      "Claude Code (Side)", "Codex"]);
    expect(views[0]).not.toHaveProperty("account");
    expect(views[1]).toMatchObject({ account: WORK, accountLabel: "Work" });
    expect(buildUsageView([rows[3]])).toEqual([{ engine: "codex", available: true, plan: "", note: "",
      costText: "", windows: [{ label: "5-hour", percent: 100, percentText: "100%", resetsAt: null }] }]);
    // The strip keeps one chip per provider: its Default account's row.
    expect(usageStripItems(views, NOW, NOW).map(item => [item.label, item.percentText]))
      .toEqual([["Claude Code", "100%"], ["Codex", "100%"]]);
  });

  it("files each account's headroom apart, and a thread reads its own account's", () => {
    expect(Object.keys(headrooms)).toEqual(["claude", `claude@${WORK}`, "codex"]);
    expect(usageAccountKey("claude", "default")).toBe("claude");
    expect(threadUsageHeadroom(headrooms, "claude", undefined)?.text).toBe("At limit");
    expect(threadUsageHeadroom(headrooms, "claude", WORK)).toMatchObject({ text: "38% left",
      title: "Claude Code (Work) plan: 62% of the 5-hour window used (read just now)" });
    expect(threadUsageHeadroom(headrooms, "claude", "acct-side"), "not measured").toBeUndefined();
  });

  it("offers Continue with the same provider's other account when its row shows headroom", () => {
    const offers = threadProviderOffers("claude", [{ engine: "codex", ready: true, consult: true }], true);
    const offer = threadLimitOffer("Rate limit reached", "claude", offers, headrooms, NOW,
      { view: accounts });
    expect(offer).toEqual({
      message: "Claude Code hit its usage limit.",
      engine: "claude", label: "Claude Code (Work)", headroom: "38% left", account: WORK,
    });
    // The thread already runs on Work: Default is at its limit and Side unmeasured, so Codex
    // (at its limit too) leaves nothing.
    expect(threadLimitOffer("Rate limit reached", "claude", offers, headrooms, NOW,
      { view: accounts, account: WORK })).toBeNull();
    // Another provider with headroom leads; the other account is offered beside it.
    const roomy = usageHeadrooms(buildUsageView([...rows.slice(0, 3),
      { engine: "codex", available: true, windows: [{ label: "5-hour", utilization: 10, resetsAt: null }] }]),
    NOW, NOW);
    expect(threadLimitOffer("Rate limit reached", "claude", offers, roomy, NOW, { view: accounts })).toEqual({
      message: "Claude Code hit its usage limit.", engine: "codex", label: "Codex", headroom: "90% left",
      also: [{ engine: "claude", label: "Claude Code (Work)", headroom: "38% left", account: WORK }],
    });
    expect(otherAccountChoices("codex", { view: accounts }, headrooms), "one account").toEqual([]);
  });
});
