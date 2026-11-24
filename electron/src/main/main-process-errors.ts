import { appendFileSync, statSync, writeFileSync } from "node:fs";

export const MAIN_PROCESS_ERROR_LOG_NAME = "main-process-errors.log";
export const MAIN_PROCESS_ERROR_LOG_LIMIT_BYTES = 1_000_000;

export function describeMainProcessError(error: unknown): string {
  if (error instanceof Error) return error.stack ?? `${error.name}: ${error.message}`;
  // String() throws on a value with no primitive conversion (a null-prototype
  // object, a throwing toString), and this runs inside the uncaughtException
  // handler, where a throw is the crash it exists to prevent.
  try {
    return String(error);
  } catch {
    return `an undescribable ${typeof error} value`;
  }
}

/** Appends one record; restarts the file past the limit. Never throws. */
export function recordMainProcessError(
  file: string,
  kind: string,
  error: unknown,
  phase: "running" | "shutdown",
  now: Date = new Date(),
): string {
  const detail = describeMainProcessError(error);
  const record = `${now.toISOString()} ${kind} (${phase})\n${detail}\n\n`;
  try {
    let size = 0;
    try {
      size = statSync(file).size;
    } catch { /* first write */ }
    if (size > MAIN_PROCESS_ERROR_LOG_LIMIT_BYTES) writeFileSync(file, record);
    else appendFileSync(file, record);
  } catch { /* diagnostics must never throw */ }
  return detail;
}

/**
 * Runs every shutdown teardown step in order, even past one that throws, so a
 * listener removal that meets an already destroyed window cannot keep the
 * host from being stopped. Each failure is handed to `onError`.
 */
export function runShutdownSteps(
  steps: readonly (() => void)[],
  onError: (error: unknown) => void,
): void {
  for (const step of steps) {
    try {
      step();
    } catch (error) {
      onError(error);
    }
  }
}
