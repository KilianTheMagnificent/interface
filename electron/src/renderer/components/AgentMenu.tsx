import { useEffect, useId, useRef, useState, type FormEvent } from "react";
import { createPortal } from "react-dom";
import type { ModelOption, SessionSummary } from "../../shared/engine";
import { findCatalogModel } from "../model-catalog";
import {
  AGENT_COLORS, type AgentColor, type AgentProfile,
} from "../agent-profiles";
import {
  AGENT_SHAPES, type AgentPermissionMode, type AgentShape, type AgentStatus, type AgentThreadState,
} from "../../shared/agents";
import { AGENT_SHAPE_PATHS, LEGACY_AGENT_SHAPES, AGENT_CHARACTERS } from "../agent-appearance";
import { sessionAgeLabel } from "../sidebar-view";
import { ATTENTION_TRAY_DOT_CSS } from "./AttentionTray";
import { FormSelect } from "./FormSelect";
import { useModalFocus } from "./ModalBehavior";
import { WorkingIndicator } from "./WorkingIndicator";

/**
 * The Agent surface's roster: the searchable list of agents, the header the
 * selected one wears above its task, and the form that creates or edits one.
 * Presentational throughout — App.tsx owns the store and every effect.
 */
export interface AgentMenuProps {
  readonly agents: readonly AgentProfile[];
  readonly selectedId: string | null;
  readonly latestTasks?: Readonly<Record<string, SessionSummary>>;
  /** Rust's one status per agent; absent or idle draws no mark. */
  readonly statuses?: Readonly<Record<string, AgentStatus>>;
  /** Each agent's thread, keyed by agent id. */
  readonly threads?: Readonly<Record<string, AgentThreadState>>;
  /** The latest line of a thread session, keyed by session key. */
  readonly threadPreviews?: Readonly<Record<string, string>>;
  readonly now?: number;
  readonly onSelect: (agentId: string) => void;
  readonly onCreate: () => void;
  /** The agent store has not answered yet, so there is nowhere to save one. */
  readonly createDisabled?: boolean;
}

export interface AgentHeaderProps {
  readonly profile: AgentProfile;
  /** The provider's own display name; the engine id stays the routing identity. */
  readonly engineLabel: string;
  readonly workingCount: number;
  readonly onEdit: () => void;
  readonly onDelete: () => void;
}

export interface AgentEngineOption {
  readonly id: string;
  readonly label: string;
  readonly available: boolean;
  /** Why it cannot be chosen, printed under its row; absent for a plain outage. */
  readonly unavailableReason?: string;
}

export interface AgentProfileDialogProps {
  readonly open: boolean;
  /** The agent being edited, or the id/colour defaults the caller minted for a new one. */
  readonly initial: AgentProfile | null;
  readonly engines: readonly AgentEngineOption[];
  readonly effortLevelsFor: (engine: string) => readonly string[];
  readonly modelsFor?: (engine: string) => readonly ModelOption[];
  /** The agent's saved permission mode; a new agent asks. */
  readonly initialPermission?: AgentPermissionMode;
  readonly onSubmit: (profile: AgentProfile, permission: AgentPermissionMode) => void;
  readonly onCancel: () => void;
}

const AGENT_PERMISSION_MODES: readonly AgentPermissionMode[] = ["gated", "auto", "bypass"];
export const AGENT_PERMISSION_LABELS: Readonly<Record<AgentPermissionMode, string>> = {
  gated: "Ask", auto: "Auto", bypass: "Bypass",
};
export const AGENT_PERMISSION_HELP: Readonly<Record<AgentPermissionMode, string>> = {
  gated: "Interface's policy decides, then asks you.",
  auto: "Claude or Codex decides with its own safety checks and asks you what it won't decide.",
  bypass: "No permission checks and no sandbox: the agent can do anything your account can.",
};
export const AGENT_PERMISSION_TIMING = "A change applies from this agent's next conversation, routine run or resume. Its running conversation keeps the mode it started with.";
export const AGENT_BYPASS_WARNING = "This includes routines that run while you're away. Use it only for an agent you'd trust with your whole account.";
export const AGENT_AUTO_MODEL_NOTE = "Claude's auto mode needs Opus or Sonnet 4.6 or newer, or Fable. With this model Claude asks before each action.";

/** Whether Claude's auto mode is unavailable for a requested model: Haiku, and
 *  Opus or Sonnet before 4.6. An empty model, an alias and Fable are fine. */
