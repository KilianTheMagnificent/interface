// Settings › Privacy and the first-run summary (owner, 2026-11-21): what
// leaves this Mac on its own, in plain words, and where each is changed. The
// copy lives here so its tests read the same sentences the page draws; each
// sentence was checked against the Rust host that does the thing.
import type { AppPreferences } from "./settings";

/** The Settings pages a Privacy row can send you to. */
export type PrivacyRowSection = "threads" | "updates" | "remote" | "providers";

export interface PrivacyRow {
  key: string;
  label: string;
  description: string;
  /** The Settings page that controls it; absent when nothing does. */
  section?: PrivacyRowSection;
}

export const PRIVACY_PAGE_INTRO =
  "Interface has no analytics, tracking or crash reports. This is everything that leaves this Mac on its own, and where to change it.";

export const PRIVACY_GROUP_LABEL = "What leaves this Mac";

/** In reading order. */
export const PRIVACY_ROWS: readonly PrivacyRow[] = [
  { key: "titles", label: "Thread names", section: "threads",
    description: "The first message of each new thread goes to Claude Haiku, through your Claude account, to name the thread. If that fails, it tries once more with the first reply too. This happens for every provider's threads." },
  { key: "pull-requests", label: "Pull requests", section: "threads",
    description: "Interface runs gh with your GitHub account to find and follow a thread's pull request." },
  { key: "updates", label: "Update checks", section: "updates",
    description: "About every four hours Interface asks GitHub whether a newer version exists. Nothing about you is sent." },
  { key: "phone", label: "Phone access", section: "remote",
    description: "Off until you turn it on. While it is on, this Mac announces itself on your network so the iPhone app can find it, and stays connected to your relay if you added one. What it carries is encrypted." },
  { key: "other-tools", label: "Other tools' settings", section: "providers",
    description: "When Muse, Antigravity or Kimi Code is installed, Interface adds a small plugin to it once, so its panes can tell Interface what they are doing. Grok and Antigravity get an iface tools entry only if you turn it on. Removing Interface leaves those entries behind." },
  { key: "prompts", label: "Your prompts",
    description: "Everything you send to an agent goes to that agent's provider, under your own account with them. Interface keeps no copy anywhere but this Mac." },
];

export const PRIVACY_FOOT =
  "Scheduled prompts, pull-request watch and Away send prompts on your subscription while you are away, only after you set them up.";

/** The first-run summary: one corner card, shown once per profile. */
export const PRIVACY_SUMMARY_ID = "privacy-summary";
export const PRIVACY_SUMMARY_TITLE = "What Interface sends, and where";
export const PRIVACY_SUMMARY_BODY =
  "Interface has no analytics or tracking. On its own it names each new thread with Claude Haiku, follows pull requests with gh, and asks GitHub whether an update exists. Settings › Privacy has the details.";
export const PRIVACY_SUMMARY_OPEN = "Open Privacy";
export const PRIVACY_SUMMARY_DONE = "Got it";

/** Whether the first-run summary still shows: until either of its actions. */
export function showPrivacySummary(preferences: AppPreferences): boolean {
  return preferences.privacySummarySeen !== true;
}

/** The preferences once the summary has been answered. */
export function privacySummarySeen(preferences: AppPreferences): AppPreferences {
  return { ...preferences, privacySummarySeen: true };
}
