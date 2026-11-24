import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { createElement } from "react";
import fixture from "../fixtures/threads-team-v57.json";
import type { SessionStatus } from "../shared/engine";
import type { ThreadView, ThreadsView } from "../shared/threads";
import { lineageWords } from "./components/ThreadPaneHead";
import { StartedThreads } from "./components/StartedThreads";
import {
  STARTED_THREADS_SHOWN, startedThreadStatus, startedThreadsByParent,
} from "./started-threads";

const VIEW = fixture.view as unknown as ThreadsView;
const PARENT = "claude#5b0c1d2e-3f40-4a51-8b62-c7d8e9f0a1b2";
const CHILD = "claude#7c1d2e3f-4051-4a62-8b73-d8e9f0a1b2c3";

function session(titles: Record<string, string>, status: SessionStatus = "working") {
  return (key: string) => titles[key] === undefined ? undefined : { title: titles[key], status };
}

function child(index: number, run: ThreadView["lastRun"]): ThreadView {
  return {
    ...(VIEW.threads[0] as ThreadView),
    key: `codex#child-${index}`,
    createdAt: 1_000 + index,
    lastRun: run,
    lineage: { parent: PARENT, relation: "child", keptTurns: 0, at: 1_000 + index },
  };
}

describe("the Started threads line (team tools, protocol 57)", () => {
  it("lists a caller's started threads from the shared view, by their registry titles", () => {
    const lines = startedThreadsByParent(VIEW, session({ [CHILD]: "append child.txt", [PARENT]: "Lead" }));
    expect([...lines.keys()]).toEqual([PARENT]);
    expect(lines.get(PARENT)).toEqual({
      items: [{ key: CHILD, title: "append child.txt", status: "running" }],
      more: 0,
    });
    // A thread the registry has not listed yet waits until it is.
    expect(startedThreadsByParent(VIEW, session({})).size).toBe(0);
    expect(startedThreadsByParent(null, session({})).size).toBe(0);
  });

  it("shows the newest six first, then how many more", () => {
    const threads = Array.from({ length: 8 }, (_, index) => child(index, { startedAt: 1 }));
    const titles = Object.fromEntries(threads.map(thread => [thread.key, `Job ${thread.key.slice(-1)}`]));
    const line = startedThreadsByParent({ ...VIEW, threads }, session(titles)).get(PARENT);
    expect(line?.items.map(item => item.title)).toEqual(["Job 7", "Job 6", "Job 5", "Job 4", "Job 3", "Job 2"]);
    expect(line?.items).toHaveLength(STARTED_THREADS_SHOWN);
    expect(line?.more).toBe(2);
  });

  it("marks each thread running, needing you, done, failed or stopped", () => {
    expect(startedThreadStatus({ lastRun: { startedAt: 1 } }, "working")).toBe("running");
    expect(startedThreadStatus({ lastRun: { startedAt: 1 } }, "waiting")).toBe("needsYou");
    expect(startedThreadStatus({ lastRun: undefined }, "idle")).toBe("running");
    expect(startedThreadStatus({ lastRun: { startedAt: 1, endedAt: 2, outcome: "completed" } }, "idle")).toBe("done");
    expect(startedThreadStatus({ lastRun: { startedAt: 1, endedAt: 2, outcome: "failed" } }, "failed")).toBe("failed");
    expect(startedThreadStatus({ lastRun: { startedAt: 1, endedAt: 2, outcome: "interrupted" } }, "idle")).toBe("stopped");
  });

  it("draws plain marks and titles that open their thread, and +N more", () => {
    const markup = renderToStaticMarkup(createElement(StartedThreads, {
      line: { items: [{ key: CHILD, title: "append child.txt", status: "needsYou" }], more: 3 },
      onOpen: () => undefined,
    }));
    expect(markup).toContain("Started threads");
    expect(markup).toContain('aria-label="Needs you"');
    expect(markup).toContain("append child.txt");
    expect(markup).toContain("+3 more");
  });

  it("a started thread's head says who started it", () => {
    expect(lineageWords("child")).toBe("Started by ");
  });
});
