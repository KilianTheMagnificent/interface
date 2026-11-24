/** Rust-owned saved workspace setup and reusable launch drafts. */
export interface ProjectSetup { prerequisites: string[]; commands: string[]; timeoutSeconds: number }
export interface RecipeContext { kind: "file" | "folder" | "url"; value: string }
export interface WorkflowRecipe {
  name: string; prompt: string; engine: string; model: string; context: RecipeContext[];
  newWorktree: boolean; baseRef?: string; setup: ProjectSetup;
  expectedOutputs: string[]; checks: string[];
}
export interface SetupStep {
  command: string; prerequisite: boolean; output: string; exitCode?: number;
  timedOut: boolean; truncated: boolean;
}
export interface SetupReport {
  cwd: string; state: "running" | "ready" | "failed" | "interrupted";
  steps: SetupStep[]; message: string;
}
export interface RecipeDraft { recipe: WorkflowRecipe; prompt: string; context: RecipeContext[] }
export interface WorkflowView {
  projectRoot: string; setup: ProjectSetup; recipes: WorkflowRecipe[];
  lastSetup?: SetupReport; draft?: RecipeDraft;
}
export type WorkflowCommand =
  | { action: "read"; projectRoot: string }
  | { action: "saveSetup"; projectRoot: string; setup: ProjectSetup }
  | { action: "saveRecipe"; projectRoot: string; recipe: WorkflowRecipe }
  | { action: "deleteRecipe"; projectRoot: string; name: string }
  | { action: "useRecipe"; projectRoot: string; name: string }
  | { action: "retrySetup"; projectRoot: string; cwd: string };
