import type { HTMLAttributes } from "react";
import "./WorkingIndicator.css";

/** Presentation only: callers retain authority over whether work is active.
 * Terminal appearance keeps the caller's existing glyph and accessibility. */
export function WorkingIndicator({ working = true, compact = true,
    className = "dot dot-working dot-pulse", children, ...props }:
    HTMLAttributes<HTMLSpanElement> & { working?: boolean; compact?: boolean }) {
  if (!working) return <span className={className} {...props}>{children}</span>;
  return <span className={`${className} working-indicator${compact ? "" : " working-indicator-large"}`}
    aria-hidden={props["aria-label"] === undefined ? true : undefined} {...props}>
    {children}
    <span className="working-wave" aria-hidden="true">
      <span /><span /><span /><span /><span />
    </span>
  </span>;
}
