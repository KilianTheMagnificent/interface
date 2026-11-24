import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import fixture from "../fixtures/threads-v37.json";
import type {
  ReviewComment, ThreadCommand, ThreadControlResponse, ThreadShip, ThreadsView,
} from "../shared/threads";
import { ThreadShipBar, type ThreadShipBarProps } from "./components/ThreadShipBar";
import { ThreadChangesCard } from "./components/ThreadTurnParts";
import {
  appendReviewComments, checksText, leftOutText, pullRequestText, shipCardView, shipNext, WRITE_WITH_AI_TITLE,
  writeWithAiBlocked,
} from "./thread-ship";
import { CommitSelectionForm } from "./components/CommitSelectionForm";
import { PullRequestTextForm } from "./components/WriteWithAi";
import {
  NO_SHIP, readSettles, shownRead, ThreadShipController, type ThreadShipState,
} from "./use-thread-ship";

/* ============================================================================
   Shipping a thread's work (protocol 37): the words and the one next step the
   changes card offers, the controller that runs each effect once and holds
   every control after an outcome nobody could confirm, and what the Changes
   panel's foot and the card's row draw. Rust decides every rule; these pin
   what the renderer makes of its answer.
   ========================================================================= */

const sample = fixture.response.ship as unknown as ThreadShip;
const view = fixture.response.view as unknown as ThreadsView;
const KEY = "claude#5f0c1d2e-8a4b-4c6d-9e7f-1a2b3c4d5e6f";

/** A clean read on a feature branch with a pushed, open pull request whose checks pass. */
function ship(patch: Partial<ThreadShip> = {}): ThreadShip {
  return {
    key: KEY, branch: "iface/fix", defaultBranch: "main", remote: "origin", upstream: "origin/iface/fix",
    ahead: 0, behind: 0, files: [], leftOut: [], filesUnknown: false, message: "Fix the login test",
    gh: "ready", remoteRead: true,
    pullRequest: { number: 42, url: "https://github.com/owner/atlas/pull/42", state: "open",
      title: "Fix the login test", base: "main", draft: false },
    checks: { state: "passing", passed: 3, failed: 0, pending: 0, skipped: 0, failing: [] },
    ...patch,
  };
}

describe("the next ship step", () => {
  it("is a branch off the default branch first, then commit, push, fix checks, merge", () => {
    expect(shipNext(ship({ suggestedBranch: "iface/x", pushBlocked: "on main", pullRequest: undefined,
      files: [{ path: "src/a.ts", status: "modified" }] }), false)).toBe("branch");
    expect(shipNext(ship({ files: [{ path: "src/a.ts", status: "modified" }] }), false)).toBe("commit");
    expect(shipNext(ship({ pullRequest: undefined, upstream: undefined, ahead: undefined }), false)).toBe("push");
    expect(shipNext(ship({ ahead: 2 }), false)).toBe("push");
    expect(shipNext(ship({ checks: { ...ship().checks!, state: "failing", failed: 1 } }), false))
      .toBe("fixChecks");
    expect(shipNext(ship(), false)).toBe("merge");
    expect(shipNext(ship({ checks: { ...ship().checks!, state: "pending", pending: 1 } }), false)).toBe("none");
  });

  it("is Check again after an outcome nobody confirmed, and nothing before a read", () => {
    expect(shipNext(ship(), true)).toBe("check");
    expect(shipNext(null, false)).toBe("none");
    // A push needs gh: signed out, there is no push step.
    expect(shipNext(ship({ gh: "signedOut", pullRequest: undefined, upstream: undefined }), false)).toBe("none");
  });
});

