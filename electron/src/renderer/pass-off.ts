/* Pure pieces of the terminal Pass Off flow: transcript shaping and
   eligibility. The capture (xterm) and the wire (createTerminal) stay in
   their owners; this module is what the tests hold still. */

export const PASS_OFF_TRANSCRIPT_MAX_BYTES = 512 * 1024;
const TRUNCATION_NOTE = "[transcript truncated]\n";

/* The host measures the transcript in UTF-8 bytes (`String::len()` against
   `TERMINAL_HANDOFF_TRANSCRIPT_MAX_BYTES`), so the budget below counts encoded
   bytes. Measuring UTF-16 code units would pass a transcript the host rejects. */
const encoder = new TextEncoder();
const byteLength = (value: string): number => encoder.encode(value).length;

/** Newest-last transcript, whole lines only, bounded for the wire. */
export function passOffTranscript(lines: readonly string[]): string {
  let end = lines.length;
  while (end > 0 && lines[end - 1].trim() === "") end--;
  const budget = PASS_OFF_TRANSCRIPT_MAX_BYTES - byteLength(TRUNCATION_NOTE);
  let start = end;
  let bytes = 0;
  while (start > 0) {
    const next = byteLength(lines[start - 1]) + (start === end ? 0 : 1);
    if (bytes + next > budget) break;
    bytes += next;
    start--;
  }
  const body = lines.slice(start, end).join("\n");
  return start > 0 ? `${TRUNCATION_NOTE}${body}` : body;
}

/* The host caps `handoff.fromTitle` at 200 UTF-8 bytes and refuses any control
   character in it, and a bad title fails the whole request rather than just
   the field. So a title the host would not take is dropped here instead of
   sent: the pass-off is worth more than the provenance line.

   The class below is Rust's `char::is_control` — the Unicode Cc category, so
   U+0080..U+009F count too, not only the ASCII controls. */
const TITLE_MAX_BYTES = 200;
const CONTROL = /[\u0000-\u001f\u007f-\u009f]/u;

/** The source title as the host will accept it, or undefined to omit the field. */
export function passOffTitle(title: string): string | undefined {
  const trimmed = title.trim();
  if (trimmed === "" || byteLength(trimmed) > TITLE_MAX_BYTES) return undefined;
  return CONTROL.test(trimmed) ? undefined : trimmed;
}

/** Null when the pass-off may proceed; otherwise the sentence to report. */
export function passOffRefusal(
  source: { status: string } | undefined,
  profile: { available: boolean; label: string; unavailableReason?: string } | undefined,
): string | null {
  if (source === undefined || source.status !== "running") {
    return "Pass off needs a running source terminal.";
  }
  if (profile === undefined) return "That launcher is not available.";
  if (!profile.available) {
    return profile.unavailableReason ?? `${profile.label} is unavailable.`;
  }
  return null;
}
