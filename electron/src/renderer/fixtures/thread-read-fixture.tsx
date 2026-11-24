import { StrictMode, useState } from "react";
import { flushSync } from "react-dom";
import type { Root } from "react-dom/client";
import type { ThreadCommand, ThreadControlResponse, ThreadsView } from "../../shared/threads";
import { useThreadDiff, useThreadIndex } from "../use-thread-pane";
import { assert, frames, until } from "./fixture-support";

/** Real hooks, rapid scope changes, unmount cleanup, and unchanged index work. */
export async function threadReadFixture(root: Root, view: ThreadsView,
    response: ThreadControlResponse): Promise<Record<string, unknown>> {
  let scans = 0;
  const threads = new Proxy(view.threads, {
    get(target, key, receiver) {
      if (key === Symbol.iterator) scans++;
      return Reflect.get(target, key, receiver);
    },
  });
  const snapshot = { ...view, threads };
  const calls: { command: ThreadCommand; resolve: (answer: ThreadControlResponse) => void }[] = [];
  const request = (command: ThreadCommand): Promise<ThreadControlResponse> =>
    new Promise(resolve => { calls.push({ command, resolve }); });
  let refresh!: () => void;
  let diff!: ReturnType<typeof useThreadDiff>;
  let index!: ReturnType<typeof useThreadIndex>;
  function Harness() {
    const [tick, setTick] = useState(0);
    refresh = () => { setTick(value => value + 1); };
    index = useThreadIndex(snapshot);
    diff = useThreadDiff(request, () => "");
    return <output data-tick={tick}>{diff.target?.key ?? "closed"}:{diff.load.status}</output>;
  }
  flushSync(() => root.render(<StrictMode><Harness /></StrictMode>));
  await frames(2);
  const firstIndex = index;
  const initialScans = scans;
  for (let i = 0; i < 100; i++) flushSync(refresh);
  assert(index === firstIndex && scans === initialScans,
    `Unchanged thread data was rescanned ${scans - initialScans} times during unrelated renders`);

  flushSync(() => { diff.open("first", { scope: "thread" }); });
  await until(() => calls.length === 1, "The initial diff read never started");
  for (let i = 0; i < 20; i++) {
    flushSync(() => { diff.open(`selection-${i}`, { scope: "thread" }); });
  }
  assert(calls.length === 1, "Rapid diff changes started overlapping Git reads");
  calls[0].resolve(response);
  await until(() => calls.length === 2, "The latest pending diff was not read");
  assert("key" in calls[1].command && calls[1].command.key === "selection-19",
    "An obsolete selection was read");
  assert(diff.load.status === "loading", "The old diff was painted under the new selection");
  calls[1].resolve(response);
  await until(() => diff.load.status === "ready", "The newest diff did not appear");

  flushSync(() => { diff.refresh(); });
  await until(() => calls.length === 3, "Explicit refresh did not run");
  flushSync(() => { diff.open("never-read", { scope: "branch" }); });
  flushSync(() => root.render(null));
  calls[2].resolve(response);
  await frames(3);
  assert(Number(calls.length) === 3, "Unmount left a queued diff read alive");
  return { unrelatedRenders: 100, additionalIndexScans: scans - initialScans,
    rapidSelections: 20, selectionReads: 2, queuedReadsAfterUnmount: 0 };
}
