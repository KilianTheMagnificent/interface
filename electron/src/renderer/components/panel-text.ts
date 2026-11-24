import { withoutIpcErrorShape } from "../ipc-message";

/* ============================================================================
   ONE PHRASE, ONE SPELLING

   The plan, orchestration, collaboration and terminal panels each grew their
   own copy of the same three one-liners: title-case an enum, read an optional
   form field, and name a failure. Four copies of the failure rule is four
   chances for one panel to start printing a bare "Error" where its neighbour
   prints what the provider actually said, so they live here once.

   Only the fallback sentence differs between panels, and that is the argument.
   ========================================================================== */

/** A snake_case state or kind, as a title-cased phrase. */
export function pretty(value: string): string {
  return value.replaceAll("_", " ")
    .replace(/(^|\s)\S/gu, character => character.toUpperCase());
}

/** A trimmed form field, or undefined where the field was left empty. */
export function optional(value: FormDataEntryValue | null): string | undefined {
  const text = String(value ?? "").trim();
  return text === "" ? undefined : text;
}

/**
 * What went wrong, in the provider's own words wherever it said anything at
 * all. `fallback` is the panel's own sentence for a failure that named nothing
 * — a blank message is not an explanation, so it never reaches the reader.
 */
export function messageFor(error: unknown, fallback: string): string {
  const said = error instanceof Error ? withoutIpcErrorShape(error.message) : "";
  return said.trim() !== "" ? said : fallback;
}
