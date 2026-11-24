/**
 * `iface:diagnostics` (protocol 39). Rust writes the Copy Diagnostics report
 * (credential-filtered), keeps this run's set-aside store notices and asks
 * Finder to reveal things; the renderer and main only show and ask. Mirrors
 * `crates/interface-contracts/src/diagnostics.rs`; both read
 * `fixtures/diagnostics-v39.json`.
 */

export type RevealTarget = "logs" | "dataFolder" | "notice";

export type DiagnosticsCommand =
  | { kind: "report" }
  | { kind: "notices" }
  | { kind: "dismissNotice"; id: string }
  | { kind: "reveal"; target: "logs" | "dataFolder" }
  | { kind: "reveal"; target: "notice"; id: string };

/** A store the host could not read this run and set aside, starting it empty. */
export interface StoreNotice {
  id: string;
  /** What was lost, as a phrase: "your Threads list". */
  what: string;
}

export interface DiagnosticsResponse {
  report?: string;
  notices?: StoreNotice[];
}

/** The banner sentence for one notice. */
export function storeNoticeText(notice: StoreNotice, product = "Interface"): string {
  return `${product} couldn't read ${notice.what} and set the file aside.`;
}

/** The notices in a response, or none for anything malformed. */
export function noticesOf(response: unknown): StoreNotice[] {
  if (typeof response !== "object" || response === null) return [];
  const notices = (response as { notices?: unknown }).notices;
  if (!Array.isArray(notices)) return [];
  return notices.flatMap(notice => {
    if (typeof notice !== "object" || notice === null) return [];
    const { id, what } = notice as { id?: unknown; what?: unknown };
    return typeof id === "string" && typeof what === "string" && id !== "" && what !== ""
      ? [{ id, what }] : [];
  });
}
