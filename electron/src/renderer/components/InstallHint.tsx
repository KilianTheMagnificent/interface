/**
 * An install hint from Rust with its `commands` set as code, so the one to
 * copy reads as one. The launcher's folded agents and Settings' Terminal
 * tools both show it (review C4).
 */
export function InstallHint({ hint }: { readonly hint: string }) {
  return <>{hint.split("`").map((part, index) => index % 2 === 1
    ? <code key={index}>{part}</code> : part)}</>;
}
