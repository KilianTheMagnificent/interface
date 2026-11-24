/** Two independent roles converge on one result. Designed to hold at 16–28px. */
export function AdversarialReviewIcon({ className, size = 16 }: {
  readonly className?: string;
  readonly size?: number;
}) {
  const classes = ["adversarial-review-icon", className].filter(Boolean).join(" ");
  return (
    <svg className={classes} viewBox="0 0 24 24" width={size} height={size}
      data-adversarial-review-icon="opposed-verdict" aria-hidden="true" focusable="false"
      fill="none">
      <path d="M4.25 6.5 9.25 12l-5 5.5" stroke="currentColor" strokeWidth="2"
        strokeLinecap="round" strokeLinejoin="round" />
      <path d="m19.75 6.5-5 5.5 5 5.5" stroke="currentColor" strokeWidth="2"
        strokeLinecap="round" strokeLinejoin="round" />
      <path d="M12 9.5 14.5 12 12 14.5 9.5 12 12 9.5Z" fill="currentColor" />
    </svg>
  );
}
