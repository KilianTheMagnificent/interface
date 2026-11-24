import { flushSync } from "react-dom";
import { useState } from "react";
import type { Root } from "react-dom/client";
import { AwayControl } from "../components/AwayControl";
import type { AwayCommand, AwayRequest, AwaySnapshot, AwayResponse } from "../../shared/away";
import fixture from "../../fixtures/project-away-v22.json";
import { assert, frames } from "./fixture-support";
export async function awayFixture(root: Root): Promise<unknown> {
  flushSync(() => root.render(null)); sessionStorage.removeItem("interface.away.pending.v1");
  const snapshot = structuredClone(fixture.snapshot) as AwaySnapshot;
  const calls: AwayCommand[] = [];
  let lose = true;
  let unavailable = false;
  const reads: string[] = [];
  let sessionKeys: string[] = [];
  let hiddenKeys: string[] = [];
  // From the third start on, Rust files each loop's thread at once (review B3).
  let fileThreads = false;
  const openedThreads: string[] = [];
  const api = async ({ command }: AwayRequest): Promise<AwayResponse> => {
    if (command.kind === "inspect" || command.kind === "status") {
      reads.push(command.kind);
      if (unavailable) throw new Error("Synthetic unavailable reconciliation");
      if (command.kind === "inspect") return structuredClone(snapshot);
      return { version: 2, revision: snapshot.revision, storageUncertain: snapshot.storageUncertain,
        activeRuns: snapshot.runs.filter(run => run.state === "running" || run.state === "stopping").map(({ id, cwd }) => ({ id, cwd })),
        runIds: snapshot.runs.map(run => run.id), cancelledStarts: snapshot.cancelledStarts };
    }
    calls.push(command);
    if (command.kind === "start" && !snapshot.runs.some(run => run.id === command.runId)) {
      // The host saves a start's settings for its project, as `save` does.
      if (!snapshot.presets.some(preset => preset.cwd === command.cwd)) {
        snapshot.presets.push({ cwd: command.cwd, options: command.options ?? snapshot.defaults });
      }
      snapshot.runs.push({ id: command.runId, cwd: command.cwd, options: command.options ?? snapshot.defaults, startedAtMs: Date.now(), deadlineAtMs: Date.now() + 3600000, state: "running", detail: "Working and checking", turns: [{ iteration: 1, role: "work", sessionKey: fileThreads ? "codex#thread" : "codex:fixture", state: "started", report: "" }],
        ...(fileThreads ? { thread: { key: "codex#thread", worktree: "/fixture/.interface-worktrees/away-1a2b3c" } } : {}) });
    }
    if (command.kind === "stop") snapshot.runs.find(run => run.id === command.runId)!.state = "stopped";
    if (command.kind === "save") snapshot.presets.push({ cwd: command.cwd, options: command.options });
    snapshot.revision++;
    if (command.kind === "start" && lose) { lose = false; unavailable = true; throw new Error("Synthetic lost start response"); }
    return structuredClone(snapshot);
  };
  let openRequest = 0;
  function Harness() {
    const [open, setOpen] = useState(false);
    return <div inert={open || undefined}><AwayControl currentProject={() => "/fixture/project"} request={api} pickDirectory={async () => "/fixture/other-project"} onInspectSession={() => {}} onOpenThread={key => { openedThreads.push(key); }} onSessionKeys={keys => { sessionKeys = keys.all; hiddenKeys = keys.hidden; }} onOpenChange={setOpen} openRequest={openRequest} inert={open} /></div>;
  }
  const mount = (): void => { flushSync(() => root.render(<Harness />)); };
  const button = (label: string): HTMLButtonElement => {
    const item = [...document.querySelectorAll<HTMLButtonElement>("button")].find(item => item.textContent === label || item.getAttribute("aria-label") === label);
    assert(item, `Missing button ${label}`); return item;
  };
  mount(); await frames();
  // The first start in a project is confirmed in the form (review A1).
  button("Start Away loop").click(); await frames();
  assert(calls.filter(call => call.kind === "start").length === 0, "The first click started a loop unconfirmed");
  assert(document.querySelector(".away-panel")?.textContent?.includes("/fixture/project"), "The form did not open on the project");
  button("Start loop").click(); button("Start loop").click(); await frames();
  assert(calls.filter(call => call.kind === "start").length === 1, "Double click started two loops");
  // Codex works and Claude, the other vendor, reviews by default (protocol 47).
  const firstStart = calls.find(call => call.kind === "start");
  assert(firstStart?.kind === "start" && firstStart.options?.reviewer === "claude",
    "The form did not default the reviewer to the other vendor");
  assert(document.querySelector(".away-reviewer")?.textContent?.includes("Reviewed by Claude Code") === true,
    "The form did not show its reviewer");
  assert(sessionStorage.getItem("interface.away.pending.v1"), "Unknown delivery marker was lost");
  flushSync(() => root.render(null)); unavailable = false; mount(); await frames();
  assert(sessionStorage.getItem("interface.away.pending.v1") === null, "Reopen did not reconcile the receipt");
  assert(calls.filter(call => call.kind === "start").length === 1, "Reopen replayed start");
  button("Away loops (1 running)").click(); await frames();
  assert(calls.filter(call => call.kind === "start").length === 1, "Viewing running loops started another");
  button("Stop loop").click(); await frames();
  assert(snapshot.runs[0]?.state === "stopped", "Stop did not reach the loop");
  assert(sessionKeys.includes("codex:fixture"), "Away did not report the sessions its run created");
  button("Choose folder…").click(); await frames();
  button("Start loop").click(); await frames();
  assert(snapshot.runs[1]?.cwd === "/fixture/other-project", "Selected folder was ignored");
  assert(snapshot.runs[1]?.options.durationSeconds === 3600, "Host default limit changed");
  const panel = document.querySelector<HTMLElement>(".away-panel")!;
  assert(panel.scrollWidth <= panel.clientWidth + 1, "Away dialog overflowed horizontally");
  assert(panel.contains(document.activeElement), "Focus escaped the dialog");
  button("Close Away loop").click(); await frames();
  assert(document.activeElement === button("Away loops (1 running)"), "Closing Away did not restore toolbar focus");
  assert(reads.at(-1) === "status", "Closed Away panel still requested report bodies");
  button("Away loop options").click(); await frames();
  assert(reads.at(-1) === "inspect", "Opening Away did not refresh reports");
  // A project with saved settings starts in one click.
  snapshot.runs[1]!.state = "stopped"; snapshot.revision++;
  button("Close Away loop").click(); await frames(); await frames();
  fileThreads = true;
  button("Start Away loop").click(); await frames(); await frames();
  assert(calls.filter(call => call.kind === "start").length === 3
    && snapshot.runs[2]?.cwd === "/fixture/project", "A confirmed project did not start in one click");
  // The loop runs as a thread: once Rust has filed it, its thread opens and
  // the panel steps aside; the thread stays listed (review B3).
  assert(openedThreads.join() === "codex#thread", "The started loop's thread did not open");
  assert(document.querySelector(".away-panel") === null, "The panel stayed over the opened thread");
  assert(sessionKeys.includes("codex#thread") && !hiddenKeys.includes("codex#thread"),
    "A loop's thread was hidden from the session lists");
  assert(hiddenKeys.includes("codex:fixture"), "A loop with no thread was listed");
  // The panel lists the running loop first; a listed loop opens its thread.
  button("Away loops (1 running)").click(); await frames();
  const rows = [...document.querySelectorAll<HTMLButtonElement>(".away-loop-open")];
  assert(rows.length === 3 && rows[0]?.textContent?.includes("Working"), "The running loop is not listed first");
  rows[0]!.click(); await frames();
  assert(openedThreads.length === 2 && document.querySelector(".away-panel") === null,
    "A listed loop did not open its thread");
  // The palette's "Away loop…" opens the panel and starts nothing (review C3).
  snapshot.runs[2]!.state = "stopped"; snapshot.revision++; await frames();
  openRequest += 1; mount(); await frames(); await frames();
  assert(document.querySelector(".away-panel")?.textContent?.includes("/fixture/project"), "The palette request did not open Away");
  assert(calls.filter(call => call.kind === "start").length === 3, "The palette request started a loop");
  mount(); await frames();
  assert(calls.filter(call => call.kind === "start").length === 3, "A re-render repeated the palette request");
  button("Close Away loop").click(); await frames();
  return { confirmedFirstStart: true, oneClickStart: true, duplicateStarts: 0, viewWhileRunning: true,
    reconciledOnReopen: true, stop: true, selectedProject: snapshot.runs[1]?.cwd, paletteOpen: true,
    threadOpened: openedThreads[0] };
}
