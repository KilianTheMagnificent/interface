import { useState } from "react";
import {
  WHATS_NEW_KIND_LABELS, whatsNew, whatsNewDate, type WhatsNewKind,
} from "../whats-new";
import {
  DEFAULT_UPDATE_LOG,
  UPDATE_LOG_KIND_LABELS,
  filterUpdateLogLandings,
  filterUpdateLogReleases,
  formatUpdateCompletedAt,
  type UpdateLogData,
  type UpdateLogKind,
  type UpdateLogLanding,
  type UpdateLogRelease,
  type UpdateLogView,
} from "../update-log";

const KIND_FILTERS: readonly { readonly id: UpdateLogKind | "all"; readonly label: string }[] = [
  { id: "all", label: "All kinds" },
  { id: "feature", label: "Features" },
  { id: "fix", label: "Fixes" },
  { id: "look", label: "Looks" },
  { id: "speed", label: "Speed" },
  { id: "internal", label: "Internal" },
  { id: "record", label: "Record" },
];

const INITIAL_RELEASE_LIMIT = 15;
const INITIAL_LANDING_LIMIT = 25;

export interface UpdateLogPanelProps {
  readonly log?: UpdateLogData;
  readonly currentVersion?: string;
  readonly initialView?: UpdateLogView;
}

function ReleaseCard({
  release,
  isCurrent,
}: {
  readonly release: UpdateLogRelease;
  readonly isCurrent: boolean;
}) {
  return (
    <article className="settings-update-log-card" data-update-release={release.version}>
      <div className="settings-update-log-card-head">
        <span className="settings-update-log-card-title">
          v{release.version}
          {isCurrent ? " (Current)" : ""}
        </span>
        <span className="settings-update-log-meta">
          <span className="settings-update-log-badge">
            {release.items.length === 1 ? "1 update" : `${release.items.length} updates`}
          </span>
          <code className="settings-update-log-code">{release.sha}</code>
          <time dateTime={release.completedAt}>
            {formatUpdateCompletedAt(release.completedAt)}
          </time>
        </span>
      </div>
      {release.items.length > 0 && (
        <ul className="settings-update-log-items">
          {release.items.map(item => (
            <li key={`${release.version}:${item.sha}:${item.name}`} className="settings-update-log-item">
              <div className="settings-update-log-item-head">
                <span className="settings-update-log-badge">
                  {UPDATE_LOG_KIND_LABELS[item.kind]}
                </span>
                <span className="settings-update-log-item-name">{item.name}</span>
                <span className="settings-update-log-meta">
                  <code className="settings-update-log-code">{item.sha}</code>
                  {item.branch !== null && (
                    <code className="settings-update-log-code">{item.branch}</code>
                  )}
                  <time dateTime={item.completedAt}>
                    {formatUpdateCompletedAt(item.completedAt)}
                  </time>
                </span>
              </div>
              {item.notes !== null && (
                <p className="settings-update-log-notes">{item.notes}</p>
              )}
              {item.pins.length > 0 && (
                <p className="settings-update-log-pins">
                  Verified by: {item.pins.join(", ")}
                </p>
              )}
            </li>
          ))}
        </ul>
      )}
    </article>
  );
}

function LandingCard({ landing }: { readonly landing: UpdateLogLanding }) {
  return (
    <article className="settings-update-log-card" data-update-landing={landing.sha}>
      <div className="settings-update-log-card-head">
        <span className="settings-update-log-card-title">{landing.name}</span>
        <span className="settings-update-log-meta">
          <span className="settings-update-log-badge">
            {UPDATE_LOG_KIND_LABELS[landing.kind]}
          </span>
          <span
            className={`settings-update-log-badge${landing.release === null ? " is-current" : ""}`}
          >
            {landing.release !== null ? `v${landing.release}` : "Unreleased (on Dev)"}
          </span>
          <code className="settings-update-log-code">{landing.sha}</code>
          {landing.branch.length > 0 && (
            <code className="settings-update-log-code">{landing.branch}</code>
          )}
          <time dateTime={landing.completedAt}>
            {formatUpdateCompletedAt(landing.completedAt)}
          </time>
        </span>
      </div>
      <p className="settings-update-log-notes">{landing.notes}</p>
      {landing.pins.length > 0 && (
        <p className="settings-update-log-pins">
          Verified by: {landing.pins.join(", ")}
        </p>
      )}
    </article>
  );
}

