import type { RepositoryReview } from "../shared/ipc";
import { productLabel } from "./product";

export const REPOSITORY_REVIEW_PROMPT_MAX_CHARS = 8_000;

/**
 * Build provider context only from patch bytes the main-process review reader
 * already admitted. Omitted paths are counted but never named, so a
 * credential-shaped filename cannot become an invitation to inspect it.
 */
export function repositoryReviewPromptContent(review: RepositoryReview,
    maxChars = REPOSITORY_REVIEW_PROMPT_MAX_CHARS): string | null {
  const usableComplete = review.error === undefined && review.clean !== null;
  const usableBoundedPartial = review.error?.code === "output-limit";
  if (!review.repository || (!usableComplete && !usableBoundedPartial)) return null;
  const limit = Math.min(REPOSITORY_REVIEW_PROMPT_MAX_CHARS,
    Math.max(256, Number.isFinite(maxChars) ? Math.floor(maxChars) : 256));
  const admitted = review.files.filter(file => file.patch !== ""
    && file.omitted === undefined && !file.binary && !file.patchTruncated);
  const branch = review.metadataComplete === false ? "branch metadata unavailable"
    : review.branch === null ? "detached HEAD"
    : review.branch.length <= 128 ? review.branch : "branch name withheld (over limit)";
  const header = (included: number): string => {
    const partial = review.truncated || included < review.files.length;
    const full = [
      `${productLabel()} Review snapshot (credential-filtered, read-only)`,
      `Branch: ${branch}`,
      `Commit: ${review.metadataComplete === false
        ? "commit metadata unavailable" : review.head?.slice(0, 12) ?? "no commits"}`,
      `Complete text patches included: ${included}`,
      `Reported changed paths: ${review.files.length}${
        review.truncated ? " (partial list)" : ""}`,
      partial
        ? "Snapshot status: partial; omitted, non-text, unsafe, truncated, or "
          + "over-budget data is not included and must not be reconstructed."
        : "Snapshot status: complete for every reported changed path.",
    ].join("\n");
    if (full.length <= limit) return full;
    // Extremely small caller bounds omit repository metadata rather than
    // clipping it into a statement that could look authoritative.
    return [
      `${productLabel()} Review snapshot (credential-filtered, read-only)`,
      `Complete text patches included: ${included}`,
      "Snapshot status: partial; metadata or patch data did not fit and must not be reconstructed.",
    ].join("\n");
  };
  const blocks: string[] = [];
  for (const file of admitted) {
    const block = `\n\n--- ${file.path} (${file.states.join(", ")}) ---\n${file.patch}`;
    const candidate = blocks.join("") + block;
    // Unified diffs are atomic at this boundary. A clipped patch can conceal a
    // missing hunk or layer while still looking complete to the provider.
    if (header(blocks.length + 1).length + candidate.length <= limit) {
      blocks.push(block);
    }
  }
  let result = header(blocks.length) + blocks.join("");
  if (blocks.length === 0) {
    const empty = "\n\nNo credential-filtered text patches were available in this snapshot.";
    if (result.length + empty.length <= limit) result += empty;
  }
  return result;
}
