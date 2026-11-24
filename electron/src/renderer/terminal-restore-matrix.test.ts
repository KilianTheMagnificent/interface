/**
 * Cold-start restore matrix, renderer half (opportunity review F2, 2026-10-15).
 *
 * `apps/interface-host/src/terminal_restore_matrix_tests.rs` saves one pane
 * per row, restarts the host and records what each reopen request got back.
 * This file plays the same rows through the renderer's own reopen planning:
 * the saved entry must become exactly the request the host answered, and
 * whatever the host answers — for that request, the shell fallbacks after it
 * and a moved folder's retry — must end as a running tab or a failed tab that
 * says why. Never a pane with neither, which draws as a blank "Starting" tab.
 * A request the host matrix never answered fails here, so the two halves
 * cannot drift apart.
 */
import { describe, expect, it } from "vitest";
import matrix from "../fixtures/terminal-restore-matrix-v1.json";
import { encodeIpcErrorMessage } from "../shared/ipc-error";
import type {
  CreateTerminalRequest, TerminalSummary, TerminalWorkspaceRecord, TerminalWorkspaceTerminal,
} from "../shared/ipc";
import { messageFor } from "./components/panel-text";
import {
  EMPTY_TERMINAL_WORKSPACE_STATE, openTerminalTab, parseTerminalWorkspaceState,
  serializeTerminalWorkspaceState, terminalIdsInWorkspaceState,
} from "./terminal-workspace";
import {
  failedReopenTerminals, reopenRequests, relocateTerminalWorkspaces, shellFallbackRequests,
  visibleWorkspaceTerminals,
} from "./terminal-workspaces";
import { reopenWithRetry } from "./terminal-reopen-retry";

const PLACES: Readonly<Record<string, string>> = {
  "<root>": "/Users/owner/Developer/project",
  "<link>": "/Users/owner/Desktop/project",
  "<moved>": "/Users/owner/Developer/moved-project",
  "<default>": "/Users/owner/Desktop/Projects",
  "<pane>": "11111111-1111-4111-8111-111111111111",
  "<front>": "22222222-2222-4222-8222-222222222222",
};
/** The id the host matrix assumed the renderer mints on a reopen. */
const MINTED_ON_REOPEN = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
const PANE = PLACES["<pane>"];
const FRONT = PLACES["<front>"];
const FALLBACK = "Interface could not complete the terminal operation.";

interface Outcome {
  readonly kind: "reopened" | "failed";
  readonly resume?: string;
  readonly code?: string;
  readonly message?: string;
  readonly retryable?: boolean;
  readonly replaySafe?: boolean;
  readonly movedTo?: string | null;
}

interface Row {
  readonly case: string;
  readonly saved: unknown;
  readonly request: unknown;
  readonly outcome: Outcome;
  readonly fallbacks?: readonly { readonly request: unknown; readonly outcome: Outcome }[];
  readonly afterMove?: Outcome;
}

function place<T>(value: T): T {
  return JSON.parse(JSON.stringify(value).replace(
    /<(root|link|moved|default|pane|front)>/gu, token => PLACES[token],
  )) as T;
}

/** One request's identity, whatever order its keys were written in. */
function key(request: unknown): string {
  const sorted = (value: unknown): unknown => {
    if (Array.isArray(value)) return value.map(sorted);
    if (value !== null && typeof value === "object") {
      return Object.fromEntries(Object.keys(value).sort()
        .map(name => [name, sorted((value as Record<string, unknown>)[name])]));
    }
    return value;
  };
  return JSON.stringify(sorted(request));
}

function layoutShowing(ids: readonly string[]): string {
  let state = EMPTY_TERMINAL_WORKSPACE_STATE;
  for (const id of ids) state = openTerminalTab(state, "pane-1", id);
  return serializeTerminalWorkspaceState(state);
}

function record(id: string, terminals: readonly TerminalWorkspaceTerminal[]): TerminalWorkspaceRecord {
  return {
    id,
    name: id,
    createdAt: "2026-10-15T00:00:00Z",
    updatedAt: "2026-10-15T00:00:00Z",
    terminals: [...terminals],
    layout: layoutShowing(terminals.map(terminal => terminal.id)),
  };
}

function running(request: CreateTerminalRequest): TerminalSummary {
  return {
    id: request.id,
    generation: 1,
    title: request.title ?? "Terminal",
    root: request.cwd,
    cwd: request.cwd,
    state: "running",
    size: { columns: 90, rows: 28 },
    createdAt: "2026-10-15T00:00:01Z",
    updatedAt: "2026-10-15T00:00:01Z",
    ...(request.launch === undefined ? {} : { launch: request.launch }),
    ...(request.launchSession === undefined ? {} : { launchSession: request.launchSession }),
  };
}

/** The host as the matrix recorded it: each request it answered, and nothing else. */
function recordedHost(row: Row, movedRequest: () => unknown): (request: CreateTerminalRequest) => Promise<TerminalSummary> {
  const answers = new Map<string, Outcome>([[key(place(row.request)), row.outcome]]);
  for (const fallback of row.fallbacks ?? []) answers.set(key(place(fallback.request)), fallback.outcome);
  if (row.afterMove !== undefined) answers.set(key(movedRequest()), row.afterMove);
  return request => {
    const answer = answers.get(key(request));
    if (answer === undefined) {
      return Promise.reject(new Error(`${row.case}: the renderer sent a request the host matrix never answered: ${key(request)}`));
    }
    if (answer.kind === "reopened") return Promise.resolve(running(request));
    return Promise.reject(new Error(encodeIpcErrorMessage(place(answer.message ?? ""), {
      code: answer.code ?? "",
      retryable: answer.retryable ?? false,
      replaySafe: answer.replaySafe ?? true,
    })));
  };
}

