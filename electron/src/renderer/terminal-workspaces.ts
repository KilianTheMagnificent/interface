/** Saved terminal workspaces: which terminals belong together, and which chat
 * each one reopens onto.
 *
 * The pane arrangement stays in `terminal-workspace.ts`; this module owns the
 * membership and the launch binding, which are the parts that must survive a
 * quit. Everything here is pure — the component performs the IPC.
 */

import {
  TERMINAL_LAUNCH_PROFILE_IDS,
  type CreateTerminalRequest, type TerminalLaunchProfileId, type TerminalLaunchRequestV1,
  type TerminalLaunchSessionV1, type TerminalSummary, type TerminalWorkspaceRecord,
  type TerminalWorkspaceTerminal,
} from "../shared/ipc";
import {
  savedLayoutFacts, terminalIdsInWorkspaceState,
  type TerminalWorkspaceState,
} from "./terminal-workspace";
import { productLabel } from "./product";
import { equalValue } from "./stable-values";
import type { TerminalWorkspaceSwitcher } from "./components/TerminalWorkspace";

export const MAX_TERMINAL_WORKSPACES = 64;
export const MAX_TERMINAL_WORKSPACE_NAME = 80;
/** Matches the host's `terminal.title` bound. A longer OSC title must not
 *  fail the whole roster write. */
const MAX_TERMINAL_TITLE = 120;

/**
 * Profiles that receive an Interface-minted pane handle. Claude and Grok
 * accept that UUID directly as a provider `--session-id`; Codex ignores it
 * and the host swaps in the v7 thread id its hook reports. Antigravity owns
 * its conversation ids, so the stable `gemini` profile deliberately receives
 * no synthetic binding.
 */
const MINTS_CHAT_ID: readonly TerminalLaunchProfileId[] = [
  "claude", "grok", "codex",
];

/**
 * Profiles whose saved pane handle the host can reconcile with provider-owned
 * resume behavior. The checkout is independent of this conversation binding.
 * OpenCode is resumable in kind but never mints: its `ses_…` ids are
 * provider-owned, so a binding only ever carries one the host's capture
 * proved from the pane's own OpenCode TUI.
 */
