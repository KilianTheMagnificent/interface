import { flushSync } from "react-dom";
import type { Root } from "react-dom/client";
import type { CommitChoice, CommitReview } from "../../shared/threads";
import { CommitSelectionForm } from "../components/CommitSelectionForm";
import { THREAD_SHIP_CSS } from "../components/ThreadShipBar";
import { assert, frames } from "./fixture-support";

export async function commitSelectionFixture(root: Root) {
  const review: CommitReview = { revision: "reviewed-1", files: [
    { path: "shared.rs", status: "modified", attributed: true, hunks: [
      { id: 0, patch: "@@ -1 +1 @@\n-old\n+selected\n" }, { id: 1, patch: "@@ -50 +50 @@\n-other\n+leave this\n" },
    ] },
    { path: "shell.txt", status: "added", attributed: false, hunks: [] },
    { path: ".env", status: "added", attributed: false, hunks: [], blocked: "Excluded path" },
  ] };
  const sent: { revision: string; choices: readonly CommitChoice[] }[] = [];
  function render(snapshot = review) {
    flushSync(() => root.render(<section className="tship" style={{ maxHeight: "100vh" }}>
      <style>{THREAD_SHIP_CSS}</style>
      <CommitSelectionForm key={snapshot.revision} review={snapshot} message="Selected changes" held={false} committing={false}
        onCommit={(_message, revision, choices) => { sent.push({ revision, choices }); }} onCancel={() => {}} onReview={() => {}} />
    </section>));
  }
  render(); await frames();
  const checkbox = (label: string) => {
    const value = document.querySelector<HTMLInputElement>(`input[aria-label="${label}"]`);
    assert(value, `Missing ${label}`); return value;
  };
  const submit = () => document.querySelector<HTMLButtonElement>('button[type="submit"]')!;
  assert(submit().disabled, "No changes selected initially");
  assert(checkbox("Include .env").disabled, "Sensitive file is not selectable");
  document.querySelector<HTMLDetailsElement>("details")!.open = true;
  checkbox("Include hunk 1 of shared.rs").click(); await frames();
  assert(!checkbox("Include hunk 2 of shared.rs").checked, "Other hunk stays out");
  checkbox("Include shell.txt").click(); await frames();
  submit().click(); await frames();
  assert(sent.length === 1 && sent[0].revision === review.revision, "Submission keeps reviewed revision");
  assert(JSON.stringify(sent[0].choices) === JSON.stringify([{ path: "shared.rs", hunks: [0] }, { path: "shell.txt" }]), "Explicit hunks and unattributed file reach host callback");
  render({ ...review, revision: "reviewed-2" }); await frames();
  assert(submit().disabled && !checkbox("Include shell.txt").checked, "New review clears stale selection");
  return { selectedHunk: true, unattributed: true, credentialBlocked: true, staleSelectionCleared: true };
}
