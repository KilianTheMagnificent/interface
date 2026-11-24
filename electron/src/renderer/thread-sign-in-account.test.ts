import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const source = readFileSync(fileURLToPath(new URL("./App.tsx", import.meta.url)), "utf8");

describe("a thread's Sign in (protocol 60)", () => {
  it("signs in the account the thread runs on, not the provider's default account", () => {
    const start = source.indexOf("signInFor: key =>");
    expect(start).toBeGreaterThan(-1);
    const end = source.indexOf("openStartedThread:", start);
    const handler = source.slice(start, end);
    // The thread's own account (absent = the inherited Default) goes to the
    // account sign-in; a provider without accounts keeps the plain sign-in.
    expect(handler).toContain("openAccountSignIn(engine, threadIndexRef.current.get(key)?.account ?? \"default\")");
    expect(handler).toContain("accountGroupFor(providerAccounts.viewRef.current, engine) === undefined");
  });
});
