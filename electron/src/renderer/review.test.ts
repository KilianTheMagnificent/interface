import { describe, expect, it } from "vitest";
import type { RepositoryReview } from "../shared/ipc";
import {
  REPOSITORY_REVIEW_PROMPT_MAX_CHARS, repositoryReviewPromptContent,
} from "./review";

const base = (): RepositoryReview => ({
  root: "/work/interface",
  repository: true,
  branch: "feature/review",
  head: "0123456789abcdef",
  clean: false,
  truncated: false,
  files: [{
    path: "src/App.tsx",
    states: ["staged", "unstaged"],
    status: "modified",
    additions: 7,
    deletions: 2,
    binary: false,
    patch: "@@ -1 +1 @@\n-old\n+new",
    patchTruncated: false,
  }],
});

describe("repositoryReviewPromptContent", () => {
  it("includes only credential-filtered admitted patch bytes", () => {
    const review = base();
    review.files.push({
      path: "client_secret.json", states: ["unstaged"], status: "modified",
      additions: null, deletions: null, binary: false, patch: "",
      patchTruncated: false, omitted: "credential",
    });
    const content = repositoryReviewPromptContent(review);
    expect(content).toContain("src/App.tsx");
    expect(content).toContain("-old");
    expect(content).toContain("Complete text patches included: 1");
    expect(content).toContain("Reported changed paths: 2");
    expect(content).toContain("Snapshot status: partial");
    expect(content).not.toContain("client_secret.json");
  });

  it("is bounded, never clips a per-file patch, and refuses non-repositories", () => {
    const review = base();
    review.files[0].patch = `BEGIN_PATCH\n${
      "x".repeat(REPOSITORY_REVIEW_PROMPT_MAX_CHARS * 2)}\nEND_PATCH`;
    const content = repositoryReviewPromptContent(review);
    expect(content?.length).toBeLessThanOrEqual(REPOSITORY_REVIEW_PROMPT_MAX_CHARS);
    expect(content).toContain("Complete text patches included: 0");
    expect(content).toContain("Snapshot status: partial");
    expect(content).not.toContain("BEGIN_PATCH");
    expect(content).not.toContain("END_PATCH");
    expect(content).not.toContain("src/App.tsx");
    expect(repositoryReviewPromptContent({
      ...review, repository: false, files: [], clean: null,
    })).toBeNull();
  });

  it("accepts explicitly bounded partial snapshots but rejects other Git errors", () => {
    const limited: RepositoryReview = {
      ...base(), clean: null, truncated: true,
      error: { code: "output-limit", message: "Bounded snapshot." },
    };
    expect(repositoryReviewPromptContent(limited)).toContain("Snapshot status: partial");

    const failed: RepositoryReview = {
      ...base(), clean: null,
      error: { code: "git-error", message: "Git failed partway through." },
    };
    expect(repositoryReviewPromptContent(failed)).toBeNull();
  });
});