export function claudeAutoUnsupported(model: string): boolean {
  const lower = model.toLowerCase();
  if (lower.includes("haiku")) return true;
  const version = /(?:opus|sonnet)-(\d+)(?:-(\d+))?/u.exec(lower);
  if (version === null) return false;
  const major = Number(version[1]);
  const minor = Number(version[2] ?? "0");
  return major < 4 || (major === 4 && minor < 6);
}

const CUSTOM_MODEL = "\u0000custom";
const NO_MODELS = (): readonly ModelOption[] => [];

/** Catalog metadata wins; an unlisted custom model keeps the provider fallback. */
export function agentModelEfforts(engine: string, model: string,
    models: readonly ModelOption[], fallback: readonly string[]): readonly string[] {
  const selected = model === "" ? models.find(row => row.isDefault)
    : findCatalogModel(model, models, engine);
  return selected === undefined ? fallback
    : selected.supportsEffort ? selected.effortLevels : [];
}

export const AGENT_MENU_CSS = `
/* Agent identities lead the sidebar; task navigation follows beneath it. */
.agent-menu { display:flex; flex-direction:column; flex:0 1 auto; min-height:150px;
  max-height:68%; padding:0 8px 12px; border-bottom:var(--border-w) solid var(--border); }
.agent-menu-heading { display:flex; flex:none; align-items:center; justify-content:space-between;
  gap:8px; padding:12px 8px 9px; }
.agent-menu-heading .sidebar-section-actions { display:flex; align-items:center; }
/* The same header as Tasks, Chats and Workspaces (owner, 2026-09-28): the
   label size, the + and the search glyph all match the section below. */
.agent-menu-heading .sidebar-section-label { color:var(--fg-strong); }
.agent-menu-search { display:flex; flex:none; align-items:center; gap:7px; margin:0 4px 10px;
  padding:0 10px; min-height:34px; border:var(--border-w) solid var(--input-border);
  border-radius:var(--radius-control); background:var(--input-bg); color:var(--fg-dim); }
.agent-menu-search:focus-within { outline:var(--focus-ring-w) solid var(--focus); outline-offset:1px; }
.agent-menu-search input { min-width:0; width:100%; padding:6px 0; border:0; outline:none;
  color:var(--input-fg); background:transparent; font:inherit; font-size:var(--text-ui); }
.agent-menu-search input::placeholder { color:var(--input-placeholder); }
.agent-menu-roster { display:flex; flex-direction:column; gap:3px; min-height:0;
  overflow:auto; padding:3px; scrollbar-gutter:stable; }
.agent-menu-row { display:flex; flex:none; align-items:center; gap:11px; min-width:0;
  width:100%; min-height:66px; padding:10px 9px; color:var(--fg); background:transparent;
  border:0; border-radius:var(--radius-overlay); font:inherit; font-family:var(--font-ui);
  text-align:left; cursor:pointer; }
.agent-menu-row:hover { background:var(--row-hover); }
.agent-menu-row[aria-selected="true"] { background:var(--row-selected); }
.agent-menu-row:focus-visible { outline:var(--focus-ring-w) solid var(--focus); outline-offset:-2px; }
.agent-menu-row-copy { display:flex; flex:1; flex-direction:column; gap:4px; min-width:0; }
.agent-menu-row-top { display:flex; align-items:baseline; gap:8px; min-width:0; }
.agent-menu-row-name { flex:1; min-width:0; overflow:hidden; color:var(--fg-strong);
  font-size:var(--text-ui); font-weight:var(--wght-medium); text-overflow:ellipsis; white-space:nowrap; }
.agent-menu-row-age { flex:none; color:var(--fg-dim); font-size:var(--text-meta); }
/* The status mark reuses the sidebar's working dot, its attention glyph and the
   tray's unread dot; this only seats it on the name line. */
.agent-menu-status { flex:none; align-self:center; }
${ATTENTION_TRAY_DOT_CSS}
.agent-menu-row-preview { overflow:hidden; color:var(--fg-muted); font-size:var(--text-meta);
  line-height:var(--lh-ui); text-overflow:ellipsis; white-space:nowrap; }
.agent-menu-empty { margin:8px; color:var(--fg-dim); font-size:var(--text-body); line-height:var(--lh-ui); }
.agent-avatar { display:inline-flex; flex:none; align-items:center; justify-content:center; }
.agent-avatar svg { display:block; width:100%; height:100%; overflow:visible; }

/* The header belongs to the agent, not to the task, so it sits above BOTH the
   zero-state and a running task and never scrolls with either. */
.agent-surface-stack { display: flex; flex-direction: column; height: 100%;
  min-height: 0; background: var(--editor-bg); }
.agent-surface-stack > :last-child { flex: 1 1 auto; min-height: 0; }
.agent-surface-head { display: flex; flex: none; align-items: center; gap: 10px;
  padding: 10px 18px; border-bottom: var(--border-w) solid var(--border);
  font-family: var(--font-ui); }
.agent-header-copy { display: flex; min-width: 0; flex-direction: column; }
.agent-header-name { overflow: hidden; color: var(--fg-strong); font-size: var(--text-ui);
  font-weight: var(--wght-strong); text-overflow: ellipsis; white-space: nowrap; }
.agent-header-provider { color: var(--fg-muted); font-size: var(--text-meta); overflow-wrap:anywhere; }
/* Placement and figures only: the capsule is theme.css's .badge-quiet. */
.agent-header-pill { margin-left: auto; font-variant-numeric: tabular-nums; }
.agent-header-action { display: inline-flex; flex: none; align-items: center;
  justify-content: center; width: 24px; height: 24px; padding: 0; color: var(--fg-dim);
  background: transparent; border: 0; border-radius: var(--radius-control); cursor: pointer; }
.agent-header-action:hover { color: var(--fg-strong); background: var(--row-hover); }

.agent-dialog-backdrop { position: fixed; inset: 0; z-index: 1250; display: flex;
  align-items: center; justify-content: center; padding: 18px;
  background: color-mix(in srgb, var(--chrome-bg) 70%, transparent); }
/* Pickers portal to body, so their menus must sit above this dialog. */
.app-select-popover.agent-dialog-picker { z-index: 1251; }
.agent-dialog { display: flex; flex-direction: column; box-sizing: border-box;
  width: min(480px, calc(100vw - 36px)); max-height: calc(100vh - 36px); overflow: auto;
  color: var(--fg); background: var(--widget-bg);
  border: var(--border-w) solid var(--border-strong); border-radius: var(--radius-overlay);
  box-shadow: var(--overlay-shadow); font-family: var(--font-ui); }
.agent-dialog-copy { display: flex; flex-direction: column; gap: 10px; padding: 16px; }
.agent-dialog-title { margin: 0; color: var(--fg-strong); font-size: var(--text-prose);
  font-weight: var(--wght-strong); }
.agent-dialog-field { display: flex; flex-direction: column; gap: 4px; }
/* FormSelect names its own field, so the picker's label is the field's first
   span rather than a class the caller passes. */
.agent-dialog-label, .agent-dialog-field > span:first-child { color: var(--fg-muted);
  font-size: var(--text-meta); }
.agent-dialog-hint { margin:0; color:var(--fg-dim); font-size:var(--text-meta);
  line-height:var(--lh-ui); }
.agent-dialog-input { box-sizing: border-box; width: 100%;
  height: var(--control-h); padding: 0 9px; color: var(--input-fg); background: var(--input-bg);
  border: var(--border-w) solid var(--input-border); border-radius: var(--radius-control);
  outline: none; font-family: var(--font-ui); font-size: var(--text-ui); }
/* FormSelect owns the shared field shape. */
.agent-dialog-textarea { box-sizing: border-box; width: 100%; padding: 7px 9px;
  color: var(--input-fg); background: var(--input-bg);
  border: var(--border-w) solid var(--input-border); border-radius: var(--radius-control);
  outline: none; resize: vertical; font-family: var(--font-ui); font-size: var(--text-ui); line-height: var(--lh-ui); }
.agent-dialog-input:focus, .agent-dialog-textarea:focus {
  border-color: var(--focus); box-shadow: 0 0 0 1px var(--focus); }
/* The chosen character at full size, redrawn as its shape and colour change. */
.agent-dialog-preview { display:flex; align-items:center; justify-content:center; padding:12px 0;
  border-radius:var(--radius-overlay); background:var(--row-hover); }
.agent-dialog-colors { display: flex; flex-wrap: wrap; gap: 8px; }
/* Room between the rows so the two selection rings never touch. */
.agent-dialog-shapes { display: grid; grid-template-columns: repeat(auto-fill, minmax(40px, 1fr)); gap: 4px; margin-bottom: 6px; }
.agent-dialog-color { display: inline-flex; cursor: pointer; }
.agent-dialog-color input { position: absolute; width: 1px; height: 1px; overflow: hidden;
  clip-path: inset(50%); }
.agent-dialog-swatch { display:inline-flex; align-items:center; justify-content:center;
  width:42px; height:42px; border-radius:var(--radius-control); border:var(--border-w) solid transparent; }
.agent-dialog-color input:checked + .agent-dialog-swatch {
  box-shadow: 0 0 0 2px var(--widget-bg), 0 0 0 4px var(--focus); }
.agent-dialog-color input:focus-visible + .agent-dialog-swatch {
  box-shadow: 0 0 0 2px var(--widget-bg), 0 0 0 4px var(--focus); }
.agent-dialog-advanced { padding-top:8px; border-top:var(--border-w) solid var(--border); }
.agent-dialog-advanced-summary { display:flex; align-items:center; gap:6px; min-height:28px;
  color:var(--fg-muted); list-style:none; font-size:var(--text-meta); cursor:pointer; }
.agent-dialog-advanced-summary::-webkit-details-marker { display:none; }
.agent-dialog-advanced-summary::marker { content:""; }
.agent-dialog-advanced-summary:hover { color:var(--fg-strong); }
.agent-dialog-advanced-summary:focus-visible { outline:var(--focus-ring-w) solid var(--focus);
  outline-offset:1px; border-radius:var(--radius-control); }
.agent-dialog-advanced-title { color:var(--fg-strong); font-weight:var(--wght-strong); }
.agent-dialog-advanced-copy { min-width:0; margin-left:auto; overflow:hidden; color:var(--fg-dim);
  text-overflow:ellipsis; white-space:nowrap; }
.agent-dialog-advanced-chevron { color:var(--fg-dim); transition:transform var(--motion-fast) var(--ease-standard); }
.agent-dialog-advanced[open] .agent-dialog-advanced-chevron { transform:rotate(90deg); }
.agent-dialog-advanced-body { display:flex; flex-direction:column; gap:10px; padding-top:8px; }
@media (prefers-reduced-motion: reduce) { .agent-dialog-advanced-chevron { transition:none; } }
.agent-dialog-actions { display: flex; justify-content: flex-end; gap: 6px; padding: 10px 12px;
  border-top: var(--border-w) solid var(--border); }
.agent-dialog-button { min-width: 72px; height: var(--control-h); padding: 0 10px;
  color: var(--fg); background: transparent;
  border: var(--border-w) solid var(--border-strong); border-radius: var(--radius-control);
  font-family: var(--font-ui); font-size: var(--text-ui); cursor: pointer; }
.agent-dialog-button:hover:not(:disabled) { background: var(--row-hover); }
.agent-dialog-button:disabled { color: var(--fg-faint); cursor: default; }
.agent-dialog-submit { color: var(--accent-fg); background: var(--accent);
  border-color: var(--accent); font-weight: var(--wght-strong); }
.agent-dialog-submit:hover:not(:disabled) { background: var(--accent-hover);
  border-color: var(--accent-hover); }
.agent-dialog-submit:disabled { color: var(--fg-faint); background: var(--surface-raised);
  border-color: var(--border); }
`;

