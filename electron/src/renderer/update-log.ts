declare const __INTERFACE_UPDATE_LOG_MD__: string | undefined;

export type UpdateLogKind = "feature" | "fix" | "look" | "speed" | "internal" | "record";

export type UpdateLogView = "releases" | "landings" | "unreleased";

export interface UpdateLogReleaseItem {
  readonly sha: string;
  readonly completedAt: string;
  readonly kind: UpdateLogKind;
  readonly name: string;
  readonly branch: string | null;
  readonly notes: string | null;
  readonly pins: readonly string[];
}

export interface UpdateLogRelease {
  readonly version: string;
  readonly sha: string;
  readonly completedAt: string;
  readonly items: readonly UpdateLogReleaseItem[];
  readonly leftOnDev: readonly string[];
}

export interface UpdateLogLanding {
  readonly sha: string;
  readonly completedAt: string;
  readonly kind: UpdateLogKind;
  /** Version string (e.g. "0.3.166"), or null when unreleased on Dev. */
  readonly release: string | null;
  readonly name: string;
  readonly branch: string;
  readonly notes: string;
  readonly commits: readonly string[];
  readonly pins: readonly string[];
  readonly supersedes: readonly string[];
}

export interface UpdateLogData {
  readonly releases: readonly UpdateLogRelease[];
  readonly landings: readonly UpdateLogLanding[];
  readonly unreleasedCount: number;
}

export interface UpdateLogFilter {
  readonly query?: string;
  readonly kind?: UpdateLogKind | "all";
}

export const UPDATE_LOG_KIND_LABELS: Readonly<Record<UpdateLogKind, string>> = {
  feature: "Feature",
  fix: "Fix",
  look: "Look",
  speed: "Speed",
  internal: "Internal",
  record: "Record",
};

const VALID_KINDS = new Set<UpdateLogKind>([
  "feature",
  "fix",
  "look",
  "speed",
  "internal",
  "record",
]);

function normalizeKind(raw: string): UpdateLogKind {
  const clean = raw.trim().toLowerCase() as UpdateLogKind;
  return VALID_KINDS.has(clean) ? clean : "feature";
}

function parseBacktickList(raw: string): readonly string[] {
  const results: string[] = [];
  const pattern = /`([^`]+)`/gu;
  for (const match of raw.matchAll(pattern)) {
    const item = match[1]?.trim();
    if (item) results.push(item);
  }
  return results;
}

function parseReleasesSection(section: string): readonly UpdateLogRelease[] {
  const releases: UpdateLogRelease[] = [];
  const blocks = section.split(/^### /mu).slice(1);
  for (const block of blocks) {
    const lines = block.split("\n");
    const header = lines[0]?.trim() ?? "";
    const headerMatch = /^v([^\s—]+)\s+—\s+([^\s(]+)\s+\(`([^`]+)`\)/u.exec(header);
    if (!headerMatch) continue;
    const version = headerMatch[1] ?? "";
    const completedAt = headerMatch[2] ?? "";
    const sha = headerMatch[3] ?? "";
    const items: {
      sha: string;
      completedAt: string;
      kind: UpdateLogKind;
      name: string;
      branch: string | null;
      notes: string | null;
      pins: readonly string[];
    }[] = [];
    const leftOnDev: string[] = [];

    for (const rawLine of lines.slice(1)) {
      const line = rawLine.trimEnd();
      if (line.startsWith("- **[")) {
        const itemMatch =
          /^- \*\*\[([^\]]+)\]\*\*\s+`([^`]+)`\s+·\s+([^\s·]+)\s+·\s+\*\*(.+?)\*\*(?:\s+\(`([^`]+)`\))?$/u.exec(
            line.trim(),
          );
        if (itemMatch) {
          items.push({
            kind: normalizeKind(itemMatch[1] ?? "feature"),
            sha: itemMatch[2] ?? "",
            completedAt: itemMatch[3] ?? "",
            name: itemMatch[4] ?? "",
            branch: itemMatch[5] ?? null,
            notes: null,
            pins: [],
          });
        }
      } else if (line.startsWith("  - Notes: ") && items.length > 0) {
        const last = items[items.length - 1];
        if (last) last.notes = line.slice("  - Notes: ".length).trim();
      } else if (line.startsWith("  - Pins: ") && items.length > 0) {
        const last = items[items.length - 1];
        if (last) last.pins = parseBacktickList(line.slice("  - Pins: ".length));
      } else if (line.startsWith("- **Left on Dev:** ")) {
        const rest = line.slice("- **Left on Dev:** ".length).trim();
        for (const part of rest.split(";")) {
          const trimmed = part.trim();
          if (trimmed) leftOnDev.push(trimmed);
        }
      }
    }

    releases.push({ version, sha, completedAt, items, leftOnDev });
  }
  return releases;
}