export function UpdateLogPanel({
  log = DEFAULT_UPDATE_LOG,
  currentVersion,
  initialView = "releases",
}: UpdateLogPanelProps) {
  const [view, setView] = useState<UpdateLogView>(initialView);
  const [kind, setKind] = useState<UpdateLogKind | "all">("all");
  const [query, setQuery] = useState("");
  const [limit, setLimit] = useState(
    initialView === "releases" ? INITIAL_RELEASE_LIMIT : INITIAL_LANDING_LIMIT,
  );

  if (log.releases.length === 0 && log.landings.length === 0) {
    return null;
  }

  const filteredReleases = filterUpdateLogReleases(log.releases, { query, kind });
  const filteredLandings = filterUpdateLogLandings(
    log.landings,
    { query, kind },
    view === "unreleased",
  );

  const switchView = (next: UpdateLogView) => {
    setView(next);
    setLimit(next === "releases" ? INITIAL_RELEASE_LIMIT : INITIAL_LANDING_LIMIT);
  };

  const totalMatching =
    view === "releases" ? filteredReleases.length : filteredLandings.length;
  const shownReleases = filteredReleases.slice(0, limit);
  const shownLandings = filteredLandings.slice(0, limit);

  return (
    <div className="settings-update-log" role="region" aria-label="Update log">
      <div className="settings-update-log-head">
        <h3 className="settings-update-log-title">Update log</h3>
        <span className="settings-update-log-summary">
          {log.releases.length} releases · {log.landings.length} Dev landings
          {log.unreleasedCount > 0 ? ` · ${log.unreleasedCount} unreleased on Dev` : ""}
        </span>
      </div>

      <div className="settings-update-log-controls">
        <div className="settings-update-log-tabs" role="group" aria-label="Update log view">
          <button
            type="button"
            className="settings-update-log-pill"
            aria-pressed={view === "releases"}
            onClick={() => {
              switchView("releases");
            }}
          >
            Releases ({log.releases.length})
          </button>
          <button
            type="button"
            className="settings-update-log-pill"
            aria-pressed={view === "landings"}
            onClick={() => {
              switchView("landings");
            }}
          >
            Dev landings ({log.landings.length})
          </button>
          <button
            type="button"
            className="settings-update-log-pill"
            aria-pressed={view === "unreleased"}
            onClick={() => {
              switchView("unreleased");
            }}
          >
            Unreleased on Dev ({log.unreleasedCount})
          </button>
        </div>

        <input
          type="search"
          className="settings-update-log-search"
          aria-label="Filter update log"
          placeholder="Search updates, notes, versions, branches, or commits…"
          value={query}
          onChange={event => {
            setQuery(event.target.value);
          }}
        />
      </div>

      <div className="settings-update-log-kinds" role="group" aria-label="Filter updates by kind">
        {KIND_FILTERS.map(entry => (
          <button
            key={entry.id}
            type="button"
            className="settings-update-log-pill"
            aria-pressed={kind === entry.id}
            onClick={() => {
              setKind(entry.id);
            }}
          >
            {entry.label}
          </button>
        ))}
      </div>

      <div className="settings-update-log-list">
        {totalMatching === 0 ? (
          <div className="settings-card settings-empty">
            No updates match the current filter.
          </div>
        ) : view === "releases" ? (
          shownReleases.map(release => (
            <ReleaseCard
              key={release.version}
              release={release}
              isCurrent={currentVersion === release.version}
            />
          ))
        ) : (
          shownLandings.map(landing => (
            <LandingCard key={`${landing.sha}:${landing.name}`} landing={landing} />
          ))
        )}
      </div>

      {totalMatching > limit && (
        <button
          type="button"
          className="settings-refresh settings-update-log-more"
          onClick={() => {
            setLimit(current => current + 25);
          }}
        >
          Show more ({totalMatching - limit} remaining)
        </button>
      )}
    </div>
  );
}