export function AgentAvatar({ profile, size = 28 }: {
  readonly profile: AgentProfile;
  readonly size?: number;
}) {
  const character = AGENT_CHARACTERS[profile.color];
  return (
    <span className="agent-avatar" aria-hidden="true" data-character={profile.color}
      style={{ width: size, height: size }}>
      <svg viewBox="0 0 64 64" focusable="false">
        <path d={AGENT_SHAPE_PATHS[profile.shape ?? LEGACY_AGENT_SHAPES[profile.color]]} fill={character.color} />
        <g fill="#fff" transform="rotate(-12 32 32)">
          <rect x="23" y="25" width="5" height="12" rx="2.5" />
          <rect x="37" y="23" width="5" height="12" rx="2.5" />
        </g>
      </svg>
    </span>
  );
}

const NO_AGENT_TASKS: Readonly<Record<string, SessionSummary>> = {};
const NO_STATUSES: Readonly<Record<string, AgentStatus>> = {};
const NO_THREADS: Readonly<Record<string, AgentThreadState>> = {};
const NO_PREVIEWS: Readonly<Record<string, string>> = {};
const AGENT_STATUS_LABELS: Readonly<Record<Exclude<AgentStatus, "idle">, string>> = {
  working: "Working", needsAttention: "Needs attention", unread: "Unread",
};

