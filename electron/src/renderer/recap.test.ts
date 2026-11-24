import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import type { EngineEvent } from "../shared/engine";
import {
  RECAP_AUTO_DISMISS_MS, RECAP_MIN_AWAY_MS, RECAP_MIN_TURNS,
  recapLine, recapSnapshot, type RecapSnapshot,
} from "./recap";

const status = (value: "working" | "waiting" | "idle" | "failed"): EngineEvent =>
  ({ kind: "status.changed", status: value });
const edit = (before: string, after: string): EngineEvent =>
  ({ kind: "diff.produced", callId: "d1", path: "/repo/a.ts", before, after });

/** One completed turn, the way a live engine emits it. */
const turn = (end: "idle" | "failed" = "idle"): EngineEvent[] =>
  [status("working"), status(end)];

const snapshot = (over: Partial<RecapSnapshot> = {}): RecapSnapshot => ({
  turns: 0, failedTurns: 0, added: 0, removed: 0, diffKnown: false, ...over,
});

/** A five-minute absence over a session that finished one turn inside it. */
const away = (over: Partial<Parameters<typeof recapLine>[0]> = {}) => recapLine({
  before: snapshot({ turns: 3 }),
  after: snapshot({ turns: 4 }),
  leftAt: 1_000_000,
  returnedAt: 1_000_000 + RECAP_MIN_AWAY_MS,
  working: false,
  approvalWaiting: false,
  checks: null,
  ...over,
});

describe("recapSnapshot", () => {
  it("counts a turn the way the shell itself counts one: working to a terminal status", () => {
    // Deliberately the same rule App uses to fire its turn-done notification.
    // A second working->idle pair is a second turn; a status repeat is not.
    expect(recapSnapshot([...turn(), ...turn()]).turns).toBe(2);
    expect(recapSnapshot([status("working"), status("working"), status("idle")]).turns).toBe(1);
  });

  it("counts a waiting turn that settles, because an approved turn still ended", () => {
    expect(recapSnapshot([status("waiting"), status("idle")]).turns).toBe(1);
  });

  it("never counts a turn that has not ended", () => {
    expect(recapSnapshot([status("working")]).turns).toBe(0);
  });

  it("separates the turns that ended badly from the ones that finished", () => {
    const snap = recapSnapshot([...turn(), ...turn("failed")]);
    expect(snap.turns).toBe(2);
    expect(snap.failedTurns).toBe(1);
  });

  it("reports lines as UNKNOWN until this window has watched a diff", () => {
    const empty = recapSnapshot([...turn()]);
    expect(empty.diffKnown).toBe(false);
    expect(empty.added).toBe(0);
    expect(empty.removed).toBe(0);
    const seen = recapSnapshot([...turn(), edit("a\nb", "a\nB\nc")]);
    expect(seen.diffKnown).toBe(true);
    expect(seen.added).toBe(2);
    expect(seen.removed).toBe(1);
  });
});

