import { readFileSync } from "node:fs";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { ProviderAccountsView } from "../shared/provider-accounts";
import { CornerNotices } from "./components/CornerNotices";
import { ProviderAccountsPanel } from "./components/ProviderAccountsSection";
import { SettingsPage, type SettingsPageProps } from "./components/SettingsPage";
import { ThreadOpinionCards } from "./components/ThreadOpinionCards";
import {
  PRIVACY_FOOT, PRIVACY_GROUP_LABEL, PRIVACY_PAGE_INTRO, PRIVACY_ROWS, PRIVACY_SUMMARY_BODY,
  PRIVACY_SUMMARY_DONE, PRIVACY_SUMMARY_ID, PRIVACY_SUMMARY_OPEN, PRIVACY_SUMMARY_TITLE,
  privacySummarySeen, showPrivacySummary,
} from "./privacy";
import { ACCOUNT_TERMS_NOTE, accountGroupsView } from "./provider-accounts-view";
import {
  DEFAULT_APP_PREFERENCES, normalizeAppPreferences, parseAppPreferences, serializeAppPreferences,
} from "./settings";
import { limitOffersAccount } from "./thread-opinion";

const props = (overrides: Partial<SettingsPageProps> = {}): SettingsPageProps => ({
  appearance: { preference: "system", resolved: "dark" },
  onAppearanceChange: vi.fn(),
  preferences: { ...DEFAULT_APP_PREFERENCES },
  onPreferencesChange: vi.fn(),
  engineHealth: [],
  onRefreshProviders: vi.fn(),
  customizationApi: {
    getDefaultDirectory: vi.fn(async () => "/work"),
    listCustomization: vi.fn(async (root: string) => ({ root, entries: [], truncated: false })),
  },
  onClose: vi.fn(),
  ...overrides,
});

const escaped = (text: string): string => text.replaceAll("'", "&#x27;");
const privacyPage = (html: string): string =>
  html.slice(html.indexOf("-privacy\" class=\"settings-section\""), html.indexOf(PRIVACY_FOOT.slice(0, 20)) + 400);

