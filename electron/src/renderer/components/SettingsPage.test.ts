import { readFileSync } from "node:fs";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { CustomizationEntry, EngineProbe, TerminalLaunchProfile } from "../../shared/ipc";
import type { UpdateState } from "../../shared/updater";
import { DEFAULT_APP_PREFERENCES } from "../settings";
import {
  CustomizationSection,
  SETTINGS_PAGE_CSS,
  NOTIFICATION_ROWS,
  SETTINGS_SHORTCUTS,
  SettingsPage,
  maskProviderIdentity,
  readCustomizationInventory,
  settingsPageKeyAction,
  threadSettleDayOptions,
  type CustomizationApi,
  type CustomizationReadSink,
  type SettingsPageProps,
} from "./SettingsPage";
import { initialThemeTab, THEME_MOTION_OPTIONS } from "./SettingsAppearance";
import { promptSubmitKeyAction } from "../prompt-submit";
import { APPROVAL_ALLOW_KEYS, APPROVAL_DENY_KEYS } from "../approval-keys";

const providers: EngineProbe[] = [
  {
    engine: "claude",
    ok: true,
    version: "2.1.0",
    info: {
      binaryPath: "/usr/local/bin/claude",
      version: "2.1.0",
      loginIdentity: null,
      capabilities: {
        streaming: true,
        approvals: true,
        resume: true,
        fork: true,
        cloudSessions: false,
        models: "dynamic",
        effortLevels: ["high"],
        permissionModes: ["default"],
      },
      modelCatalog: [
        { value: "one", displayName: "Model One", supportsEffort: false, effortLevels: [] },
        { value: "two", displayName: "Model Two", supportsEffort: false, effortLevels: [] },
        { value: "three", displayName: "Model Three", supportsEffort: false, effortLevels: [] },
        { value: "four", displayName: "Model Four", supportsEffort: false, effortLevels: [] },
        { value: "five", displayName: "Model Five", supportsEffort: false, effortLevels: [] },
      ],
    },
  },
  { engine: "codex", ok: false, error: "Codex CLI is not installed" },
];

const props = (overrides: Partial<SettingsPageProps> = {}): SettingsPageProps => ({
  appearance: { preference: "system", resolved: "dark" },
  onAppearanceChange: vi.fn(),
  preferences: { ...DEFAULT_APP_PREFERENCES },
  onPreferencesChange: vi.fn(),
  engineHealth: providers,
  onRefreshProviders: vi.fn(),
  customizationApi: {
    getDefaultDirectory: vi.fn(async () => "/work"),
    listCustomization: vi.fn(async (root: string) => ({ root, entries: [], truncated: false })),
  },
  onClose: vi.fn(),
  ...overrides,
});

const updates = (overrides: Partial<UpdateState> = {}): UpdateState => ({
  phase: "idle",
  source: "remote",
  currentVersion: "0.1.1",
  availableVersion: null,
  progress: null,
  lastCheckedAt: null,
  error: null,
  canCheck: true,
  canRestart: false,
  ...overrides,
});

describe("SettingsPage keyboard contract", () => {
  it("closes only for Escape so controls keep their own keyboard behavior", () => {
    expect(settingsPageKeyAction("Escape")).toBe("close");
    expect(settingsPageKeyAction("Enter")).toBeNull();
    expect(settingsPageKeyAction("Tab")).toBeNull();
  });
});

/* After the owner's BridgeMind settings reference (2026-09-14), the rail pages
   between sections instead of spying on a long scroll. Every section stays
   mounted so drafts, open disclosures and the remote-access poll survive a page
   switch; only the chosen one is shown. */