function parseLandingsSection(section: string): readonly UpdateLogLanding[] {
  const landings: UpdateLogLanding[] = [];
  const blocks = section.split(/^### /mu).slice(1);
  for (const block of blocks) {
    const lines = block.split("\n");
    const name = lines[0]?.trim() ?? "";
    if (!name) continue;
    let completedAt = "";
    let kind: UpdateLogKind = "feature";
    let release: string | null = null;
    let sha = "";
    let branch = "";
    let notes = name;
    let commits: readonly string[] = [];
    let pins: readonly string[] = [];
    let supersedes: readonly string[] = [];

    for (const rawLine of lines.slice(1)) {
      const line = rawLine.trim();
      if (line.startsWith("- **Completed:** ")) {
        completedAt = line.slice("- **Completed:** ".length).trim();
      } else if (line.startsWith("- **Kind:** ")) {
        kind = normalizeKind(line.slice("- **Kind:** ".length));
      } else if (line.startsWith("- **Release:** ")) {
        const value = line.slice("- **Release:** ".length).trim();
        release = value.startsWith("v") ? value.slice(1) : null;
      } else if (line.startsWith("- **Commit:** ")) {
        const commitMatch = /^`([^`]+)`(?:\s+\(`([^`]+)`\))?/u.exec(
          line.slice("- **Commit:** ".length).trim(),
        );
        if (commitMatch) {
          sha = commitMatch[1] ?? "";
          branch = commitMatch[2] ?? "";
        }
      } else if (line.startsWith("- **Notes:** ")) {
        notes = line.slice("- **Notes:** ".length).trim();
      } else if (line.startsWith("- **Commits:** ")) {
        commits = line
          .slice("- **Commits:** ".length)
          .split(";")
          .map(entry => entry.trim())
          .filter(Boolean);
      } else if (line.startsWith("- **Pins:** ")) {
        pins = parseBacktickList(line.slice("- **Pins:** ".length));
      } else if (line.startsWith("- **Supersedes:** ")) {
        supersedes = line
          .slice("- **Supersedes:** ".length)
          .split(";")
          .map(entry => entry.trim())
          .filter(Boolean);
      }
    }
    if (sha) {
      landings.push({
        sha,
        completedAt,
        kind,
        release,
        name,
        branch,
        notes,
        commits,
        pins,
        supersedes,
      });
    }
  }
  return landings;
}

/** Parses `docs/UPDATES.md` into structured release and Dev landing records. */
export function parseUpdateLog(markdown: string): UpdateLogData {
  if (!markdown.trim()) {
    return { releases: [], landings: [], unreleasedCount: 0 };
  }
  const releasesMarker = "\n## Production Releases\n";
  const landingsMarker = "\n## All Dev Landings\n";
  const releasesStart = markdown.indexOf(releasesMarker);
  const landingsStart = markdown.indexOf(landingsMarker);

  const releasesSection =
    releasesStart >= 0
      ? markdown.slice(
          releasesStart + releasesMarker.length,
          landingsStart >= 0 ? landingsStart : undefined,
        )
      : "";
  const landingsSection =
    landingsStart >= 0 ? markdown.slice(landingsStart + landingsMarker.length) : "";

  const releases = parseReleasesSection(releasesSection);
  const landings = parseLandingsSection(landingsSection);
  const unreleasedCount = landings.filter(landing => landing.release === null).length;
  return { releases, landings, unreleasedCount };
}

function matchesSearch(haystack: readonly (string | null | undefined)[], query: string): boolean {
  if (!query) return true;
  return haystack.some(part => part !== null && part !== undefined && part.toLowerCase().includes(query));
}

export function filterUpdateLogLandings(
  landings: readonly UpdateLogLanding[],
  filter: UpdateLogFilter = {},
  unreleasedOnly = false,
): readonly UpdateLogLanding[] {
  const query = (filter.query ?? "").trim().toLowerCase();
  const kind = filter.kind ?? "all";
  return landings.filter(landing => {
    if (unreleasedOnly && landing.release !== null) return false;
    if (kind !== "all" && landing.kind !== kind) return false;
    return matchesSearch(
      [
        landing.name,
        landing.notes,
        landing.branch,
        landing.sha,
        landing.release !== null ? `v${landing.release}` : "unreleased dev",
        ...landing.commits,
        ...landing.pins,
      ],
      query,
    );
  });
}

export function filterUpdateLogReleases(
  releases: readonly UpdateLogRelease[],
  filter: UpdateLogFilter = {},
): readonly UpdateLogRelease[] {
  const query = (filter.query ?? "").trim().toLowerCase();
  const kind = filter.kind ?? "all";
  const result: UpdateLogRelease[] = [];
  for (const release of releases) {
    const releaseHeaderMatches = matchesSearch(
      [`v${release.version}`, release.version, release.sha, release.completedAt],
      query,
    );
    const matchingItems = release.items.filter(item => {
      if (kind !== "all" && item.kind !== kind) return false;
      if (releaseHeaderMatches) return true;
      return matchesSearch(
        [item.name, item.notes, item.branch, item.sha, ...item.pins],
        query,
      );
    });
    if (matchingItems.length > 0 || (kind === "all" && releaseHeaderMatches && release.items.length === 0)) {
      result.push(
        matchingItems.length === release.items.length
          ? release
          : { ...release, items: matchingItems },
      );
    }
  }
  return result;
}

/** Human-readable completion timestamp (`YYYY-MM-DD HH:MM`) from an ISO-8601 string. */
export function formatUpdateCompletedAt(iso: string): string {
  const match = /^(\d{4}-\d{2}-\d{2})T(\d{2}:\d{2})/u.exec(iso.trim());
  if (match) {
    return `${match[1]} ${match[2]}`;
  }
  return iso;
}

const EMBEDDED_MARKDOWN =
  typeof __INTERFACE_UPDATE_LOG_MD__ === "string" ? __INTERFACE_UPDATE_LOG_MD__ : "";

export const DEFAULT_UPDATE_LOG: UpdateLogData = parseUpdateLog(EMBEDDED_MARKDOWN);