describe("Settings › Privacy (owner, 2026-11-21)", () => {
  it("says what leaves this Mac in the owner's words", () => {
    expect(PRIVACY_PAGE_INTRO).toBe("Interface has no analytics, tracking or crash reports. This is everything that leaves this Mac on its own, and where to change it.");
    expect(PRIVACY_GROUP_LABEL).toBe("What leaves this Mac");
    expect(PRIVACY_ROWS.map(row => [row.label, row.section])).toEqual([
      ["Thread names", "threads"], ["Pull requests", "threads"], ["Update checks", "updates"],
      ["Phone access", "remote"], ["Other tools' settings", "providers"], ["Your prompts", undefined],
    ]);
    const words = Object.fromEntries(PRIVACY_ROWS.map(row => [row.label, row.description]));
    expect(words["Pull requests"]).toBe("Interface runs gh with your GitHub account to find and follow a thread's pull request.");
    expect(words["Update checks"]).toBe("About every four hours Interface asks GitHub whether a newer version exists. Nothing about you is sent.");
    expect(words["Your prompts"]).toBe("Everything you send to an agent goes to that agent's provider, under your own account with them. Interface keeps no copy anywhere but this Mac.");
    expect(words["Thread names"]).toContain("Claude Haiku, through your Claude account");
    expect(words["Thread names"]).toContain("This happens for every provider's threads.");
    expect(words["Phone access"]).toMatch(/^Off until you turn it on\. While it is on, this Mac announces itself on your network/u);
    expect(words["Other tools' settings"]).toMatch(/Removing Interface leaves those entries behind\.$/u);
    expect(PRIVACY_FOOT).toBe("Scheduled prompts, pull-request watch and Away send prompts on your subscription while you are away, only after you set them up.");
  });

  it("pages in right after Updates and links each row to the page that controls it", () => {
    const html = renderToStaticMarkup(createElement(SettingsPage, props({
      updateState: { phase: "idle", source: "remote", currentVersion: "0.1.1", availableVersion: null, progress: null,
        lastCheckedAt: null, error: null, canCheck: true, canRestart: false },
      onCheckForUpdates: vi.fn(),
      threadSettings: { autoSettleDays: 3, settleOnMerge: true, generatedTitles: true, trackPullRequests: true },
      onThreadSettingsChange: vi.fn(),
      onRemoteAccess: vi.fn(),
    })));
    const rail = [...html.matchAll(/class="settings-nav-link"[^>]*>([^<]+)</gu)].map(match => match[1]);
    expect(rail.slice(rail.indexOf("Updates"), rail.indexOf("Updates") + 3))
      .toEqual(["Updates", "Privacy", "iPhone remote access"]);
    const page = privacyPage(html);
    expect(page).toContain(">Privacy</h2>");
    expect(page).toContain(escaped(PRIVACY_PAGE_INTRO));
    expect(page).toContain(`>${PRIVACY_GROUP_LABEL}</h3>`);
    for (const row of PRIVACY_ROWS) expect(page).toContain(escaped(row.description));
    expect([...page.matchAll(/class="settings-link" aria-label="Go to ([^"]+)"/gu)].map(match => match[1]))
      .toEqual(["Threads", "Threads", "Updates", "iPhone remote access", "Models &amp; providers"]);
    expect(page).toContain(`<p class="settings-foot">${escaped(PRIVACY_FOOT)}</p>`);
  });

  it("draws a row's link only when its page is in the menu", () => {
    const page = privacyPage(renderToStaticMarkup(createElement(SettingsPage, props())));
    expect([...page.matchAll(/aria-label="Go to ([^"]+)"/gu)].map(match => match[1]))
      .toEqual(["Models &amp; providers"]);
    for (const row of PRIVACY_ROWS) expect(page).toContain(`>${escaped(row.label)}</span>`);
  });
});

describe("the first-run privacy summary", () => {
  it("shows once per profile, until either answer", () => {
    expect(PRIVACY_SUMMARY_TITLE).toBe("What Interface sends, and where");
    expect(PRIVACY_SUMMARY_BODY).toBe("Interface has no analytics or tracking. On its own it names each new thread with Claude Haiku, follows pull requests with gh, and asks GitHub whether an update exists. Settings › Privacy has the details.");
    expect([PRIVACY_SUMMARY_OPEN, PRIVACY_SUMMARY_DONE]).toEqual(["Open Privacy", "Got it"]);
    const fresh = parseAppPreferences(null);
    expect(showPrivacySummary(fresh)).toBe(true);
    const seen = privacySummarySeen(fresh);
    expect(showPrivacySummary(seen)).toBe(false);
    // Survives a reload, and is stored only once true.
    expect(showPrivacySummary(parseAppPreferences(serializeAppPreferences(seen)))).toBe(false);
    expect(serializeAppPreferences(fresh)).not.toContain("privacySummarySeen");
    expect(normalizeAppPreferences({ privacySummarySeen: "yes" })).not.toHaveProperty("privacySummarySeen");
    expect(normalizeAppPreferences({ privacySummarySeen: false })).not.toHaveProperty("privacySummarySeen");
  });

  it("is a corner status card with Open Privacy and Got it, never a dialog", () => {
    const html = renderToStaticMarkup(createElement(CornerNotices, { items: [{
      id: PRIVACY_SUMMARY_ID, title: PRIVACY_SUMMARY_TITLE, message: PRIVACY_SUMMARY_BODY, tone: "status",
      action: { label: PRIVACY_SUMMARY_OPEN, onClick: () => undefined },
      dismissLabel: "Got it: hide the privacy summary", dismissText: PRIVACY_SUMMARY_DONE, onDismiss: () => undefined,
    }] }));
    expect(html).toContain('<div class="corner-notice is-status" role="status" data-corner-notice="privacy-summary">');
    expect(html).toContain(`<strong>${PRIVACY_SUMMARY_TITLE}</strong>`);
    expect(html).toContain('<button type="button" class="corner-notice-action">Open Privacy</button>'
      + '<button type="button" class="corner-notice-action corner-notice-dismiss" aria-label="Got it: hide the privacy summary">Got it</button>');
    expect(html).not.toMatch(/role="dialog"|aria-modal|autofocus/iu);
    // App wires it to the stored flag and to Settings › Privacy.
    const app = readFileSync(new URL("./App.tsx", import.meta.url), "utf8");
    expect(app).toContain('openSettingsAt("privacy")');
    expect(app).toContain("persistPreferences(privacySummarySeen(appPreferences))");
  });
});

describe("the account-terms note", () => {
  const limit = {
    message: "Claude Code hit its usage limit.", engine: "codex" as const, label: "Codex",
    also: [{ engine: "claude" as const, label: "Claude Code (Work)", account: "acct-1" }],
  };
  const opinions = (over: Partial<Parameters<typeof ThreadOpinionCards>[0]>) =>
    renderToStaticMarkup(createElement(ThreadOpinionCards, {
      cards: [], limit: null, pending: [], onAction: () => undefined, onSwitch: () => undefined, ...over }));

  it("goes under a thread's limit line only when it offers another account", () => {
    expect(ACCOUNT_TERMS_NOTE).toBe("Use only accounts you pay for. Each provider's terms still apply.");
    const html = opinions({ limit, onContinue: () => undefined });
    const note = `<p class="thread-opinion-line">${escaped(ACCOUNT_TERMS_NOTE)}</p>`;
    expect(html).toContain(note);
    // Outside the one-row limit line, after it.
    const row = html.slice(html.indexOf('class="thread-limit"'), html.indexOf(note));
    expect(row).toContain("Continue with Claude Code (Work)");
    // The note starts right after the limit line's own element closes.
    expect(row).not.toContain("<div");
    expect(row).toMatch(/<\/div>$/u);
    expect(opinions({ limit: { ...limit, also: [] }, onContinue: () => undefined })).not.toContain(note);
    expect(opinions({ limit })).not.toContain(note);
    expect(limitOffersAccount(null, true)).toBe(false);
    expect(limitOffersAccount({ ...limit, also: undefined, account: "acct-1" }, true)).toBe(true);
  });

  it("sits under Add account in Settings › Models & providers", () => {
    const fixture = JSON.parse(readFileSync(
      new URL("../fixtures/provider-accounts-v60.json", import.meta.url), "utf8")) as { view: ProviderAccountsView };
    const html = renderToStaticMarkup(createElement(ProviderAccountsPanel, {
      id: "accounts", open: true, group: accountGroupsView(fixture.view)[0]!, pending: false,
      onChange: () => undefined, mask: identity => identity }));
    expect(html).toContain(`Add Claude Code account</button><p class="settings-foot">${escaped(ACCOUNT_TERMS_NOTE)}</p>`);
  });
});