describe("ship words", () => {
  it("sums checks up without saying zero", () => {
    expect(checksText(undefined)).toBe("No checks");
    expect(checksText(ship().checks)).toBe("All 3 checks passed");
    expect(checksText({ state: "failing", passed: 3, failed: 1, pending: 1, skipped: 0, failing: [] }))
      .toBe("1 failing · 1 running · 3 passed");
  });

  it("says what Commit leaves out, and why, grouped", () => {
    expect(leftOutText(sample.leftOut))
      .toBe("Left out: .env (credentials are never committed); README.md (not this thread's)");
    expect(leftOutText([])).toBe("");
  });

  it("titles a pull request by the message's first line and describes it by the rest", () => {
    expect(pullRequestText("Fix the login test\n\n- one\n- two"))
      .toEqual({ title: "Fix the login test", body: "- one\n- two" });
  });

  it("brings review comments into the draft the way a diff line comment goes", () => {
    const comments: ReviewComment[] = [
      { path: "src/a.ts", startLine: 12, endLine: 14, side: "new", code: "+a\n+b\n+c",
        comment: "@reviewer: name these", url: "u1", outdated: false },
      { path: "README.md", side: "new", code: "", comment: "@r: on the whole file", url: "u2", outdated: false },
    ];
    expect(appendReviewComments("Please also", comments)).toBe(
      "Please also\n\nsrc/a.ts:12-14\n```diff\n+a\n+b\n+c\n```\n@reviewer: name these\n\n"
      + "README.md\n@r: on the whole file\n\n");
  });

  it("gives the card one row and one step from Rust's read", () => {
    const thread = view.threads[0]!;
    expect(shipCardView(null, false, false, thread)).toBeUndefined();
    expect(shipCardView(ship({ files: [{ path: "a", status: "added" }, { path: "b", status: "modified" }] }),
      false, false, thread)).toEqual({ text: "2 files to commit", next: "commit", label: "Commit…", busy: false });
    expect(shipCardView(ship({ ahead: 1 }), false, true, thread))
      .toEqual({ text: "#42 · All 3 checks passed", next: "push", label: "Push", busy: true });
    expect(shipCardView(ship(), true, false, thread)?.label).toBe("Check again");
    expect(shipCardView(ship({ branch: "main", suggestedBranch: "iface/x", pullRequest: undefined,
      files: [{ path: "a", status: "added" }] }), false, false, thread))
      .toEqual({ text: "On main · 1 file to commit", next: "branch", label: "Create branch…", busy: false });
  });
});

/** A controller over scripted answers, recording every command it sends. */
function controller(answers: ((command: ThreadCommand) => Promise<ThreadControlResponse>)) {
  const sent: ThreadCommand[] = [];
  const prompts: string[] = [];
  const quoted: ReviewComment[][] = [];
  let states: ReadonlyMap<string, ThreadShipState> = new Map();
  const ship = new ThreadShipController({
    request: command => { sent.push(command); return answers(command); },
    send: async (_key, text) => { prompts.push(text); },
    quote: (_key, comments) => { quoted.push([...comments]); },
    now: () => 1_000,
  }, next => { states = next; });
  return { ship, sent, prompts, quoted, state: () => states.get(KEY) ?? NO_SHIP };
}

const answer = (patch: Partial<ThreadControlResponse>): ThreadControlResponse =>
  ({ view, ...patch }) as ThreadControlResponse;