describe("the settings rail pages between sections", () => {
  const sectionTags = (html: string) =>
    html.match(/<section id="[^"]+-(general|providers|customization|updates|shortcuts)"[^>]*>/g) ?? [];

  it("shows Appearance alone and keeps the other sections mounted but hidden", () => {
    const tags = sectionTags(renderToStaticMarkup(createElement(SettingsPage, props())));
    expect(tags).toHaveLength(4);
    const shown = tags.filter(tag => !tag.includes('hidden=""'));
    expect(shown).toHaveLength(1);
    expect(shown[0]).toMatch(/-general"/);
  });

  it("opens straight onto Updates when a restartable update is ready", () => {
    const tags = sectionTags(renderToStaticMarkup(createElement(SettingsPage, props({
      updateState: updates({ phase: "ready", availableVersion: "0.2.0", canCheck: false, canRestart: true }),
      onRestartToUpdate: vi.fn(),
    }))));
    expect(tags).toHaveLength(5);
    expect(tags.filter(tag => !tag.includes('hidden=""'))[0]).toMatch(/-updates"/);
  });

  // Review A4 (2026-10-14): the sidebar's "N providers need setup" alert
  // opens here, not on Appearance.
  it("opens on the section it is asked for", () => {
    const tags = sectionTags(renderToStaticMarkup(createElement(SettingsPage, props({
      initialSection: "providers" }))));
    expect(tags.filter(tag => !tag.includes('hidden=""'))[0]).toMatch(/-providers"/);
  });

  /* Owner, 2026-11-15: Settings fills the window and its menu takes the
     sidebar's place, so the version the sidebar showed sits at its foot. */
  it("ends the menu with the version and notes the pages that want you", () => {
    const html = renderToStaticMarkup(createElement(SettingsPage, props({
      engineHealth: [...providers, { engine: "acp:grok", ok: false, error: "Grok timed out" }],
      updateState: updates({ phase: "ready", currentVersion: "0.3.178", availableVersion: "0.3.179",
        canCheck: false, canRestart: true }),
      onRestartToUpdate: vi.fn(),
    })));
    const menu = html.slice(html.indexOf('<nav class="settings-menu"'), html.indexOf("</nav>"));
    expect(menu).toMatch(/<span class="settings-menu-version">Interface 0\.3\.178<\/span>$/u);
    expect(menu).toContain('>Models &amp; providers<span class="settings-nav-badge is-attention">2 need setup</span></button>');
    expect(menu).toContain('>Updates<span class="settings-nav-badge is-ready">1 ready</span></button>');
    // Without an updater there is no version to show, and nothing is guessed.
    const bare = renderToStaticMarkup(createElement(SettingsPage, props({ engineHealth: [providers[0]] })));
    expect(bare).not.toContain('class="settings-menu-version"');
    expect(bare).not.toContain('class="settings-nav-badge');
  });

  it("pages by state, not by scroll position", () => {
    const source = SettingsPage.toString();
    expect(source).not.toMatch(/addEventListener\(\s*"scroll"/);
    expect(source).not.toContain("scrollIntoView");
    expect(SETTINGS_PAGE_CSS).toContain(".settings-section[hidden] { display:none; }");
  });
});

/* Threads (owner, 2026-10-06): Rust holds four thread settings; the page
   draws them as one card and sends the whole object back on every change. */
describe("Settings → Threads", () => {
  const settings = { autoSettleDays: 3, settleOnMerge: true, generatedTitles: false, trackPullRequests: true };

  it("is absent from the rail without a thread host", () => {
    const html = renderToStaticMarkup(createElement(SettingsPage, props()));
    expect(html).not.toContain(">Threads</button>");
  });

  it("pages in after Agent customization with its four settings", () => {
    const html = renderToStaticMarkup(createElement(SettingsPage, props({
      threadSettings: settings, onThreadSettingsChange: vi.fn(),
      threadSchedules: { rows: [], pending: false, error: null, onEnable: vi.fn(), onDelete: vi.fn() },
      threadSecrets: { rows: [], pending: false, error: null, onShown: vi.fn(), onForget: vi.fn() } })));
    const rail = [...html.matchAll(/class="settings-nav-link"[^>]*>([^<]+)</g)].map(match => match[1]);
    expect(rail).toEqual(["Appearance", "Models &amp; providers", "Agent customization",
      "Threads", "Notifications", "Privacy", "Shortcuts"]);
    expect(html).toMatch(/<section id="[^"]+-threads" class="settings-section" hidden=""/);
    // Three groups, in the mockup's words (owner, 2026-11-15).
    const threads = html.slice(html.indexOf("-threads-title"), html.indexOf(">Notifications</h2>"));
    expect([...threads.matchAll(/<h3 class="settings-group-label">([^<]+)<\/h3>/gu)].map(match => match[1]))
      .toEqual(["Tidying up", "Agents working together", "Kept for your threads"]);
    for (const label of ["Settle quiet threads", "Settle when the pull request merges", "Follow pull requests on GitHub",
      "Name threads automatically"]) {
      expect(html).toContain(`>${label}</span>`);
    }
    expect(html).toContain("Uses a little of your Claude plan.");
    // A row is its words and its control: no icon tile leads it.
    expect(threads).not.toContain("settings-row-mark");
    expect(threads).toContain('<div class="settings-row"><div class="settings-row-copy"><span id=');
  });

  it("says Claude Haiku 5.5 names a thread from your first message, on the plan", () => {
    const html = renderToStaticMarkup(createElement(SettingsPage, props({
      threadSettings: { ...settings, generatedTitles: true }, onThreadSettingsChange: vi.fn() })));
    const row = html.slice(html.indexOf(">Name threads automatically</span>"));
    expect(row).toContain("Claude Haiku 5.5 names a thread from your first message. Uses a little of your Claude plan.");
    expect(row).not.toContain("after its first run");
    expect(row.slice(0, row.indexOf("</label>"))).toContain('checked=""');
  });

  it("lets agents start and read other threads by default, and says running sessions keep the tools", () => {
    const html = renderToStaticMarkup(createElement(SettingsPage, props({
      threadSettings: settings, onThreadSettingsChange: vi.fn() })));
    expect(html).toContain(">Let agents start helper threads</span>");
    expect(html).toContain("Running agents keep this until they restart.");
    const team = html.slice(html.indexOf(">Let agents start helper threads</span>"));
    // Absent in Rust's settings means on (protocol 57); only false turns it off.
    expect(team.slice(0, team.indexOf("</label>"))).toContain('checked=""');
    const off = renderToStaticMarkup(createElement(SettingsPage, props({
      threadSettings: { ...settings, teamTools: false }, onThreadSettingsChange: vi.fn() })));
    const offRow = off.slice(off.indexOf(">Let agents start helper threads</span>"));
    expect(offRow.slice(0, offRow.indexOf("</label>"))).not.toContain('checked=""');
  });

  it("has Pane tools without asking, off unless Rust says it is on, and saves the switch", () => {
    const html = renderToStaticMarkup(createElement(SettingsPage, props({
      threadSettings: settings, onThreadSettingsChange: vi.fn() })));
    const row = html.slice(html.indexOf(">Pane tools without asking</span>"));
    expect(row).toContain("Let an agent in one pane type into another pane, or start one, without asking you first.");
    // Absent in Rust's settings means off (protocol 71): the tools ask.
    expect(row.slice(0, row.indexOf("</label>"))).not.toContain('checked=""');
    const on = renderToStaticMarkup(createElement(SettingsPage, props({
      threadSettings: { ...settings, paneToolsWithoutAsking: true }, onThreadSettingsChange: vi.fn() })));
    const onRow = on.slice(on.indexOf(">Pane tools without asking</span>"));
    expect(onRow.slice(0, onRow.indexOf("</label>"))).toContain('checked=""');
  });

  it("has Outside agents, off unless Rust says otherwise, with each tool's setup behind How to connect one", () => {
    const html = renderToStaticMarkup(createElement(SettingsPage, props({
      threadSettings: settings, onThreadSettingsChange: vi.fn() })));
    const row = html.slice(html.indexOf(">Agents outside Interface</span>"));
    const copy = row.slice(0, row.indexOf('class="settings-row-action"'));
    expect(copy).toContain("Lets Claude Code or Codex in any terminal read or start your threads.");
    // The commands stay, out of sight until the link opens them (owner, 2026-11-15).
    expect(copy).toMatch(/<button type="button" class="settings-link" aria-expanded="false" aria-controls="([^"]+)">How to connect one<\/button>[\s\S]*<div id="\1" class="settings-row-more" hidden="">/u);
    expect(copy).toContain("through iface mcp");
    expect(copy).toContain("Claude Code: <code>claude mcp add interface -- iface mcp</code>");
    expect(copy).toContain("Codex: <code>codex mcp add interface -- iface mcp</code>");
    const action = row.slice(row.indexOf('class="settings-row-action"'), row.indexOf("</section>"));
    expect(action).toContain(">Off<");
    const full = renderToStaticMarkup(createElement(SettingsPage, props({
      threadSettings: { ...settings, outsideAgents: "full" }, onThreadSettingsChange: vi.fn() })));
    const fullRow = full.slice(full.indexOf(">Agents outside Interface</span>"));
    expect(fullRow.slice(fullRow.indexOf('class="settings-row-action"'), fullRow.indexOf("</section>")))
      .toContain(">Can start and message threads<");
  });

  it("offers Never and day counts, keeping a count Rust holds that is not one of them", () => {
    expect(threadSettleDayOptions(3).map(option => option.label))
      .toEqual(["Never", "1 day", "3 days", "7 days", "14 days", "30 days"]);
    expect(threadSettleDayOptions(3).map(option => option.value)).toEqual(["0", "1", "3", "7", "14", "30"]);
    expect(threadSettleDayOptions(5).map(option => option.label))
      .toEqual(["Never", "1 day", "3 days", "5 days", "7 days", "14 days", "30 days"]);
  });

  it("holds every control until Rust's settings arrive, and while a change is saving", () => {
    const unread = renderToStaticMarkup(createElement(SettingsPage, props({
      threadSettings: null, onThreadSettingsChange: vi.fn() })));
    expect(unread).toContain("Reading thread settings…");
    const section = unread.slice(unread.indexOf("-threads-title"), unread.indexOf("Shortcut reference</h2>"));
    expect(section.match(/<input type="checkbox" role="switch"[^>]*disabled=""/g) ?? []).toHaveLength(5);
    const saving = renderToStaticMarkup(createElement(SettingsPage, props({
      threadSettings: settings, threadSettingsPending: true, threadSettingsError: "threads-unavailable",
      onThreadSettingsChange: vi.fn() })));
    expect(saving).toContain('aria-busy="true"');
    expect(saving).toContain("Thread settings could not be saved: threads-unavailable");
    // Settle on merge rests while pull requests are not tracked.
    const untracked = renderToStaticMarkup(createElement(SettingsPage, props({
      threadSettings: { ...settings, trackPullRequests: false }, onThreadSettingsChange: vi.fn() })));
    expect(untracked.match(/<input type="checkbox" role="switch"[^>]*disabled=""/g) ?? []).toHaveLength(1);
  });
});

/* Sessions have always inherited rules files, skills, hooks, commands and MCP
   servers from the project and from ~/.claude, and nothing in the app said so.
   This section is the answer, and it is read-only on purpose: Interface does not
   own those files. */
describe("Agent customization inventory", () => {
  const inventory: CustomizationEntry[] = [
    { kind: "rules", name: "CLAUDE.md", detail: "Project rules", scope: "project",
      sourcePath: "/work/app/CLAUDE.md" },
    { kind: "rules", name: "CLAUDE.md", detail: "Universal instructions", scope: "user",
      sourcePath: "/home/me/.claude/CLAUDE.md" },
    { kind: "skill", name: "deploy-app", detail: "Ship the packaged app.", scope: "project",
      sourcePath: "/work/app/.claude/skills/deploy/SKILL.md" },
    { kind: "command", name: "ship", detail: "", scope: "user",
      sourcePath: "/home/me/.claude/commands/ship.md" },
    { kind: "hook", name: "PreToolUse", detail: "Bash · ./scripts/guard.sh", scope: "project",
      sourcePath: "/work/app/.claude/settings.json" },
    { kind: "mcp", name: "github", detail: "stdio", scope: "project",
      sourcePath: "/work/app/.mcp.json" },
  ];

  const section = (overrides: Partial<Parameters<typeof CustomizationSection>[0]> = {}) =>
    renderToStaticMarkup(createElement(CustomizationSection, {
      id: "settings-customization", root: "/work/app", entries: inventory,
      onRefresh: vi.fn(), ...overrides,
    }));

  it("groups every kind and shows each entry's scope and defining file", () => {
    const html = section();
    expect(html).toContain("Rules files");
    expect(html).toContain("Skills");
    expect(html).toContain("Commands");
    expect(html).toContain("Hooks");
    expect(html).toContain("MCP servers");
    expect(html).toContain("deploy-app");
    expect(html).toContain("Ship the packaged app.");
    expect(html).toContain("Bash · ./scripts/guard.sh");
    expect(html).toContain('title="/work/app/.claude/skills/deploy/SKILL.md"');
    expect(html).toContain('title="/home/me/.claude/CLAUDE.md"');
    expect((html.match(/class="settings-scope-badge">Project</g) ?? [])).toHaveLength(4);
    expect((html.match(/class="settings-scope-badge">User</g) ?? [])).toHaveLength(2);
    expect(html).toContain('aria-label="Refresh agent customization"');
    // Read-only: no control here may claim to change what an agent loads.
    expect(html).not.toContain('role="switch"');
    expect(html).not.toContain("Disable");
  });

  it("names the folder it found nothing in rather than showing a blank card", () => {
    const html = section({ entries: [] });
    expect(html).toContain("Nothing found in app or in your own settings.");
    expect(section({ entries: [], root: "" })).toContain("No project folder is selected.");
    expect(section({ entries: [], loading: true }))
      .toContain("Reading project and user customization…");
  });

  it("surfaces a parse failure beside the file and admits when the list is cut short", () => {
    const html = section({
      entries: [...inventory, { kind: "hook", name: "settings.local.json", detail: "",
        scope: "project", sourcePath: "/work/app/.claude/settings.local.json",
        error: "Could not be parsed as JSON." }],
      truncated: true,
    });
    expect(html).toContain("Could not be parsed as JSON.");
    expect(html).toContain('class="settings-inventory-error"');
    expect(html).toContain("stop at 200 entries per kind");

    const failed = section({ entries: [], error: "Choose an open project first." });
    expect(failed).toContain('role="alert"');
    expect(failed).toContain("Customization could not be read: Choose an open project first.");
  });

  /* Owner, 2026-11-15: folder paths sit behind a link, and a home folder
     reads as ~. */
  it("names the project by its folder and keeps the full folders behind Show folders", () => {
    const html = section({ root: "/Users/me/work/app", userRoot: "/Users/me/.claude", onProviderChange: vi.fn() });
    const copy = html.slice(html.indexOf('class="settings-section-copy"'), html.indexOf("</p>"));
    expect(copy).not.toContain("/Users/me");
    expect(html).toContain("In app and your own Claude settings.");
    expect(html).toMatch(/aria-controls="([^"]+)">Show folders<\/button>[\s\S]*<div id="\1" class="settings-row-more" hidden="">/u);
    expect(html).toContain('<code title="/Users/me/work/app">~/work/app</code>');
    expect(html).toContain('<code title="/Users/me/.claude">~/.claude</code>');
    expect(html).toContain('aria-label="Customization provider: Claude"');
    expect(html).not.toContain("form-field");
    expect(section({ entries: [], root: "/Users/me/empty" })).toContain("Nothing found in empty or in your own settings.");
  });

  it("re-reads the inventory when asked", () => {
    const source = SettingsPage.toString();
    expect(source).toContain("readCustomizationInventory(customizationApi, customizationRoot");
    expect(/setCustomizationRevision\(\s*\(?revision\)?\s*=>\s*revision \+ 1\)/.test(source))
      .toBe(true);
    expect(source).toContain("customizationRevision");
  });

  describe("readCustomizationInventory", () => {
    const recorder = () => {
      const calls: string[] = [];
      const sink: CustomizationReadSink = {
        root: label => { calls.push(`root ${label}`); },
        inventory: value => { calls.push(`inventory ${value.root} ${value.entries.length.toString()}`); },
        failed: message => { calls.push(`failed ${message}`); },
        settled: () => { calls.push("settled"); },
      };
      return { calls, sink };
    };
    const api = (overrides: Partial<CustomizationApi> = {}): CustomizationApi => ({
      getDefaultDirectory: vi.fn(async () => "/home/me/Projects"),
      listCustomization: vi.fn(async (root: string) =>
        ({ root: `${root}/canonical`, entries: inventory, truncated: false })),
      ...overrides,
    });

    it("reads the requested project without asking for the default folder", async () => {
      const reads = api();
      const { calls, sink } = recorder();
      await readCustomizationInventory(reads, "/work/app", () => false, sink);
      expect(reads.getDefaultDirectory).not.toHaveBeenCalled();
      expect(reads.listCustomization).toHaveBeenCalledWith("/work/app");
      expect(calls).toEqual(["root /work/app", "inventory /work/app/canonical 6", "settled"]);
    });

    it("falls back to the default folder when no project is given", async () => {
      for (const root of [undefined, ""]) {
        const reads = api();
        const { calls, sink } = recorder();
        await readCustomizationInventory(reads, root, () => false, sink);
        expect(reads.listCustomization).toHaveBeenCalledWith("/home/me/Projects");
        expect(calls).toEqual(["root /home/me/Projects",
          "inventory /home/me/Projects/canonical 6", "settled"]);
      }
    });

    it("reports a refused read in its own words", async () => {
      const { calls, sink } = recorder();
      await readCustomizationInventory(api({
        listCustomization: vi.fn(async () => { throw new Error("Choose an open project first."); }),
      }), "/work/app", () => false, sink);
      expect(calls).toEqual(["root /work/app", "failed Choose an open project first.", "settled"]);
      const failedDefault = recorder();
      await readCustomizationInventory(api({
        getDefaultDirectory: vi.fn(() => Promise.reject("no default folder")),
      }), undefined, () => false, failedDefault.sink);
      expect(failedDefault.calls).toEqual(["failed no default folder", "settled"]);
    });

    it("reports nothing once the page has moved on", async () => {
      let cancelled = false;
      const { calls, sink } = recorder();
      const read = readCustomizationInventory(api({
        getDefaultDirectory: vi.fn(async () => { cancelled = true; return "/home/me/Projects"; }),
      }), undefined, () => cancelled, sink);
      await read;
      expect(calls).toEqual([]);
      cancelled = false;
      const late = recorder();
      await readCustomizationInventory(api({
        listCustomization: vi.fn(async () => { cancelled = true; throw new Error("late"); }),
      }), "/work/app", () => cancelled, late.sink);
      // The folder was reported before the read was abandoned; nothing after.
      expect(late.calls).toEqual(["root /work/app"]);
    });
  });

  it("sits in the rail and the page between providers and updates", () => {
    const html = renderToStaticMarkup(createElement(SettingsPage, props({
      updateState: updates(),
      onCheckForUpdates: vi.fn(),
    })));
    expect(html).toContain(">Agent customization</button>");
    expect(html).toMatch(/id="[^"]+-customization"/);
    const providers = html.indexOf("Models &amp; providers</h2>");
    const customization = html.indexOf("Agent customization</h2>");
    const updatesHeading = html.indexOf("Updates</h2>");
    expect(providers).toBeGreaterThan(-1);
    expect(customization).toBeGreaterThan(providers);
    expect(updatesHeading).toBeGreaterThan(customization);
  });
});

describe("SettingsPage server rendering", () => {
  it("keeps exact-argv custom ACP management behind Custom providers' button", () => {
    const html = renderToStaticMarkup(createElement(SettingsPage, props({
      acpProfiles: [{ slug: "gemini", displayName: "Gemini",
        executablePath: "/usr/local/bin/gemini", args: ["agent", "stdio"] }],
      onSaveAcpProfiles: vi.fn(),
    })));
    expect(html).toContain(">Custom providers</span>");
    expect(html).toContain("1 added: Gemini.");
    // Today's editor, mounted (its draft survives) but out of sight until opened.
    expect(html).toMatch(/aria-expanded="false" aria-controls="([^"]+)">Edit<\/button>[\s\S]*<div id="\1" class="settings-row-more" hidden="">/u);
    expect(html).toContain("Custom ACP providers");
    expect(html).toContain("exact argument array");
    expect(html).toContain("gemini");
    const none = renderToStaticMarkup(createElement(SettingsPage, props({ onSaveAcpProfiles: vi.fn() })));
    expect(none).toContain("None added. For connecting a provider that is not listed above.");
    expect(none).toContain(">Add provider</button>");
  });

  it("puts the actual custom-provider error in the Custom providers row", () => {
    const html = renderToStaticMarkup(createElement(SettingsPage, props({
      acpProfilesError: "Executable not found: gemini",
      onSaveAcpProfiles: vi.fn(),
    })));
    expect(html).toContain('<span class="is-error" title="Executable not found: gemini">Executable not found: gemini</span>');
    expect(html).not.toContain("Custom providers need attention");
  });

  it("renders a labelled, keyboard-closeable page with navigable sections", () => {
    const html = renderToStaticMarkup(createElement(SettingsPage, props()));
    expect(html).toContain('class="settings-page"');
    expect(html).toContain('tabindex="-1"');
    expect(html).toContain('aria-keyshortcuts="Escape"');
    // The page draws NO header bar of its own. It sits under the app's 44px
    // toolbar, which carries the ✕. Its menu takes the sidebar's place
    // (owner, 2026-11-15): Back to app, the heading, the pages.
    expect(html).not.toContain("settings-header");
    expect(html).not.toContain('aria-label="Close settings"');
    expect(html).toContain('aria-label="Settings sections"');
    expect(html).toMatch(/aria-labelledby="([^" ]+)-title"/);
    expect(html).toMatch(/<nav class="settings-menu" aria-label="Settings sections"><button type="button" class="settings-back"><span class="codicon codicon-chevron-left" aria-hidden="true"><\/span>Back to app<\/button><h1 id="[^"]+-title" class="settings-title">Settings<\/h1><ul class="settings-nav-list">/);
    expect(html).toMatch(/\.settings-title \{[^}]*font-size:var\(--text-h1\)/);
    // Each page is one centred column, at most 760px wide.
    expect(SETTINGS_PAGE_CSS).toContain(".settings-column { width:100%; max-width:760px; margin:0 auto; }");
    // The column does not jump sideways between a page that scrolls and one that does not.
    expect(SETTINGS_PAGE_CSS).toMatch(/\.settings-content \{[^}]*overflow:auto;[^}]*scrollbar-gutter:stable;/u);
    // No setting row leads with an icon tile (owner, 2026-11-15), and groups
    // are named in sentence case, each one small heading over one card.
    expect(html).not.toContain("settings-row-mark");
    expect(html).toContain('<h3 class="settings-group-label">Theme</h3><div class="settings-card">');
    expect(html).toContain('<h3 class="settings-group-label">Text and style</h3>');
    expect(SETTINGS_PAGE_CSS).not.toMatch(/\.settings-inventory-group-title \{[^}]*text-transform/);
    expect(html).toContain('aria-current="page"');
    expect((html.match(/class="settings-nav-link"/g) ?? [])).toHaveLength(6);
    // The menu is words only: no glyph beside a page's name.
    expect(html).not.toContain("settings-nav-glyph");
    for (const glyph of ["◐", "Aa", "●", "◆", "↻"]) expect(html).not.toContain(`>${glyph}<`);
    // One glyph, one meaning: the gauge is Usage, the lightbulb is a skill, the
    // sidebar glyph toggles the sidebar and the external arrow opens a link.
    for (const glyph of ["lightbulb", "dashboard", "layout-sidebar-left", "editor-layout", "link-external"]) {
      expect(html).not.toContain(`codicon-${glyph}`);
    }
    expect(html).toMatch(/id="([^" ]+)-providers"[^>]*aria-labelledby=/);
    expect(html).toContain("Appearance");
    expect(html).toContain("Colours, backgrounds and text size for the whole window.");
    expect(html).toContain("Serif replies");
    expect(html).not.toContain("Anthropic");
    expect(html).not.toContain("Use a worktree by default");
    expect(html).toContain("Models &amp; providers");
    expect(html).toContain(">Shortcuts</button>");
    expect(html).not.toContain("Make Interface yours");
    expect(html).not.toContain("Tune the workspace once");
    expect(SETTINGS_PAGE_CSS).not.toContain(".settings-intro");
  });

  it("picks themes from swatches and switches, never a native select", () => {
    const html = renderToStaticMarkup(createElement(SettingsPage, props({
      appearance: { preference: "dark", resolved: "dark" },
    })));
    // Every current theme is a radio swatch in its own colours; the pinned one is checked.
    expect(html.match(/class="settings-theme-swatch[" ]/gu)).toHaveLength(10);
    expect(html).toMatch(/role="radio" aria-checked="true" class="settings-theme-swatch is-selected"[^>]*title="Warm graphite frames/u);
    expect(html).toContain('aria-label="Light themes"');
    expect(html).toContain('aria-label="Dark themes"');
    for (const name of ["Porcelain", "Clay Light", "Clay Dark", "Carbon", "Navy", "Ice"]) {
      expect(html).toContain(`<span class="settings-theme-name">${name}</span>`);
    }
    for (const retired of ["Sauteed", "Warm Editorial", "Nightshade", "Grok Build", "Hermes"]) {
      expect(html).not.toContain(retired);
    }
    // Following macOS is a switch named by its row, with no word beside it
    // (owner, 2026-11-15); off here because the preference pins a theme.
    const follow = (markup: string): string =>
      /<input type="checkbox" role="switch"[^>]*aria-labelledby="[^"]+-theme"[^>]*\/>/u.exec(markup)?.[0] ?? "";
    expect(html).toContain(">Follow macOS light and dark</span>");
    expect(follow(html)).not.toBe("");
    expect(follow(html)).not.toContain('checked=""');
    expect(html).not.toContain("<select");
    expect(html).not.toContain("<option");
    const system = renderToStaticMarkup(createElement(SettingsPage, props({
      appearance: { preference: "system", resolved: "light" },
    })));
    expect(follow(system)).toContain('checked=""');
    expect(system).toMatch(/aria-checked="true" class="settings-theme-swatch is-selected"[^>]*title="Pale porcelain/u);
  });

  /* Owner, 2026-11-15: "I'm not seeing where the option for the live/active
     background themes are". The host says which files paint a backdrop that
     can move; their swatches carry a play mark. */
  it("marks the theme files whose backdrop can move", () => {
    const customThemes = [
      { id: "custom:Aurora", name: "Aurora", kind: "dark", revision: "a", preview: { "--theme-look": "moving" } },
      { id: "custom:Dusk", name: "Dusk", kind: "dark", revision: "b", preview: { "--theme-look": "still" } },
      { id: "custom:Moss", name: "Moss", kind: "dark", revision: "c", preview: { "--theme-panel": "#121713" } },
    ] as const;
    const html = renderToStaticMarkup(createElement(SettingsPage, props({
      appearance: { preference: "dark", resolved: "dark", customThemes: [...customThemes] },
    })));
    const swatch = (name: string): string =>
      new RegExp(`<button[^>]*title="${name}\\.css[^>]*>.*?</button>`, "u").exec(html)?.[0] ?? "";
    expect(swatch("Aurora")).toContain('<span class="settings-theme-moving codicon codicon-play"></span>');
    expect(swatch("Aurora")).toContain('aria-label="Aurora, moving backdrop"');
    expect(swatch("Aurora")).toContain("Moving backdrop.");
    for (const plain of ["Dusk", "Moss"]) {
      expect(swatch(plain), plain).not.toBe("");
      expect(swatch(plain), plain).not.toContain("settings-theme-moving");
      expect(swatch(plain), plain).not.toContain("aria-label=");
    }
    // Built-in themes never carry it.
    expect(html.match(/settings-theme-moving codicon/gu)).toHaveLength(1);
  });

  /* Owner, 2026-11-15, after the play mark shipped nowhere he looked: "I'm
     still not seeing the live backgrounds in the appearance settings". They
     were unlabelled among the other files at the foot of the picker. */
  it("leads the picker with the theme files that move, under Live backgrounds", () => {
    const customThemes = [
      { id: "custom:Apple", name: "Apple", kind: "light", revision: "a", preview: { "--theme-panel": "#ffffff" } },
      { id: "custom:Aurora", name: "Aurora", kind: "dark", revision: "b", preview: { "--theme-look": "moving" } },
      { id: "custom:Dusk", name: "Dusk", kind: "dark", revision: "c", preview: { "--theme-look": "still" } },
      { id: "custom:Orbit", name: "Orbit", kind: "dark", revision: "d", preview: { "--theme-look": "moving" } },
    ] as const;
    const html = renderToStaticMarkup(createElement(SettingsPage, props({
      appearance: { preference: "system", resolved: "dark", customThemes: [...customThemes] },
    })));
    const group = (name: string): string =>
      new RegExp(`<div class="settings-theme-swatches" role="radiogroup" aria-label="${name}">.*?</button></div>`, "u")
        .exec(html)?.[0] ?? "";
    const live = group("Themes with a live background");
    expect(html).toContain(">Live backgrounds</span>");
    expect(live.match(/title="(\w+)\.css/gu)).toEqual(['title="Aurora.css', 'title="Orbit.css']);
    // First in the picker, before the built-in light and dark themes.
    expect(html.indexOf("Themes with a live background")).toBeLessThan(html.indexOf('aria-label="Light themes"'));
    // The files that do not move stay under Your themes, and no file is listed twice.
    expect(group("Your theme files").match(/title="(\w+)\.css/gu)).toEqual(['title="Apple.css', 'title="Dusk.css']);
    expect(html.match(/\.css in ~/gu)).toHaveLength(4);
    // Follow macOS can pair with them under the same name.
    expect(html).toContain("Themes Follow macOS uses");
    // With no file that moves there is no such block.
    const plain = renderToStaticMarkup(createElement(SettingsPage, props({
      appearance: { preference: "dark", resolved: "dark", customThemes: [customThemes[0], customThemes[2]] },
    })));
    expect(plain).not.toContain("Live backgrounds");
    expect(plain).toContain('aria-label="Your theme files"');
  });

  /* Owner, 2026-11-15: ONE Motion menu in the Live backgrounds block, there
     whenever a theme file moves, in place of the two switches that appeared
     only after a look was picked. */
  it("sets how live backgrounds move with one Motion menu", () => {
    const appearance = { preference: "dark", resolved: "dark", customThemes: [
      { id: "custom:Aurora", name: "Aurora", kind: "dark", revision: "b", preview: { "--theme-look": "moving" } },
    ] } as const;
    const motion = (themeMotion?: "on" | "plugged" | "off"): string => renderToStaticMarkup(createElement(SettingsPage, props({
      appearance: { ...appearance, customThemes: [...appearance.customThemes] },
      preferences: { ...DEFAULT_APP_PREFERENCES, ...themeMotion === undefined ? {} : { themeMotion } },
    })));
    expect(motion()).toContain('aria-label="Motion: On"');
    expect(motion("plugged")).toContain('aria-label="Motion: Still on battery"');
    expect(motion("off")).toContain('aria-label="Motion: Off"');
    expect(THEME_MOTION_OPTIONS.map(option => `${option.value}:${option.label}`))
      .toEqual(["on:On", "plugged:Still on battery", "off:Off"]);
    expect(motion()).not.toContain("Moving backdrops");
    expect(motion()).not.toContain("Still on battery</span>");
  });

  it("shows one group of themes at a time, opening on the tab that holds the selected one", () => {
    const customThemes = [
      { id: "custom:Moss", name: "Moss", kind: "dark", revision: "a", preview: { "--theme-panel": "#121713" } },
      { id: "custom:Tide", name: "Tide", kind: "light", revision: "b", preview: { "--theme-look": "moving" } },
    ] as const;
    const shownTab = (preference: string, resolved = preference): string => {
      const html = renderToStaticMarkup(createElement(SettingsPage, props({
        appearance: { preference, resolved, customThemes: [...customThemes] } as SettingsPageProps["appearance"],
      })));
      const tabs = [...html.matchAll(/<button type="button" role="tab"[^>]*aria-selected="(true|false)"[^>]*>([^<]+)<\/button>/gu)];
      expect(tabs.map(match => match[2])).toEqual(["Light", "Dark", "Your themes · 1"]);
      const panels = [...html.matchAll(/<div role="tabpanel"[^>]*?( hidden="")?>/gu)];
      expect(panels.filter(match => match[1] === undefined)).toHaveLength(1);
      return tabs.find(match => match[1] === "true")?.[2] ?? "";
    };
    expect(shownTab("dark")).toBe("Dark");
    expect(shownTab("light")).toBe("Light");
    expect(shownTab("custom:Moss")).toBe("Your themes · 1");
    // A live background is not in a tab: its light or dark kind's tab opens.
    expect(shownTab("custom:Tide")).toBe("Light");
    expect(shownTab("system", "dark")).toBe("Dark");
    expect(initialThemeTab("custom:Gone", [])).toBe("dark");
  });

  it("lists theme files under Yours and pairs Follow macOS with any theme", () => {
    const customThemes = [
      { id: "custom:Moss", name: "Moss", kind: "dark", revision: "a", preview: { "--theme-panel": "#121713" } },
      { id: "custom:Huge", name: "Huge", kind: "dark", revision: "b", preview: {}, problem: "The file is larger than 1 MB." },
    ] as const;
    const pinned = renderToStaticMarkup(createElement(SettingsPage, props({
      appearance: { preference: "custom:Moss", resolved: "custom:Moss", customThemes: [...customThemes] },
    })));
    expect(pinned).toContain('aria-label="Your theme files"');
    expect(pinned.match(/class="settings-theme-swatch[" ]/gu)).toHaveLength(12);
    expect(pinned).toMatch(/aria-checked="true" class="settings-theme-swatch is-selected"[^>]*title="Moss.css in ~\/.config\/interface\/themes"[^>]*--swatch-chrome:#121713/u);
    // A file that cannot be painted is listed, disabled, with the reason.
    expect(pinned).toMatch(/title="The file is larger than 1 MB." disabled=""/u);
    expect(pinned).toContain("~/.config/interface/themes");
    expect(pinned).not.toContain("Themes Follow macOS uses");
    const following = renderToStaticMarkup(createElement(SettingsPage, props({
      appearance: {
        preference: "system", resolved: "custom:Moss", customThemes: [...customThemes],
        systemThemes: { version: 1, light: "solarized-light", dark: "custom:Moss" },
      },
      onSystemThemesChange: vi.fn(),
    })));
    expect(following).toContain('aria-label="Themes Follow macOS uses"');
    expect(following).toContain("Theme when macOS is light");
    expect(following).toContain("Theme when macOS is dark");
    expect(following).toMatch(/aria-checked="true" class="settings-theme-swatch is-selected"[^>]*title="Moss.css/u);
    // No theme files: no Yours group at all.
    const none = renderToStaticMarkup(createElement(SettingsPage, props()));
    expect(none).not.toContain("Your theme files");
  });

  it.each(["terminal", "clean"] as const)("keeps native settings with a stored %s pane preference", paneStyle => {
    const html = renderToStaticMarkup(createElement(SettingsPage, props({
      appearance: { preference: "dark", resolved: "dark", panePresentation: { version: 1, paneStyle } },
      preferences: { ...DEFAULT_APP_PREFERENCES, cleanInput: "native" },
    })));
    expect(html).not.toContain("Pane appearance");
    expect(html).not.toContain("Clean input");
    expect(html).toContain("Terminal text size");
  });

  it("has a Notifications page with sound, finished and needs-input switches (review D5)", () => {
    const html = renderToStaticMarkup(createElement(SettingsPage, props({
      preferences: { ...DEFAULT_APP_PREFERENCES, notifySound: false },
    })));
    expect(html).toContain(">Notifications</button>");
    expect(html).toContain("When a run finishes");
    expect(html).toContain("When an agent needs input");
    expect(html).toContain("Play the macOS notification sound.");
    const notifications = html.slice(html.indexOf(">Notifications</h2>"));
    const switches = notifications.match(/<input type="checkbox" role="switch"[^>]*>/gu) ?? [];
    expect(switches.length).toBeGreaterThanOrEqual(3);
    expect(switches.slice(0, 3).map(input => input.includes('checked=""')))
      .toEqual([true, true, false]);
    expect(NOTIFICATION_ROWS.map(row => row.key))
      .toEqual(["notifyFinished", "notifyNeedsInput", "notifySound"]);
  });

  it("does not expose a global worktree default", () => {
    const html = renderToStaticMarkup(createElement(SettingsPage, props({
      preferences: {
        uiFont: "serif",
        uiChrome: "refined",
        lastCliTheme: "grok-build",
        lastWorkspaceTheme: "system",
        notifySound: true,
        notifyFinished: true,
        notifyNeedsInput: true,
      },
    })));
    const switches = html.match(/<button[^>]*role="switch"[^>]*>/g) ?? [];
    expect(switches).toHaveLength(0);
    expect(html).not.toContain("Use a worktree by default");
  });

  /* Owner, 2026-11-15: what needs setting up comes first, each with its
     whole sentence; then what is ready, each with the account it runs on. */
  it("puts the providers that need setup first, then the ready ones, diagnostics behind Details", () => {
    const html = renderToStaticMarkup(createElement(SettingsPage, props({
      engineHealth: [...providers, { engine: "acp:grok", ok: true, version: "1.0",
        info: { ...providers[0].info!, displayName: "Grok", auth: "signedOut", modelCatalog: [] } }],
      onSignIn: vi.fn(),
    })));
    expect(html).toContain("1 of 3 providers are ready to use.");
    const groups = [...html.matchAll(/<h3 class="settings-group-label( is-attention)?">([^<]+)<\/h3>/gu)]
      .map(match => `${match[2]}${match[1] === undefined ? "" : "!"}`);
    expect(groups.slice(groups.indexOf("Needs setup!"), groups.indexOf("Needs setup!") + 2)).toEqual(["Needs setup!", "Ready"]);
    const needs = html.slice(html.indexOf(">Needs setup</h3>"), html.indexOf(">Ready</h3>"));
    expect(needs).toContain('data-provider="codex"');
    expect(needs).toContain('data-provider="acp:grok"');
    // The whole sentence, never cut off, and the row's one fix as the primary button.
    expect(needs).toContain('class="settings-row-description is-error">Codex CLI is not installed');
    expect(needs).toContain('<button type="button" class="settings-button is-primary">Sign in to Grok</button>');
    expect(needs).not.toContain("Sign in to Codex");
    const ready = html.slice(html.indexOf(">Ready</h3>"));
    expect(ready).toContain('data-provider="claude"');
    expect(ready).toContain("5 models");
    expect(ready).toContain('<span class="settings-provider-dot is-ready" aria-hidden="true"></span>Ready');
    expect(html).toContain('data-provider-logo="claude"');
    // Version, models, access modes and effort levels stay, behind Details.
    expect(html).toMatch(/aria-expanded="false" aria-controls="([^"]+)">Details<\/button>/u);
    expect(html).toContain("Version 2.1.0");
    expect(html).toContain("Model One");
    expect(html).toContain("Model Four");
    expect(html).not.toContain("Model Five</span>");
    expect(html).toContain("+1 more");
    expect(html).toContain("Authentication managed by the provider CLI.");
    expect(html).toContain('aria-label="Claude Code access modes"');
    expect(html).toContain("Manual");
    expect(html).toContain('aria-label="Claude Code effort levels"');
    expect(html).toContain(">Refresh</button>");
    expect(html).not.toContain("<details");
  });

  it("opens each provider's accounts from its own Accounts menu", () => {
    const view = JSON.parse(readFileSync(
      new URL("../../fixtures/provider-accounts-v60.json", import.meta.url), "utf8")) as {
      view: NonNullable<SettingsPageProps["providerAccounts"]>["view"] };
    const html = renderToStaticMarkup(createElement(SettingsPage, props({
      providerAccounts: { view: view.view, pending: false, checking: false, error: null, notice: null },
      onProviderAccountsChange: vi.fn(), onAccountSignIn: vi.fn(),
    })));
    const claude = html.slice(html.indexOf('data-provider="claude"'), html.indexOf(">Other accounts</h3>"));
    expect(claude).toMatch(/aria-expanded="false" aria-controls="([^"]+)" aria-label="Claude Code accounts">Accounts/u);
    expect(claude).toMatch(/<div id="[^"]+" class="settings-accounts" hidden="" data-account-provider="claude">/u);
    expect(claude).toContain(">Add Claude Code account</button>");
    // Grok has accounts but no probe here: its accounts still have a row.
    expect(html).toContain(">Other accounts</h3>");
    expect(html).toContain('aria-label="Grok accounts">Accounts');
  });

  it("masks provider account identities by default for safer screen sharing", () => {
    expect(maskProviderIdentity("andreromero@example.com")).toBe("an•••@example.com");
    expect(maskProviderIdentity("local-account")).toBe("lo•••nt");
    expect(maskProviderIdentity("grok")).toBe("grok");
    const first = providers[0];
    if (first.info === undefined) throw new Error("provider fixture must include info");
    const html = renderToStaticMarkup(createElement(SettingsPage, props({
      engineHealth: [{ ...first, info: { ...first.info,
        loginIdentity: "andreromero@example.com" } }],
    })));
    expect(html).toContain("Signed in as an•••@example.com");
    expect(html).not.toContain("andreromero@example.com");
  });

  it("announces pending refresh and both controlled error surfaces", () => {
    const html = renderToStaticMarkup(createElement(SettingsPage, props({
      providersRefreshing: true,
      appearancePending: true,
      appearanceError: "Theme service offline",
      preferencesError: "Preferences are active but not saved",
      providerRefreshError: "Probe timed out",
    })));
    expect(html).toContain(">Refreshing…</button>");
    expect(html).toContain('aria-busy="true"');
    expect(html).toContain('class="settings-refresh-note" role="status" aria-live="polite"');
    expect(html).toContain("Refreshing provider health and model catalogs…");
    expect((html.match(/role="alert"/g) ?? [])).toHaveLength(3);
    expect(html).toContain("Theme service offline");
    expect(html).toContain("Preferences are active but not saved");
    expect(html).toContain("Probe timed out");
  });

  it("keeps the provider status live region mounted and announces completion", () => {
    const html = renderToStaticMarkup(createElement(SettingsPage, props()));
    expect(html).toContain('class="settings-refresh-note" role="status" aria-live="polite"');
    expect(html).toContain("Provider status is up to date.");
  });

  it("shows a controlled update center without adding chrome when updater state is absent", async () => {
    const hidden = renderToStaticMarkup(createElement(SettingsPage, props()));
    expect((hidden.match(/class="settings-nav-link"/g) ?? [])).toHaveLength(6);
    expect(hidden).not.toContain("Check now");

    const html = renderToStaticMarkup(createElement(SettingsPage, props({
      updateState: updates({
        phase: "idle",
        lastCheckedAt: "2026-08-13T22:00:00.000Z",
      }),
      onCheckForUpdates: vi.fn(),
    })));
    expect((html.match(/class="settings-nav-link"/g) ?? [])).toHaveLength(7);
    expect(html).toContain(">Updates</button>");
    // One card in plain words for the state it is in (owner, 2026-11-15).
    expect(html).toContain("You are on Interface 0.1.1");
    expect(html).toContain("Check now to look for a newer version.");
    expect(html).toContain("Last checked");
    expect(html).toContain('dateTime="2026-08-13T22:00:00.000Z"');
    expect(html).toContain(">Check now</button>");

    const renderUpdates = (): string => renderToStaticMarkup(createElement(SettingsPage, props({
      initialSection: "updates",
      updateState: updates({
        phase: "idle",
        currentVersion: "0.3.166",
        lastCheckedAt: "2026-08-13T22:00:00.000Z",
      }),
      onCheckForUpdates: vi.fn(),
    })));
    // What's new loads in its own chunk the first time Updates opens; the
    // developer log stays closed behind its link.
    renderUpdates();
    await import("./UpdateLogPanel");
    await new Promise(resolve => setTimeout(resolve, 0));
    const openUpdates = renderUpdates();
    expect(openUpdates).toContain(">What’s new</h3>");
    expect(openUpdates).toMatch(/aria-expanded="false">Developer log<\/button>/u);
    expect(openUpdates).not.toContain('aria-label="Update log"');
  });

  /* Owner, 2026-11-15: a status card for each real state, and What's new in
     plain sentences; hashes, branches and test names only in the log. */
  it("says in one card which state the updater is in", () => {
    const card = (state: Partial<UpdateState>, extra: Partial<SettingsPageProps> = {}): string => {
      const html = renderToStaticMarkup(createElement(SettingsPage, props({
        initialSection: "updates", updateState: updates({ currentVersion: "0.3.178", ...state }), ...extra })));
      return /<div class="settings-update-card[^"]*" data-update-state="[^"]+">.*?<\/div><\/div>/u.exec(html)?.[0] ?? "";
    };
    const ready = card({ phase: "ready", availableVersion: "0.3.179", canCheck: false, canRestart: true },
      { onRestartToUpdate: vi.fn() });
    expect(ready).toContain("Interface 0.3.179 is ready");
    expect(ready).toContain("You are on 0.3.178; your open panes come back after the restart.");
    expect(ready).toContain('<button type="button" class="settings-button is-quiet">Later</button>');
    expect(ready).toContain('<button type="button" class="settings-button is-primary">Restart to update</button>');
    expect(card({ phase: "up-to-date" })).toContain("You are up to date");
    expect(card({ phase: "checking" })).toContain("Checking for updates…");
    const none = card({ phase: "disabled", canCheck: false });
    expect(none).toContain("This build has no update channel");
    expect(none).toContain("as in Dev and the test app");
    expect(none).not.toContain("<button");
  });

  it("announces bounded download progress and disables checks while busy", () => {
    const html = renderToStaticMarkup(createElement(SettingsPage, props({
      updateState: updates({
        phase: "downloading",
        availableVersion: "0.2.0",
        progress: 47.8,
        canCheck: true,
      }),
      onCheckForUpdates: vi.fn(),
      onRestartToUpdate: vi.fn(),
    })));
    expect(html).toContain("Downloading Interface 0.2.0");
    expect(html).toContain("48% downloaded");
    expect(html).toMatch(/role="progressbar"[^>]+aria-valuemin="0"[^>]+aria-valuemax="100"[^>]+aria-valuenow="48"/);
    expect(html).toContain('aria-valuetext="48% downloaded"');
    expect(html).toMatch(/<button type="button" class="settings-link" disabled=""[^>]*>Check now<\/button>/);
    expect(html).not.toContain("Restart to update");
  });

  it("offers restart only for a ready, restartable update and surfaces updater errors", () => {
    const available = renderToStaticMarkup(createElement(SettingsPage, props({
      updateState: updates({
        phase: "available",
        availableVersion: "0.2.0",
        canRestart: true,
      }),
      onRestartToUpdate: vi.fn(),
    })));
    expect(available).not.toContain("Restart to update");

    const ready = renderToStaticMarkup(createElement(SettingsPage, props({
      updateState: updates({
        phase: "ready",
        availableVersion: "0.2.0",
        canCheck: false,
        canRestart: true,
      }),
      onCheckForUpdates: vi.fn(),
      onRestartToUpdate: vi.fn(),
    })));
    expect(ready).toContain("Interface 0.2.0 is ready");
    expect(ready).toContain("Restart to update");
    // It opens on Updates, and the menu says an update is waiting.
    expect(ready).toContain('aria-current="page">Updates<span class="settings-nav-badge is-ready">1 ready</span>');

    const failed = renderToStaticMarkup(createElement(SettingsPage, props({
      updateState: updates({ phase: "error", error: "Signature check failed" }),
    })));
    expect(failed).toContain("Update check failed");
    expect(failed).toContain('role="alert"');
    expect(failed).toContain("Updates could not be completed: Signature check failed");
  });

  it("renders the audited shortcut registry as semantic keyboard keys", () => {
    const html = renderToStaticMarkup(createElement(SettingsPage, props()));
    expect(SETTINGS_SHORTCUTS).toHaveLength(28);
    expect(html).toContain("Open or close this settings page.");
    expect(html).toContain("Filter threads, workspaces, or tasks on the current surface.");
    expect(html).toContain('aria-label="Shortcut: ⌘ plus ⇧ plus P"');
    expect(html).toContain('<kbd class="settings-shortcut-key" aria-hidden="true">⌘</kbd>');
    expect(html).toContain("Find in conversation");
    expect(html).toContain("New line");
    expect(html).toContain('aria-label="Shortcut: ↩"');
    // The key cluster is a labelled role="img", so the label is announced at all.
    expect(html).toContain('class="settings-shortcut-keys" role="img" aria-label="Shortcut: ↩"');
    // The terminal chords App and the terminal center bind (terminal-keys.ts).
    for (const chord of ["⌃ plus `", "⌘ plus D", "⌘ plus ⇧ plus D", "⌘ plus ⇧ plus [",
      "⌘ plus ⇧ plus ]", "⌃ plus ⇧ plus Tab", "⌘ plus ⌥ plus ←/→", "Space"]) {
      expect(html).toContain(`aria-label="Shortcut: ${chord}"`);
    }
    // ⌘↩ opens a pane and ⌘W closes the selected one; the window took ⇧⌘W.
    for (const chord of ["⌘ plus ↩", "⌘ plus W", "⇧ plus ⌘ plus W"]) {
      expect(html).toContain(`aria-label="Shortcut: ${chord}"`);
    }
    // The Threads chords (thread-inbox.ts threadChord); ⌘⇧P stays the palette's.
    for (const label of ["Previous thread", "Next thread", "Settle thread", "Pin thread", "Undo in Threads",
      "Thread changes"]) {
      expect(SETTINGS_SHORTCUTS.some(row => row.label === label)).toBe(true);
    }
    expect(html).toContain('aria-label="Shortcut: ⌘ plus ⇧ plus S"');
    expect(html).toContain('aria-label="Shortcut: ⌘ plus ⌥ plus P"');
    // Both Tab chords fall through to a focused shell (terminal-keys.ts shellOwnsChord).
    expect(html.match(/a focused terminal pane keeps this chord for the shell\./g)).toHaveLength(2);
    expect(promptSubmitKeyAction({ key: "Enter", shiftKey: false })).toBe("submit");
    expect(promptSubmitKeyAction({ key: "Enter", shiftKey: true })).toBe("newline");
  });

  /* The reference documents keys the app actually binds. Quick open (⌘P),
     Search project (⌘⇧F) and Save file (⌘S) have no handler anywhere — ⌘P is
     swallowed only so Electron's Print never fires — so the page must not
     advertise them. */
  it("omits chords no handler implements", () => {
    const html = renderToStaticMarkup(createElement(SettingsPage, props()));
    for (const label of ["Quick open", "Search project", "Save file"]) {
      expect(SETTINGS_SHORTCUTS.some(row => row.label === label)).toBe(false);
      expect(html).not.toContain(label);
    }
    expect(html).not.toContain('aria-label="Shortcut: ⌘ plus P"');
    expect(html).not.toContain('aria-label="Shortcut: ⌘ plus ⇧ plus F"');
    expect(html).not.toContain('aria-label="Shortcut: ⌘ plus S"');
  });

  /* The reference and the chip printed on the approval button are ONE fact.
     They read the same two arrays, so the page cannot document a key the button
     does not print, or keep documenting one after it is rebound. */
  it("documents the approval chord from the same constants the buttons print", () => {
    const allow = SETTINGS_SHORTCUTS.find(row => row.label === "Allow request");
    const deny = SETTINGS_SHORTCUTS.find(row => row.label === "Deny request");
    expect(allow?.keys).toEqual([...APPROVAL_ALLOW_KEYS]);
    expect(deny?.keys).toEqual([...APPROVAL_DENY_KEYS]);

    const html = renderToStaticMarkup(createElement(SettingsPage, props()));
    expect(html).toContain('aria-label="Shortcut: ⌘ plus ↩"');
    expect(html).toContain('aria-label="Shortcut: ⌘ plus ⌫"');
    // The scope is part of the copy: a chord that fires while you are typing
    // would answer a request nobody read.
    expect(html).toContain("Never while you are typing.");
    // Deny opens the reason field first in a conversation, and the page says
    // so rather than promising an immediate refusal.
    expect(html).toContain("opens the reason field first");
  });

  it("ships a token-only responsive layout and explicit focus treatment", () => {
    expect(SETTINGS_PAGE_CSS).toContain("var(--editor-bg)");
    expect(SETTINGS_PAGE_CSS).toContain("var(--surface)");
    expect(SETTINGS_PAGE_CSS).toContain("var(--focus)");
    expect(SETTINGS_PAGE_CSS).toContain("@media (max-width:840px)");
    expect(SETTINGS_PAGE_CSS).toContain("@media (max-width:480px)");
    expect(SETTINGS_PAGE_CSS).toContain("prefers-reduced-motion:reduce");
    expect(SETTINGS_PAGE_CSS).toContain(".settings-update-progress-track");
    expect(SETTINGS_PAGE_CSS).toContain(".settings-button.is-primary");
    expect(SETTINGS_PAGE_CSS).toContain("summary, [tabindex]");
    expect(SETTINGS_PAGE_CSS).toContain("grid-template-columns:240px minmax(0, 1fr)");
    expect(SETTINGS_PAGE_CSS).toContain("padding:0 10px");
    expect(SETTINGS_PAGE_CSS).toContain("-webkit-app-region:no-drag");
    expect(SETTINGS_PAGE_CSS).toContain("outline:var(--focus-ring-w) solid var(--focus)");
    expect(SETTINGS_PAGE_CSS).toContain("box-shadow:0 0 0 1px var(--focus-contrast)");
    expect(SETTINGS_PAGE_CSS).not.toMatch(/#[0-9a-f]{3,8}\b/i);
    expect(SETTINGS_PAGE_CSS).not.toContain("linear-gradient");
  });

  /* The type tokens declare an 11px floor and exactly two weights, and the
     page stays on the shared radius vocabulary. */
  it("stays on the declared type and radius ramps", () => {
    expect(SETTINGS_PAGE_CSS).not.toMatch(/font-weight:\s*500/);
    expect(SETTINGS_PAGE_CSS).not.toMatch(/font-size:\s*10px/);
    expect(SETTINGS_PAGE_CSS).toContain("border-radius:var(--radius-container)");
    expect(SETTINGS_PAGE_CSS).toContain("border-radius:var(--radius-control)");
    expect(SETTINGS_PAGE_CSS).not.toMatch(/border-radius:\s*\d+px/);
  });

  it("keeps a health probe in flight out of the attention hue", () => {
    // Amber says an agent is blocked on you. A provider being probed is a
    // process: the dot holds the idle grey until the probe answers, and the
    // label beside it says "Checking" in words.
    // The menu's "needs setup" note is amber (owner, 2026-11-15); a provider's
    // health dot never is.
    expect(SETTINGS_PAGE_CSS).not.toMatch(/\.settings-provider-dot[^{]*\{[^}]*--status-waiting/);
    expect(SETTINGS_PAGE_CSS).not.toContain(".settings-provider-dot.is-checking");
    expect(SETTINGS_PAGE_CSS).toContain(
      ".settings-provider-dot { width:7px; height:7px; background:var(--status-idle);");
    expect(SETTINGS_PAGE_CSS).toContain(
      ".settings-provider-dot.is-unavailable { background:var(--status-failed); }");
  });
});

it("renders independent enabled translucency sliders with acknowledged percentages", () => {
  const html = renderToStaticMarkup(createElement(SettingsPage, props({
    appearance: { preference: "dark", resolved: "dark", translucency: { version: 1, terminal: 75, sidebar: 30 } },
    onTranslucencyChange: vi.fn(),
  })));
  expect(html).toContain("Terminals</label>");
  expect(html).toContain("Sidebar</label>");
  const ranges = html.match(/<input[^>]+type="range"[^>]*>/g)!;
  expect(ranges).toHaveLength(2);
  expect(ranges[0]).toContain('value="75"');
  expect(ranges[1]).toContain('value="30"');
  for (const range of ranges) {
    expect(range).toContain('min="0"'); expect(range).toContain('max="100"');
    expect(range).not.toContain("disabled");
  }
  expect(html).toContain('aria-valuetext="75% translucency"');
});

describe("SettingsPage Terminal tools (review C4)", () => {
  it("shows a missing tool's install hint alone, its commands set as code", () => {
    const { profiles } = JSON.parse(readFileSync(
      new URL("../../fixtures/terminal-tools-v43.json", import.meta.url), "utf8")) as {
      profiles: TerminalLaunchProfile[];
    };
    const html = renderToStaticMarkup(createElement(SettingsPage, props({
      initialSection: "providers", terminalTools: profiles })));
    expect(html).toContain('data-terminal-tool="codex"');
    expect(html).toContain('<span class="settings-tool-missing">Install it with '
      + "<code>npm install -g @openai/codex</code>, then press Refresh.</span>");
    expect(html).toContain("Install OMP so <code>omp</code> runs in your login shell, then press Refresh.");
    // The pill says Not installed; the reason sentence is not repeated under it.
    expect(html).not.toContain("was not found in the login shell");
    expect(html).not.toMatch(/settings-tool-missing">[^<]*`/u);
    expect(SETTINGS_PAGE_CSS).toContain(
      ".settings-tool-missing code { color:var(--fg-muted); font-family:var(--font-mono); }");
  });

  it("says what changed in a CLI since the last provider check (protocol 51)", () => {
    const { profiles } = JSON.parse(readFileSync(
      new URL("../../fixtures/terminal-tool-change-v51.json", import.meta.url), "utf8")) as {
      profiles: TerminalLaunchProfile[];
    };
    const html = renderToStaticMarkup(createElement(SettingsPage, props({
      initialSection: "providers", terminalTools: profiles })));
    expect(html).toContain('<span class="settings-tool-missing" data-tool-change="">'
      + "Changed since last check: 2.1.300 → 2.1.301. What Interface depends on still matches.</span>");
    expect(html).toContain("<code>codex exec --help</code> no longer lists --ephemeral.");
    expect(html.match(/data-tool-change=""/gu)).toHaveLength(3);
  });
});
