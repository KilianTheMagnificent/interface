import { describe, expect, it, vi } from "vitest";
import { launchThreadCard } from "./thread-card";
import { COMPOSER_SCOPE_DEFAULTS, COMPOSER_TEAM_DEFAULTS } from "./composer-model";
import fixture from "../fixtures/threads-v34.json";
import type { ThreadControlResponse, ThreadsView } from "../shared/threads";

describe("workspace readiness at provider launch", () => {
  it("never launches for failed, interrupted or running setup, even if a token is present", async () => {
    for (const state of ["failed", "interrupted", "running"] as const) {
      const launch = vi.fn();
      const response: ThreadControlResponse = {
        view: fixture.view as unknown as ThreadsView,
        prepared: { token: "must-not-launch", cwd: "/worktree", workspace: { mode: "worktree", projectRoot: "/project", worktreePath: "/worktree", owned: true } },
        setup: { cwd: "/worktree", state, steps: [], message: `Setup ${state}` },
      };
      const result = await launchThreadCard({ launch, request: async () => response }, {
        project: "/project", workspace: { mode: "worktree" }, context: [], scope: {}, background: false,
        forms: [{ ...COMPOSER_SCOPE_DEFAULTS, ...COMPOSER_TEAM_DEFAULTS,
          engine: "claude", model: "sonnet", effort: "default", permissionMode: "default", cwd: "/project", prompt: "Build it" }],
      });
      expect(result).toEqual({ status: "prepare-failed", message: `Setup ${state}` });
      expect(launch).not.toHaveBeenCalled();
    }
  });
});
