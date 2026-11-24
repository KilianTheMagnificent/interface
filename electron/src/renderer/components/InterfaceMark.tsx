/** The Fold: Interface's mark, shared by every in-app identity slot. It sits on
 *  its own rounded plate — the app icon in miniature — because the bare stroke
 *  reads as a » control at toolbar size. The plate takes `--brand-tile-bg` and
 *  the stroke `currentColor`; callers set both (theme.css `.interface-mark`). */
export function InterfaceMark({ className }: { readonly className?: string }) {
  const classes = ["interface-mark", className].filter(Boolean).join(" ");
  return (
    <svg className={classes} viewBox="100 100 824 824" aria-hidden="true"
      focusable="false" data-interface-mark="fold">
      <rect className="interface-mark-tile" x="100" y="100" width="824" height="824" rx="186" />
      <g transform="translate(-18 12) translate(512 512) scale(0.92) translate(-512 -512)"
        fill="none" stroke="currentColor" strokeWidth="96"
        strokeLinecap="butt" strokeLinejoin="miter">
        <path d="M 256 256 L 672 672 L 864 480 L 704 320 L 585.5 438.5" />
        <path d="M 438.5 585.5 L 256 768" />
      </g>
    </svg>
  );
}
