import { useEffect, useId, useState } from "react";
import type { RecipeDraft, WorkflowCommand, WorkflowRecipe, WorkflowView } from "../../shared/workflows";
import type { ComposerLaunchForm } from "../composer-model";
import type { PromptContextItem } from "../prompt-context";
import type { ThreadWorkspaceChoice } from "../thread-card";
import { ipcMessage } from "../ipc-message";
import "./WorkflowControls.css";

export interface WorkflowApi {
  view: WorkflowView | null;
  error: string | null;
  request(command: WorkflowCommand): Promise<WorkflowView>;
}

export interface WorkflowControlsProps {
  project: string;
  api: WorkflowApi;
  form: ComposerLaunchForm;
  context: readonly PromptContextItem[];
  workspace: ThreadWorkspaceChoice;
  launching: boolean;
  onUse(draft: RecipeDraft): void;
}

/** Draws Rust's saved state. Every save/use/prepare decision stays in Rust. */
export function WorkflowControls({ project, api, form, context, workspace, launching, onUse }: WorkflowControlsProps) {
  const id = useId();
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [selected, setSelected] = useState("");
  const [prerequisites, setPrerequisites] = useState("");
  const [commands, setCommands] = useState("");
  const [timeout, setTimeoutSeconds] = useState(120);
  const [outputs, setOutputs] = useState("");
  const [checks, setChecks] = useState("");
  const savedSetup = api.view?.setup;
  useEffect(() => {
    if (savedSetup === undefined) return;
    setPrerequisites(savedSetup.prerequisites.join("\n"));
    setCommands(savedSetup.commands.join("\n"));
    setTimeoutSeconds(savedSetup.timeoutSeconds);
  }, [savedSetup]);
  const lines = (value: string): string[] => value.split("\n").map(line => line.trim()).filter(Boolean);
  const setup = { prerequisites: lines(prerequisites), commands: lines(commands), timeoutSeconds: timeout };
  const disabled = launching || busy || api.view === null;
  const temporary = context.some(item => !["file", "folder", "url"].includes(item.kind) || item.previewUrl !== undefined);
  const run = async (command: WorkflowCommand, message: string): Promise<void> => {
    if (busy || launching) return;
    setBusy(true); setNotice(null);
    try {
      const view = await api.request(command);
      if (view.draft !== undefined) {
        onUse(view.draft);
        setName(view.draft.recipe.name);
        setOutputs(view.draft.recipe.expectedOutputs.join("\n"));
        setChecks(view.draft.recipe.checks.join("\n"));
      }
      setNotice(message);
    } catch (error: unknown) { setNotice(ipcMessage(error)); }
    finally { setBusy(false); }
  };
  const saveRecipe = (): void => {
    const recipe: WorkflowRecipe = {
      name, prompt: form.prompt, engine: form.engine, model: form.model,
      context: context.flatMap(item => item.kind === "file" || item.kind === "folder" || item.kind === "url"
        ? [{ kind: item.kind, value: item.value }] : []),
      newWorktree: workspace.mode !== "local",
      ...(workspace.mode === "worktree" && workspace.baseRef !== undefined ? { baseRef: workspace.baseRef } : {}),
      setup, expectedOutputs: lines(outputs), checks: lines(checks),
    };
    void run({ action: "saveRecipe", projectRoot: project, recipe }, "Recipe saved. Reusing it fills the draft; Start launches it.");
  };
  const report = api.view?.lastSetup;
  return <details className="workflow-controls">
    <summary>Setup & recipes{report === undefined ? "" : ` · ${report.state}`}</summary>
    <div className="workflow-controls-body" aria-busy={busy || launching}>
      <p>Save commands you trust for this project. New worktrees run prerequisite checks first, then setup; a failed step stops launch. Existing checkouts are never set up automatically.</p>
      <label htmlFor={`${id}-prerequisites`}>Prerequisite checks <span>one shell command per line</span></label>
      <textarea id={`${id}-prerequisites`} rows={2} placeholder="command -v node" value={prerequisites}
        disabled={disabled} onChange={event => setPrerequisites(event.target.value)} />
      <label htmlFor={`${id}-setup`}>Setup commands <span>one shell command per line</span></label>
      <textarea id={`${id}-setup`} rows={2} placeholder="npm ci" value={commands}
        disabled={disabled} onChange={event => setCommands(event.target.value)} />
      <div className="workflow-controls-actions">
        <label htmlFor={`${id}-timeout`}>Total timeout (seconds)</label>
        <input id={`${id}-timeout`} type="number" min={1} max={600} value={timeout} disabled={disabled}
          onChange={event => setTimeoutSeconds(event.target.valueAsNumber)} />
        <button type="button" className="btn" disabled={disabled}
          onClick={() => { void run({ action: "saveSetup", projectRoot: project, setup }, "Saved. These commands run when you start a new worktree."); }}>Save setup</button>
      </div>
      <p>Recipes keep this prompt, provider/model, file/folder/URL context, workspace choice and setup, plus the requested outputs and checks below. File and folder attachments refer to the original project; setup commands run in the new worktree.</p>
      <label htmlFor={`${id}-recipe`}>Saved recipe</label>
      <select id={`${id}-recipe`} value={selected} disabled={disabled}
        onChange={event => setSelected(event.target.value)}>
        <option value="">Choose a recipe…</option>
        {api.view?.recipes.map(recipe => <option key={recipe.name} value={recipe.name}>{recipe.name} · {recipe.engine} / {recipe.model}</option>)}
      </select>
      <div className="workflow-controls-actions">
        <button type="button" className="btn" disabled={disabled || selected === ""}
          onClick={() => { void run({ action: "useRecipe", projectRoot: project, name: selected }, "Recipe loaded. Its saved setup is now active for this project. Review the draft, then Start."); }}>Use recipe & setup</button>
        <button type="button" className="btn" disabled={disabled || selected === ""}
          onClick={() => { void run({ action: "deleteRecipe", projectRoot: project, name: selected }, "Recipe deleted."); setSelected(""); }}>Delete recipe</button>
      </div>
      <label htmlFor={`${id}-name`}>Recipe name <span>saving the same name replaces it</span></label>
      <input id={`${id}-name`} value={name} maxLength={100} disabled={disabled} onChange={event => setName(event.target.value)} />
      <label htmlFor={`${id}-outputs`}>Expected outputs <span>one per line</span></label>
      <textarea id={`${id}-outputs`} rows={2} value={outputs} disabled={disabled} onChange={event => setOutputs(event.target.value)} />
      <label htmlFor={`${id}-checks`}>Required checks <span>instructions for the agent, one per line</span></label>
      <textarea id={`${id}-checks`} rows={2} value={checks} disabled={disabled} onChange={event => setChecks(event.target.value)} />
      {temporary && <p role="status">Temporary images, diffs and chat context cannot be reused in a recipe. Remove those attachments before saving.</p>}
      <button type="button" className="btn" disabled={disabled || temporary || name.trim() === "" || form.prompt.trim() === ""}
        onClick={saveRecipe}>Save current draft as recipe</button>
      {launching && <p role="status">Preparing the workspace before starting the provider…</p>}
      {(notice ?? api.error) !== null && <p role="status">{notice ?? api.error}</p>}
      {report !== undefined && <details className="workflow-setup-log" open={report.state !== "ready"}>
        <summary>Last setup: {report.state}</summary>
        <p>{report.message}</p><code>{report.cwd}</code>
        {report.steps.map((step, index) => <section key={`${index}-${step.command}`}>
          <strong>{step.prerequisite ? "Check" : "Setup"}: {step.command}</strong>
          <span>{step.exitCode === undefined ? "No exit code" : `Exit ${step.exitCode}`}{step.timedOut ? " · timed out" : ""}{step.truncated ? " · output limit reached" : ""}</span>
          <pre>{step.output || "No output."}</pre>
        </section>)}
        {(report.state === "failed" || report.state === "interrupted") && <button type="button" className="btn" disabled={disabled}
          onClick={() => { void run({ action: "retrySetup", projectRoot: project, cwd: report.cwd }, "Setup finished. Review its result before selecting the existing worktree."); }}>
          Run saved setup again in this worktree
        </button>}
      </details>}
    </div>
  </details>;
}
