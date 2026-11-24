import type { CommandPaletteCommand } from "./components/CommandPalette";
import {
  isResolvedTheme, THEME_DESCRIPTIONS, THEME_LABELS, THEME_PREFERENCES,
  type CustomThemeSummary, type ThemeId,
} from "../shared/appearance";

/** What the shell's palette commands depend on, read from App's state. */
export interface ShellPaletteState {
  /** The surface's New label ("New thread", "New workspace", "New agent"…). */
  readonly newLabel: string;
  readonly newDescription: string;
  readonly attentionItems: number;
  /** Code's terminals are on screen: Open in external terminal has a project. */
  readonly terminalsShown: boolean;
  /** The conversation panes in the surface's layout. */
  readonly openPanes: number;
  readonly focusedPane: boolean;
  readonly maximized: boolean;
  readonly sidebarOpen: boolean;
  readonly launching: boolean;
  readonly refreshing: boolean;
  /** The voice assistant is on (listening, thinking or speaking). */
  readonly voiceOn?: boolean;
  /** Voice is starting or stopping; its row waits. */
  readonly voiceBusy?: boolean;
  /** Code's agent overview column is open. */
  readonly overviewOpen?: boolean;
  /** One row per colour theme the Settings picker offers, in its order. */
  readonly themes?: readonly PaletteTheme[];
}

/** A colour theme the palette can switch to. */
export interface PaletteTheme {
  readonly id: string;
  readonly label: string;
  readonly description: string;
  readonly current: boolean;
}

/**
 * The themes Settings offers, as palette rows: Follow macOS, the built-in
 * palettes in the picker's order, then the owner's theme files that paint.
 */
export function paletteThemes(preference: ThemeId,
    customThemes: readonly CustomThemeSummary[]): readonly PaletteTheme[] {
  return [
    { id: "system", label: "Follow macOS", description: THEME_DESCRIPTIONS.system,
      current: preference === "system" },
    ...THEME_PREFERENCES.filter(isResolvedTheme).map(theme => ({
      id: theme, label: THEME_LABELS[theme], description: THEME_DESCRIPTIONS[theme],
      current: preference === theme,
    })),
    ...customThemes.filter(theme => theme.problem === undefined).map(theme => ({
      id: theme.id, label: theme.name, description: "Your theme file.",
      current: preference === theme.id,
    })),
  ];
}

/** Palette ids for theme rows: `theme:<theme id>`. */
export const THEME_COMMAND_PREFIX = "theme:";

/**
 * Commands that act where they are and must not dismiss the composer behind
 * the palette (App's runPaletteCommand reveals the workspace for the rest).
 */
export function paletteCommandKeepsComposer(id: string): boolean {
  return id.startsWith(THEME_COMMAND_PREFIX) || [
    "workspace.new", "workspace.sidebar", "workspace.refresh", "workspace.settings",
    "settings.remote", "voice.toggle", "away.open", "workspace.newProject",
  ].includes(id);
}

/**
 * The command palette's shell commands, in their listed order. App runs them
 * (`runPaletteCommand`) and appends one row per session while the palette is
 * open. Every disabled command says why.
 */
