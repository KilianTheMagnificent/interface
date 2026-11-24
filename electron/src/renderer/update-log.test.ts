import { readdirSync, readFileSync } from "node:fs";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import {
  DEFAULT_UPDATE_LOG,
  filterUpdateLogLandings,
  filterUpdateLogReleases,
  formatUpdateCompletedAt,
  parseUpdateLog,
} from "./update-log";
import { UpdateLogPanel } from "./components/UpdateLogPanel";

declare const __INTERFACE_UPDATE_LOG_MD__: string | undefined;

const SAMPLE_MARKDOWN = `# Interface Update Log

Complete history of production releases.

- **Production releases:** 2 (\`v0.3.165\` – \`v0.3.166\`)
- **Dev landings:** 3 (\`1\` unreleased on Dev)

## Unreleased on Dev

### Harden terminal pass-off

- **Completed:** 2026-10-21T14:48:47-05:00
- **Kind:** fix
- **Release:** Unreleased (on Dev)
- **Commit:** \`1c2a9b5\` (\`fix/pass-off-review\`)
- **Notes:** Passed-off panes launch for every profile and custom command
- **Commits:** fix(pass-off): quote custom shell commands; test(pass-off): cover profiles
- **Pins:** \`apps/interface-host/src/pane_mcp.rs::pass_off\`, \`electron/src/renderer/pass-off.test.ts::custom\`

## Production Releases

### v0.3.166 — 2026-10-21T09:17:39-05:00 (\`8f11382\`)

- **[feature]** \`2222222\` · 2026-10-21T08:52:32-05:00 · **Name Antigravity panes from the conversation title** (\`feat/antigravity-pane-titles\`)
  - Notes: Antigravity pane adopts its conversation title automatically
  - Pins: \`crates/interface-terminal/src/antigravity_names.rs::title\`
- **Left on Dev:** \`1c2a9b5\` Harden terminal pass-off

### v0.3.165 — 2026-10-20T18:00:00-05:00 (\`1111111\`)

- **[speed]** \`3333333\` · 2026-10-20T17:30:00-05:00 · **Cache terminal font metrics** (\`speed/font-metrics\`)
  - Notes: Startup terminal measurement runs once per font family

## All Dev Landings

### Harden terminal pass-off

- **Completed:** 2026-10-21T14:48:47-05:00
- **Kind:** fix
- **Release:** Unreleased (on Dev)
- **Commit:** \`1c2a9b5\` (\`fix/pass-off-review\`)
- **Notes:** Passed-off panes launch for every profile and custom command
- **Commits:** fix(pass-off): quote custom shell commands; test(pass-off): cover profiles
- **Pins:** \`apps/interface-host/src/pane_mcp.rs::pass_off\`, \`electron/src/renderer/pass-off.test.ts::custom\`

### Name Antigravity panes from the conversation title

- **Completed:** 2026-10-21T08:52:32-05:00
- **Kind:** feature
- **Release:** v0.3.166
- **Commit:** \`2222222\` (\`feat/antigravity-pane-titles\`)
- **Notes:** Antigravity pane adopts its conversation title automatically
- **Pins:** \`crates/interface-terminal/src/antigravity_names.rs::title\`

### Cache terminal font metrics

- **Completed:** 2026-10-20T17:30:00-05:00
- **Kind:** speed
- **Release:** v0.3.165
- **Commit:** \`3333333\` (\`speed/font-metrics\`)
- **Notes:** Startup terminal measurement runs once per font family
- **Supersedes:** \`0000000\` Earlier font cache
`;