/** One mark per state that is not idle, each one the app already uses for it. */
function AgentStatusMark({ status }: { readonly status: AgentStatus | undefined }) {
  if (status === undefined || status === "idle") return null;
  const label = AGENT_STATUS_LABELS[status];
  if (status === "working") {
    return <WorkingIndicator className="agent-menu-status dot dot-working dot-pulse"
      data-status={status} role="img" aria-label={label} title={label} />;
  }
  return <span className={status === "needsAttention"
    ? "agent-menu-status codicon codicon-warning session-wait" : "agent-menu-status attention-tray-dot"}
    data-status={status} role="img" aria-label={label} title={label} />;
}

export function AgentMenu({ agents, selectedId, latestTasks = NO_AGENT_TASKS, statuses = NO_STATUSES,
    threads = NO_THREADS, threadPreviews = NO_PREVIEWS, now,
    onSelect, onCreate, createDisabled = false }: AgentMenuProps) {
  const [search, setSearch] = useState("");
  // The search box opens from its glyph, like the filter under Tasks, and
  // stays open while it holds a query.
  const [searchOpen, setSearchOpen] = useState(false);
  const showSearch = searchOpen || search !== "";
  const query = search.trim().toLocaleLowerCase();
  const visible = agents.filter(profile => profile.name.toLocaleLowerCase().includes(query));
  return (
    <div className="agent-menu">
      <style>{AGENT_MENU_CSS}</style>
      <div className="agent-menu-heading">
        <span className="sidebar-section-label">Agents</span>
        <span className="sidebar-section-actions">
          <button type="button" className="sidebar-nav-button" aria-label="New agent"
            title="New agent" disabled={createDisabled} onClick={onCreate}>
            <span className="codicon codicon-add sidebar-nav-icon" aria-hidden="true" />
            <span className="sidebar-nav-label">New agent</span>
          </button>
          {agents.length > 0 && <button type="button"
            className={showSearch ? "sidebar-nav-button is-active" : "sidebar-nav-button"}
            aria-label="Filter agents" title="Filter agents" aria-expanded={showSearch}
            onClick={() => { setSearchOpen(!showSearch); if (showSearch) setSearch(""); }}>
            <span className="codicon codicon-search sidebar-nav-icon" aria-hidden="true" />
            <span className="sidebar-nav-label">Filter agents</span>
          </button>}
        </span>
      </div>
      {showSearch && <label className="agent-menu-search">
        <span className="codicon codicon-search" aria-hidden="true" />
        <input type="search" aria-label="Search agents" placeholder="Search agents" autoFocus
          value={search} onChange={event => { setSearch(event.currentTarget.value); }}
          onKeyDown={event => {
            if (event.key !== "Escape") return;
            event.preventDefault(); event.stopPropagation();
            if (search !== "") setSearch(""); else setSearchOpen(false);
          }} />
      </label>}
      {agents.length === 0 ? (
        <p className="agent-menu-empty">No agents yet. Create an agent with its own model and instructions.</p>
      ) : visible.length === 0 ? <p className="agent-menu-empty" role="status">No matching agents.</p> : (
        <div className="agent-menu-roster" role="listbox" aria-label="Agents" onKeyDown={event => {
          const rows = [...event.currentTarget.querySelectorAll<HTMLButtonElement>('[role="option"]')];
          const index = rows.indexOf(document.activeElement as HTMLButtonElement);
          let next: number;
          if (event.key === "ArrowDown") next = Math.min(index + 1, rows.length - 1);
          else if (event.key === "ArrowUp") next = Math.max(index - 1, 0);
          else if (event.key === "Home") next = 0;
          else if (event.key === "End") next = rows.length - 1;
          else return;
          event.preventDefault(); event.stopPropagation(); rows[next]?.focus();
        }}>
          {visible.map(profile => {
            const task = latestTasks[profile.id];
            const thread = threads[profile.id]?.current;
            const preview = (thread === undefined ? undefined : threadPreviews[thread.sessionKey])
              || task?.title || profile.instructions.split(/\r?\n/u)[0] || "Ready for a new task";
            return <button key={profile.id} type="button" role="option"
              className="agent-menu-row" aria-selected={profile.id === selectedId}
              title={profile.name} onClick={() => { onSelect(profile.id); }}>
              <AgentAvatar profile={profile} size={42} />
              <span className="agent-menu-row-copy">
                <span className="agent-menu-row-top">
                  <span className="agent-menu-row-name">{profile.name}</span>
                  <AgentStatusMark status={statuses[profile.id]} />
                  {task && <time className="agent-menu-row-age" dateTime={task.lastActivity}
                    title={task.lastActivity}>{sessionAgeLabel(task.lastActivity, now)}</time>}
                </span>
                <span className="agent-menu-row-preview" title={preview}>{preview}</span>
              </span>
            </button>;
          })}
        </div>
      )}
    </div>
  );
}