describe("recapLine", () => {
  it("stays silent for an absence shorter than the threshold", () => {
    expect(away({ returnedAt: 1_000_000 + RECAP_MIN_AWAY_MS - 1 })).toBeNull();
  });

  it("stays silent for a session with fewer turns than the floor", () => {
    const fresh = { before: snapshot() };
    expect(away({ ...fresh, after: snapshot({ turns: RECAP_MIN_TURNS - 1 }) })).toBeNull();
    expect(away({ ...fresh, after: snapshot({ turns: RECAP_MIN_TURNS }) })).not.toBeNull();
  });

  it("stays silent when nothing it can prove happened while the user was gone", () => {
    // Three turns, all of them finished BEFORE the user left. A line reading
    // only "While you were away" is not a briefing.
    expect(away({ before: snapshot({ turns: 3 }), after: snapshot({ turns: 3 }) })).toBeNull();
  });

  it("reads as the mock's one line: lead, turns, lines, checks", () => {
    const line = away({
      before: snapshot({ turns: 3, diffKnown: true, added: 4, removed: 1 }),
      after: snapshot({ turns: 5, diffKnown: true, added: 66, removed: 12 }),
      checks: { passed: 3, failed: 0 },
    });
    expect(line?.parts).toEqual([
      "While you were away", "2 turns finished", "+62 −11", "checks passed",
    ]);
    expect(line?.text).toBe("While you were away · 2 turns finished · +62 −11 · checks passed");
  });

  it("counts only what happened during the absence, never the session's totals", () => {
    const line = away({
      before: snapshot({ turns: 3, diffKnown: true, added: 900, removed: 900 }),
      after: snapshot({ turns: 4, diffKnown: true, added: 901, removed: 900 }),
    });
    expect(line?.parts).toContain("1 turn finished");
    expect(line?.parts).toContain("+1");
  });

  it("omits the line count entirely rather than claiming +0 −0", () => {
    // The session finished a turn that edited nothing this window saw. A
    // measurement of zero over an agent that may well have written a file is
    // worse than no measurement at all.
    const line = away({ after: snapshot({ turns: 4 }) });
    expect(line?.parts).toEqual(["While you were away", "1 turn finished"]);
    expect(line?.text).not.toContain("+0");
  });

  it("prints only the side that moved", () => {
    const removedOnly = away({
      after: snapshot({ turns: 4, diffKnown: true, added: 0, removed: 7 }),
    });
    expect(removedOnly?.parts).toContain("−7");
  });

  it("names the turns that failed rather than folding them into the finished count", () => {
    const line = away({
      after: snapshot({ turns: 8, failedTurns: 2 }),
    });
    expect(line?.parts).toEqual(["While you were away", "3 turns finished", "2 turns failed"]);
  });

  it("says which checks did not pass, and never calls a failed run green", () => {
    const line = away({ checks: { passed: 2, failed: 1 } });
    expect(line?.parts).toContain("1 check failed");
    expect(line?.text).not.toContain("checks passed");
  });

  it("omits checks that produced no result at all", () => {
    expect(away({ after: snapshot({ turns: 4 }), checks: { passed: 0, failed: 0 } })?.parts)
      .toEqual(["While you were away", "1 turn finished"]);
  });

  it("reports an agent still mid-turn and an approval still holding it up", () => {
    const line = away({
      after: snapshot({ turns: 3 }), working: true, approvalWaiting: true,
    });
    expect(line?.parts).toEqual([
      "While you were away", "approval waiting", "still working",
    ]);
  });

  it("quotes no part of the approval request", () => {
    // SessionPeek may quote a permission prompt verbatim because it has room
    // for the whole of it. One clipped clause in a 12px line is how someone
    // approves the wrong thing, so this line only ever says that one is open.
    const line = away({ after: snapshot({ turns: 3 }), approvalWaiting: true });
    expect(line?.text).toBe("While you were away · approval waiting");
  });

  it("refuses to run a count backwards when the log it compares against shrank", () => {
    // A history page prepended, a session closed and reopened: the return
    // snapshot can legitimately hold LESS than the leave snapshot. Negative
    // deltas must read as "nothing proven", never as a negative turn count.
    const line = away({
      before: snapshot({ turns: 9, diffKnown: true, added: 40, removed: 40 }),
      after: snapshot({ turns: 4, diffKnown: true, added: 1, removed: 1 }),
    });
    expect(line).toBeNull();
  });

  it("uses singular wording for one of anything", () => {
    const line = away({
      after: snapshot({ turns: 4, diffKnown: true, added: 1, removed: 1 }),
      checks: { passed: 1, failed: 0 },
    });
    expect(line?.parts).toEqual([
      "While you were away", "1 turn finished", "+1 −1", "checks passed",
    ]);
  });
});

/* The derivation above is worth nothing if it never reaches a screen, so these
   pin the wiring in App.tsx by source. Same reachability rule workspace-ui.test
   applies: a value computed and never rendered is the failure mode. */
