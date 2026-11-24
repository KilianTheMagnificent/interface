import { APP_SELECT_CSS } from "../components/AppSelect";
import { useState } from "react";
import { flushSync } from "react-dom";
import type { Root } from "react-dom/client";
import { Composer, type ComposerThreadCard } from "../components/Composer";
import type { WorkflowCommand, WorkflowView } from "../../shared/workflows";
import type { ComposerLaunchForm, EngineChoices } from "../composer-model";
import type { PromptContextItem } from "../prompt-context";
import type { ThreadWorkspaceChoice } from "../thread-card";
import workflowFixture from "../../fixtures/workflows-v49.json";
import { assert, frames, noop, until } from "./fixture-support";

/** Real DOM coverage of setup save, recipe save/load, context and launch fields. */
export async function workflowsFixture(root: Root, theme: string): Promise<Record<string, unknown>> {
  document.documentElement.dataset.theme = theme;
  document.documentElement.dataset.chrome = "refined";
  const commands: WorkflowCommand[] = [];
  const launches: { forms: readonly ComposerLaunchForm[]; context: readonly PromptContextItem[] }[] = [];
  const choices: EngineChoices = { models: ["claude-opus-5"], efforts: [], permissionModes: ["default"] };
  const sample = workflowFixture.workflow as WorkflowView;
  function Harness() {
    const [view, setView] = useState<WorkflowView>(sample);
    const [context, setContext] = useState<PromptContextItem[]>([{ id: "readme", kind: "file", label: "README.md", value: "/workspace/project/README.md" }]);
    const [workspace, setWorkspace] = useState<ThreadWorkspaceChoice>({ mode: "worktree", baseRef: "main" });
    const thread: ComposerThreadCard = {
      projects: [{ root: sample.projectRoot, label: "Project" }], project: sample.projectRoot,
      onProjectChange: noop, onChooseFolder: async () => null, workspace, onWorkspaceChange: setWorkspace,
      branches: { current: "main", branches: ["main"] }, worktrees: [], worktreeUnavailable: null,
      error: null, busy: false,
      onLaunch: async (forms, _scope, attachments) => { launches.push({ forms, context: attachments }); return true; },
      workflow: { view, error: null, request: async command => {
        commands.push(command);
        const next: WorkflowView = { ...view, draft: undefined };
        if (command.action === "saveSetup") next.setup = command.setup;
        if (command.action === "saveRecipe") next.recipes = [command.recipe];
        if (command.action === "useRecipe") next.draft = sample.draft;
        setView(next); return next;
      } },
    };
    return <div className="workspace-composer-panel" style={{ height: "100vh" }}>
      <style>{APP_SELECT_CSS}</style>
      <Composer open kind="chat" thread={thread}
        defaults={{ engine: "claude", cwd: sample.projectRoot, prompt: "Review the current changes", model: "claude-opus-5", effort: "default", permissionMode: "default" }}
        engines={[{ id: "claude" }]} choicesFor={() => choices} onLaunch={noop}
        onBrowse={async () => null} onClose={noop} contextItems={context} onRecipeContext={setContext} />
    </div>;
  }
  flushSync(() => root.render(<Harness />));
  await frames(3);
  const controls = document.querySelector<HTMLElement>(".workflow-controls")!;
  assert(controls, "Setup & recipes controls are absent from the card");
  controls.querySelector<HTMLElement>("summary")!.click();
  await frames();
  function fill(suffix: string, value: string): void {
    const field = controls.querySelector<HTMLInputElement | HTMLTextAreaElement>(`[id$="-${suffix}"]`)!;
    assert(field, `Missing ${suffix} input`);
    const prototype = field instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
    Object.getOwnPropertyDescriptor(prototype, "value")!.set!.call(field, value);
    field.dispatchEvent(new Event("input", { bubbles: true }));
  }
  function click(text: string): void {
    const button = [...document.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.trim() === text);
    assert(button && !button.disabled, `Missing/enabled button: ${text}`);
    button.click();
  }
  fill("setup", "npm ci\nnpm run build");
  await frames();
  click("Save setup");
  await until(() => commands.some(command => command.action === "saveSetup"), "Setup was not saved");
  const setup = commands.find(command => command.action === "saveSetup");
  assert(setup?.action === "saveSetup" && setup.setup.commands.length === 2, "Setup commands did not reach Rust's command intact");
  await frames();
  fill("name", "Release review"); fill("outputs", "review.md"); fill("checks", "npm test");
  await frames();
  click("Save current draft as recipe");
  await until(() => commands.some(command => command.action === "saveRecipe"), "Recipe was not saved");
  const saved = commands.find(command => command.action === "saveRecipe");
  assert(saved?.action === "saveRecipe", "Missing save command");
  assert(saved.recipe.prompt === "Review the current changes", "Recipe lost the prompt");
  assert(saved.recipe.context[0]?.value === "/workspace/project/README.md", "Recipe lost file context");
  assert(saved.recipe.expectedOutputs[0] === "review.md" && saved.recipe.checks[0] === "npm test", "Recipe lost output/check requirements");
  await frames();
  const select = controls.querySelector<HTMLSelectElement>("select")!;
  select.value = "Release review"; select.dispatchEvent(new Event("change", { bubbles: true }));
  await frames(); click("Use recipe & setup");
  await until(() => document.querySelector<HTMLTextAreaElement>(".composer-ta")!.value === sample.draft!.prompt, "Recipe prompt did not load");
  const prompt = document.querySelector<HTMLTextAreaElement>(".composer-ta")!;
  prompt.focus();
  prompt.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", metaKey: true, bubbles: true }));
  await until(() => launches.length === 1, "Recipe draft did not launch");
  assert(launches[0]!.forms[0]!.prompt === sample.draft!.prompt, "Expected outputs/checks did not reach launch");
  assert(launches[0]!.context[0]!.value === sample.draft!.context[0]!.value, "Recipe context did not reach launch");
  assert(controls.scrollWidth <= controls.clientWidth + 1, "Workflow controls overflow sideways");
  return { savedSetup: true, savedRecipe: true, loadedRecipe: true, launchedRecipe: true, theme };
}
