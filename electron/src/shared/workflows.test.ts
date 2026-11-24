import { describe, expect, it } from "vitest";
import fixture from "../fixtures/workflows-v49.json";
import type { ProjectSetup, RecipeContext, RecipeDraft, SetupReport, SetupStep, WorkflowCommand, WorkflowRecipe, WorkflowView } from "./workflows";
import type { ThreadCommand } from "./threads";
import { encodeRendererPayload } from "../main/ipc-payload";
import { CHANNELS } from "./ipc";
type Fields<T> = { [K in keyof T]-?: undefined extends T[K] ? false : true };
const SETUP: Fields<ProjectSetup> = { prerequisites: true, commands: true, timeoutSeconds: true };
const CONTEXT: Fields<RecipeContext> = { kind: true, value: true };
const RECIPE: Fields<WorkflowRecipe> = { name: true, prompt: true, engine: true, model: true, context: true, newWorktree: true, baseRef: false, setup: true, expectedOutputs: true, checks: true };
const STEP: Fields<SetupStep> = { command: true, prerequisite: true, output: true, exitCode: false, timedOut: true, truncated: true };
const REPORT: Fields<SetupReport> = { cwd: true, state: true, steps: true, message: true };
const DRAFT: Fields<RecipeDraft> = { recipe: true, prompt: true, context: true };
const VIEW: Fields<WorkflowView> = { projectRoot: true, setup: true, recipes: true, lastSetup: false, draft: false };
type Action = WorkflowCommand["action"];
const COMMANDS: { [K in Action]: Fields<Omit<Extract<WorkflowCommand, {action: K}>, "action">> } = {
  read: { projectRoot: true }, saveSetup: { projectRoot: true, setup: true },
  saveRecipe: { projectRoot: true, recipe: true }, deleteRecipe: { projectRoot: true, name: true },
  useRecipe: { projectRoot: true, name: true }, retrySetup: { projectRoot: true, cwd: true },
};
function shape(object: object, fields: Record<string, boolean>): void {
  expect(Object.keys(object).every(key => key in fields)).toBe(true);
  for (const [key, required] of Object.entries(fields)) if (required) expect(object).toHaveProperty(key);
}
function recipe(value: WorkflowRecipe): void {
  shape(value, RECIPE); shape(value.setup, SETUP);
  value.context.forEach(item => shape(item, CONTEXT));
}
describe("workflow protocol 49", () => {
  it("pins every nested Rust DTO and command through the production encoder", () => {
    expect(fixture.protocolVersion).toBe(49);
    const view = fixture.workflow as WorkflowView;
    shape(view, VIEW); shape(view.setup, SETUP); view.recipes.forEach(recipe);
    shape(view.draft!, DRAFT); recipe(view.draft!.recipe); view.draft!.context.forEach(item => shape(item, CONTEXT));
    for (const report of fixture.reports) { shape(report, REPORT); report.steps.forEach(step => shape(step, STEP)); }
    expect(fixture.reports.map(report => report.state).sort()).toEqual(["failed", "interrupted", "ready", "running"]);
    const seen = new Set<string>();
    for (const wire of fixture.commands as ThreadCommand[]) {
      expect(encodeRendererPayload(CHANNELS.threadControl, [wire])).toEqual(wire);
      if (wire.kind !== "workflow") throw new Error("fixture must use workflow");
      const { action, ...fields } = wire.command;
      shape(fields, COMMANDS[action]); seen.add(action);
    }
    expect([...seen].sort()).toEqual(Object.keys(COMMANDS).sort());
  });
});
