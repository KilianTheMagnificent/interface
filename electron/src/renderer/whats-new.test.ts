import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { UpdateLogRelease } from "./update-log";
import { plainSentence, whatsNew } from "./whats-new";
import { WhatsNew } from "./components/UpdateLogPanel";

const item = (kind: UpdateLogRelease["items"][number]["kind"], name: string) => ({
  sha: "abc1234", completedAt: "2026-11-15T14:33:14-06:00", kind, name, branch: "fix/x",
  notes: "a check line", pins: ["electron/src/renderer/x.test.ts::proves it"],
});
const releases: UpdateLogRelease[] = [
  { version: "0.3.179", sha: "1111111", completedAt: "2026-11-16T09:00:00-06:00", leftOnDev: [],
    items: [item("feature", "Something newer")] },
  { version: "0.3.178", sha: "78ca45a", completedAt: "2026-11-15T14:34:05-06:00", leftOnDev: [], items: [
    item("fix", "Terminal text is the terminal font again, not tiny sans-serif"),
    item("record", "Record the TN Code themes landing"),
    item("feature", "The in-app Browser works like a Chrome tab"),
    item("internal", "Split the gate"),
    item("speed", "Moving backdrops cost about half as much"),
    item("look", "Browser opens as a tab in the pane you open it from"),
  ] },
  { version: "0.3.177", sha: "5404c46", completedAt: "2026-11-15T09:00:21-06:00", leftOnDev: [],
    items: [item("fix", "A new thread's first team tool call waits")] },
];

describe("What's new (owner, 2026-11-15)", () => {
  it("starts at the version you have, in plain sentences of four kinds, without internal or record landings", () => {
    const shown = whatsNew({ releases }, "0.3.178");
    expect(shown.map(release => [release.version, release.isCurrent])).toEqual([["0.3.178", true], ["0.3.177", false]]);
    expect(shown[0]!.changes).toEqual([
      { kind: "fixed", text: "Terminal text is the terminal font again, not tiny sans-serif." },
      { kind: "new", text: "The in-app Browser works like a Chrome tab." },
      { kind: "faster", text: "Moving backdrops cost about half as much." },
      { kind: "looks", text: "Browser opens as a tab in the pane you open it from." },
    ]);
    // A build the log does not list (Dev) starts at the newest release.
    expect(whatsNew({ releases }, "0.0.0-dev")[0]!.version).toBe("0.3.179");
    expect(plainSentence("Done already.")).toBe("Done already.");
  });

  it("keeps hashes, branches and test names out of it", () => {
    const html = renderToStaticMarkup(createElement(WhatsNew, {
      log: { releases, landings: [], unreleasedCount: 0 }, currentVersion: "0.3.178" }));
    expect(html).toContain("0.3.178<span class=\"settings-whats-new-yours\"> · the version you have</span>");
    expect(html).toContain("4 changes");
    expect(html).toContain('<span class="settings-whats-new-kind">Faster</span>');
    for (const detail of ["78ca45a", "abc1234", "fix/x", "x.test.ts", "Record the TN Code", "Split the gate", "a check line"]) {
      expect(html).not.toContain(detail);
    }
    // Older versions wait behind Show.
    expect(html).toMatch(/0\.3\.177<\/span><span class="settings-whats-new-meta">.*?aria-expanded="false">Show<\/button>/u);
    expect(html).not.toContain("A new thread&#x27;s first team tool call waits");
  });
});
