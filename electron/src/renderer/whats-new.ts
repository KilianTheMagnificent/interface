/**
 * Settings › Updates › What's new (owner, 2026-11-15): each release's
 * landings as plain sentences, kind by kind. Internal and record landings,
 * commit hashes, branches and the tests that verify a landing stay in the
 * developer log. Types only from `update-log`, so the log text itself still
 * loads only with the Updates chunk.
 */
import type { UpdateLogData, UpdateLogKind } from "./update-log";

export type WhatsNewKind = "new" | "fixed" | "looks" | "faster";

export const WHATS_NEW_KIND_LABELS: Readonly<Record<WhatsNewKind, string>> = {
  new: "New", fixed: "Fixed", looks: "Looks", faster: "Faster",
};

const KIND_OF: Readonly<Partial<Record<UpdateLogKind, WhatsNewKind>>> = {
  feature: "new", fix: "fixed", look: "looks", speed: "faster",
};

export interface WhatsNewChange {
  readonly kind: WhatsNewKind;
  readonly text: string;
}

export interface WhatsNewRelease {
  readonly version: string;
  readonly completedAt: string;
  /** The version this app is. */
  readonly isCurrent: boolean;
  readonly changes: readonly WhatsNewChange[];
}

/** A landing's name as a sentence: it already reads as one; it gains a full stop. */
export function plainSentence(name: string): string {
  const text = name.trim();
  return text === "" || /[.!?…]$/u.test(text) ? text : `${text}.`;
}

/**
 * The version you have and the ones before it, newest first, at most
 * `limit`. A build whose version the log does not list (Dev) starts at the
 * newest release.
 */
export function whatsNew(log: Pick<UpdateLogData, "releases">, currentVersion: string | undefined,
    limit = 5): readonly WhatsNewRelease[] {
  const start = Math.max(0, log.releases.findIndex(release => release.version === currentVersion));
  return log.releases.slice(start, start + limit).map(release => ({
    version: release.version,
    completedAt: release.completedAt,
    isCurrent: release.version === currentVersion,
    changes: release.items.flatMap(item => {
      const kind = KIND_OF[item.kind];
      return kind === undefined ? [] : [{ kind, text: plainSentence(item.name) }];
    }),
  }));
}

/** "15 November": the day a release shipped, in the reader's own words. */
export function whatsNewDate(iso: string): string {
  const time = Date.parse(iso);
  return Number.isFinite(time)
    ? new Intl.DateTimeFormat(undefined, { day: "numeric", month: "long" }).format(time) : "";
}
