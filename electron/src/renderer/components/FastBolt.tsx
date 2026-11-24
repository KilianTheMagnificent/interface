/** Fast mode's one bolt, shared by the pane header toggle and the composer's.
 *  An outlined 16px stroke at the pane header's weight; it inherits `color`. */
export function FastBolt() {
  return (
    <svg className="fast-bolt" width="16" height="16" viewBox="0 0 16 16" fill="none"
      aria-hidden="true" focusable="false">
      <path d="M9.5 1.5 3 9h4.5l-1 5.5L13 7H8.5l1-5.5Z"
        stroke="currentColor" strokeWidth="1.5" strokeLinejoin="round" />
    </svg>
  );
}