describe("the ship controller", () => {
  it("keeps the exact commit review when opening Changes concurrently refreshes shipping", async () => {
    let finish!: (answer: ThreadControlResponse) => void;
    const reviewed = { revision: "reviewed-tree", files: [] };
    const run = controller(command => command.kind === "reviewCommit"
      ? new Promise(resolve => { finish = resolve; }) : Promise.resolve(answer({ ship: ship() })));
    const pending = run.ship.reviewCommit(KEY);
    await run.ship.refresh(KEY);
    expect(run.sent.map(command => command.kind)).toEqual(["reviewCommit"]);
    finish(answer({ commitReview: reviewed }));
    await pending;
    expect(run.state().busy).toBeNull();
    expect(run.state().commitReview).toEqual(reviewed);
    await run.ship.refresh(KEY);
    expect(run.state().ship).toEqual(ship());
    expect(run.state().commitReview).toEqual(reviewed);
  });

  it("carries the exact reviewed revision and choices once, and blocks after a lost selected-commit response", async () => {
    const reviewed = { revision: "reviewed-tree", files: [] };
    const run = controller(async command => {
      if (command.kind === "reviewCommit") return answer({ commitReview: reviewed });
      throw new Error("Connection ended after sending the request");
    });
    await run.ship.reviewCommit(KEY);
    expect(run.state().commitReview).toEqual(reviewed);
    await run.ship.commitSelected(KEY, "one hunk", reviewed.revision, [{ path: "shared.rs", hunks: [1] }]);
    expect(run.sent[1]).toEqual({ kind: "commitSelected", key: KEY, message: "one hunk", revision: reviewed.revision, choices: [{ path: "shared.rs", hunks: [1] }] });
    expect(run.state().needsCheck).toBe(true);
    expect(run.state().commitReview).toBeNull();
    await run.ship.commitSelected(KEY, "again", reviewed.revision, [{ path: "shared.rs" }]);
    await run.ship.reviewCommit(KEY);
    expect(run.sent).toHaveLength(2);
  });
  it("runs one effect at a time and never retries an unknown outcome until a read settles it", async () => {
    let unknownOnce = true;
    const run = controller(async command => {
      if (command.kind === "openPullRequest" && unknownOnce) {
        unknownOnce = false;
        return answer({ ship: ship({ gh: "unavailable" }), shipped: { action: "openPullRequest",
          certainty: "unknown", message: "Interface couldn't confirm. Check again." } });
      }
      return answer({ ship: ship({ gh: command.kind === "shipStatus" && command.remote ? "ready" : "unavailable" }) });
    });
    const first = run.ship.openPullRequest(KEY);
    // A second click while the first runs sends nothing.
    await run.ship.openPullRequest(KEY);
    await first;
    expect(run.sent.filter(command => command.kind === "openPullRequest")).toHaveLength(1);
    expect(run.state().needsCheck).toBe(true);
    expect(run.state().notice).toEqual({ tone: "unknown", message: "Interface couldn't confirm. Check again." });
    // Held: nothing runs, nothing is retried by itself.
    await run.ship.openPullRequest(KEY);
    await run.ship.merge(KEY, 42, "squash");
    expect(run.sent.map(command => command.kind)).toEqual(["openPullRequest"]);
    // A read without gh does not settle a pull request's outcome; one with it does.
    await run.ship.refresh(KEY, false);
    expect(run.state().needsCheck).toBe(true);
    await run.ship.refresh(KEY, true);
    expect(run.state().needsCheck).toBe(false);
    await run.ship.openPullRequest(KEY);
    expect(run.sent.filter(command => command.kind === "openPullRequest")).toHaveLength(2);
  });

  it("titles the pull request with the message the owner committed with", async () => {
    const run = controller(async command => command.kind === "commit"
      ? answer({ ship: ship(), shipped: { action: "commit", certainty: "done", message: "Committed 1 file as abc1234." } })
      : answer({ ship: ship(), shipped: { action: "openPullRequest", certainty: "done", message: "Opened #42." } }));
    await run.ship.commit(KEY, "Trim the user name\n\nOnce, at login.", ["src/login.js"]);
    expect(run.state().notice?.message).toBe("Committed 1 file as abc1234.");
    await run.ship.openPullRequest(KEY);
    expect(run.sent.at(-1)).toEqual({ kind: "openPullRequest", key: KEY, title: "Trim the user name",
      body: "Once, at login." });
  });

  it("reads again after a refusal and keeps the refusal on screen", async () => {
    const run = controller(async command => {
      if (command.kind === "commit") throw new Error("The files to commit changed since you looked.");
      return answer({ ship: ship() });
    });
    await run.ship.commit(KEY, "Fix", ["src/a.ts"]);
    await Promise.resolve();
    expect(run.sent.map(command => command.kind)).toEqual(["commit", "shipStatus"]);
    expect(run.state().notice?.tone).toBe("error");
    expect(run.state().notice?.message).toContain("changed since you looked");
  });

  it("sends Rust's failing-checks prompt through the pane and quotes review comments into the draft", async () => {
    const comments = fixture.response.reviewComments as unknown as ReviewComment[];
    const run = controller(async command => command.kind === "failingChecksPrompt"
      ? answer({ prompt: "The checks on #42 are failing" })
      : answer({ reviewComments: comments }));
    await run.ship.fixChecks(KEY);
    expect(run.prompts).toEqual(["The checks on #42 are failing"]);
    expect(run.state().notice).toEqual({ tone: "done", message: "Asked the thread to fix the failing checks." });
    await run.ship.bringComments(KEY);
    expect(run.quoted).toEqual([comments]);
    expect(run.state().notice?.message).toBe("Added 1 review comment to the prompt.");
  });

  it("keeps what gh said across a read that asked git only", () => {
    const local = ship({ remoteRead: false, gh: "unavailable", pullRequest: undefined, checks: undefined,
      branch: "iface/next", ahead: 1 });
    const shown = shownRead(ship(), local);
    expect(shown.gh).toBe("ready");
    expect(shown.pullRequest?.number).toBe(42);
    expect(shown.branch).toBe("iface/next");
    expect(shownRead(null, local)).toBe(local);
  });

  it("settles a commit's or a branch's unknown with any read", () => {
    expect(readSettles("commit", ship({ remoteRead: false, gh: "unavailable" }))).toBe(true);
    expect(readSettles("merge", ship({ remoteRead: false }))).toBe(false);
    expect(readSettles("merge", ship())).toBe(true);
  });
});