const WHATS_NEW_FILTERS: readonly { readonly id: WhatsNewKind | "all"; readonly label: string }[] = [
  { id: "all", label: "Everything" },
  { id: "new", label: WHATS_NEW_KIND_LABELS.new },
  { id: "fixed", label: WHATS_NEW_KIND_LABELS.fixed },
  { id: "looks", label: WHATS_NEW_KIND_LABELS.looks },
  { id: "faster", label: WHATS_NEW_KIND_LABELS.faster },
];

/**
 * Settings › Updates › What's new (owner, 2026-11-15): the version you have,
 * open, and the ones before it behind Show, each change one plain sentence
 * marked New, Fixed, Looks or Faster. Everything else is the developer log's.
 */
export function WhatsNew({ log = DEFAULT_UPDATE_LOG, currentVersion }: {
  readonly log?: UpdateLogData;
  readonly currentVersion?: string;
}) {
  const [kind, setKind] = useState<WhatsNewKind | "all">("all");
  const releases = whatsNew(log, currentVersion, 3);
  const [shown, setShown] = useState<ReadonlySet<string>>(() => new Set(releases.slice(0, 1).map(release => release.version)));
  if (releases.length === 0) return null;
  return (
    <section className="settings-whats-new" aria-labelledby="settings-whats-new-title">
      <div className="settings-whats-new-head">
        <h3 id="settings-whats-new-title" className="settings-whats-new-title">What&rsquo;s new</h3>
        <div className="settings-whats-new-filters" role="group" aria-label="Show changes of one kind">
          {WHATS_NEW_FILTERS.map(entry => (
            <button key={entry.id} type="button" className="settings-update-log-pill"
              aria-pressed={kind === entry.id} onClick={() => { setKind(entry.id); }}>{entry.label}</button>
          ))}
        </div>
      </div>
      {releases.map(release => {
        const open = shown.has(release.version);
        const changes = release.changes.filter(change => kind === "all" || change.kind === kind);
        const date = whatsNewDate(release.completedAt);
        const count = `${release.changes.length} change${release.changes.length === 1 ? "" : "s"}`;
        return (
          <article key={release.version} className="settings-card settings-whats-new-release"
            data-whats-new-release={release.version}>
            <div className="settings-whats-new-release-head">
              <span className="settings-whats-new-version">
                {release.version}
                {release.isCurrent && <span className="settings-whats-new-yours"> · the version you have</span>}
              </span>
              <span className="settings-whats-new-meta">
                <span>{date === "" ? count : `${date} · ${count}`}</span>
                {!release.isCurrent && (
                  <button type="button" className="settings-button" aria-expanded={open}
                    onClick={() => {
                      setShown(current => {
                        const next = new Set(current);
                        if (next.has(release.version)) next.delete(release.version);
                        else next.add(release.version);
                        return next;
                      });
                    }}>{open ? "Hide" : "Show"}</button>
                )}
              </span>
            </div>
            {(open || release.isCurrent) && (changes.length === 0
              ? <p className="settings-whats-new-none">Nothing of this kind in {release.version}.</p>
              : changes.map((change, index) => (
                <div key={`${change.kind}:${index}`} className="settings-whats-new-change">
                  <span className="settings-whats-new-kind">{WHATS_NEW_KIND_LABELS[change.kind]}</span>
                  <span>{change.text}</span>
                </div>
              )))}
          </article>
        );
      })}
    </section>
  );
}
