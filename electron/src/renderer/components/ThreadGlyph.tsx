import type { ReactElement } from "react";
import "./ThreadGlyph.css";

/*
 * THE THREADS ICONS (owner, 2026-10-23, "make the icons slicker"): one line
 * set on a 16px grid with a 1.5 stroke and round ends and joins, drawn from the
 * approved canvas (board 8, "Icons"). Every one is decorative: the control that
 * owns it carries the accessible name. They inherit `color`, so each theme and
 * each state tints them as it tints text.
 */
const GLYPHS = {
  read: <>
    <path d="M9 1.75H4.75a1.5 1.5 0 0 0-1.5 1.5v9.5a1.5 1.5 0 0 0 1.5 1.5h6.5a1.5 1.5 0 0 0 1.5-1.5V5.5Z" />
    <path d="M9 1.75V4a1.5 1.5 0 0 0 1.5 1.5h2.25M5.75 8.75h4.5M5.75 11.25h2.75" />
  </>,
  edit: <>
    <path d="M10.25 3.1a1.75 1.75 0 0 1 2.47 0l.18.18a1.75 1.75 0 0 1 0 2.47L6.5 12.15l-3.4.85.85-3.4z" />
    <path d="M9.25 4.1l2.65 2.65" />
  </>,
  write: <>
    <path d="M9 1.75H4.75a1.5 1.5 0 0 0-1.5 1.5v9.5a1.5 1.5 0 0 0 1.5 1.5h6.5a1.5 1.5 0 0 0 1.5-1.5V5.5Z" />
    <path d="M9 1.75V4a1.5 1.5 0 0 0 1.5 1.5h2.25M8 7.75v4.5M5.75 10h4.5" />
  </>,
  run: <>
    <rect x="1.75" y="2.75" width="12.5" height="10.5" rx="2.25" />
    <path d="M4.75 6.25 6.75 8l-2 1.75M8.5 10h2.75" />
  </>,
  plan: <path d="M2.75 4.5l1.1 1.1 2.15-2.35M2.75 10.75l1.1 1.1 2.15-2.35M8.25 4.75h5M8.25 11h5" />,
  search: <>
    <circle cx="7" cy="7" r="4.25" />
    <path d="M10.25 10.25 13.5 13.5" />
  </>,
  web: <>
    <circle cx="8" cy="8" r="5.75" />
    <path d="M2.25 8h11.5M8 2.25c1.75 1.6 2.6 3.5 2.6 5.75S9.75 12.15 8 13.75C6.25 12.15 5.4 10.25 5.4 8S6.25 3.85 8 2.25z" />
  </>,
  agent: <>
    <rect x="2.75" y="4.75" width="10.5" height="8.5" rx="2.5" />
    <path d="M8 2.25v2.5M6 8.5v.5M10 8.5v.5" />
  </>,
  tool: <path d="M9.9 2.4a3.1 3.1 0 0 0-3.55 4.05L2.6 10.2a1.45 1.45 0 0 0 2.05 2.05l3.75-3.75a3.1 3.1 0 0 0 4.05-3.55l-1.95 1.95-1.95-.45-.45-1.95z" />,
  pass: <path d="M3.5 8.5l2.75 2.75L12.5 5" />,
  error: <>
    <circle cx="8" cy="8" r="5.75" />
    <path d="M6.1 6.1l3.8 3.8M9.9 6.1l-3.8 3.8" />
  </>,
  warning: <>
    <path d="M7.13 2.75a1 1 0 0 1 1.74 0l5.2 9.25a1 1 0 0 1-.87 1.5H2.8a1 1 0 0 1-.87-1.5z" />
    <path d="M8 6.5v2.75M8 11.25v.01" />
  </>,
  changes: <>
    <rect x="2.25" y="1.75" width="11.5" height="12.5" rx="2.5" />
    <path d="M8 4.5v4M6 6.5h4M6 11.25h4" />
  </>,
  branch: <>
    <circle cx="4.75" cy="3.5" r="1.5" />
    <circle cx="4.75" cy="12.5" r="1.5" />
    <circle cx="11.25" cy="4.75" r="1.5" />
    <path d="M4.75 5v6M11.25 6.25v.5a2.5 2.5 0 0 1-2.5 2.5h-1.5a2.5 2.5 0 0 0-2.5 2.5" />
  </>,
  pr: <>
    <circle cx="4.5" cy="3.75" r="1.5" />
    <circle cx="4.5" cy="12.25" r="1.5" />
    <circle cx="11.5" cy="12.25" r="1.5" />
    <path d="M4.5 5.25v5.5M11.5 10.75V7a2.25 2.25 0 0 0-2.25-2.25H7.5M9 3.25 7.5 4.75 9 6.25" />
  </>,
  merge: <>
    <circle cx="4.5" cy="3.75" r="1.5" />
    <circle cx="4.5" cy="12.25" r="1.5" />
    <circle cx="11.5" cy="8" r="1.5" />
    <path d="M4.5 5.25v5.5M4.5 5.5c0 1.5 1.25 2.5 3 2.5H10" />
  </>,
  copy: <>
    <rect x="5.25" y="5.25" width="8.5" height="8.5" rx="2" />
    <path d="M10.75 5.25V4a1.75 1.75 0 0 0-1.75-1.75H4A1.75 1.75 0 0 0 2.25 4v5A1.75 1.75 0 0 0 4 10.75h1.25" />
  </>,
  retry: <path d="M13.25 8a5.25 5.25 0 1 1-1.54-3.71M13.25 2.25v3.25H10" />,
  fork: <>
    <circle cx="4.5" cy="3.5" r="1.5" />
    <circle cx="11.5" cy="3.5" r="1.5" />
    <circle cx="8" cy="12.5" r="1.5" />
    <path d="M4.5 5v.75A2.25 2.25 0 0 0 6.75 8h2.5a2.25 2.25 0 0 0 2.25-2.25V5M8 8v3" />
  </>,
  undo: <>
    <path d="M5.75 3.75 2.5 7l3.25 3.25" />
    <path d="M2.75 7H10a3.25 3.25 0 0 1 0 6.5H7.25" />
  </>,
  access: <>
    <path d="M8 1.75l5 1.9v3.9c0 3.05-2.1 5.35-5 6.7-2.9-1.35-5-3.65-5-6.7v-3.9z" />
    <path d="M5.75 7.9 7.3 9.45l2.95-3.2" />
  </>,
  add: <path d="M8 3.25v9.5M3.25 8h9.5" />,
  send: <path d="M8 12.75v-9.5M4.25 7 8 3.25 11.75 7" />,
  stop: <rect x="3" y="3" width="10" height="10" rx="2.5" fill="currentColor" stroke="none" />,
  comment: <path d="M3.75 2.75h8.5a2 2 0 0 1 2 2v4.75a2 2 0 0 1-2 2H8.4L5.5 13.6v-2.1H3.75a2 2 0 0 1-2-2V4.75a2 2 0 0 1 2-2z" />,
  preview: <>
    <path d="M1.75 8S4 3.75 8 3.75 14.25 8 14.25 8 12 12.25 8 12.25 1.75 8 1.75 8z" />
    <circle cx="8" cy="8" r="1.75" />
  </>,
  close: <path d="M4.25 4.25l7.5 7.5M11.75 4.25l-7.5 7.5" />,
  more: <>
    <circle cx="3.5" cy="8" r="1.25" fill="currentColor" stroke="none" />
    <circle cx="8" cy="8" r="1.25" fill="currentColor" stroke="none" />
    <circle cx="12.5" cy="8" r="1.25" fill="currentColor" stroke="none" />
  </>,
  expand: <path d="M4.25 6.25 8 10l3.75-3.75" />,
  open: <path d="M6.25 4.25 10 8l-3.75 3.75" />,
  back: <path d="M9.75 4.25 6 8l3.75 3.75" />,
  down: <path d="M8 3.25v9.5M4.25 9 8 12.75 11.75 9" />,
} satisfies Record<string, ReactElement>;

export type ThreadGlyphName = keyof typeof GLYPHS;

/** Every name the set draws, for the board that shows them and its test. */
export const THREAD_GLYPH_NAMES = Object.keys(GLYPHS) as ThreadGlyphName[];

/** One Threads icon, 16px unless asked otherwise (`size`). */
export function ThreadGlyph({ name, size = 16, className }: {
  name: ThreadGlyphName;
  size?: number;
  className?: string;
}): ReactElement {
  return (
    <svg className={className === undefined ? "thread-glyph" : `thread-glyph ${className}`}
      width={size} height={size} viewBox="0 0 16 16" fill="none" stroke="currentColor"
      strokeWidth={1.5} strokeLinecap="round" strokeLinejoin="round"
      aria-hidden="true" focusable="false">
      {GLYPHS[name]}
    </svg>
  );
}