export function AgentHeader({ profile, engineLabel, workingCount,
    onEdit, onDelete }: AgentHeaderProps) {
  // No <style> of its own: this header only ever draws on the Agent surface,
  // where `AgentMenu` in the sidebar has already injected AGENT_MENU_CSS. The
  // dialog keeps its copy because it renders through a portal.
  return (
    <header className="agent-surface-head">
      <AgentAvatar profile={profile} size={32} />
      <span className="agent-header-copy">
        <span className="agent-header-name">{profile.name}</span>
        <span className="agent-header-provider" title={profile.model || "Choose model at launch"}>
          {`New tasks: ${engineLabel}${profile.model ? ` · ${profile.model}` : " · Choose model at launch"}`}
        </span>
      </span>
      <span className="agent-header-pill badge badge-quiet">{`${workingCount} working`}</span>
      <button type="button" className="agent-header-action" aria-label="Edit agent"
        title="Edit agent" onClick={onEdit}>
        <span className="codicon codicon-sm codicon-edit" aria-hidden="true" />
      </button>
      <button type="button" className="agent-header-action" aria-label="Delete agent"
        title="Delete agent" onClick={onDelete}>
        <span className="codicon codicon-sm codicon-trash" aria-hidden="true" />
      </button>
    </header>
  );
}

