import { describe, it, expect } from "vitest";
import {
  contextAutoCompactRemaining, contextUsageChange,
  contextWindowRemaining, formatContextTokens, isSessionKind,
  sessionKey, SESSION_KINDS, WIRE_SESSION_KINDS, sessionKindOf,
} from "./engine";
import fixture from "../fixtures/session-kind-contract.json";

describe("sessionKey", () => {
  it("is engine#id", () => {
    expect(sessionKey("claude", "abc")).toBe("claude#abc");
  });
});

describe("session kind contract", () => {
  it("mirrors the shared fixture kinds, default, and later-use prose", () => {
    expect(WIRE_SESSION_KINDS).toEqual(fixture.kinds);
    expect(SESSION_KINDS).toEqual(fixture.kinds.filter((k: string) => k !== "voice"));
    expect(sessionKindOf({ kind: "voice" })).toBe("chat");
    expect(isSessionKind("voice")).toBe(false);
    expect(sessionKindOf({})).toBe(fixture.defaultKind);
    expect(sessionKindOf({ kind: "chat" })).toBe("chat");
    expect(isSessionKind("chat")).toBe(true);
    expect(isSessionKind("banana")).toBe(false);
    expect(fixture.codexChatRefusal).toBe(
      "Codex cannot run a tool-less chat, so this session was NOT started: the codex app-server protocol has no way to remove its tools. Choose a Claude model for Chat.",
    );
    expect(fixture.chatUnavailable).toBe(
      "Chat is unavailable. This build has no chat folder configured.",
    );
  });
});

describe("context usage derivations", () => {
  const first = { used: 80, size: 200, percent: 40 };
  const next = { used: 125, size: 200, percent: 63,
    autoCompact: true, autoCompactThreshold: 180 };

  it("reports remaining window and signed change only from complete snapshots", () => {
    expect(contextWindowRemaining(first)).toBe(120);
    expect(contextWindowRemaining({ used: 250, size: 200, percent: 100 })).toBe(0);
    expect(contextWindowRemaining({ percent: null, compacting: true })).toBeUndefined();
    expect(contextUsageChange(first, next)).toBe(45);
    expect(contextUsageChange(undefined, next)).toBeUndefined();
    expect(contextUsageChange(first,
      { used: Number.NaN, size: 200, percent: 0 })).toBeUndefined();
  });

  it("reports distance to an enabled provider threshold without correcting it", () => {
    expect(contextAutoCompactRemaining(next)).toBe(55);
    expect(contextAutoCompactRemaining({ ...next, used: 190 })).toBe(0);
    expect(contextAutoCompactRemaining({ ...next, autoCompact: false })).toBeUndefined();
    expect(contextAutoCompactRemaining({
      ...next, autoCompactThreshold: undefined,
    })).toBeUndefined();
  });
});

describe("formatContextTokens", () => {
  it("uses a short token label and refuses junk", () => {
    expect(formatContextTokens(12)).toBe("12");
    expect(formatContextTokens(1500)).toBe("1.5k");
    expect(formatContextTokens(12_400)).toBe("12k");
    expect(formatContextTokens(-3)).toBe("");
    expect(formatContextTokens(Number.NaN)).toBe("");
  });
});
