export interface OutputInspectionRequest {
  version: 1;
  action: "list" | "read";
  root: string;
  path: string;
}
export interface OutputPreviewEntry { name: string; directory: boolean }
export type OutputPreviewContent =
  | { kind: "text"; text: string; format: "plain" | "markdown" | "diff" }
  | { kind: "image"; mediaType: string; bytes: number[] }
  | { kind: "pdf"; bytes: number[] }
  | { kind: "unsupported"; reason: string };
export interface OutputPreviewSnapshot {
  root: string;
  path: string;
  byteLength: number;
  revision: string | null;
  content: OutputPreviewContent;
}
export type OutputPreviewResponse = OutputReviewResponse
  | { kind: "directory"; version: 1; root: string; path: string; entries: OutputPreviewEntry[] }
  | { kind: "file"; version: 1; snapshot: OutputPreviewSnapshot };

export type OutputAnchor =
  | { kind: "file" }
  | { kind: "text"; quote: string; startLine?: number; endLine?: number }
  | { kind: "page"; page: number }
  | { kind: "region"; x: number; y: number; width: number; height: number };
export interface OutputReview {
  id: string;
  revision: number;
  snapshot: OutputPreviewSnapshot;
  anchor: OutputAnchor;
  note: string;
  threadKey: string | null;
  task: string;
  updatedAt: number;
  delivery: "sending" | "sent" | "unknown" | "unsent" | null;
}
export type OutputReviewRequest = { version: 2; root: string } & (
  | { action: "listReviews" }
  | { action: "captureReview"; path: string }
  | { action: "saveReview"; id: string; revision: number; anchor: OutputAnchor; note: string; threadKey: string | null; task: string }
  | { action: "reserveFeedback"; id: string; revision: number }
  | { action: "settleFeedback"; id: string; revision: number; delivery: "sent" | "unknown" | "unsent" }
  | { action: "deleteReview"; id: string; revision: number }
);
export type OutputPreviewRequest = OutputInspectionRequest | OutputReviewRequest;
export type OutputReviewResponse =
  | { kind: "reviews"; version: 2; reviews: OutputReview[] }
  | { kind: "review"; version: 2; review: OutputReview }
  | { kind: "deletedReview"; version: 2; id: string };