export function shellPaletteCommands(state: ShellPaletteState): CommandPaletteCommand[] {
  return [
    // One row for the one Task details dialog; its context inventory and
    // presets load inside it (four rows used to open the same modal).
    { id: "harness.results", label: "Task details…", group: "Task",
      description: "Results, checks, context inventory and workspace presets for this checkout",
      keywords: ["inspect", "review", "result", "checks", "diff", "context", "inventory",
        "instructions", "presets", "worktree"] },
    { id: "harness.next", label: "Next actionable item", group: "Task", description: "Visit a waiting session without rearranging panes", disabled: state.attentionItems === 0, disabledReason: "No attention items." },

    { id: "workspace.new", label: state.newLabel, group: "Workspace",
      description: state.newDescription, shortcut: "⌘N", keywords: ["new", "session", "agent", "chat", "workspace", "task"] },
    { id: "surface.chat", label: "Go to Threads", group: "Workspace",
      keywords: ["threads", "chat", "conversation"] },
    { id: "surface.code", label: "Go to Code", group: "Workspace",
      keywords: ["code", "files", "terminal"] },
    { id: "surface.agent", label: "Go to Agent", group: "Workspace",
      keywords: ["agent", "task", "cowork"] },
    { id: "workspace.newProject", label: "New project…", group: "Workspace",
      description: "Create a project folder and open it in Code",
      keywords: ["project", "folder", "create", "new"] },
    { id: "workspace.canvas", label: "Coordination canvas", group: "Workspace",
      description: "Arrange coding sessions on a graph in Code",
      keywords: ["canvas", "orchestration", "worker", "coordination"] },
    { id: "workspace.overview", label: "Live panes", group: "Workspace",
      description: state.overviewOpen ? "Hide the live pane cards beside Code's panes"
        : "Show the live pane cards beside Code's panes",
      keywords: ["overview", "live", "column", "panes", "working"] },
    { id: "tool.files", label: "Files", group: "Workspace",
      description: "Preview this project's files and outputs beside its terminals",
      keywords: ["files", "preview", "outputs", "pdf", "image", "markdown"] },
    { id: "tool.browser", label: "Browser", group: "Workspace",
      description: "Open a Browser tab in the focused pane",
      keywords: ["browser", "web", "localhost", "page", "url"] },
    { id: "away.open", label: "Away loop…", group: "Workspace",
      description: "Unattended work with a review from another model",
      keywords: ["away", "loop", "unattended", "coffee", "review"] },
    { id: "terminals.review", label: "Start adversarial review…", group: "Workspace",
      description: "Hand one objective to a builder and a cross-provider reviewer",
      keywords: ["adversarial", "review", "builder", "reviewer", "critique"] },
    { id: "workspace.library", label: "Search transcripts", group: "Workspace",
      description: "Find stored transcripts",
      keywords: ["history", "archive", "worktree", "sessions"] },
    { id: "workspace.archives", label: "Archived sessions", group: "Workspace",
      description: "Browse or restore hidden sessions" },
    { id: "workspace.worktrees", label: "Worktrees", group: "Workspace",
      description: "Inspect or clean up isolated checkouts" },
    // The project of the terminal in front, read when the command runs.
    { id: "workspace.externalTerminal", label: "Open in external terminal", group: "Workspace",
      description: "Prefer Ghostty and fall back to Terminal",
      disabled: !state.terminalsShown,
      disabledReason: "Show Code's terminals first." },
    { id: "workspace.next", label: "Next pane", group: "Layout",
      shortcut: "⌃Tab", disabled: state.openPanes < 2,
      disabledReason: "Open at least two panes." },
    { id: "workspace.previous", label: "Previous pane", group: "Layout",
      shortcut: ["⌃", "⇧", "Tab"], disabled: state.openPanes < 2,
      disabledReason: "Open at least two panes." },
    { id: "workspace.maximize", label: state.maximized
      ? "Restore layout" : "Maximize pane", group: "Layout",
      shortcut: ["⌘", "⇧", "M"], disabled: !state.focusedPane,
      disabledReason: "Open or focus a pane first." },
    { id: "workspace.close", label: "Close pane", group: "Layout",
      description: "Hide it without stopping the agent",
      disabled: !state.focusedPane,
      disabledReason: "Focus an agent pane first." },
    { id: "workspace.sidebar", label: state.sidebarOpen ? "Hide sidebar" : "Show sidebar",
      group: "View", keywords: ["sessions", "pane", "navigation"] },
    { id: "workspace.settings", label: "Settings", group: "View",
      description: "Appearance, defaults, providers, and shortcuts",
      shortcut: "⌘,", disabled: state.launching,
      disabledReason: "Wait for the new chat to start." },
    { id: "workspace.usage", label: "Provider usage", group: "View",
      description: "Compare current plan usage" },
    { id: "voice.toggle", label: state.voiceOn ? "Stop voice assistant" : "Start voice assistant",
      group: "View", description: "Talk to Interface hands-free",
      keywords: ["voice", "microphone", "talk", "speech", "assistant"],
      disabled: state.voiceBusy === true,
      disabledReason: "The voice assistant is starting or stopping." },
    { id: "settings.remote", label: "iPhone remote access", group: "View",
      description: "Pair an iPhone and manage trusted devices",
      keywords: ["iphone", "phone", "remote", "pair", "mobile", "devices"] },
    { id: "workspace.refresh", label: "Refresh providers & sessions", group: "View",
      disabled: state.refreshing, disabledReason: "A refresh is already running." },
    ...(state.themes ?? []).map(theme => ({
      id: `${THEME_COMMAND_PREFIX}${theme.id}`, label: `Theme: ${theme.label}`, group: "Theme",
      description: theme.current ? `Current theme. ${theme.description}` : theme.description,
      keywords: ["theme", "color", "colour", "appearance", "dark", "light"],
    })),
  ];
}