describe("update-log parser and filters", () => {
  it("parses releases, release items, landings, notes, pins, and completion timestamps", () => {
    const parsed = parseUpdateLog(SAMPLE_MARKDOWN);
    expect(parsed.releases).toHaveLength(2);
    expect(parsed.landings).toHaveLength(3);
    expect(parsed.unreleasedCount).toBe(1);
    const unreleased = filterUpdateLogLandings(parsed.landings, {}, true);
    expect(unreleased).toHaveLength(1);
    expect(unreleased[0]?.name).toBe("Harden terminal pass-off");

    const latestRelease = parsed.releases[0]!;
    expect(latestRelease.version).toBe("0.3.166");
    expect(latestRelease.completedAt).toBe("2026-10-21T09:17:39-05:00");
    expect(latestRelease.sha).toBe("8f11382");
    expect(latestRelease.leftOnDev).toEqual(["`1c2a9b5` Harden terminal pass-off"]);
    expect(latestRelease.items).toHaveLength(1);
    expect(latestRelease.items[0]).toEqual({
      kind: "feature",
      sha: "2222222",
      completedAt: "2026-10-21T08:52:32-05:00",
      name: "Name Antigravity panes from the conversation title",
      branch: "feat/antigravity-pane-titles",
      notes: "Antigravity pane adopts its conversation title automatically",
      pins: ["crates/interface-terminal/src/antigravity_names.rs::title"],
    });

    const firstLanding = parsed.landings[0]!;
    expect(firstLanding).toEqual({
      name: "Harden terminal pass-off",
      completedAt: "2026-10-21T14:48:47-05:00",
      kind: "fix",
      release: null,
      sha: "1c2a9b5",
      branch: "fix/pass-off-review",
      notes: "Passed-off panes launch for every profile and custom command",
      commits: [
        "fix(pass-off): quote custom shell commands",
        "test(pass-off): cover profiles",
      ],
      pins: [
        "apps/interface-host/src/pane_mcp.rs::pass_off",
        "electron/src/renderer/pass-off.test.ts::custom",
      ],
      supersedes: [],
    });

    expect(parsed.landings[2]?.supersedes).toEqual(["`0000000` Earlier font cache"]);
  });

  it("filters releases and landings by search query and kind", () => {
    const parsed = parseUpdateLog(SAMPLE_MARKDOWN);
    expect(filterUpdateLogReleases(parsed.releases, { query: "antigravity", kind: "all" })).toHaveLength(1);
    expect(filterUpdateLogReleases(parsed.releases, { query: "", kind: "speed" })).toHaveLength(1);
    expect(filterUpdateLogReleases(parsed.releases, { query: "", kind: "fix" })).toHaveLength(0);

    expect(filterUpdateLogLandings(parsed.landings, { query: "pass-off", kind: "all" })).toHaveLength(1);
    expect(filterUpdateLogLandings(parsed.landings, { query: "", kind: "fix" })).toHaveLength(1);
    expect(filterUpdateLogLandings(parsed.landings, { query: "v0.3.166", kind: "all" })).toHaveLength(1);
  });

  it("formats valid completion timestamps and preserves invalid ones", () => {
    expect(formatUpdateCompletedAt("2026-10-21T14:48:47-05:00")).toContain("2026");
    expect(formatUpdateCompletedAt("not-a-timestamp")).toBe("not-a-timestamp");
  });

  // `cargo xtask` writes the log with `update_log::render_markdown`, whose
  // own test renders this same fixture byte for byte. The fixture is in both
  // suites' receipts; `docs/UPDATES.md` is in neither, so assertions about
  // the format belong here and not on the live log.
  it("parses the update log fixture the Rust renderer writes", () => {
    const parsed = parseUpdateLog(
      readFileSync(new URL("../../../docs/fixtures/update-log-v1.md", import.meta.url), "utf8"),
    );
    expect(parsed.releases.map(release => release.version)).toEqual(["0.3.166", "0.3.165", "0.3.164"]);
    expect(parsed.releases.map(release => release.sha)).toEqual(["8f11382", "1111111", "4444444"]);
    expect(parsed.releases[0]?.leftOnDev).toEqual(["`1c2a9b5` Harden terminal pass-off"]);
    expect(parsed.releases[0]?.items).toEqual([{
      kind: "feature",
      sha: "2222222",
      completedAt: "2026-10-21T08:52:32-05:00",
      name: "Name Antigravity panes from the conversation title",
      branch: "feat/antigravity-pane-titles",
      notes: "Antigravity pane adopts its conversation title automatically",
      pins: ["crates/interface-terminal/src/antigravity_names.rs::title"],
    }]);
    expect(parsed.releases[1]?.items[0]?.branch).toBeNull();
    expect(parsed.releases[1]?.items[0]?.notes).toBe("Startup terminal measurement runs once per font family");
    expect(parsed.releases[2]?.items).toEqual([]);

    expect(parsed.unreleasedCount).toBe(1);
    expect(parsed.landings.map(landing => [landing.name, landing.release, landing.kind])).toEqual([
      ["Harden terminal pass-off", null, "fix"],
      ["Name Antigravity panes from the conversation title", "0.3.166", "feature"],
      ["Cache terminal font metrics", "0.3.165", "speed"],
    ]);
    expect(parsed.landings[0]).toMatchObject({
      sha: "1c2a9b5",
      branch: "fix/pass-off-review",
      completedAt: "2026-10-21T14:48:47-05:00",
      notes: "Passed-off panes launch for every profile and custom command",
      commits: ["fix(pass-off): quote custom shell commands", "test(pass-off): cover profiles"],
      pins: [
        "apps/interface-host/src/pane_mcp.rs::pass_off",
        "electron/src/renderer/pass-off.test.ts::custom",
      ],
    });
    // A landing whose only commit repeats its notes has no Commits line.
    expect(parsed.landings[1]?.commits).toEqual([]);
    expect(parsed.landings[2]?.supersedes).toEqual(["`0000000` Earlier font cache"]);
  });

  it("embeds the repository update log, which parses", () => {
    expect(typeof __INTERFACE_UPDATE_LOG_MD__).toBe("string");
    expect(__INTERFACE_UPDATE_LOG_MD__).toMatch(/^# Interface Update Log\n/u);
    expect(DEFAULT_UPDATE_LOG.releases.length).toBeGreaterThan(0);
    expect(DEFAULT_UPDATE_LOG.landings.length).toBeGreaterThan(0);
  });

  it("renders UpdateLogPanel with release cards, completion timestamps, and notes", () => {
    const parsed = parseUpdateLog(SAMPLE_MARKDOWN);
    const html = renderToStaticMarkup(
      createElement(UpdateLogPanel, { log: parsed, currentVersion: "0.3.166" }),
    );
    expect(html).toContain("Update log");
    expect(html).toContain('data-update-release="0.3.166"');
    expect(html).toContain("v0.3.166 (Current)");
    expect(html).toContain("Name Antigravity panes from the conversation title");
    expect(html).toContain("Antigravity pane adopts its conversation title automatically");
    expect(html).toContain('dateTime="2026-10-21T09:17:39-05:00"');
    expect(html).toContain('aria-label="Filter update log"');
  });
});

describe("update log loading", () => {
  // The log is about 500 KB of text that only Settings -> Updates reads. It
  // stays out of the main chunk only while the panel is its sole value
  // importer and nothing imports the panel except through import().
  const root = new URL("./", import.meta.url);
  const sources = (readdirSync(root, { recursive: true }) as string[])
    .map(name => name.replaceAll("\\", "/"))
    .filter(name => /\.tsx?$/u.test(name) && !/\.test\.tsx?$/u.test(name) && !name.startsWith("fixtures/"))
    .map(name => ({ name, source: readFileSync(new URL(name, root), "utf8") }));
  const valueImporters = (module: string): string[] => sources
    .filter(({ source }) => [...source.matchAll(/^(?:import|export)\s+(?!type\b)[^;]*?\sfrom\s+"([^"]+)"/gmu)]
      .some(match => match[1]?.split("/").pop() === module))
    .map(({ name }) => name);

  it("loads the log and its panel only when Settings -> Updates opens", () => {
    expect(sources.length).toBeGreaterThan(100);
    expect(valueImporters("update-log")).toEqual(["components/UpdateLogPanel.tsx"]);
    expect(valueImporters("UpdateLogPanel")).toEqual([]);
    const settings = readFileSync(new URL("./components/SettingsPage.tsx", import.meta.url), "utf8");
    expect(settings).toContain('const UpdateLogPanel = lazy(() => import("./UpdateLogPanel")');
    // Until the chunk arrives Settings draws exactly what it drew before the log.
    expect(settings).toMatch(/<Suspense fallback=\{null\}>\s*<UpdateLogPanel /u);
  });
});