const RESUMABLE: readonly TerminalLaunchProfileId[] = [
  "claude", "grok", "codex", "opencode", "gemini", "muse", "kimi",
];
/** Interface and Claude mint UUID v4; Codex threads are UUID v7. */
const MANAGED_CHAT_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[47][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
/** OpenCode mints its own ids; the host capture only ever files this shape. */
const OPENCODE_CHAT_ID = /^ses_[0-9a-zA-Z]{8,64}$/;
const AGY_CHAT_ID = /^agy-[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const MUSE_CHAT_ID = /^muse-[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
/** Kimi Code's `session_<v4 UUID>`, as the host's Kimi plugin files it. */
const KIMI_CHAT_ID = /^kimi-[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
/** Providers that mint their own ids and report them through host hooks. */
function nativeProfile(profile: TerminalLaunchProfileId | undefined): boolean {
  return profile === "gemini" || profile === "muse" || profile === "kimi";
}
function nativeChatId(profile: TerminalLaunchProfileId | undefined, id: unknown): id is string {
  return typeof id === "string" && ((profile === "gemini" && AGY_CHAT_ID.test(id))
    || (profile === "muse" && MUSE_CHAT_ID.test(id))
    || (profile === "kimi" && KIMI_CHAT_ID.test(id)));
}

function isManagedChatId(value: string): boolean {
  return MANAGED_CHAT_ID.test(value) || OPENCODE_CHAT_ID.test(value)
    || AGY_CHAT_ID.test(value) || MUSE_CHAT_ID.test(value) || KIMI_CHAT_ID.test(value);
}

export function canReopenChat(profileId: TerminalLaunchProfileId | undefined): boolean {
  return profileId !== undefined && RESUMABLE.includes(profileId);
}

/**
 * The binding a brand-new terminal launches with.
 *
 * `mintId` is injected rather than called here so the caller owns randomness —
 * the same reason terminal ids are minted at the call site.
 */
export function newChatBinding(profileId: TerminalLaunchProfileId | undefined,
    mintId: () => string): TerminalLaunchSessionV1 | undefined {
  if (profileId === undefined || !canReopenChat(profileId)) return undefined;
  if (nativeProfile(profileId)) return undefined;
  return MINTS_CHAT_ID.includes(profileId)
    ? { version: 1, id: mintId(), resume: false }
    : { version: 1, resume: false };
}

/**
 * Ask for the exact saved chat when it has a usable pane handle.
 *
 * Builds predating provider capture may have saved a Claude request id that did
 * not become the provider's transcript id. The host proves that exact legacy
 * id against Claude history and otherwise opens Claude's native picker. An
 * id-less capture-era record also takes the picker; other providers preserve
 * their existing managed-handle migration.
 */
export function reopenedChatBinding(
    binding: TerminalLaunchSessionV1 | undefined,
    profileId?: TerminalLaunchProfileId,
    mintId?: () => string): TerminalLaunchSessionV1 | undefined {
  // Antigravity cannot consume legacy Gemini CLI bindings. Drop those saved
  // handles so the stable `gemini` profile reopens as a fresh Antigravity
  // session instead of spawning `agy` with unsupported argv.
  if (profileId !== undefined && !canReopenChat(profileId)) return undefined;
  if (nativeProfile(profileId)) {
    return binding?.version === 1 && nativeChatId(profileId, binding.id)
      ? { version: 1, id: binding.id, resume: true } : undefined;
  }
  if (typeof binding === "object" && binding !== null && binding.version === 1
      && profileId === "claude" && binding.id === undefined) {
    // An id-less Claude binding means the provider hook could not confirm the
    // conversation before shutdown (for example, managed policy blocked the
    // hook). Preserve the conversation by opening Claude's native picker; a
    // fresh minted id would silently strand the existing transcript again.
    return { version: 1, resume: true };
  }
  if (profileId === "opencode") {
    // Preserve exact provider identity; legacy id-less panes ask the host
    // to show OpenCode's own session picker instead of silently starting over.
    if (typeof binding === "object" && binding !== null && binding.version === 1
        && typeof binding.id === "string" && OPENCODE_CHAT_ID.test(binding.id)) {
      return { version: 1, id: binding.id, resume: true };
    }
    return { version: 1, resume: true };
  }
  if (typeof binding === "object" && binding !== null && binding.version === 1
      && typeof binding.id === "string" && MANAGED_CHAT_ID.test(binding.id)) {
    // Pre-capture Interface builds persisted fresh Claude request IDs with
    // `resume:false`. Preserve that exact ID on reopen so the host can prove it
    // against Claude history and either resume it or downgrade only an absent
    // orphan to Claude's native picker. New capture-era unconfirmed bindings
    // are persisted id-less, so they take the picker branch above.
    // Saved workspace JSON is untrusted. Rebuild the accepted v1 shape rather
    // than spreading unknown fields back across IPC, where the Rust contract
    // deliberately rejects them.
    return { version: 1, id: binding.id, resume: true };
  }
  if (canReopenChat(profileId) && mintId !== undefined) {
    return newChatBinding(profileId, mintId);
  }
  // A malformed or legacy binding must never be promoted to a resume request
  // when the caller did not provide enough context to migrate it safely.
  return undefined;
}

function usableText(value: unknown): value is string {
  return typeof value === "string" && value.trim().length > 0;
}

function normalizedLaunchRequest(value: unknown): TerminalLaunchRequestV1 | undefined {
  if (typeof value !== "object" || value === null) return undefined;
  const candidate = value as Partial<TerminalLaunchRequestV1>;
  if (candidate.version !== 1 || typeof candidate.profileId !== "string"
      || !TERMINAL_LAUNCH_PROFILE_IDS.includes(candidate.profileId as TerminalLaunchProfileId)) {
    return undefined;
  }
  return { version: 1, profileId: candidate.profileId as TerminalLaunchProfileId };
}

function normalizedLaunchSession(value: unknown,
    profileId: TerminalLaunchProfileId | undefined): TerminalLaunchSessionV1 | undefined {
  if (profileId !== undefined && !canReopenChat(profileId)) return undefined;
  if (typeof value !== "object" || value === null) return undefined;
  const candidate = value as Partial<TerminalLaunchSessionV1>;
  if (candidate.version !== 1) return undefined;
  if (nativeProfile(profileId)) {
    return nativeChatId(profileId, candidate.id)
      ? { version: 1, id: candidate.id, resume: candidate.resume === true } : undefined;
  }
  if (candidate.id === undefined && (profileId === "claude" || profileId === "opencode")) {
    return { version: 1, resume: candidate.resume === true };
  }
  if (typeof candidate.id !== "string"
      || !(MANAGED_CHAT_ID.test(candidate.id)
        || (profileId === "opencode" && OPENCODE_CHAT_ID.test(candidate.id)))) {
    return undefined;
  }
  return { version: 1, id: candidate.id, resume: candidate.resume === true };
}

function stripControlCharacters(value: string): string {
  return value.replace(/[\u0000-\u001f\u007f]/gu, "");
}

/** Names cross a process boundary and are drawn as a tab, so drop controls. */
export function normalizeWorkspaceName(value: string): string {
  return stripControlCharacters(value).trim().slice(0, MAX_TERMINAL_WORKSPACE_NAME);
}

/** Host `bounded_text` for `terminal.title`: non-empty, no controls, ≤120. */
export function usableTerminalTitle(value: string): string {
  const printable = stripControlCharacters(value).trim();
  const clipped = [...printable].slice(0, MAX_TERMINAL_TITLE).join("");
  return clipped === "" ? "Terminal" : clipped;
}

function currentProfileTitle(title: string,
    profileId: TerminalLaunchProfileId | undefined): string {
  const normalized = usableTerminalTitle(title);
  if (profileId !== "gemini") return normalized;
  // Preserve custom names while migrating the default titles left by the
  // retired Gemini CLI launcher. Batch creation generated only 1 through 10.
  // The stable profile id remains frozen for persisted-wire compatibility,
  // but its current product is Antigravity.
  if (normalized === "Gemini") return "Antigravity";
  const batchTitle = /^Gemini ([1-9]|10)$/u.exec(normalized);
  return batchTitle === null ? normalized : `Antigravity ${batchTitle[1]}`;
}

export function newTerminalWorkspace(name: string, mintId: () => string,
    now: () => string): TerminalWorkspaceRecord {
  const timestamp = now();
  return {
    id: mintId(),
    name: normalizeWorkspaceName(name) || "Workspace",
    createdAt: timestamp,
    updatedAt: timestamp,
    terminals: [],
  };
}

/**
 * First-run migration: keep a saved roster verbatim, otherwise adopt every
 * already-running terminal into one workspace with the pre-workspace layout.
 */
export function initialTerminalWorkspaceRecords(
    saved: readonly TerminalWorkspaceRecord[], live: readonly TerminalSummary[], layout: string,
    mintId: () => string, now: () => string): TerminalWorkspaceRecord[] {
  if (saved.length > 0) return [...saved];
  const seed = { ...newTerminalWorkspace("Workspace 1", mintId, now), layout };
  return live.reduce<TerminalWorkspaceRecord[]>((records, terminal) => rememberTerminal(
    records, seed.id, terminal, terminal.launchSession, now,
  ), [seed]);
}

function sameLaunchSession(left: TerminalLaunchSessionV1 | undefined,
    right: TerminalLaunchSessionV1 | undefined): boolean {
  return left?.version === right?.version
    && left?.id === right?.id
    && left?.resume === right?.resume;
}

function sameLaunchRequest(left: TerminalLaunchRequestV1 | undefined,
    right: TerminalLaunchRequestV1 | undefined): boolean {
  return left?.version === right?.version
    && left?.profileId === right?.profileId;
}

function isConfirmedLaunchSession(binding: TerminalLaunchSessionV1 | undefined): boolean {
  return binding?.version === 1
    && typeof binding.id === "string"
    && binding.resume === true;
}

/**
 * Whether host-authoritative provider identity must be written to the saved
 * workspace roster.
 *
 * Most shell lifecycle traffic is intentionally ignored. The one shell state
 * that carries provider meaning is a demotion from a previously confirmed
 * Claude chat: persisting that exact transition prevents a later workspace
 * save from resurrecting a provider process that has already exited.
 */
export function shouldPersistAuthoritativeTerminalIdentity(
    saved: Pick<TerminalWorkspaceTerminal, "launch" | "launchSession"> | undefined,
    terminal: Pick<TerminalSummary, "launch" | "launchSession">): boolean {
  const binding = terminal.launchSession;
  if (isConfirmedLaunchSession(binding)) return true;
  if (binding?.version === 1 && terminal.launch?.profileId === "claude") return true;
  // Claude and Codex are the providers an armed shell can be promoted to, so
  // their demotion back to Shell must clear the finished chat.
  const savedConfirmedProvider = isConfirmedLaunchSession(saved?.launchSession)
    && (saved?.launch === undefined || saved.launch.profileId === "claude"
      || saved.launch.profileId === "codex");
  const incomingShell = terminal.launch === undefined
    || terminal.launch.profileId === "shell";
  return incomingShell
    && binding === undefined
    && savedConfirmedProvider;
}

/**
 * Reconcile provider identity that arrived before the saved roster finished
 * loading.
 *
 * The lifecycle subscription is intentionally installed before the async
 * workspace read. Its accepted upserts already live in the fenced terminal
 * registry, but cannot be filed while the workspace roster is still empty.
 * Apply that current provider truth to matching saved rows before the caller
 * exposes the loaded roster. Unknown live terminals are not adopted here.
 */
export function reconcileLiveTerminalWorkspaceBindings(
    records: readonly TerminalWorkspaceRecord[], live: readonly TerminalSummary[],
    now: () => string): readonly TerminalWorkspaceRecord[] {
  let reconciled = records;
  for (const terminal of live) {
    const binding = terminal.launchSession;
    const workspace = workspaceForTerminal(reconciled, terminal.id);
    const saved = workspace?.terminals.find(entry => entry.id === terminal.id);
    if (workspace === undefined || saved === undefined) continue;
    if (!shouldPersistAuthoritativeTerminalIdentity(saved, terminal)) continue;
    // Older host summaries can confirm a provider binding without echoing the
    // optional launch DTO. Absence is not authority to erase the saved profile:
    // doing so would retain the chat id while losing the launcher that resumes it.
    const terminalWithLaunch = terminal.launch === undefined && saved.launch !== undefined
      ? { ...terminal, launch: saved.launch }
      : terminal;
    if ((sameLaunchRequest(saved.launch, terminalWithLaunch.launch)
          && sameLaunchSession(saved.launchSession, binding))) continue;
    reconciled = rememberTerminal(
      reconciled, workspace.id, terminalWithLaunch, binding, now, "authoritative",
    );
  }
  return reconciled;
}

/** Reject a roster the host would refuse, so a corrupt file degrades to empty. */
export function normalizeTerminalWorkspaces(value: unknown): TerminalWorkspaceRecord[] {
  if (!Array.isArray(value)) return [];
  const ids = new Set<string>();
  const claimed = new Set<string>();
  const records: TerminalWorkspaceRecord[] = [];
  for (const candidate of value) {
    if (records.length === MAX_TERMINAL_WORKSPACES) break;
    if (typeof candidate !== "object" || candidate === null) continue;
    const record = candidate as TerminalWorkspaceRecord;
    if (!usableText(record.id) || ids.has(record.id)) continue;
    const name = normalizeWorkspaceName(record.name ?? "");
    if (name === "") continue;
    const terminals: TerminalWorkspaceTerminal[] = [];
    for (const entry of Array.isArray(record.terminals) ? record.terminals : []) {
      if (typeof entry !== "object" || entry === null) continue;
      const terminal = entry as TerminalWorkspaceTerminal;
      if (!usableText(terminal.id) || !MANAGED_CHAT_ID.test(terminal.id)
          || !usableText(terminal.cwd) || !terminal.cwd.startsWith("/")
          || !usableText(terminal.title)) continue;
      const launch = normalizedLaunchRequest(terminal.launch);
      // A present but invalid launch cannot cross the strict Rust IPC contract.
      // Drop that corrupt pane from the normalized roster instead of retrying
      // the same rejected payload after every app restart.
      if (terminal.launch !== undefined && launch === undefined) continue;
      // A terminal belongs to exactly one workspace: two claims on one id would
      // have both workspaces trying to recreate it.
      if (claimed.has(terminal.id)) continue;
      claimed.add(terminal.id);
      const launchSession = normalizedLaunchSession(
        terminal.launchSession, launch?.profileId,
      );
      terminals.push({
        id: terminal.id,
        title: currentProfileTitle(terminal.title, launch?.profileId),
        cwd: terminal.cwd,
        ...(usableText(terminal.sessionKey) ? { sessionKey: terminal.sessionKey } : {}),
        ...(launch === undefined ? {} : { launch }),
        ...(launchSession === undefined ? {} : { launchSession }),
      });
    }
    ids.add(record.id);
    records.push({ ...record, name, terminals });
  }
  return records;
}

export function workspaceById(records: readonly TerminalWorkspaceRecord[],
    id: string | null): TerminalWorkspaceRecord | undefined {
  return id === null ? undefined : records.find(record => record.id === id);
}

/** The workspace a terminal is filed under, if any. */
export function workspaceForTerminal(records: readonly TerminalWorkspaceRecord[],
    terminalId: string): TerminalWorkspaceRecord | undefined {
  return records.find(record => record.terminals.some(entry => entry.id === terminalId));
}

/** Merge only host-returned names whose submitted value is still current.
 * A response cannot roll back a newer rename, layout, or membership edit. */
export function reconcileSavedWorkspaceNames(
    current: readonly TerminalWorkspaceRecord[], submitted: readonly TerminalWorkspaceRecord[],
    saved: readonly TerminalWorkspaceRecord[]): readonly TerminalWorkspaceRecord[] {
  let changed = false;
  const next = current.map(record => {
    const before = submitted.find(item => item.id === record.id);
    const after = saved.find(item => item.id === record.id);
    if (before === undefined || after === undefined || record.name !== before.name || record.name === after.name) return record;
    changed = true;
    return { ...record, name: after.name };
  });
  return changed ? next : current;
}

function replace(records: readonly TerminalWorkspaceRecord[],
    updated: TerminalWorkspaceRecord, now: () => string): TerminalWorkspaceRecord[] {
  return records.map(record => record.id === updated.id
    ? { ...updated, updatedAt: now() } : record);
}

/** File a live terminal under one workspace, replacing any earlier entry. */
export function rememberTerminal(records: readonly TerminalWorkspaceRecord[],
    workspaceId: string, terminal: TerminalSummary,
    binding: TerminalLaunchSessionV1 | undefined,
    now: () => string,
    bindingAuthority: "monotonic" | "authoritative" = "monotonic"):
    TerminalWorkspaceRecord[] {
  const workspace = workspaceById(records, workspaceId);
  if (workspace === undefined) return records as TerminalWorkspaceRecord[];
  const previous = workspace.terminals.find(entry => entry.id === terminal.id);
  // The live summary is authoritative about its own binding; the caller's is
  // the fallback for a host that did not echo the field back. A confirmed
  // provider identity is monotonic, though: the immediate create response may
  // arrive after the lifecycle upsert that confirmed it, and its intentionally
  // id-less fresh binding must not erase the resumable identity. The launch and
  // binding are one reopen identity, so a stale shell summary must not retain
  // the Claude session while replacing the profile that knows how to resume it.
  // A binding-bearing legacy summary may omit the optional launch DTO; preserve
  // the saved half of the pair unless the host supplies an explicit profile.
  const candidate = terminal.launchSession ?? binding;
  const incomingLaunch = terminal.launch === undefined && candidate !== undefined
    ? previous?.launch
    : terminal.launch;
  const previousConfirmed = previous?.launchSession?.version === 1
    && typeof previous.launchSession.id === "string"
    && isManagedChatId(previous.launchSession.id)
    && previous.launchSession.resume === true;
  const candidateConfirmed = candidate?.version === 1
    && typeof candidate.id === "string"
    && isManagedChatId(candidate.id)
    && candidate.resume === true;
  const preservePreviousIdentity = bindingAuthority !== "authoritative"
    && previousConfirmed
    && (!candidateConfirmed || !sameLaunchRequest(previous?.launch, incomingLaunch));
  const launch = preservePreviousIdentity ? previous?.launch : incomingLaunch;
  const launchSession = preservePreviousIdentity ? previous?.launchSession : candidate;
  const cwd = usableText(terminal.cwd)
    ? terminal.cwd
    : previous?.cwd;
  if (!usableText(cwd)) return records as TerminalWorkspaceRecord[];
  const entry: TerminalWorkspaceTerminal = {
    id: terminal.id,
    title: usableTerminalTitle(terminal.title),
    cwd,
    ...(terminal.sessionKey === undefined ? {} : { sessionKey: terminal.sessionKey }),
    ...(launch === undefined ? {} : { launch }),
    ...(launchSession === undefined ? {} : { launchSession }),
  };
  // Resize and fast-mode Upserts change nothing saved here. Re-stamping
  // updatedAt would re-render the center and write an unchanged roster.
  if (previous !== undefined && equalValue(previous, entry)) {
    return records as TerminalWorkspaceRecord[];
  }
  const terminals = workspace.terminals.some(candidate => candidate.id === terminal.id)
    ? workspace.terminals.map(candidate => candidate.id === terminal.id ? entry : candidate)
    : [...workspace.terminals, entry];
  return replace(records, { ...workspace, terminals }, now);
}

/** Drop a terminal from wherever it was filed. */
export function forgetTerminal(records: readonly TerminalWorkspaceRecord[],
    terminalId: string, now: () => string): TerminalWorkspaceRecord[] {
  const workspace = workspaceForTerminal(records, terminalId);
  if (workspace === undefined) return records as TerminalWorkspaceRecord[];
  return replace(records, {
    ...workspace,
    terminals: workspace.terminals.filter(entry => entry.id !== terminalId),
  }, now);
}

/** Drop a saved workspace. Missing ids leave the roster untouched. */
export function forgetWorkspace(records: readonly TerminalWorkspaceRecord[],
    id: string): TerminalWorkspaceRecord[] {
  if (records.every(record => record.id !== id)) {
    return records as TerminalWorkspaceRecord[];
  }
  return records.filter(record => record.id !== id);
}

/**
 * The workspace to show after `id` is removed: the following neighbor, else
 * the previous one. None means the roster will be empty.
 */
export function neighboringWorkspaceId(records: readonly TerminalWorkspaceRecord[],
    id: string): string | null {
  const index = records.findIndex(record => record.id === id);
  if (index < 0) return null;
  return records[index + 1]?.id ?? records[index - 1]?.id ?? null;
}

/** Confirm-dialog copy for deleting a named workspace. */
export function workspaceDeleteConfirm(name: string, terminalCount: number): {
  title: string;
  description: string;
  confirmLabel: string;
} {
  const title = `Delete ${name}?`;
  const confirmLabel = "Delete workspace";
  if (terminalCount <= 0) {
    return {
      title,
      confirmLabel,
      description: `This removes ${name} from ${productLabel()}. It cannot be undone.`,
    };
  }
  const unit = terminalCount === 1 ? "terminal" : "terminals";
  const those = terminalCount === 1 ? "that terminal" : "those terminals";
  return {
    title,
    confirmLabel,
    description: `This removes ${name} and stops its ${terminalCount.toString()} ${unit}. `
      + `Running processes in ${those} end. This cannot be undone.`,
  };
}

/**
 * A ⌘D split mints an unlaunched shell and files it. Closing that tab must
 * not keep respawning an `Interface-Rust`-titled clone in the overview.
 * Agent launches keep their roster row even when parked off-layout.
 */
function keepWorkspaceTerminal(entry: {
  readonly id: string;
  readonly launch?: TerminalLaunchRequestV1;
}, layoutTerminalIds: ReadonlySet<string>): boolean {
  return entry.launch !== undefined || layoutTerminalIds.has(entry.id);
}

function layoutTerminalIdsFor(layout: string | undefined): ReadonlySet<string> {
  return savedLayoutFacts(layout).terminalIds;
}

/**
 * Which saved panes come back with their workspace: the ones its saved layout
 * still shows. Closing a tab ends its terminal (owner, 2026-10-01); before
 * that, a closed tab kept its saved pane and respawned unseen on every launch.
 * A record saved before layouts existed brings back every launched pane.
 */
function returnsWithWorkspace(workspace: Pick<TerminalWorkspaceRecord, "layout">):
    (entry: TerminalWorkspaceTerminal) => boolean {
  if (workspace.layout === undefined) return entry => entry.launch !== undefined;
  const shown = layoutTerminalIdsFor(workspace.layout);
  return entry => shown.has(entry.id);
}

/**
 * The roster without tabs closed in an earlier run: saved panes their own
 * layout no longer shows and that have no live terminal. Nothing will reopen
 * them, so they would only count against the workspace and its delete prompt.
 */
export function forgetClosedTabs(records: readonly TerminalWorkspaceRecord[],
    live: readonly TerminalSummary[], now: () => string): TerminalWorkspaceRecord[] {
  const running = new Set(live.map(terminal => terminal.id));
  let next = records as TerminalWorkspaceRecord[];
  for (const workspace of records) {
    const returns = returnsWithWorkspace(workspace);
    const terminals = workspace.terminals.filter(entry => running.has(entry.id) || returns(entry));
    if (terminals.length !== workspace.terminals.length) {
      next = replace(next, { ...workspace, terminals }, now);
    }
  }
  return next;
}

/**
 * Store the pane arrangement with the workspace it belongs to. A caller that
 * holds the layout it serialized passes its terminal ids rather than having
 * the string parsed back; the saved roster list is kept when nothing leaves it.
 */
export function rememberLayout(records: readonly TerminalWorkspaceRecord[],
    workspaceId: string, layout: string, now: () => string,
    layoutTerminalIds: ReadonlySet<string> = layoutTerminalIdsFor(layout)): TerminalWorkspaceRecord[] {
  const workspace = workspaceById(records, workspaceId);
  if (workspace === undefined) return records as TerminalWorkspaceRecord[];
  const kept = workspace.terminals.filter(entry =>
    keepWorkspaceTerminal(entry, layoutTerminalIds));
  const terminals = kept.length === workspace.terminals.length ? workspace.terminals : kept;
  if (workspace.layout === layout && terminals === workspace.terminals) {
    return records as TerminalWorkspaceRecord[];
  }
  return replace(records, { ...workspace, layout, terminals }, now);
}

type TerminalSwitcherRecord = TerminalWorkspaceSwitcher["records"][number];

/**
 * The switcher's rows: only what the sidebar shows. The active workspace's
 * open count comes from its live layout; the others are parsed once per saved
 * layout string. The previous list is returned while every row is equal, so a
 * splitter drag or pane focus (a new layout each frame) leaves App alone.
 */
export function terminalSwitcherRecords(previous: readonly TerminalSwitcherRecord[] | undefined,
    records: readonly TerminalWorkspaceRecord[], activeId: string | null,
    activeOpenIds: readonly string[]): readonly TerminalSwitcherRecord[] {
  const next = records.map(record => {
    // Membership deliberately retains launched terminals after their tabs
    // close so they can be resumed. The sidebar count is a layout fact, not
    // that durable roster: use the live layout for the active workspace so
    // opening or closing a tab updates the badge in the same render.
    const firstTerminalTitle = record.terminals[0]?.title;
    const openTerminalIds = record.id === activeId
      ? activeOpenIds : savedLayoutFacts(record.layout).terminalIdList;
    return {
      id: record.id,
      name: record.name,
      ...(firstTerminalTitle === undefined ? {} : { firstTerminalTitle }),
      openTerminalCount: openTerminalIds.length,
      openTerminalIds,
      terminalIds: record.terminals.map(terminal => terminal.id),
      cwd: record.terminals[0]?.cwd,
    };
  });
  return previous !== undefined && equalValue(previous, next) ? previous : next;
}

/**
 * Live terminals the open workspace should draw: owned by this roster, and
 * either a launched profile or still attached as a pane/tab.
 */
export function visibleWorkspaceTerminals(
    live: readonly TerminalSummary[],
    workspaceTerminalIds: ReadonlySet<string>,
    layoutTerminalIds: ReadonlySet<string>): TerminalSummary[] {
  return live.filter(terminal => layoutTerminalIds.has(terminal.id)
    || (workspaceTerminalIds.has(terminal.id) && terminal.launch !== undefined));
}

function directoryText(value: string | null | undefined): string | null {
  if (typeof value !== "string") return null;
  const trimmed = value.trim();
  if (trimmed === "" || trimmed === "/") return trimmed === "/" ? "/" : null;
  return trimmed.endsWith("/") ? trimmed.replace(/\/+$/u, "") : trimmed;
}

/** True when `child` is inside `parent`, not the same directory. */
function isStrictAncestor(parent: string, child: string): boolean {
  if (parent === "/") return child !== "/";
  return child.startsWith(`${parent}/`);
}

/**
 * Where a new pane in an existing workspace starts.
 *
 * The pane being split, or the last explicit folder, wins — unless that
 * directory is only a parent of the project this workspace already works in.
 * `~/Desktop/Projects` is that parent for every project under it, and a Grok
 * pane opened beside those files was starting there instead of in the project.
 * The first saved terminal is the workspace's project; a deeper checkout the
 * user is actually in is left alone.
 */
export function paneStartDirectory(
  sourceCwd: string | null | undefined,
  workspaceCwds: readonly (string | null | undefined)[],
  fallbackCwd: string | null | undefined,
): string | null {
  const chosen = directoryText(sourceCwd) ?? directoryText(fallbackCwd);
  const project = workspaceCwds
    .map(directoryText)
    .find((cwd): cwd is string => cwd !== null) ?? null;
  if (chosen !== null && project !== null && isStrictAncestor(chosen, project)) return project;
  return chosen ?? project;
}

/**
 * What to try when the host refuses to reopen a plain shell: the same pane as
 * a standard terminal, first in its saved folder, then in the always-approved
 * default directory. A pane with a chat binding gets nothing here — a refused
 * resume must not quietly turn into a shell.
 */
export function shellFallbackRequests(
  request: CreateTerminalRequest,
  defaultCwd: string | null,
): CreateTerminalRequest[] {
  if (request.launchSession !== undefined) return [];
  const plain: CreateTerminalRequest = { id: request.id, cwd: request.cwd, title: "Terminal" };
  if (defaultCwd === null || defaultCwd === request.cwd) return [plain];
  return [plain, { ...plain, cwd: defaultCwd }];
}

/** `path` moved from under `from` to the same place under `to`, or `null`. */
export function movedFolderPath(path: string, from: string, to: string): string | null {
  if (path === from) return to;
  return isStrictAncestor(from, path) ? `${to}${path.slice(from.length)}` : null;
}

/**
 * The roster after a folder the owner confirmed has moved: every saved pane
 * inside `from` now lives at the same place under `to`. Folders merely beside
 * it (`app-archive` next to `app`) are untouched. The same roster comes back
 * when no pane lived there.
 */
export function relocateTerminalWorkspaces(
  records: readonly TerminalWorkspaceRecord[], from: string, to: string,
): readonly TerminalWorkspaceRecord[] {
  let changed = false;
  const next = records.map(record => {
    let recordChanged = false;
    const terminals = record.terminals.map(terminal => {
      const cwd = movedFolderPath(terminal.cwd, from, to);
      if (cwd === null) return terminal;
      recordChanged = true;
      return { ...terminal, cwd };
    });
    if (!recordChanged) return record;
    changed = true;
    return { ...record, terminals };
  });
  return changed ? next : records;
}

/**
 * Saved terminals whose reopen the host refused and whose tabs are still in
 * the layout. Each carries the host's reason so the pane can read as failed
 * instead of as an empty pane that is forever starting.
 */
export function failedReopenTerminals(
  workspace: Pick<TerminalWorkspaceRecord, "terminals"> | undefined,
  failures: ReadonlyMap<string, string>,
  live: readonly { readonly id: string }[],
  layout: Pick<TerminalWorkspaceState, "panes">,
): { id: string; title: string; cwd: string; profileId?: TerminalLaunchProfileId; message: string }[] {
  if (workspace === undefined || failures.size === 0) return [];
  const running = new Set(live.map(terminal => terminal.id));
  const shown = terminalIdsInWorkspaceState(layout);
  return workspace.terminals
    .filter(entry => !running.has(entry.id) && shown.has(entry.id) && failures.has(entry.id))
    .map(entry => ({
      id: entry.id,
      title: currentProfileTitle(entry.title, entry.launch?.profileId),
      cwd: entry.cwd,
      ...(entry.launch === undefined ? {} : { profileId: entry.launch.profileId }),
      message: failures.get(entry.id) ?? "",
    }));
}

/**
 * What has to be spawned to put a saved workspace back on screen.
 *
 * Terminals already in the registry are skipped rather than recreated: within
 * one app run switching away only hid them, so they are still running.
 * Saved panes the layout no longer shows are skipped too: they are closed
 * tabs, not a session the layout still shows.
 */
export function reopenRequests(workspace: TerminalWorkspaceRecord,
    live: readonly TerminalSummary[],
    mintId: () => string): CreateTerminalRequest[] {
  // A saved agent a live shell `replaces` (protocol 50) ended by itself and
  // was handed that shell by Rust: the shell takes its tab, it is not reopened.
  const running = new Set(live.flatMap(terminal => terminal.replaces === undefined
    ? [terminal.id] : [terminal.id, terminal.replaces]));
  const returns = returnsWithWorkspace(workspace);
  return workspace.terminals
    .filter(entry => !running.has(entry.id) && returns(entry))
    .map(entry => {
      const launch: TerminalLaunchRequestV1 | undefined = entry.launch;
      const binding = reopenedChatBinding(entry.launchSession, launch?.profileId, mintId);
      return {
        id: entry.id,
        cwd: entry.cwd,
        title: currentProfileTitle(entry.title, launch?.profileId),
        ...(entry.sessionKey === undefined ? {} : { sessionKey: entry.sessionKey }),
        ...(launch === undefined ? {} : { launch }),
        ...(binding === undefined ? {} : { launchSession: binding }),
      };
    });
}
