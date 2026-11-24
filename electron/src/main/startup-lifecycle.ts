export const STARTUP_STEP_TIMEOUT_MS = 10_000;

export class StartupStepSupersededError extends Error {
  constructor() {
    super("The Interface startup step was superseded by shutdown.");
    this.name = "StartupStepSupersededError";
  }
}

/**
 * Attach rejection handling immediately, then expose a reusable await that
 * refuses both late success and late failure after the owning startup has
 * been superseded. This prevents deferred work from reviving disposed state.
 */
export function createStartupGate<T>(
  operation: Promise<T>,
  isActive: () => boolean,
): () => Promise<T> {
  const outcome = operation.then(
    value => ({ ok: true as const, value }),
    (error: unknown) => ({ ok: false as const, error }),
  );
  return async () => {
    const result = await outcome;
    if (!isActive()) throw new StartupStepSupersededError();
    if (!result.ok) throw result.error;
    return result.value;
  };
}

/** Bound pre-renderer host calls so startup always becomes visible or fails clearly. */
export async function withStartupDeadline<T>(
  label: string,
  operation: Promise<T>,
  timeoutMs = STARTUP_STEP_TIMEOUT_MS,
): Promise<T> {
  let timer: NodeJS.Timeout | undefined;
  const deadline = new Promise<never>((_resolve, reject) => {
    timer = setTimeout(() => {
      reject(new Error(`The Interface ${label} did not start within ${String(timeoutMs)} ms.`));
    }, timeoutMs);
  });
  try {
    return await Promise.race([operation, deadline]);
  } finally {
    if (timer !== undefined) clearTimeout(timer);
  }
}