/** Running or failed with a reason: what the center draws for the pane. */
function drawn(workspace: TerminalWorkspaceRecord, live: readonly TerminalSummary[],
    failures: ReadonlyMap<string, string>): "running" | "failed" | "blank" {
  const layout = parseTerminalWorkspaceState(workspace.layout ?? null);
  const shown = terminalIdsInWorkspaceState(layout);
  const ids = new Set(workspace.terminals.map(terminal => terminal.id));
  if (visibleWorkspaceTerminals(live, ids, shown).some(terminal => terminal.id === PANE)) {
    return "running";
  }
  const failed = failedReopenTerminals(workspace, failures, live, layout)
    .find(terminal => terminal.id === PANE);
  return failed !== undefined && failed.message.trim() !== "" ? "failed" : "blank";
}

const rows = matrix.rows as unknown as readonly Row[];

describe("cold-start restore matrix", () => {
  it("covers every row the host matrix writes", () => {
    expect(matrix.version).toBe(1);
    expect(rows.length).toBe(288);
    expect(new Set(rows.map(row => row.case)).size).toBe(rows.length);
  });

  it.each(rows.map(row => [row.case, row] as const))(
    "%s comes back running or says why, never blank",
    async (_name, row) => {
      const saved = place(row.saved) as TerminalWorkspaceTerminal;
      const front: TerminalWorkspaceTerminal = { id: FRONT, title: "Build", cwd: PLACES["<root>"] };
      const parked = row.case.split("/")[3] === "parked";
      const records = parked
        ? [record("front", [front]), record("parked", [saved])]
        : [record("front", [front, saved])];
      const workspace = records.find(entry => entry.terminals.some(terminal => terminal.id === PANE));
      if (workspace === undefined) throw new Error("the pane's workspace");
      const mint = (): string => MINTED_ON_REOPEN;

      // The front workspace comes back first; a parked pane is not part of it.
      expect(reopenRequests(records[0], [], mint).map(request => request.id))
        .toEqual(parked ? [FRONT] : [FRONT, PANE]);

      // The renderer sends exactly the request the host matrix answered.
      const request = reopenRequests(workspace, [], mint).find(candidate => candidate.id === PANE);
      expect(request).toEqual(place(row.request));
      if (request === undefined) return;

      const movedTo = row.outcome.movedTo === undefined || row.outcome.movedTo === null
        ? null : place(row.outcome.movedTo);
      const movedRecords = movedTo === null ? records
        : relocateTerminalWorkspaces(records, request.cwd, movedTo);
      const movedRequest = (): unknown => {
        const moved = movedRecords.find(entry => entry.id === workspace.id);
        return moved === undefined ? undefined
          : reopenRequests(moved, [], mint).find(candidate => candidate.id === PANE);
      };
      const host = recordedHost(row, movedRequest);
      const attempt = await reopenWithRetry(request, {
        createTerminal: host,
        rebuild: id => Promise.resolve(reopenRequests(workspace, [], mint)
          .find(candidate => candidate.id === id)),
        shellFallbacks: candidate => shellFallbackRequests(candidate, PLACES["<default>"]),
        sleep: () => Promise.resolve(),
        cancelled: () => false,
      });
      expect(attempt.ok || !attempt.cancelled).toBe(true);
      if (!attempt.ok) {
        expect(String(attempt.error)).not.toContain("never answered");
      }
      const live = attempt.ok ? [attempt.terminal] : [];
      const failures = attempt.ok ? new Map<string, string>()
        : new Map([[PANE, messageFor(attempt.error, FALLBACK)]]);
      const state = drawn(workspace, live, failures);
      expect(state).not.toBe("blank");
      expect(state).toBe(row.outcome.kind === "reopened"
        || (row.fallbacks ?? []).some(fallback => fallback.outcome.kind === "reopened")
        ? "running" : "failed");

      // A failed tab whose folder moved and left a link offers the new place;
      // following it brings the pane back.
      if (state === "failed" && movedTo !== null) {
        const moved = movedRecords.find(entry => entry.id === workspace.id);
        if (moved === undefined) throw new Error("the moved workspace");
        const retry = reopenRequests(moved, [], mint).find(candidate => candidate.id === PANE);
        expect(retry?.cwd).toBe(movedTo);
        expect(retry?.launch).toEqual(request.launch);
        expect(retry?.launchSession).toEqual(request.launchSession);
        if (retry === undefined) return;
        const again = await reopenWithRetry(retry, {
          createTerminal: host,
          rebuild: () => Promise.resolve(retry),
          shellFallbacks: candidate => shellFallbackRequests(candidate, PLACES["<default>"]),
          sleep: () => Promise.resolve(),
          cancelled: () => false,
        });
        expect(again.ok).toBe(true);
        expect(drawn(moved, again.ok ? [again.terminal] : [], new Map())).toBe("running");
      }
    },
  );
});