const noop = (): void => undefined;

function bar(state: Partial<ThreadShipState>, patch: Partial<ThreadShipBarProps> = {}): string {
  return renderToStaticMarkup(createElement(ThreadShipBar, {
    state: { ...NO_SHIP, ...state }, onRefresh: noop, onReviewCommit: noop, onCommit: noop, onCreateBranch: noop,
    onOpenPullRequest: noop, onFixChecks: noop, onBringComments: noop, onMerge: noop, onDismiss: noop, ...patch,
  }));
}

describe("the Changes panel's ship section", () => {
  it("offers Commit, a branch off the default branch, and says how to fix gh", () => {
    const html = bar({ ship: { ...sample, commitBlocked: undefined } });
    expect(html).toContain("2 files to commit");
    expect(html).toContain("Commit…");
    expect(html).toContain("Create branch");
    expect(html).toContain('value="iface/fix-the-login-test-3f9a2c"');
    expect(html).toContain("Interface won&#x27;t push to main");
    expect(html).toContain("Run gh auth login in a terminal, then check again.");
  });

  it("shows the pull request, its checks, Fix failing checks, review comments and Merge", () => {
    const html = bar({ ship: ship({ checks: { ...ship().checks!, state: "failing", failed: 1 } }) });
    expect(html).toContain('href="https://github.com/owner/atlas/pull/42"');
    expect(html).toContain("1 failing · 3 passed");
    expect(html).toContain("Fix failing checks");
    expect(html).toContain("Bring in review comments");
    expect(html).toContain("Merge…");
    expect(html).not.toContain("Push &amp; open PR");
  });

  it("reads the branch and where it stands in one line, its actions beside it", () => {
    const html = bar({ ship: ship({ upstream: undefined, ahead: undefined, pullRequest: undefined, checks: undefined,
      files: [{ path: "src/a.ts", status: "modified" }, { path: "src/b.ts", status: "added" }] }) });
    const head = html.slice(html.indexOf('<div class="tship-head">'), html.indexOf('<div class="tship-head">') + 2000);
    expect(head.replace(/<svg[\s\S]*?<\/svg>/gu, "").replace(/<[^>]+>/gu, "|").replace(/\|+/gu, "|"))
      .toMatch(/^\|iface\/fix\|·\|Not pushed yet\|·\|2 files to commit\|Push &amp; open PR\|Commit…\|/u);
    // No pull request and gh working: nothing else to say below.
    expect(html.replace(/<style>[\s\S]*?<\/style>/u, "")).not.toContain("tship-pr");
  });

  it("fills only the next step: a branch, then commit, push, fix checks, merge, or check again", () => {
    /** The labels of the filled buttons. */
    const filled = (html: string): string[] =>
      [...html.matchAll(/<button [^>]*class="tship-btn is-primary"[^>]*>([^<]*)</gu)].map(match => match[1]);
    expect(filled(bar({ ship: { ...sample, commitBlocked: undefined } }))).toEqual(["Create branch"]);
    expect(filled(bar({ ship: ship({ files: [{ path: "src/a.ts", status: "modified" }] }) }))).toEqual(["Commit…"]);
    expect(filled(bar({ ship: ship({ pullRequest: undefined, upstream: undefined, ahead: undefined }) })))
      .toEqual(["Push &amp; open PR"]);
    expect(filled(bar({ ship: ship({ checks: { ...ship().checks!, state: "failing", failed: 1 } }) })))
      .toEqual(["Fix failing checks"]);
    expect(filled(bar({ ship: ship() }))).toEqual(["Merge…"]);
    expect(filled(bar({ ship: ship(), needsCheck: true }))).toEqual(["Check again"]);
    expect(filled(bar({ ship: ship({ checks: { ...ship().checks!, state: "pending", pending: 1 } }) }))).toEqual([]);
  });

  it("holds every control and asks to check again after an outcome nobody confirmed", () => {
    const html = bar({ ship: ship({ ahead: 1 }), needsCheck: true, unknown: "merge",
      notice: { tone: "unknown", message: "Interface couldn't confirm whether #42 merged." } });
    expect(html).toContain("Check again");
    expect(html).toContain("couldn&#x27;t confirm whether #42 merged");
    for (const label of ["Push", "Merge…", "Bring in review comments"]) {
      expect(html).toMatch(new RegExp(`<button[^>]*disabled=""[^>]*>(<[^>]+>)*${label}`));
    }
  });
});

