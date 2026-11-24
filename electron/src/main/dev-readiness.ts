import { withStartupDeadline } from "./startup-lifecycle.js";

// This dev-only observation uses the existing read-only API. It adds no IPC
// operation and does not open a provider, terminal, or canonical store.
export const DEV_READINESS_PROBE = `(async () => {
  if (!document.querySelector("#root")?.firstElementChild) return false;
  if (typeof window.iface?.getAppearance !== "function") return false;
  const appearance = await window.iface.getAppearance();
  return appearance !== null && typeof appearance === "object";
})()`;

/** A refresh that `cargo xtask dev-restart` (and so every closeout) started
 * carries INTERFACE_DEV_LAUNCH. It creates its window hidden, so a landing
 * never pulls Dev over the owner's work or takes focus; the window appears the
 * first time the owner switches to Interface Dev. The one exception is
 * INTERFACE_DEV_QUIET=0, which dev-restart sends when Dev was the app in
 * front. Hidden is the default because an agent closes out with the xtask of
 * its own branch, which may predate that flag. A Dock launch, `npm run dev`
 * and production never match. */
export function devLaunchStaysHidden(appName: string, env: Readonly<Record<string, string | undefined>>): boolean {
  return appName === "Interface Dev" && env.INTERFACE_DEV_SUPERVISED === "1"
    && Boolean(env.INTERFACE_DEV_LAUNCH) && env.INTERFACE_DEV_QUIET !== "0";
}

export async function reportDevReadiness(options: {
  launch: string | undefined;
  revision: string | undefined;
  active: () => boolean;
  probe: () => Promise<unknown>;
  emit: (line: string) => void;
  wait?: () => Promise<void>;
}): Promise<void> {
  const { launch, revision } = options;
  if (!launch || !revision || !/^[0-9a-f]{40}$/.test(revision)) return;
  const wait = options.wait ?? (() => new Promise(resolve => setTimeout(resolve, 250)));
  try {
    for (let attempt = 0; attempt < 60 && options.active(); attempt += 1) {
      if (await withStartupDeadline("Dev readiness", options.probe()) === true && options.active()) {
        options.emit(`INTERFACE_DEV_READY ${JSON.stringify({ launch, revision, pid: process.pid })}`);
        return;
      }
      await wait();
    }
    throw new Error("renderer and Rust bridge did not become ready");
  } catch {
    options.emit(`INTERFACE_DEV_FAILED ${JSON.stringify({ launch, revision })}`);
  }
}