export function AgentProfileDialog({ open, initial, engines, effortLevelsFor, modelsFor = NO_MODELS,
    initialPermission = "gated", onSubmit, onCancel }: AgentProfileDialogProps) {
  const baseId = useId();
  const dialogRef = useRef<HTMLElement>(null);
  const nameRef = useRef<HTMLInputElement>(null);
  // Seeded at mount as well as on open: the caller remounts this dialog per
  // editing session, and a first paint from state alone would show a blank form
  // over the agent being edited.
  const [name, setName] = useState(initial?.name ?? "");
  const [engine, setEngine] = useState(initial?.engine ?? "");
  const [model, setModel] = useState(initial?.model ?? "");
  const [customModel, setCustomModel] = useState(false);
  const [effort, setEffort] = useState(initial?.effort ?? "");
  const [instructions, setInstructions] = useState(initial?.instructions ?? "");
  const [shape, setShape] = useState<AgentShape>(initial?.shape ?? LEGACY_AGENT_SHAPES[initial?.color ?? "teal"]);
  const [color, setColor] = useState<AgentColor>(initial?.color ?? AGENT_COLORS[0]);
  const [permission, setPermission] = useState<AgentPermissionMode>(initialPermission);
  const handleKeyDown = useModalFocus({
    open,
    containerRef: dialogRef,
    initialFocus: () => nameRef.current,
    onEscape: onCancel,
  });

  // Once per opening, not on every change of `initial`'s identity: a store
  // write elsewhere (a task resolving against another agent) re-normalizes the
  // roster, and re-seeding on that would wipe a half-typed form.
  const seeded = useRef(false);
  useEffect(() => {
    if (!open) { seeded.current = false; return; }
    if (seeded.current || initial === null) return;
    seeded.current = true;
    setName(initial.name);
    setEngine(initial.engine);
    setModel(initial.model);
    setCustomModel(false);
    setEffort(initial.effort);
    setInstructions(initial.instructions);
    setColor(initial.color);
    setShape(initial.shape ?? LEGACY_AGENT_SHAPES[initial.color]);
    setPermission(initialPermission);
    window.requestAnimationFrame(() => { nameRef.current?.select(); });
  }, [initial, initialPermission, open]);

  if (!open || initial === null) return null;
  // A new agent arrives with the id and colour already minted and no name yet;
  // a saved one always has a name, because the form below requires it.
  const creating = initial.name.trim() === "";
  const trimmed = name.trim();
  const models = modelsFor(engine).filter(row => row.value !== "" && row.value !== "default");
  const listedModel = findCatalogModel(model, models, engine);
  const custom = customModel || (model !== "" && listedModel === undefined);
  const modelValue = custom ? CUSTOM_MODEL : model === "" ? "" : listedModel?.value ?? "";
  const efforts = agentModelEfforts(engine, model, models, effortLevelsFor(engine));
  const selectedEffort = efforts.includes(effort) ? effort : "";
  const advancedSummary = [
    engines.find(option => option.id === engine)?.label ?? engine,
    custom ? model.trim() || "Custom model ID needed"
      : listedModel?.displayName ?? "Model at launch",
    ...(selectedEffort === "" ? [] : [selectedEffort]),
    ...(permission === "gated" ? [] : [AGENT_PERMISSION_LABELS[permission]]),
  ].join(" · ");
  // A provider that cannot be a teammate is never picked for a new agent. An
  // agent saved on one before this rule keeps it, with the reason shown.
  const engineRefusal = engines.find(option => option.id === engine)?.unavailableReason;
  const refusedNew = creating && engineRefusal !== undefined;
  const submit = (event: FormEvent): void => {
    event.preventDefault();
    if (trimmed === "" || (custom && model.trim() === "") || refusedNew) return;
    onSubmit({
      id: initial.id,
      name: trimmed,
      engine,
      model: model.trim(),
      effort: selectedEffort,
      instructions: instructions.trim(),
      color, shape,
    }, permission);
  };
  const surface = <>
    <style>{AGENT_MENU_CSS}</style>
    <div className="agent-dialog-backdrop" onPointerDown={event => {
      if (event.target === event.currentTarget) onCancel();
    }}>
      <section ref={dialogRef} className="agent-dialog is-floating" role="dialog" aria-modal="true"
        aria-labelledby={`${baseId}-title`} tabIndex={-1} onKeyDown={handleKeyDown}>
        <form onSubmit={submit}>
          <div className="agent-dialog-copy">
            <h2 id={`${baseId}-title`} className="agent-dialog-title">
              {creating ? "New agent" : "Edit agent"}
            </h2>
            <div className="agent-dialog-field">
              <label className="agent-dialog-label" htmlFor={`${baseId}-name`}>Name</label>
              <input ref={nameRef} id={`${baseId}-name`} className="agent-dialog-input"
                value={name} maxLength={60} autoComplete="off" required
                onChange={event => { setName(event.currentTarget.value); }} />
            </div>
            <div className="agent-dialog-field">
              <label className="agent-dialog-label"
                htmlFor={`${baseId}-instructions`}>What it does</label>
              <textarea id={`${baseId}-instructions`} className="agent-dialog-textarea"
                value={instructions} rows={4} maxLength={8000}
                placeholder="Researches competitors and writes a weekly brief"
                onChange={event => { setInstructions(event.currentTarget.value); }} />
            </div>
            <div className="agent-dialog-field">
              <span className="agent-dialog-label" title="New agents receive an unused combination if this character is taken.">Icon</span>
              <span className="agent-dialog-preview"><AgentAvatar profile={{ ...initial, color, shape }} size={64} /></span>
              <div className="agent-dialog-colors agent-dialog-shapes" role="group" aria-label="Shape">
                {AGENT_SHAPES.map(option => <label key={option} className="agent-dialog-color" title={option}>
                  <input type="radio" name={`${baseId}-shape`} checked={option === shape}
                    aria-label={`${option} shape`} onChange={() => { setShape(option); }} />
                  <span className="agent-dialog-swatch"><AgentAvatar profile={{ ...initial, color, shape: option }} size={30} /></span>
                </label>)}
              </div>
              <div className="agent-dialog-colors" role="group" aria-label="Color">
                {AGENT_COLORS.map(option => (
                  <label key={option} className="agent-dialog-color" title={AGENT_CHARACTERS[option].name}>
                    <input type="radio" name={`${baseId}-color`} value={option}
                      checked={option === color} aria-label={AGENT_CHARACTERS[option].name}
                      onChange={() => { setColor(option); }} />
                    <span className="agent-dialog-swatch" aria-hidden="true">
                      <AgentAvatar profile={{ ...initial, color: option, shape }} size={32} />
                    </span>
                  </label>
                ))}
              </div>
            </div>
            {/* Provider and model have working defaults, so they wait behind
                Advanced; the summary still says what a new task will use. */}
            <details className="agent-dialog-advanced">
              <summary className="agent-dialog-advanced-summary">
                <span className="codicon codicon-chevron-right agent-dialog-advanced-chevron"
                  aria-hidden="true" />
                <span className="agent-dialog-advanced-title">Advanced</span>
                <span className="agent-dialog-advanced-copy">{advancedSummary}</span>
              </summary>
              <div className="agent-dialog-advanced-body">
                <FormSelect fieldClassName="agent-dialog-field" popoverClassName="agent-dialog-picker" label="Provider" value={engine}
                  onChange={value => {
                    setEngine(value); setModel(""); setEffort(""); setCustomModel(false);
                  }}
                  options={engines.map(option => ({
                    value: option.id,
                    label: option.label,
                    ...(option.available ? {} : { disabled: true }),
                    ...(option.unavailableReason === undefined
                      ? {} : { description: option.unavailableReason }),
                  }))} />
                {engineRefusal !== undefined && <p className="agent-dialog-hint" role="note">
                  {engineRefusal}</p>}
                <FormSelect fieldClassName="agent-dialog-field" popoverClassName="agent-dialog-picker" label="Model" value={modelValue}
                  onChange={value => {
                    setCustomModel(value === CUSTOM_MODEL);
                    setModel(value === CUSTOM_MODEL ? "" : value); setEffort("");
                  }} options={[
                    { value: "", label: "Choose at launch" },
                    ...models.map(row => ({ value: row.value, label: row.displayName,
                      ...(row.description ? { description: row.description } : {}) })),
                    { value: CUSTOM_MODEL, label: "Custom model ID…" },
                  ]} />
                {custom && <div className="agent-dialog-field">
                  <label className="agent-dialog-label" htmlFor={`${baseId}-model`}>Custom model ID</label>
                  <input id={`${baseId}-model`} className="agent-dialog-input" value={model}
                    maxLength={512} autoComplete="off" required placeholder="Model ID from your provider"
                    onChange={event => { setModel(event.currentTarget.value); }} />
                  <p className="agent-dialog-hint">Your provider must support this model. It will be checked when the task starts.</p>
                </div>}
                <p className="agent-dialog-hint">These defaults apply to new tasks. Running tasks keep their current model.</p>
                {efforts.length > 0 && <FormSelect fieldClassName="agent-dialog-field" popoverClassName="agent-dialog-picker" label="Effort" value={selectedEffort}
                  onChange={setEffort}
                  options={[{ value: "", label: "Default" },
                    ...efforts.map(level => ({ value: level, label: level }))]} />}
                <FormSelect fieldClassName="agent-dialog-field" popoverClassName="agent-dialog-picker" label="Permissions"
                  value={permission}
                  onChange={value => { setPermission(value as AgentPermissionMode); }}
                  options={AGENT_PERMISSION_MODES.map(mode => ({ value: mode, label: AGENT_PERMISSION_LABELS[mode] }))} />
                <p className="agent-dialog-hint">{AGENT_PERMISSION_HELP[permission]}</p>
                {!creating && <p className="agent-dialog-hint">{AGENT_PERMISSION_TIMING}</p>}
                {permission === "bypass" && <p className="agent-dialog-hint" role="note">
                  <span className="codicon codicon-warning" aria-hidden="true" /> {AGENT_BYPASS_WARNING}</p>}
                {permission === "auto" && engine === "claude" && claudeAutoUnsupported(model)
                  && <p className="agent-dialog-hint" role="note">{AGENT_AUTO_MODEL_NOTE}</p>}
              </div>
            </details>
          </div>
          <div className="agent-dialog-actions">
            <button type="button" className="agent-dialog-button"
              onClick={onCancel}>Cancel</button>
            <button type="submit" className="agent-dialog-button agent-dialog-submit"
              disabled={trimmed === "" || (custom && model.trim() === "") || refusedNew}>{creating ? "Create" : "Save"}</button>
          </div>
        </form>
      </section>
    </div>
  </>;
  return typeof document === "undefined" ? surface : createPortal(surface, document.body);
}
