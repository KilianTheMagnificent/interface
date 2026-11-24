import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { ThreadSecretCard } from "./components/ThreadSecretCard";
import { ThreadSecretsSettings } from "./components/ThreadSecretsSettings";
import { heldSecretRows, SECRET_CARD_NOTE, secretExpiryText } from "./thread-secrets";

/* The private secret card (protocol 67, T3 section 12): who asks, for what and
   why, a password field, Give and Decline, docked above the composer; and
   Settings' list of held secrets by name and thread with Forget. Rust keeps
   every rule; these pin the words and what the renderer never holds. */

const request = { id: "secret-1", name: "STRIPE_TEST_KEY", reason: "To run the payment tests.", agent: "Claude" };
const noGive = async () => null;
const noop = (): void => undefined;

describe("the private secret card", () => {
  it("says who asks for which secret and why, with a password field, Give and Decline", () => {
    const html = renderToStaticMarkup(createElement(ThreadSecretCard, { request, onGive: noGive, onDecline: noop }));
    expect(html).toContain('role="alert" aria-label="Claude asks for a secret" data-thread-secret="secret-1"');
    expect(html).toContain('data-voice-guard="secret"');
    expect(html).toContain("Claude asks for <code>STRIPE_TEST_KEY</code>: To run the payment tests.");
    expect(html).toMatch(/<input [^>]*type="password"[^>]*autoComplete="off"|<input [^>]*type="password"[^>]*autocomplete="off"/u);
    expect(html).toContain('value=""');
    expect(html).toMatch(/<button type="submit" class="thread-secret-btn is-primary" disabled="">Give<\/button>/u);
    expect(html).toContain(">Decline</button>");
    expect(html).toContain(SECRET_CARD_NOTE.replace("'", "&#x27;"));
    expect(SECRET_CARD_NOTE).toContain("never into the conversation");
  });
});

describe("Settings → Threads → Secrets", () => {
  it("lists each held secret by name and thread, with when it goes and Forget", () => {
    const now = 1_000_000;
    const rows = heldSecretRows({ version: 1, secrets: [
      { key: "claude#a", name: "STRIPE_TEST_KEY", expiresAt: now + 23 * 3_600_000 + 5 },
      { key: "codex#b", name: "DB_PASSWORD", expiresAt: now + 40 * 60_000 },
    ] }, key => (key === "claude#a" ? "Payments" : "Migrations"), now);
    expect(rows.map(row => [row.name, row.threadTitle, row.expires])).toEqual([
      ["STRIPE_TEST_KEY", "Payments", "Removed in 23 h"],
      ["DB_PASSWORD", "Migrations", "Removed in 40 min"],
    ]);
    expect(secretExpiryText(now, now)).toBe("Removed soon");
    expect(heldSecretRows(null, key => key, now)).toEqual([]);
    const html = renderToStaticMarkup(createElement(ThreadSecretsSettings, {
      rows, locked: false, error: null, onForget: noop,
    }));
    expect(html).toContain("STRIPE_TEST_KEY · Payments");
    expect(html.match(/>Forget<\/button>/gu)).toHaveLength(2);
    expect(renderToStaticMarkup(createElement(ThreadSecretsSettings, {
      rows: [], locked: false, error: null, onForget: noop,
    }))).toContain("None held.");
  });
});
