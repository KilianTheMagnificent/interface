import {
  useEffect, useId, useMemo, useRef, useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import type {
  TerminalLaunchProfile, TerminalLaunchProfileId,
} from "../../shared/ipc";
import { PROVIDER_LOGO_CSS, ProviderLogo, providerLogoKind } from "./ProviderLogo";
import { InterfaceMark } from "./InterfaceMark";
import { InstallHint } from "./InstallHint";
import { homeRelativePath, lastSegment } from "../display-paths";
import "../terminal.css";

export const TERMINAL_LAUNCH_COUNTS = Object.freeze(
  Array.from({ length: 8 }, (_, index) => index + 1),
);

export interface TerminalAdversarialReviewLaunchRequest {
  readonly builderProfileId: TerminalLaunchProfileId;
  readonly reviewerProfileId: TerminalLaunchProfileId;
  readonly cwd: string;
  readonly objective: string;
}

export interface TerminalAdversarialReviewPair {
  readonly builder: TerminalLaunchProfile;
  readonly reviewer: TerminalLaunchProfile;
}

export interface TerminalLauncherNotice {
  readonly kind: "error" | "partial";
  readonly message: string;
}

export interface TerminalLauncherProps {
  readonly profiles: readonly TerminalLaunchProfile[];
  readonly profilesLoading: boolean;
  readonly cwd: string | null;
  readonly launching: boolean;
  readonly notice: TerminalLauncherNotice | null;
  readonly onLaunch: (
    profileId: TerminalLaunchProfileId,
    count: number,
    cwd: string,
  ) => void;
  /**
   * Open the adversarial-review dialog. Absent until the host-owned workflow is
   * connected — the renderer never approximates review with two terminals, so
   * there is nothing to offer a link to.
   */
  readonly onOpenReview?: () => void;
  /** Profile and count the last launch used, remembered for the session. */
  readonly defaultProfileId?: TerminalLaunchProfileId | null;
  readonly defaultCount?: number;
  /** False while the terminal surface is hidden: focus waits rather than burning. */
  readonly visible?: boolean;
  readonly onBrowse: (preferredPath?: string) => Promise<string | null>;
  /** A folder chosen with Change…, reported up so the review dialog sees it too. */
  readonly onCwdChange?: (cwd: string) => void;
  readonly onRetryProfiles: () => void;
  readonly onCancel?: () => void;
  /**
   * Each provider's plan headroom, by engine id (review E): a quiet "38% left"
   * under an agent tile's name, its window and reset in the tooltip.
   */
  readonly headroom?: Readonly<Record<string, { readonly text: string; readonly title: string }>>;
}

function orderedProfiles(profiles: readonly TerminalLaunchProfile[]): TerminalLaunchProfile[] {
  const agents = profiles.filter(profile => profile.engine !== undefined);
  const shells = profiles.filter(profile => profile.engine === undefined);
  return [...agents, ...shells];
}

/**
 * What the grid shows and what folds under More (review C4). Claude Code, Codex
 * and Terminal are always tiles, installed or not; any other tool that is not
 * installed folds, so a new user's grid holds what they can actually run.
 */
const ALWAYS_TILED: ReadonlySet<TerminalLaunchProfileId> = new Set(["claude", "codex", "shell"]);

export function terminalLauncherFolds(profiles: readonly TerminalLaunchProfile[]): {
  readonly tiles: TerminalLaunchProfile[];
  readonly folded: TerminalLaunchProfile[];
} {
  const ordered = orderedProfiles(profiles);
  const tiled = (profile: TerminalLaunchProfile) => profile.available || ALWAYS_TILED.has(profile.id);
  return { tiles: ordered.filter(tiled), folded: ordered.filter(profile => !tiled(profile)) };
}

/** A folded tool's one line: Rust's install hint, else its reason, else ours. */
export function terminalLauncherInstallHint(profile: TerminalLaunchProfile): string {
  return profile.installHint?.trim() || profile.unavailableReason?.trim()
    || `Install ${profile.label}, then press Refresh.`;
}

function preferredProfile(profiles: readonly TerminalLaunchProfile[]): TerminalLaunchProfile | null {
  return profiles.find(profile => profile.available && profile.engine !== undefined)
    ?? profiles.find(profile => profile.available)
    ?? null;
}

/**
 * Adversarial review is intentionally cross-provider: Codex builds and Claude
 * reviews, or Claude builds and Codex reviews. Other profiles can still launch
 * ordinary terminal batches, but are not represented as review-capable until a
 * real reviewer workflow exists for them.
 */
export function terminalLauncherAdversarialPair(
    profiles: readonly TerminalLaunchProfile[],
    builderId: TerminalLaunchProfileId | null): TerminalAdversarialReviewPair | null {
  const builder = profiles.find(profile => profile.id === builderId && profile.available);
  if (builder === undefined || (builder.id !== "claude" && builder.id !== "codex")) return null;
  const reviewerId: TerminalLaunchProfileId = builder.id === "codex" ? "claude" : "codex";
  const reviewer = profiles.find(profile => profile.id === reviewerId && profile.available);
  return reviewer === undefined ? null : { builder, reviewer };
}

/**
 * One roving selection over one list: Home and End take the ends, the arrows
 * step and wrap, and anything else is not ours. A current value the list does
 * not hold enters from whichever end the arrow came from. All three of the
 * launcher's dials are this, over three different lists.
 */
function rovingChoice<T>(values: readonly T[], current: T | null, key: string): T | null {
  if (values.length === 0) return null;
  if (key === "Home") return values[0] ?? null;
  if (key === "End") return values.at(-1) ?? null;
  const delta = key === "ArrowLeft" || key === "ArrowUp" ? -1
    : key === "ArrowRight" || key === "ArrowDown" ? 1 : 0;
  if (delta === 0) return null;
  const at = current === null ? -1 : values.indexOf(current);
  const fallback = delta < 0 ? values.length - 1 : 0;
  return values[at < 0 ? fallback : (at + delta + values.length) % values.length] ?? null;
}

export function terminalLauncherProfileForKey(
    profiles: readonly TerminalLaunchProfile[], currentId: TerminalLaunchProfileId | null,
    key: string): TerminalLaunchProfileId | null {
  const enabled = orderedProfiles(profiles).filter(profile => profile.available);
  return rovingChoice(enabled.map(profile => profile.id), currentId, key);
}

export function terminalLauncherCountForKey(count: number, key: string): number | null {
  return rovingChoice(TERMINAL_LAUNCH_COUNTS, count, key);
}

export function terminalLauncherActionLabel(
    profile: TerminalLaunchProfile | null, count: number): string {
  if (profile === null) return "Choose an agent";
  const subject = profile.engine === undefined ? "shell" : profile.label;
  return `Launch ${count.toString()} ${subject} terminal${count === 1 ? "" : "s"}`;
}

/** The Isolation line, counted the way the action label counts terminals. */
export function terminalLauncherIsolationNote(count: number): string {
  return count === 1 ? "The terminal works in the folder below."
    : `All ${count.toString()} terminals work in the folder below.`;
}

/** Whether a key press is the launcher's ⌘↵ (Ctrl+Enter off macOS). */
export function terminalLauncherIsLaunchKey(event: {
  readonly key: string; readonly metaKey: boolean; readonly ctrlKey: boolean;
}): boolean {
  return event.key === "Enter" && (event.metaKey || event.ctrlKey);
}

export function terminalLauncherPickedDirectory(
    current: string, picked: string | null): string {
  return picked === null || picked.trim() === "" ? current : picked;
}

function usableLauncherDirectory(value: string | null | undefined): string | null {
  return typeof value === "string" && value.trim() !== "" ? value : null;
}

/**
 * The folder the launcher SHOWS and the review dialog launches in. An explicit
 * Change… wins until the surrounding context itself moves, because
 * `terminalLauncherDefaultCwd` below prefers the focused terminal's folder:
 * without this the launcher named the folder you had just browsed to and the
 * review dialog beside it named the focused pane's, then launched in that one.
 */
export function terminalLauncherChosenCwd(
    browsedCwd: string | null | undefined,
    contextualCwd: string | null | undefined): string | null {
  return terminalLauncherDefaultCwd(browsedCwd, null, contextualCwd);
}

/**
 * Whether the launcher may claim focus yet. The terminal centre is mounted from
 * app start behind `display:none`, and an empty workspace forces the launcher
 * open inside it — so a one-shot focus guard that burned on the first hidden
 * render meant the launcher never took focus at all. `offsetParent` is null for
 * everything inside a display:none subtree.
 */
export function terminalLauncherCanTakeFocus(card: {
  readonly isConnected: boolean;
  readonly offsetParent: unknown;
} | null | undefined): boolean {
  return card !== null && card !== undefined
    && card.isConnected && card.offsetParent !== null;
}

/**
 * Empty-workspace launch starts at the last directory the user picked, not
 * the first-run Projects folder. A focused terminal still wins because that
 * workspace already has a working folder.
 */
export function terminalLauncherDefaultCwd(
    focusedCwd: string | null | undefined,
    lastSelectedCwd: string | null | undefined,
    fallbackCwd: string | null | undefined): string | null {
  return usableLauncherDirectory(focusedCwd)
    ?? usableLauncherDirectory(lastSelectedCwd)
    ?? usableLauncherDirectory(fallbackCwd);
}

function terminalLauncherChoiceTitle(profile: TerminalLaunchProfile): string {
  if (profile.available) return profile.engine === undefined
    ? "Open ordinary interactive shells" : `Launch ${profile.label} in each terminal`;
  return profile.unavailableReason?.trim() || `${profile.label} is unavailable`;
}

export function TerminalLauncher({ profiles, profilesLoading, cwd, launching, notice,
    onLaunch, onOpenReview, defaultProfileId, defaultCount = 1, visible = true,
    onBrowse, onCwdChange, onRetryProfiles, onCancel, headroom }: TerminalLauncherProps) {
  const baseId = `${useId()}-terminal-launcher`;
  // Rust owns which profiles are exposed; React only lays that projection out.
  const choices = useMemo(() => orderedProfiles(profiles), [profiles]);
  const { tiles, folded } = useMemo(() => terminalLauncherFolds(profiles), [profiles]);
  // The launcher unmounts on close, so "remembered" is a default handed in by
  // the surface that outlives it, not state kept here.
  const [profileId, setProfileId] = useState<TerminalLaunchProfileId | null>(() =>
    defaultProfileId ?? preferredProfile(choices)?.id ?? null);
  const [count, setCount] = useState(defaultCount);
  const [workingDirectory, setWorkingDirectory] = useState(
    cwd !== null && cwd.trim() !== "" ? cwd : "",
  );
  const [browsing, setBrowsing] = useState(false);
  const [directoryError, setDirectoryError] = useState<string | null>(null);
  const profileRefs = useRef(new Map<TerminalLaunchProfileId, HTMLButtonElement>());
  const countRefs = useRef(new Map<number, HTMLButtonElement>());

  useEffect(() => {
    const selected = choices.find(profile => profile.id === profileId && profile.available);
    if (selected === undefined) setProfileId(preferredProfile(choices)?.id ?? null);
  }, [choices, profileId]);

  useEffect(() => {
    setWorkingDirectory(cwd !== null && cwd.trim() !== "" ? cwd : "");
    setDirectoryError(null);
  }, [cwd]);

  const selected = choices.find(profile => profile.id === profileId && profile.available) ?? null;
  const rovingProfileId = selected?.id ?? preferredProfile(choices)?.id ?? null;

  // Focus lands ONCE, on the card the launcher opens with — a keyboard user
  // otherwise arrived at a surface-filling form with focus wherever it was.
  // The guard burns only on a card that could actually take it: Rust answers
  // the profile roster after mount, and the centre is mounted from app start
  // behind display:none, so both a missing card and a hidden one have to mean
  // "not yet" rather than "done". `visible` is the retry trigger — the surface
  // becoming visible changes no other prop this effect reads.
  const focusedForOpen = useRef(false);
  useEffect(() => {
    if (focusedForOpen.current || !visible || rovingProfileId === null) return;
    const card = profileRefs.current.get(rovingProfileId);
    if (!terminalLauncherCanTakeFocus(card)) return;
    focusedForOpen.current = true;
    card?.focus();
  }, [choices, rovingProfileId, visible]);
  const launchDisabled = launching || browsing || selected === null || workingDirectory === "";
  const launch = (): void => {
    if (!launchDisabled && selected !== null) onLaunch(selected.id, count, workingDirectory);
  };
  const folderName = workingDirectory === "" ? "" : lastSegment(workingDirectory) || workingDirectory;

  const browse = async (): Promise<void> => {
    if (browsing || launching) return;
    setBrowsing(true);
    setDirectoryError(null);
    try {
      const picked = await onBrowse(workingDirectory || undefined);
      const chosen = terminalLauncherPickedDirectory(workingDirectory, picked);
      setWorkingDirectory(chosen);
      // The centre keeps this, not the launcher: the review dialog reads the
      // same value, and the launcher unmounts on close.
      if (chosen !== workingDirectory) onCwdChange?.(chosen);
    } catch {
      setDirectoryError("Could not open the folder picker. Try again.");
    } finally {
      setBrowsing(false);
    }
  };

  const moveProfile = (event: ReactKeyboardEvent<HTMLButtonElement>): void => {
    const nextId = terminalLauncherProfileForKey(choices, profileId, event.key);
    if (nextId === null) return;
    event.preventDefault();
    setProfileId(nextId);
    profileRefs.current.get(nextId)?.focus();
  };

  const moveCount = (event: ReactKeyboardEvent<HTMLButtonElement>): void => {
    const next = terminalLauncherCountForKey(count, event.key);
    if (next === null) return;
    event.preventDefault();
    setCount(next);
    countRefs.current.get(next)?.focus();
  };

  const renderProfile = (profile: TerminalLaunchProfile) => {
    const checked = profile.id === profileId && profile.available;
    const reasonId = `${baseId}-profile-${encodeURIComponent(profile.id)}-reason`;
    const title = terminalLauncherChoiceTitle(profile);
    const left = profile.available && profile.engine !== undefined
      ? headroom?.[profile.engine] : undefined;
    return (
      <button key={profile.id} ref={node => {
        if (node === null) profileRefs.current.delete(profile.id);
        else profileRefs.current.set(profile.id, node);
      }} type="button" role="radio" aria-checked={checked}
        aria-label={profile.available
          ? (left === undefined ? profile.label : `${profile.label}, ${left.text}`)
          : `${profile.label}, unavailable. ${title}`}
        aria-describedby={!profile.available ? reasonId : undefined}
        tabIndex={profile.id === rovingProfileId ? 0 : -1}
        className={`terminal-launcher-profile${checked ? " is-selected" : ""}${
          profile.engine === undefined ? " is-shell" : ""}${profile.available ? "" : " is-unavailable"}`}
        data-terminal-launch-profile={profile.id}
        data-provider={profile.engine === undefined ? undefined : providerLogoKind(profile.engine)}
        disabled={!profile.available || launching}
        title={left === undefined ? title : `${title} — ${left.title}`}
        onKeyDown={moveProfile} onClick={() => { setProfileId(profile.id); }}>
        <span className="terminal-launcher-profile-mark" aria-hidden="true">
          {profile.engine === undefined
            ? <span className="codicon codicon-terminal" />
            : <ProviderLogo engine={profile.engine} label={profile.label} size={20} />}
        </span>
        <span className="terminal-launcher-profile-copy">
          <span className="terminal-launcher-profile-label">{profile.label}</span>
          {/* The full reason stays in the tooltip and the accessible name; the
              tile says only enough to tell it apart from a launchable one. */}
          {!profile.available && (
            <span className="terminal-launcher-profile-note" aria-hidden="true">Needs setup</span>
          )}
          {left !== undefined && (
            <span className="terminal-launcher-profile-note terminal-launcher-headroom"
              aria-hidden="true">{left.text}</span>
          )}
        </span>
        {checked && (
          <span className="terminal-launcher-profile-check codicon codicon-check"
            aria-hidden="true" />
        )}
        {!profile.available && (
          <span id={reasonId} className="terminal-launcher-profile-reason">{title}</span>
        )}
      </button>
    );
  };

  return (
    <section className="terminal-launcher" aria-labelledby={`${baseId}-title`}
      aria-busy={launching || undefined}
      /* Escape is the safe exit every other overlay on this surface already
         gives — the three menus, the rename dialog, both confirms. NOT a focus
         trap: the toolbar above the launcher stays live. */
      onKeyDown={event => {
        if (terminalLauncherIsLaunchKey(event)) {
          event.preventDefault();
          event.stopPropagation();
          launch();
          return;
        }
        if (event.key !== "Escape" || onCancel === undefined || launching) return;
        event.preventDefault();
        event.stopPropagation();
        onCancel();
      }}>
      {/* The launcher renders no `AppSelect`, so nothing else guarantees
          `PROVIDER_LOGO_CSS` is in the document when it mounts: it ships its
          own copy every time rather than taking an opt-out no caller could
          honestly pass. */}
      <style>{PROVIDER_LOGO_CSS}</style>
      <div className="terminal-launcher-card">
        <div className="terminal-launcher-heading">
          <div className="terminal-launcher-heading-context">
            <span className="terminal-launcher-product" aria-hidden="true">
              <InterfaceMark className="terminal-launcher-product-mark" />
            </span>
            <h2 id={`${baseId}-title`}>New workspace</h2>
          </div>
          {onCancel !== undefined && (
            <button type="button" className="terminal-launcher-close"
              aria-label="Back to terminal workspace" title="Back"
              disabled={launching} onClick={onCancel}>
              <span className="codicon codicon-close" aria-hidden="true" />
            </button>
          )}
        </div>

        <div className="terminal-launcher-form">
          <div className="terminal-launcher-section">
            <h3 id={`${baseId}-agent-label`}>Agent</h3>
            <div className="terminal-launcher-profile-group" role="radiogroup"
              aria-labelledby={`${baseId}-agent-label`}>
              <div className="terminal-launcher-agent-grid">
                {tiles.map(renderProfile)}
              </div>
            </div>
            {folded.length > 0 && (
              <details className="terminal-launcher-more">
                <summary>
                  <span className="terminal-launcher-more-chevron codicon codicon-chevron-right"
                    aria-hidden="true" />
                  <span className="terminal-launcher-more-label">More agents</span>
                  <span className="terminal-launcher-more-count">
                    {folded.length.toString()} not installed
                  </span>
                </summary>
                <ul className="terminal-launcher-more-list">
                  {folded.map(profile => (
                    <li key={profile.id} className="terminal-launcher-more-item"
                      data-terminal-launch-folded={profile.id}>
                      <span className="terminal-launcher-profile-mark" aria-hidden="true">
                        {profile.engine === undefined
                          ? <span className="codicon codicon-terminal" />
                          : <ProviderLogo engine={profile.engine} label={profile.label} size={20} />}
                      </span>
                      <span className="terminal-launcher-more-copy">
                        <span className="terminal-launcher-more-name">{profile.label}</span>
                        <span className="terminal-launcher-more-hint">
                          <InstallHint hint={terminalLauncherInstallHint(profile)} />
                        </span>
                      </span>
                    </li>
                  ))}
                </ul>
                {/* The hints end "then press Refresh": this is that Refresh,
                    the same catalog rebuild as Try again and Settings. */}
                <div className="terminal-launcher-more-actions">
                  <button type="button" className="btn terminal-launcher-more-refresh"
                    disabled={launching || profilesLoading} onClick={onRetryProfiles}>
                    <span className="codicon codicon-refresh" aria-hidden="true" />
                    Refresh
                  </button>
                </div>
              </details>
            )}
            {profilesLoading && (
              <p className="terminal-launcher-profile-status" role="status">
                Checking installed agents…
              </p>
            )}
            {!profilesLoading && choices.length === 0 && (
              <div className="terminal-launcher-profile-status is-error" role="alert">
                <span>No terminal launchers are available.</span>
                <button type="button" className="btn" onClick={onRetryProfiles}>Try again</button>
              </div>
            )}
          </div>

          <div className="terminal-launcher-section">
            <h3 id={`${baseId}-count-label`}>How many</h3>
            <div className="terminal-launcher-count-row">
              <div className="terminal-launcher-count-group" role="radiogroup"
                aria-labelledby={`${baseId}-count-label`}>
                {TERMINAL_LAUNCH_COUNTS.map(value => (
                  <button key={value} ref={node => {
                    if (node === null) countRefs.current.delete(value);
                    else countRefs.current.set(value, node);
                  }} type="button" role="radio"
                    aria-checked={value === count}
                    aria-label={`${value.toString()} terminal${value === 1 ? "" : "s"}`}
                    tabIndex={value === count ? 0 : -1}
                    className={`terminal-launcher-count${
                      value === count ? " is-selected" : ""}`}
                    disabled={launching} onKeyDown={moveCount}
                    onClick={() => { setCount(value); }}>
                    {value}
                  </button>
                ))}
              </div>
              <span className="terminal-launcher-count-unit">
                {count === 1 ? "terminal" : "terminals"}
              </span>
            </div>
          </div>

          <div className="terminal-launcher-section">
            <h3 id={`${baseId}-isolation-label`}>Isolation</h3>
            <p className="terminal-launcher-isolation" aria-labelledby={`${baseId}-isolation-label`}>
              <span className="codicon codicon-folder" aria-hidden="true" />
              <strong>Local checkout</strong>
              <span>{terminalLauncherIsolationNote(count)}</span>
            </p>
          </div>

          <div className="terminal-launcher-section">
            <h3 id={`${baseId}-cwd-label`}>Working directory</h3>
            <div className="terminal-launcher-cwd" aria-labelledby={`${baseId}-cwd-label`}>
              <span className="terminal-launcher-cwd-icon codicon codicon-folder-opened"
                aria-hidden="true" />
              <span className="terminal-launcher-cwd-copy">
                {folderName !== "" && (
                  <span className="terminal-launcher-cwd-name">{folderName}</span>
                )}
                {/* `~/…` as a terminal prints it; the title keeps the absolute path. */}
                <span id={`${baseId}-cwd`} className="terminal-launcher-cwd-path"
                  title={workingDirectory || undefined}>
                  {workingDirectory === "" ? "Choose a working folder" : homeRelativePath(workingDirectory)}
                </span>
              </span>
              <button type="button" className="terminal-launcher-cwd-browse"
                aria-label={workingDirectory === ""
                  ? "Choose working folder" : "Change working folder"}
                aria-describedby={`${baseId}-cwd`} aria-busy={browsing || undefined}
                disabled={launching || browsing}
                onClick={() => { void browse(); }}>
                {browsing ? "Opening…" : workingDirectory === "" ? "Choose…" : "Change…"}
              </button>
            </div>
          </div>

          <div className="terminal-launcher-section">
            <h3 id={`${baseId}-preview-label`}>Will launch</h3>
            <div aria-live="polite" aria-atomic="true">
              {selected === null
                ? <p className="terminal-launcher-hint">Choose an available agent to see your lineup.</p>
                /* One line for the whole lineup (owner, 2026-09-28): every seat
                   runs the same agent, so eight identical rows said one thing. */
                : <p className="terminal-launcher-lineup">
                    {selected.engine === undefined
                      ? <span className="codicon codicon-terminal" aria-hidden="true" />
                      : <ProviderLogo engine={selected.engine} label={selected.label} size={18} />}
                    <span className="terminal-launcher-lineup-label">{selected.label}</span>
                    <span className="terminal-launcher-lineup-count">× {count}</span>
                    {folderName !== "" && (
                      <span className="terminal-launcher-lineup-folder">in {folderName}</span>
                    )}
                  </p>}
            </div>
          </div>

          {directoryError !== null && (
            <div className="terminal-launcher-notice is-error" role="alert">
              {directoryError}
            </div>
          )}

          {notice !== null && (
            <div className={`terminal-launcher-notice is-${notice.kind}`}
              role={notice.kind === "error" ? "alert" : "status"}>
              {notice.message}
            </div>
          )}

          <div className="terminal-launcher-actions">
            {/* Review is not a mode of "New terminal": it opens no terminal and
                lands on another surface. One line, and the dialog it opens
                carries the objective the launcher has nowhere to put. */}
            {onOpenReview !== undefined && (
              <button type="button" className="terminal-launcher-review-link"
                title="Start an adversarial review: one agent builds, another reviews"
                disabled={launching} onClick={onOpenReview}>
                <span className="codicon codicon-git-compare" aria-hidden="true" />
                Builder and reviewer instead
              </button>
            )}
            <kbd className="terminal-launcher-shortcut" aria-hidden="true">⌘↵</kbd>
            <button type="button" className="btn btn-primary" disabled={launchDisabled}
              aria-keyshortcuts="Meta+Enter Control+Enter" onClick={launch}>
              {launching ? "Launching…" : terminalLauncherActionLabel(selected, count)}
            </button>
          </div>
        </div>
      </div>
    </section>
  );
}