describe("the latest changes card's ship row", () => {
  const changes = { files: 1, stat: { added: 1, removed: 3 }, rows: [], more: 0 } as never;
  it("draws Rust's read and the one next step, and nothing on a card without it", () => {
    const html = renderToStaticMarkup(createElement(ThreadChangesCard, {
      view: changes, onReview: noop, onShip: noop,
      ship: { text: "1 file to commit", next: "commit", label: "Commit…", busy: false },
    }));
    expect(html).toContain('aria-label="Ship this thread&#x27;s work"');
    expect(html).toContain("1 file to commit");
    expect(html).toContain(">Commit…</button>");
    expect(renderToStaticMarkup(createElement(ThreadChangesCard, { view: changes, onReview: noop })))
      .not.toContain("thread-ship");
  });
});

/* Write with AI (protocol 67, T3 section 12): one tool-less Claude call writes
   the commit message or the pull request's text, which lands in the field for
   the owner to edit. Rust reads the changes and refuses when Claude cannot. */
describe("Write with AI", () => {
  const writeNothing = async () => ({ text: "" });

  it("is off with the reason when Claude is not installed or signed out, and on before any probe", () => {
    expect(writeWithAiBlocked([])).toBeNull();
    expect(writeWithAiBlocked([{ engine: "codex", ok: true, info: { binaryPath: "/c", version: "1" } } as never]))
      .toContain("not installed");
    expect(writeWithAiBlocked([{ engine: "claude", ok: false, error: "missing" }])).toContain("not installed");
    expect(writeWithAiBlocked([{ engine: "claude", ok: true,
      info: { binaryPath: "/c", version: "1", auth: "signedOut" } } as never])).toContain("signed out");
    expect(writeWithAiBlocked([{ engine: "claude", ok: true,
      info: { binaryPath: "/c", version: "1", auth: "signedIn" } } as never])).toBeNull();
  });

  it("asks Rust for the text of the files being committed and changes nothing itself", async () => {
    const run = controller(async command => command.kind === "suggestShipText"
      ? answer({ shipText: "Fix the login test\n\nIt waited too little." })
      : answer({ ship: ship() }));
    await expect(run.ship.suggestText(KEY, "commit", ["src/login.ts"]))
      .resolves.toEqual({ text: "Fix the login test\n\nIt waited too little." });
    await expect(run.ship.suggestText(KEY, "pullRequest")).resolves.toHaveProperty("text");
    expect(run.sent).toEqual([
      { kind: "suggestShipText", key: KEY, what: "commit", paths: ["src/login.ts"] },
      { kind: "suggestShipText", key: KEY, what: "pullRequest" },
    ]);
    expect(run.state()).toEqual(NO_SHIP);
    const refused = controller(async () => { throw new Error("Write with AI uses Claude, and Claude is signed out."); });
    await expect(refused.ship.suggestText(KEY, "commit")).resolves.toEqual({
      error: "Write with AI uses Claude, and Claude is signed out.",
    });
  });

  it("opens the pull request with the title and body the owner left in the form", async () => {
    const run = controller(async () => answer({ ship: ship() }));
    await run.ship.openPullRequestWith(KEY, "  Fix login  ", "- waits for the form");
    expect(run.sent.at(-1)).toEqual({ kind: "openPullRequest", key: KEY, title: "Fix login",
      body: "- waits for the form" });
    expect(run.ship.pullRequestDraft(KEY)).toEqual({ title: "Fix the login test", body: "" });
  });

  it("sits beside the commit message and the pull request's title, saying what it costs or why it is off", () => {
    const review = { revision: "r", files: [] };
    const form = (blocked: string | null): string => renderToStaticMarkup(createElement(CommitSelectionForm, {
      review, message: "Fix", held: false, committing: false, onCommit: noop, onCancel: noop, onReview: noop,
      writeWithAi: { blocked, write: writeNothing },
    }));
    const on = form(null);
    expect(on).toMatch(/Commit message<\/label><button type="button" class="tship-btn" data-write-with-ai="commit"/u);
    expect(on).toContain(`title="${WRITE_WITH_AI_TITLE.replace("'", "&#x27;")}"`);
    expect(WRITE_WITH_AI_TITLE).toContain("uses a little of your Claude usage");
    expect(on).not.toMatch(/data-write-with-ai="commit" disabled/u);
    const off = form("Write with AI uses Claude, and Claude is signed out. Sign in, then try again.");
    expect(off).toMatch(/data-write-with-ai="commit" disabled="" title="Write with AI uses Claude, and Claude is signed out/u);
    // Without the port, nothing is offered.
    expect(renderToStaticMarkup(createElement(CommitSelectionForm, {
      review, message: "Fix", held: false, committing: false, onCommit: noop, onCancel: noop, onReview: noop,
    }))).not.toContain("Write with AI");
    const pr = renderToStaticMarkup(createElement(PullRequestTextForm, {
      draft: { title: "Fix login", body: "- waits" }, held: false, pushing: false,
      port: { blocked: null, write: writeNothing }, onSubmit: noop, onCancel: noop,
    }));
    expect(pr).toMatch(/Pull request title<\/label><button type="button" class="tship-btn" data-write-with-ai="pullRequest"/u);
    expect(pr).toContain('value="Fix login"');
    expect(pr).toContain("- waits</textarea>");
    expect(pr).toContain("Push &amp; open PR");
  });
});