describe("what the shell does with it", () => {
  // The recap's state and timers live in use-return-recap.ts; App wires them.
  const app = `${readFileSync(new URL("./App.tsx", import.meta.url), "utf8")}\n${
    readFileSync(new URL("./use-return-recap.ts", import.meta.url), "utf8")}`;
  // The strip itself and its stylesheet live with the pane, in SessionTile.
  const tile = readFileSync(new URL("./components/SessionTile.tsx", import.meta.url), "utf8");

  it("derives the line from this module rather than restating the rules inline", () => {
    expect(app).toMatch(/import \{[^}]*recapLine[^}]*\} from "\.\/recap"/su);
    expect(app).toContain("recapSnapshot(");
    expect(app).toContain("RECAP_AUTO_DISMISS_MS");
  });

  it("renders the strip above the composer of the pane the user came back to", () => {
    expect(tile).toContain('className="tile-recap"');
    expect(app).toContain("<style>{RECAP_CSS}</style>");
    // The stack wrapper is unconditional AND STAYS, even though the strip has
    // moved inside the pane. Mounting or unmounting it would move
    // ProjectedConversation in the tree and remount the pane — losing the draft
    // and the scroll position the recap exists to return to.
    expect(app).toContain('<div className="tile-stack">');
    // The strip now rides the pane's own slot, which renders it directly above
    // the composer rather than under the transcript. The slot itself is always
    // filled — see the live-region pin below — and the STRIP is what the recap
    // gates, one level in.
    expect(tile).toMatch(/beforeComposer=\{\(/u);
    expect(tile.indexOf("beforeComposer={"))
      .toBeLessThan(tile.indexOf('className="tile-recap"'));
  });

  it("takes the reading measure from the pane it now sits in", () => {
    // --recap-measure/--recap-gutter were declared on .tile-stack only. Inside
    // the slot the strip is a .conv descendant, so a private copy would resolve
    // to nothing (width:auto, full-bleed, no error anywhere) and a third copy of
    // the same measure would drift from the two that already agree.
    const css = tile.slice(tile.indexOf("const RECAP_CSS"));
    const block = css.slice(0, css.indexOf("`;"));
    expect(block).toContain("var(--conv-measure)");
    expect(block).toContain("var(--conv-gutter)");
    expect(app + tile).not.toContain("--recap-measure");
    expect(app + tile).not.toContain("--recap-gutter");
  });

  it("offers a dismiss control that says what it dismisses", () => {
    expect(tile).toMatch(/aria-label=\{`Dismiss the away recap/u);
  });

  it("consumes the absence on return, so one absence can draw only one line", () => {
    // THE never-twice-in-a-row INVARIANT, and the only line that carries it.
    // Delete it and every behavioural test in this file still passes while the
    // briefing starts redrawing itself each time the pointer comes back — the
    // one failure this feature cannot be allowed to have.
    expect(app).toContain("recapAwayRef.current = withoutKey(recapAwayRef.current, attended)");
  });

  it("keeps the live region mounted so the line it later holds is announced", () => {
    const slot = tile.slice(tile.indexOf("beforeComposer={"));
    const block = slot.slice(0, slot.indexOf("pendingEchoes="));
    // Unconditional region, conditional contents. A role="status" that arrives
    // already holding its text announces nothing, because a live region has to
    // be in the tree before its contents change.
    expect(block).toContain('<div className="tile-recap-live" role="status">');
    expect(block).toContain("{tileRecap !== undefined && (");
    expect(block).not.toContain("tileRecap === undefined ? undefined");
    // and the strip inside it carries no second live-region role of its own.
    expect(block).toContain('<div className="tile-recap">');
    expect(block).not.toContain('className="tile-recap" role=');
  });

  it("tears the recap down with the rest of a deleted session's client state", () => {
    // A caller that closes the tile first moves attention off the key and files
    // a fresh absence entry for it on the way out. Without these lines a reused
    // key opens onto a briefing about the session it replaced.
    const start = app.indexOf("const dropSessionClientState");
    const teardown = app.slice(start, app.indexOf("}, []);", start));
    expect(teardown).toContain("window.clearTimeout(recapTimer)");
    expect(teardown).toContain("recapTimersRef.current = withoutKey(recapTimersRef.current, key)");
    expect(teardown).toContain("recapAwayRef.current = withoutKey(recapAwayRef.current, key)");
    expect(teardown).toContain("setRecaps(map => withoutKey(map, key))");
  });

  it("clears the line on the next send and on the auto-dismiss timer", () => {
    expect(tile).toContain("dismissRecap(key)");
    expect(app).toContain("}, RECAP_AUTO_DISMISS_MS)");
  });

  it("draws the strip with no animation at all, so reduced motion needs no branch", () => {
    const css = tile.slice(tile.indexOf("const RECAP_CSS"));
    const block = css.slice(0, css.indexOf("`;"));
    expect(block).toContain(".tile-recap");
    expect(block).not.toContain("animation");
    expect(block).not.toContain("transition");
    // The live region stays in the accessibility tree while it is empty.
    // display:none or visibility:hidden would take it out of the tree, and a
    // region that is not in the tree cannot announce the line it later holds.
    expect(block).toContain(".tile-recap-live");
    expect(block).not.toMatch(/\.tile-recap-live[^}]*(display:\s*none|visibility:\s*hidden)/u);
    // Tokens, never the mock's hexes.
    expect(block).not.toMatch(/#[0-9a-f]{3,8}\b/iu);
    expect(block).toContain("var(--fg-dim)");
  });
});

it("auto-dismisses on the half-minute the brief names", () => {
  expect(RECAP_AUTO_DISMISS_MS).toBe(30_000);
  expect(RECAP_MIN_AWAY_MS).toBe(300_000);
  expect(RECAP_MIN_TURNS).toBe(3);
});
