import { readFileSync } from "node:fs";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { ProviderAccountsView } from "../../shared/provider-accounts";
import { accountGroupsView } from "../provider-accounts-view";
import { ProviderAccountsButton, ProviderAccountsPanel } from "./ProviderAccountsSection";

const fixture = JSON.parse(readFileSync(
  new URL("../../fixtures/provider-accounts-v60.json", import.meta.url), "utf8",
)) as { view: ProviderAccountsView };

const groups = accountGroupsView(fixture.view);
const render = (index = 0, over: Partial<Parameters<typeof ProviderAccountsPanel>[0]> = {}): string =>
  renderToStaticMarkup(createElement(ProviderAccountsPanel, {
    id: "accounts", open: true, group: groups[index]!, pending: false,
    onChange: () => undefined, onSignIn: () => undefined, mask: identity => `masked:${identity}`, ...over,
  }));

describe("Settings → Providers' accounts", () => {
  it("lists one provider's accounts with their status, the Default badge and the row actions", () => {
    const html = render();
    expect(html).toContain('aria-label="Claude Code accounts"');
    const row = (id: string): string => {
      const start = html.indexOf(`data-account="${id}"`);
      return html.slice(start, html.indexOf("</li>", start));
    };
    const inherited = row("default");
    expect(inherited).toContain("Signed in as masked:me@example.test");
    // Signed in already, so there is nothing to sign in to (owner, 2026-11-15).
    expect(inherited).not.toContain(">Sign in</button>");
    expect(inherited).toContain(">Set as default</button>");
    expect(inherited).not.toContain("Remove");
    expect(inherited).not.toContain("Rename");
    const work = row("acct-0123456789ab");
    expect(work).toContain('class="settings-account-badge">Default</span>');
    expect(work).toContain("Not signed in");
    expect(work).toContain(">Sign in</button>");
    expect(work).toContain(">Rename</button>");
    expect(work).toContain(">Remove…</button>");
    expect(work).not.toContain("Set as default");
    expect(html).toContain(">Add Claude Code account</button>");
    expect(render(1)).toContain("Not checked");
  });

  it("opens under its row from a bordered Accounts menu button", () => {
    expect(render(0, { open: false })).toMatch(/^<div id="accounts" class="settings-accounts" hidden=""/u);
    const button = renderToStaticMarkup(createElement(ProviderAccountsButton, {
      label: "Claude Code", open: false, controls: "accounts", onToggle: () => undefined }));
    expect(button).toContain('class="settings-button" aria-expanded="false" aria-controls="accounts" aria-label="Claude Code accounts">Accounts');
  });
});
