import { join } from "node:path";
import { fileURLToPath } from "node:url";

export const root = fileURLToPath(new URL("..", import.meta.url));

/** What `npm run check` (scripts/check.mjs) runs: independent parts side by
 * side, each part's commands in order, as [part, [[label, command, args]]].
 *   unit      test discovery, Vitest and the release-script tests
 *   types     the renderer typecheck
 *   renderer  the full build (which rewrites dist-main), then the renderer DOM
 *             lanes against that fresh dist-main
 * The build completes before the check passes, so a passing check leaves the
 * same dist, dist-remote and dist-main as `npm run build`: the gate's
 * packaging step reuses them. */
export const CHECK_PARTS = [
  ["unit", [["npm test", "npm", ["test"]]]],
  ["types", [["tsc -p tsconfig.json --noEmit", join(root, "node_modules/.bin/tsc"), ["-p", "tsconfig.json", "--noEmit"]]]],
  ["renderer", [["npm run build", "npm", ["run", "build"]],
    ["node scripts/test-renderer-dom.mjs", process.execPath, ["scripts/test-renderer-dom.mjs"]]]],
];
