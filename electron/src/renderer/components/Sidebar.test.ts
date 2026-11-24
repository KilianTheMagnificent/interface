import { readFileSync } from "node:fs";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import {
  Sidebar,
  SidebarSessionRow,
  sameSidebarSessionRow,
  type SidebarSessionRowProps,
  SESSION_PEEK_HOVER_MS,
  SIDEBAR_PROJECT_ROW_LIMIT,
  groupAccessibleName,
  isAttentionRow,
  parseArchivedProjects,
  projectArchiveBlocked,
  sessionRowActivateAction,
  visibleSidebarProjectRows,
  withArchivedProject,
  withoutArchivedProject,
  withoutProtectedArchivedProjects,
  withAllProjectsCollapsed,
} from "./Sidebar";
import type { SessionPeekFacts } from "./SessionPeek";
import {
  OTHER_PROJECT, type SidebarGroup, type SidebarRow, type SidebarTerminalRow,
} from "../viewmodels";

const source = readFileSync(new URL("./Sidebar.tsx", import.meta.url), "utf8");
// The menu keyboard model the Sidebar shares with the Threads inbox.
const controls = readFileSync(new URL("./sidebar-controls.ts", import.meta.url), "utf8");
const themeCss = readFileSync(new URL("../theme.css", import.meta.url), "utf8");
const noop = (): void => undefined;

function group(projectPath: string, projectName: string,
    counts = { total: 1, waiting: 0, working: 0 }, rows: SidebarRow[] = []): SidebarGroup {
  return { projectPath, projectName, counts, rows };
}

function row(key: string, badge: SidebarRow["badge"], title = key): SidebarRow {
  return {
    key, title, engine: "claude", status: badge, badge,
    cloud: false, selected: false, lastActivity: "2026-08-11T10:00:00.000Z",
    external: false,
  };
}

function terminalRow(id: string,
    over: Partial<SidebarTerminalRow> = {}): SidebarTerminalRow {
  return {
    id,
    title: `${id} terminal`,
    state: "running",
    root: "/work/interface",
    cwd: "/work/interface",
    lastActivity: "2026-08-11T10:00:00.000Z",
    selected: false,
    ...over,
  };
}

function sidebarMarkup(groups: SidebarGroup[], projectCount = groups.length,
    over: {
      loading?: boolean; hideable?: number;
      hidden?: { external: number; trivial: number };
      diffStats?: Record<string, { added: number; removed: number }>;
      peeks?: Record<string, SessionPeekFacts>;
      terminalWorkspaces?: {
        records: readonly {
          readonly id: string;
          readonly name: string;
          readonly firstTerminalTitle?: string;
          readonly openTerminalCount: number;
          readonly openTerminalIds?: readonly string[];
          readonly terminalIds: readonly string[];
          readonly cwd?: string;
        }[];
        activeId: string | null;
        onSelect: (workspaceId: string) => void;
        onCreate: () => void;
        onRename: (workspaceId: string) => void;
        onDelete: (workspaceId: string) => void;
        busy: boolean;
        canCreate: boolean;
      };
      pinnedProjects?: readonly string[];
      onPinProject?: (path: string, pinned: boolean) => void;
      terminalsActive?: boolean;
      codeWorkspaces?: boolean;
      protectedProjects?: readonly string[];
      terminalProjects?: Readonly<Record<string, string>>;
      newLabel?: string;
      sectionLabel?: string;
      zeroStateText?: string;
      query?: string;
      onQuery?: (value: string) => void;
      searching?: boolean;
      onSearchTranscripts?: (query: string) => void;
    } = {},
): string {
  return renderToStaticMarkup(createElement(Sidebar, {
    groups,
    projectCount,
    rateLimits: {},
    hidden: { external: 0, trivial: 0 },
    hideable: 0,
    onSelect: noop,
    onNew: noop,
    onNewInProject: noop,
    onArchive: noop,
    onToggleFilters: noop,
    ...over,
  }));
}

function sidebarMarkupWithArchivedProject(
  group: SidebarGroup,
  otherGroups: SidebarGroup[] = [],
  protectedProjects: readonly string[] = [],
): string {
  vi.stubGlobal("window", {
    localStorage: {
      getItem: (key: string) => key === "interface.sidebar.archivedProjects"
        ? JSON.stringify([{ projectPath: group.projectPath, projectName: group.projectName }])
        : null,
      setItem: noop,
    },
  });
  try {
    return sidebarMarkup([group, ...otherGroups], 1 + otherGroups.length, {
      terminalWorkspaces: {
        records: [
          {
            id: "workspace-one",
            name: "Workspace one",
            openTerminalCount: 1,
            terminalIds: ["shell"],
            cwd: group.projectPath,
          },
          {
            id: "workspace-detached",
            name: "Detached workspace",
            openTerminalCount: 0,
            terminalIds: [],
            cwd: `${group.projectPath}/nested`,
          },
        ],
        activeId: "workspace-one",
        onSelect: noop,
        onCreate: noop,
        onRename: noop,
        onDelete: noop,
        busy: false,
        canCreate: true,
      },
      terminalsActive: true,
      protectedProjects,
      terminalProjects: { shell: group.projectPath },
    });
  } finally {
    vi.unstubAllGlobals();
  }
}

describe("calm agent sidebar", () => {
  it("lists project-owned terminal workspaces without a separate workspace area", () => {
    const selectedChat = { ...row("claude#chat", "idle", "Project chat"), selected: true };
    const project = {
      ...group("/work/interface", "Interface", { total: 1, waiting: 0, working: 0 },
        [selectedChat]),
      terminals: [
        terminalRow("terminal-claude", {
          title: "Claude Code",
          profileId: "claude",
          selected: true,
        }),
        terminalRow("terminal-shell", { title: "Build shell", state: "exited" }),
      ],
    };
    const html = sidebarMarkup([project], 1, {
      terminalWorkspaces: {
        records: [
          { id: "w1", name: "Workspace 1", firstTerminalTitle: "Claude Code",
            openTerminalCount: 1,
            terminalIds: ["terminal-claude"] },
          { id: "w2", name: "Workspace 2", firstTerminalTitle: "Build shell",
            openTerminalCount: 1,
            terminalIds: ["terminal-shell"] },
        ],
        activeId: "w2",
        onSelect: noop,
        onCreate: noop,
        onRename: noop,
        onDelete: noop,
        busy: false,
        canCreate: true,
      },
      terminalsActive: true,
      terminalProjects: {
        "terminal-claude": "/work/interface",
        "terminal-shell": "/work/interface",
      },
    });

    // A terminal is reached inside its workspace, not hunted for in this pane.
    // Each row takes that workspace's own focused terminal title, so two
    // workspaces in the same project remain distinguishable.
    expect(html).not.toContain("data-terminal-sidebar-id");
    expect(html).not.toContain("codicon-terminal");
    expect(html).not.toContain('aria-label="Terminal workspace"');
    expect(html).not.toContain('aria-label="Terminal workspaces"');
    expect(html).toContain('aria-label="New terminal workspace"');
    expect(html).toContain(">Workspaces<");
    expect(html).toContain('data-terminal-workspace-id="w1"');
    expect(html.match(
      /class="workspace-tree-row(?: is-selected)?" data-terminal-workspace-id=/gu,
    )).toHaveLength(2);
    expect(html).not.toContain("is-nested-workspace");
    expect(html).toContain('class="workspace-tree-title">Claude Code<');
    expect(html).toContain('class="workspace-tree-title">Build shell<');
    expect(html).not.toContain("Workspace 1");
    expect(html).not.toContain("Workspace 2");
    expect(html.indexOf("Claude Code")).toBeLessThan(html.indexOf("Project chat"));
    // The active workspace and the selected chat are two different destinations,
    // so only the one the center is actually showing claims aria-current.
    expect(html).toContain('data-terminal-workspace-id="w2" aria-current="page"');
  });

  it("does not file a saved workspace under a terminal-only ancestor folder", () => {
    // A shell launched in the folder that merely CONTAINS projects makes a
    // terminal-only "Work" group. Chat projects are git roots and may claim
    // saved cwds beneath them; that folder group must not, or every saved
    // workspace under it reads as "Work" until its PTYs reopen.
    const chat = group("/work/interface", "Interface", { total: 1, waiting: 0, working: 0 },
      [row("claude#chat", "idle", "Project chat")]);
    const folder = {
      ...group("/work", "Work", { total: 0, waiting: 0, working: 0 }, []),
      terminals: [terminalRow("folder-shell", { root: "/work", cwd: "/work" })],
    };
    const html = sidebarMarkup([chat, folder], 2, {
      terminalWorkspaces: {
        records: [
          { id: "w-saved", name: "Saved", openTerminalCount: 0,
            terminalIds: ["gone"], cwd: "/work/other" },
          { id: "w-nested", name: "Nested", openTerminalCount: 0,
            terminalIds: ["gone-too"], cwd: "/work/interface/electron" },
        ],
        activeId: null,
        onSelect: noop,
        onCreate: noop,
        onRename: noop,
        onDelete: noop,
        busy: false,
        canCreate: true,
      },
      terminalsActive: true,
      terminalProjects: { "folder-shell": "/work" },
    });

    // "/work/other" gets its own group, and a cwd inside the chat project still
    // joins that project.
    expect(html).toContain('class="group-name">other<');
    expect(html.indexOf('data-terminal-workspace-id="w-saved"'))
      .toBeLessThan(html.indexOf('class="group-name">Work<'));
    expect(html.indexOf('data-terminal-workspace-id="w-nested"'))
      .toBeLessThan(html.indexOf('class="group-name">other<'));
  });

  it("counts open tabs without dropping saved members from workspace identity", () => {
    const project = {
      ...group("/work/interface", "Interface", { total: 0, waiting: 0, working: 0 }, []),
      terminals: [
        terminalRow("terminal-claude", {
          title: "Claude Code",
          profileId: "claude",
          lastActivity: "2026-08-29T10:00:00.000Z",
        }),
        terminalRow("terminal-grok", {
          title: "Grok",
          profileId: "grok",
          selected: true,
          lastActivity: "2026-08-29T09:00:00.000Z",
        }),
      ],
    };
    const html = sidebarMarkup([project], 1, {
      terminalWorkspaces: {
        records: [{
          id: "w1",
          name: "Workspace 3",
          firstTerminalTitle: "Claude Code",
          openTerminalCount: 1,
          openTerminalIds: ["terminal-grok"],
          terminalIds: ["terminal-claude", "terminal-grok"],
        }],
        activeId: "w1",
        onSelect: noop,
        onCreate: noop,
        onRename: noop,
        onDelete: noop,
        busy: false,
        canCreate: true,
      },
      terminalsActive: true,
      terminalProjects: {
        "terminal-claude": "/work/interface",
        "terminal-grok": "/work/interface",
      },
    });
    expect(html).toContain('class="workspace-tree-title">Claude Code<');
    // One pip per OPEN pane (owner, 2026-10-01): the closed tab's terminal
    // keeps its saved membership but takes no pip.
    // The pip names its agent so the row can paint it in that provider's
    // colour (owner, 2026-10-02).
    expect(html).toContain('class="workspace-tree-pips" aria-hidden="true"><i class="is-idle" data-provider="grok"></i></span>');
    expect(html).toContain('class="workspace-tree-count-label">1 open terminal<');
    expect(html).toContain('class="group-count" aria-hidden="true">1<');
    expect(html).not.toContain("Workspace 3");
    expect(html).not.toContain('class="workspace-tree-title">Grok<');
    expect(html).not.toContain("codicon-terminal");
  });

  it("paints a workspace blue when an agent finished a turn nobody has looked at", () => {
    const project = {
      ...group("/work/interface", "Interface", { total: 1, waiting: 0, working: 0 },
        [row("claude#chat", "idle", "Project chat")]),
      terminals: [
        terminalRow("finished", { state: "running", done: true }),
        terminalRow("resting", { state: "running" }),
      ],
    };
    const html = sidebarMarkup([project], 1, {
      terminalWorkspaces: {
        records: [
          { id: "w-finished", name: "Finished workspace", openTerminalCount: 1,
            terminalIds: ["finished"] },
          { id: "w-resting", name: "Resting workspace", openTerminalCount: 1,
            terminalIds: ["resting"] },
        ],
        activeId: null,
        onSelect: noop,
        onCreate: noop,
        onRename: noop,
        onDelete: noop,
        busy: false,
        canCreate: true,
      },
      terminalProjects: { finished: "/work/interface", resting: "/work/interface" },
    });
    const finished = html.slice(
      html.indexOf('data-terminal-workspace-id="w-finished"'),
      html.indexOf('data-terminal-workspace-id="w-resting"'),
    );
    expect(finished).toContain('class="dot dot-done"');
    expect(finished).toContain('aria-label="Done"');
    expect(finished).not.toContain('class="workspace-tree-ticker"');
    const resting = html.slice(html.indexOf('data-terminal-workspace-id="w-resting"'));
    expect(resting).toContain("dot dot-idle");
    expect(themeCss).toContain(".dot-done::before    { background: var(--status-done); }");
  });

  it("paints compact workspace tree rows with a status dot and a terminal count", () => {
    const project = {
      ...group("/work/interface", "Interface", { total: 1, waiting: 0, working: 0 },
        [row("claude#chat", "idle", "Project chat")]),
      terminals: [
        terminalRow("live", { state: "running", activity: "working" }),
        terminalRow("dead", { state: "failed" }),
        terminalRow("ask", { state: "running", activity: "waiting" }),
        terminalRow("quiet", { state: "exited" }),
      ],
    };
    const html = sidebarMarkup([project], 1, {
      terminalWorkspaces: {
        records: [
          { id: "w-live", name: "Live workspace", openTerminalCount: 1,
            terminalIds: ["live"] },
          { id: "w-dead", name: "Dead workspace", openTerminalCount: 1,
            terminalIds: ["dead"] },
          { id: "w-ask", name: "Asking workspace", openTerminalCount: 1,
            terminalIds: ["ask"] },
          { id: "w-quiet", name: "Quiet workspace", openTerminalCount: 1,
            terminalIds: ["quiet"] },
        ],
        activeId: null,
        onSelect: noop,
        onCreate: noop,
        onRename: noop,
        onDelete: noop,
        busy: false,
        canCreate: true,
      },
      terminalProjects: {
        live: "/work/interface",
        dead: "/work/interface",
        ask: "/work/interface",
        quiet: "/work/interface",
      },
    });

    // Live work rises: the working and the asking workspace lead, then the
    // saved order (dead, quiet) stands.
    const order = ["w-live", "w-ask", "w-dead", "w-quiet"]
      .map(id => html.indexOf(`data-terminal-workspace-id="${id}"`));
    expect(order.every(index => index >= 0)).toBe(true);
    expect(order).toEqual([...order].sort((a, b) => a - b));
    const live = html.slice(
      html.indexOf('data-terminal-workspace-id="w-live"'),
      html.indexOf('data-terminal-workspace-id="w-ask"'),
    );
    // Both appearances retain the solid sidebar dot.
    expect(live).toContain('class="dot dot-working"');
    expect(live).toContain('class="workspace-tree-ticker" aria-hidden="true"');
    expect(live).not.toContain("dot-pulse");
    expect(live).toContain('class="workspace-tree-title">Live workspace<');
    expect(live).toContain('class="workspace-tree-pips" aria-hidden="true"><i class="is-working"></i></span>');
    // The count sits in a soft filled pill with a dim, regular-weight digit —
    // quieter than the row title but still legible. The fill is tinted from
    // the text tier: --surface-raised sits within a point or two of the pane
    // background in several themes and the chip vanished. No ring.
    // The project header's pane total wears the same chip.
    const count = /\.group-count,\n\.workspace-tree-count \{[^}]*\}/u.exec(source)?.[0] ?? "";
    expect(count).toContain("color: var(--fg-dim);");
    expect(count).toContain("background: color-mix(in srgb, var(--fg-dim) 14%, transparent);");
    expect(count).toContain("border-radius: var(--radius-container);");
    expect(count).toContain("font-weight: var(--wght-regular);");
    expect(count).not.toContain("box-shadow");
    expect(count).toContain("justify-content: center;");
    expect(count).toMatch(/min-width: (\d+)px;[^}]*height: \1px;/u);
    // On the selected row the digit lifts one tier with the rest of the row.
    expect(source).toContain(
      ".workspace-tree-row.is-selected .workspace-tree-count { color: var(--fg); }");
    expect(live).not.toContain("codicon-terminal");
    expect(live).not.toContain("session-row-meta");

    const dead = html.slice(
      html.indexOf('data-terminal-workspace-id="w-dead"'),
      html.indexOf('data-terminal-workspace-id="w-quiet"'),
    );
    expect(dead).toContain("codicon-error session-fail");
    expect(dead).not.toContain('class="workspace-tree-ticker"');
    expect(dead).not.toContain("session-row-status");

    const ask = html.slice(
      html.indexOf('data-terminal-workspace-id="w-ask"'),
      html.indexOf('data-terminal-workspace-id="w-dead"'),
    );
    expect(ask).toContain("codicon-warning session-wait");
    expect(ask).not.toContain('class="workspace-tree-ticker"');
    expect(ask).toContain('aria-label="Needs attention"');
    expect(ask).not.toContain("session-row-status");

    const quiet = html.slice(html.indexOf('data-terminal-workspace-id="w-quiet"'));
    expect(quiet).toContain("dot dot-idle");
    expect(quiet).not.toContain('class="workspace-tree-ticker"');
    expect(quiet).not.toContain("session-row-meta");
    expect(html).not.toContain("is-nested-workspace");
    expect(html).not.toContain('aria-label="Actions for live terminal"');
    expect(html).toContain('class="group-count" aria-hidden="true">4<');
  });

  it("keeps a workspace idle when its running terminals are not working", () => {
    const html = sidebarMarkup([{
      ...group("/work/interface", "Interface", { total: 0, waiting: 0, working: 0 }),
      terminals: [terminalRow("rest", { state: "running" })],
    }], 1, {
      terminalWorkspaces: {
        records: [{ id: "w-rest", name: "Resting", openTerminalCount: 1,
          terminalIds: ["rest"] }],
        activeId: null,
        onSelect: noop,
        onCreate: noop,
        onRename: noop,
        onDelete: noop,
        busy: false,
        canCreate: true,
      },
      terminalProjects: { rest: "/work/interface" },
    });
    const row = html.slice(html.indexOf('data-terminal-workspace-id="w-rest"'));
    expect(row).toContain("dot dot-idle");
    expect(row).not.toContain("dot-pulse");
    expect(row).not.toContain("dot-working");
  });

  it("opens a delete action from a terminal workspace's context menu", () => {
    expect(source).toContain("onDelete: (workspaceId: string) => void");
    expect(source).toContain("onContextMenu={event => {");
    expect(source).toContain('kind: "workspace"');
    expect(source).toContain("workspaceId: record.id");
    expect(source).toContain("event.preventDefault()");
    expect(source).toContain('event.key === "ContextMenu"');
    expect(source).toContain("event.key === \"F10\" && event.shiftKey");
    expect(source).toContain("Delete workspace…");
    expect(source).toContain("terminalWorkspaces.onDelete(auxWorkspace.id)");
    expect(source).toContain('className="row-menu-item row-menu-danger"');
  });

  it("lists terminal workspaces for a project that has no chat rows", () => {
    const project = {
      ...group("/work/interface", "Interface", { total: 0, waiting: 0, working: 0 }, []),
      terminals: [
        terminalRow("terminal-claude", { title: "Claude Code", profileId: "claude" }),
      ],
    };
    const html = sidebarMarkup([project], 1, {
      terminalWorkspaces: {
        records: [{ id: "w1", name: "Workspace 1", openTerminalCount: 1,
          terminalIds: ["terminal-claude"] }],
        activeId: "w1",
        onSelect: noop,
        onCreate: noop,
        onRename: noop,
        onDelete: noop,
        busy: false,
        canCreate: true,
      },
      terminalsActive: true,
      terminalProjects: { "terminal-claude": "/work/interface" },
    });

    expect(html).toContain(">Workspaces<");
    expect(html).toContain('data-terminal-workspace-id="w1"');
    expect(html).toContain('class="workspace-tree-title">Claude Code<');
    expect(html).not.toContain("Workspace 1");
    expect(html).not.toContain("codicon-terminal");
    expect(html).toContain('class="workspace-tree-row is-selected" data-terminal-workspace-id="w1"');
    expect(html).not.toContain("claude#");
    expect(html).not.toContain("Project chat");
  });

  it("does not render unassigned records in a separate terminal-workspace area", () => {
    const html = sidebarMarkup([
      group("/work/interface", "Interface", { total: 1, waiting: 0, working: 0 },
        [row("claude#chat", "idle")]),
    ], 1, {
      terminalWorkspaces: {
        records: [{ id: "unassigned", name: "Unassigned", openTerminalCount: 0,
          terminalIds: [] }],
        activeId: "unassigned",
        onSelect: noop,
        onCreate: noop,
        onRename: noop,
        onDelete: noop,
        busy: false,
        canCreate: true,
      },
      terminalsActive: true,
    });
    expect(html).not.toContain('aria-label="Terminal workspaces"');
    expect(html).not.toContain('data-terminal-workspace-id="unassigned"');
    expect(html).not.toContain(">Unassigned<");
    expect(html).toContain('aria-label="New terminal workspace"');
    expect(html).toContain(">Workspaces<");
    expect(source).not.toContain("sidebar-workspace-nav");
  });

  it("still lists a saved workspace from its cwd before PTYs reopen", () => {
    const html = sidebarMarkup([], 0, {
      terminalWorkspaces: {
        records: [{
          id: "saved",
          name: "Last session",
          openTerminalCount: 0,
          terminalIds: ["terminal-grok"],
          cwd: "/work/interface",
        }],
        activeId: "saved",
        onSelect: noop,
        onCreate: noop,
        onRename: noop,
        onDelete: noop,
        busy: false,
        canCreate: true,
      },
      terminalsActive: true,
    });
    expect(html).toContain('data-terminal-workspace-id="saved"');
    expect(html).toContain("Last session");
    expect(html).toContain('class="workspace-tree-count" aria-hidden="true">0<');
    expect(html).toContain('class="workspace-tree-count-label">0 open terminals<');
  });

  it("adds terminal state to the collapsed project's accessible name", () => {
    const project = {
      ...group("/work/interface", "Interface", { total: 1, waiting: 0, working: 0 },
        [row("claude#chat", "idle")]),
      terminals: [
        terminalRow("failed", { state: "failed" }),
        terminalRow("running"),
      ],
    };
    expect(groupAccessibleName(project))
      .toBe("Interface, 1 session, 2 terminals, 1 failed");
  });

  it("bounds quiet project rows while preserving every active and selected session", () => {
    const rows = Array.from({ length: 14 }, (_, index) => row(`row-${index}`, "idle"));
    rows[8] = { ...rows[8], badge: "waiting", status: "waiting" };
    rows[9] = { ...rows[9], badge: "failed", status: "failed" };
    rows[10] = { ...rows[10], badge: "working", status: "working" };
    rows[13] = { ...rows[13], selected: true };
    const visible = visibleSidebarProjectRows(rows, false, false);
    expect(visible.rows).toHaveLength(SIDEBAR_PROJECT_ROW_LIMIT);
    expect(visible.rows.map(item => item.key)).toContain("row-10");
    expect(visible.rows.map(item => item.key)).toContain("row-8");
    expect(visible.rows.map(item => item.key)).toContain("row-9");
    expect(visible.rows.map(item => item.key)).toContain("row-13");
    expect(visible.hidden).toBe(6);
    expect(visibleSidebarProjectRows(rows, true, false).rows).toHaveLength(14);
    expect(visibleSidebarProjectRows(rows, false, true).rows).toHaveLength(14);
  });

  it("renders a truthful keyboard-operable reveal for a crowded project", () => {
    const rows = Array.from({ length: 12 }, (_, index) => row(`row-${index}`, "idle"));
    const html = sidebarMarkup([group("/work/one", "one",
      { total: rows.length, waiting: 0, working: 0 }, rows)], 1);
    // The row button itself, not the two line wrappers inside it: the class
    // attribute has to END at "session-row" or continue with a modifier.
    expect(html.match(/class="session-row[ "]/g)).toHaveLength(SIDEBAR_PROJECT_ROW_LIMIT);
    expect(html).toContain("Show 4 more");
    expect(html).toContain('aria-label="Show 4 more sessions in one"');
    expect(html).not.toContain(">row-11<");
  });

  it("leads with one header row: the pane's name, then New thread and Filter as icons", () => {
    expect(source).toContain('className="sidebar-primary-nav"');
    // The label and the actions share the header; there is no second title row.
    expect(source).not.toContain('className="sidebar-section-title"');
    expect(source).not.toContain("sidebar-nav-shortcut");
    expect(source).toContain('"New workspace or project" : `${newLabel} (⌘N)`');
    expect(source).toContain('title={`Filter ${sectionLabel.toLowerCase()} (⌘K)`}');
    expect(source).toContain('aria-label="Sessions"');
    expect(source).toContain('<span className="sidebar-nav-label">{newLabel}</span>');
    // The row and the field it reveals say the same thing: this filters the
    // list in place. Searching transcripts is the Library, and the palette
    // entry that opens it says so.
    expect(source).toContain('<span className="sidebar-nav-label">Filter {sectionLabel.toLowerCase()}</span>');
    expect(source).toContain('openAuxMenu({ kind: "create" }, event.currentTarget)');
    expect(source).toContain('aria-current={newChatActive ? "page" : undefined}');
    expect(source).toContain('event.key.toLowerCase() !== "k"');
    // A window listener, so it asks the same question App's handler does:
    // Ctrl+K in a focused shell is kill-to-end-of-line, not this field.
    expect(source).toContain("if (shellOwnsChord(event, event.target instanceof Element");
    expect(source).toContain('setSearchOpen(true)');
    expect(sidebarMarkup([group("/work/one", "one")], 1))
      .toContain('<span class="sidebar-nav-label">New thread</span>');
  });

  it("lets the showing surface name the create button and its true zero-state", () => {
    const task = sidebarMarkup([], 0, {
      newLabel: "New task",
      zeroStateText: "No tasks yet.",
    });
    expect(task).toContain('<span class="sidebar-nav-label">New task</span>');
    expect(task).toContain('aria-label="New task"');
    expect(task).toContain('title="New task (⌘N)"');
    // Every surface creates with the same + (icon review, 2026-09-24).
    expect(task).toContain('<span class="codicon codicon-add sidebar-nav-icon" aria-hidden="true"></span>');
    expect(task).not.toContain("comment-discussion");
    expect(task).toContain("No tasks yet.");
    expect(task).not.toContain("No agent sessions yet.");
    expect(sidebarMarkup([], 0, { zeroStateText: "No threads yet." }))
      .toContain("No threads yet.");
    expect(sidebarMarkup([], 0)).toContain("No agent sessions yet.");
  });

  it("names the pane's list after the surface showing it", () => {
    expect(sidebarMarkup([group("/work/one", "one")], 1, { sectionLabel: "Threads" }))
      .toContain('<span class="sidebar-section-label">Threads</span>');
    expect(sidebarMarkup([group("/work/one", "one")], 1, { sectionLabel: "Tasks" }))
      .toContain('<span class="sidebar-section-label">Tasks</span>');
  });

  it("hands an empty filter result on to the transcript search", () => {
    // The field filters the rows already listed; the Library reads further.
    // A filter that found nothing offers its query to the one that can.
    const handed = sidebarMarkup([], 0, {
      query: "parser", onQuery: noop, searching: true, onSearchTranscripts: noop,
    });
    expect(handed).toContain('class="sidebar-search-handoff"');
    expect(handed).toContain("Search transcripts for");
    expect(handed).toContain("parser");
    expect(handed).toContain('placeholder="Filter workspaces…"');
    // Nothing typed, nothing to hand over.
    expect(sidebarMarkup([], 0, { onQuery: noop, onSearchTranscripts: noop }))
      .not.toContain('class="sidebar-search-handoff"');
  });

  it("keeps filter explanations surface-neutral", () => {
    const filtered = sidebarMarkup([], 0, {
      hidden: { external: 3, trivial: 2 },
      zeroStateText: "No tasks yet.",
    });
    expect(filtered).toContain("Both filters are hiding everything here.");
    expect(filtered).not.toContain("No tasks yet.");
  });

  it("keeps every workspace grouped with its item count and moves maintenance into one menu", () => {
    const single = sidebarMarkup([group("/work/one", "one")], 1);
    const grouped = sidebarMarkup([
      group("/work/one", "one"),
      group("/work/two", "two"),
    ], 2);
    const withMaintenance = sidebarMarkup([group("/work/one", "one")], 1, {
      hideable: 2,
      hidden: { external: 1, trivial: 1 },
    });
    expect(source).toContain('<span className="sidebar-section-label">{sectionLabel}</span>');
    expect(source).not.toContain('className="sidebar-section-meta"');
    expect(source).toContain('className="group-count"');
    expect(single).toContain('class="group-header"');
    expect(single).toContain('class="group-count"');
    expect(grouped.match(/class="group-count"/g)).toHaveLength(2);
    expect(single).not.toContain('aria-label="Project options"');
    expect(withMaintenance).toContain('aria-label="Project options"');
    expect(source).toContain('className="sidebar-section-action"');
    expect(source).toContain('aria-label="Project options"');
    expect(source).toContain('openAuxMenu({ kind: "projects" }, event.currentTarget)');
    expect(source).toContain("onToggleFilters();");
    expect(source).toContain("importer.onImportAll();");
    expect(source).toContain("Import sessions…");
    expect(source).not.toContain('className="sidebar-repo-context"');
  });

  it("makes whole-project archiving local, confirmed, and visibly reversible", () => {
    expect(source).toContain("setConfirmProjectPath(auxProject.projectPath)");
    expect(source).toContain('role="alertdialog"');
    expect(source).toContain("This only hides it in {productLabel()}");
    expect(source).toContain('className="archived-projects-title"');
    expect(source).toContain("Archived projects");
    expect(source).toContain('aria-label={`Restore ${project.projectName}`}');
    expect(source).toContain("restoreProject(project.projectPath)");
    expect(source).toContain("Nothing in the project is stopped or deleted");
    expect(source).toContain(
      "disabled={projectArchiveBlocked(auxProject, protectedProjectSet)}");
    expect(source).toContain(
      "if (projectArchiveBlocked(auxProject, protectedProjectSet)) return;");
    expect(source).not.toContain("onArchiveProject");
  });

  it("starts a new chat in the exact project from its overflow menu", () => {
    expect(source).toContain("onNewInProject: (projectPath: string) => void");
    expect(source).toContain(
      'aria-label={`Project actions for ${g.projectName} at ${g.projectPath}`}',
    );
    expect(source).toContain("onNewInProject(auxProject.projectPath)");
    expect(source).toContain('"project-menu-button is-open"');
    expect(source).not.toContain("project-new-chat-button");
    expect(source).not.toContain("project-archive-button");
    expect(source).not.toContain("onNewInProject(OTHER_PROJECT)");
  });

  it("keeps project commands out of the resting list and orders New chat before Archive", () => {
    const single = sidebarMarkup([group("/work/one", "one")], 1);
    const grouped = sidebarMarkup([
      group("/work/one", "one"),
      group("/work/two", "two"),
    ], 2);
    expect(single).toContain('aria-label="Project actions for one at /work/one"');
    expect(grouped).toContain('aria-label="Project actions for one at /work/one"');
    expect(single).not.toMatch(/role="menuitem"[^>]*>New chat</u);
    expect(single).not.toContain("Archive project…");
    expect(source).toMatch(
      /auxProject !== null[\s\S]*?>\s*New chat\s*<\/button>[\s\S]*?>\s*Archive project…\s*<\/button>/,
    );
  });

  it("keeps project and maintenance menus keyboard reachable", () => {
    expect(source).toContain('aria-haspopup="menu"');
    expect(source).toContain('id={`${uid}-aux-menu`}');
    expect(source).toContain('role="menu"');
    // BOTH menus, not just the aux one: a disabled item is never a landing
    // place for an arrow key.
    expect(source.match(/menuKeyNav\(event, '\[role="menuitem"\]:not\(:disabled\)'/gu))
      .toHaveLength(2);
    expect(source).not.toMatch(/menuKeyNav\(event, '\[role="menuitem"\]',/u);
    expect(controls).toContain('event.key === "ArrowDown"');
    expect(controls).toContain('event.key === "ArrowUp"');
    expect(controls).toContain('event.key === "Home"');
    expect(controls).toContain('event.key === "End"');
    expect(source).toContain("requestAnimationFrame(() => { trigger?.focus(); })");
  });

  it("gives each menu one Tab stop", () => {
    // Roving tabindex: the focused menuitem is the only Tab stop, applied from
    // the DOM because the items are conditional JSX, not a mapped list.
    expect(source).toContain(
      "onFocus={event => { roveMenuTabIndex(event.currentTarget, event.target); }}");
    expect(controls).toContain("item.tabIndex = item === target ? 0 : -1;");
    expect(source.match(/role="menuitem"(?!\]| tabIndex=\{-1\})/gu)).toBeNull();
    expect(source).toMatch(/\.attention \{[^}]*overscroll-behavior: contain;/u);
  });

  it("keeps portaled menus inside the mobile drawer focus loop", () => {
    expect(source).toContain("const portalRoots = [auxMenuRef.current, menuRef.current]");
    expect(source).toContain(
      "...portalRoots.flatMap(root => [...root.querySelectorAll<HTMLElement>(selector)])",
    );
    expect(source).toContain("if (portalRoots.length > 0)");
    expect(source).toContain("current + (event.shiftKey ? -1 : 1) + focusable.length");
    expect(source).toContain("focusable[next]?.focus()");
  });

  it("disambiguates duplicate names and never scopes the synthetic Other group", () => {
    const markup = sidebarMarkup([
      group("/a/web", "web"),
      group("/b/web", "web"),
      group(OTHER_PROJECT, "Other"),
    ], 3);
    expect(markup).toContain('aria-label="Project actions for web at /a/web"');
    expect(markup).toContain('aria-label="Project actions for web at /b/web"');
    expect(markup).not.toContain("Project actions for Other");
  });

  it("keeps the project menu reachable when an active agent blocks Archive", () => {
    const markup = sidebarMarkup([
      group("/work/one", "one", { total: 1, waiting: 0, working: 1 }),
    ], 1);
    const menuButton = markup.match(
      /<button[^>]*aria-label="Project actions for one at \/work\/one"[^>]*>/,
    )?.[0];
    expect(menuButton).toBeDefined();
    expect(menuButton).not.toContain("disabled");
    expect(source).toContain(
      "disabled={projectArchiveBlocked(auxProject, protectedProjectSet)}");
  });

  it("keeps an archived Code project hidden with a persistent idle terminal workspace", () => {
    const idleProject = {
      ...group("/work/one", "one", { total: 1, waiting: 0, working: 0 }),
      terminals: [terminalRow("shell", { state: "running", activity: "idle" })],
    };
    const markup = sidebarMarkupWithArchivedProject(idleProject);
    expect(markup).toContain("All projects are archived");
    expect(markup).toContain('aria-label="Project options"');
    expect(markup).not.toContain("Project actions for one at /work/one");
    expect(markup).not.toContain('data-terminal-workspace-id="workspace-one"');
    expect(markup).not.toContain("Workspace one");
    expect(markup).not.toContain("Detached workspace");
  });

  it("restores an archived Code project when full-registry agent work protects it", () => {
    const idleTerminalProject = {
      ...group("/work/one", "one", { total: 0, waiting: 0, working: 0 }),
      terminals: [terminalRow("shell", { state: "running", activity: "idle" })],
    };
    const markup = sidebarMarkupWithArchivedProject(
      idleTerminalProject,
      [],
      ["/work/one"],
    );

    expect(markup).toContain("Project actions for one at /work/one");
    expect(markup).toContain('data-terminal-workspace-id="workspace-one"');
    expect(markup).not.toContain("All projects are archived");
  });

  it("keeps an archived nested workspace hidden beside a broader project root", () => {
    const archived = group("/work/one", "one");
    const markup = sidebarMarkupWithArchivedProject(archived, [group("/", "/")]);
    expect(markup).toContain("Project actions for / at /");
    expect(markup).not.toContain("Project actions for one at /work/one");
    expect(markup).not.toContain("Detached workspace");
  });

  it("keeps Settings below workspaces while provider health stays in the statusbar", () => {
    expect(source).not.toContain("sidebar-provider-footer");
    expect(source).not.toContain("sidebar-provider-avatar");
    expect(source).toContain('className="sidebar-footer"');
    expect(source).toContain('className="sidebar-settings-button"');
    expect(source).not.toContain("Local runtime");
    expect(source).not.toContain("providerSummary");
    // The pane no longer reads provider health at all; the statusbar owns it.
    expect(source).not.toContain("health.length");
    expect(source).not.toContain('className="banner"');
  });

  it("puts Settings and the setup notice in one quiet footer row, without the version", () => {
    // Owner, 2026-11-15: "Settings" on the left; while providers need setup,
    // an amber dot and "N providers need setup" on its right in the same row.
    // The red banner and the version label are gone; ⌘, lives in the tooltip.
    const markup = renderToStaticMarkup(createElement(Sidebar, {
      groups: [], projectCount: 0, rateLimits: {}, hidden: { external: 0, trivial: 0 }, hideable: 0,
      onSelect: noop, onNew: noop, onNewInProject: noop, onArchive: noop, onToggleFilters: noop,
      onSettings: noop, onFooterAlert: noop,
      footerAlert: { label: "2 providers need setup", shortLabel: "2 need setup", description: "Codex: sign in",
        accessibleName: "2 providers need setup: open Models & providers" },
    }));
    const footer = markup.slice(markup.indexOf('<footer class="sidebar-footer">'), markup.indexOf("</footer>"));
    expect(footer).toMatch(/^<footer class="sidebar-footer"><div class="sidebar-footer-row"><button[^>]*class="sidebar-settings-button"[^>]*title="Settings \(⌘,\)"[^>]*><span class="sidebar-settings-label">Settings<\/span><\/button><button[^>]*class="sidebar-footer-alert"/u);
    expect(footer).toContain('aria-label="2 providers need setup: open Models &amp; providers"');
    expect(footer).toContain('<span class="sidebar-footer-alert-dot" aria-hidden="true"></span><span class="sidebar-footer-alert-label">2 providers need setup</span>'
      + '<span class="sidebar-footer-alert-label is-short" aria-hidden="true">2 need setup</span>');
    expect(footer).not.toContain("codicon-error");
    expect(footer).not.toContain("<kbd");
    expect(source).not.toContain("sidebar-settings-version");
    expect(source).not.toMatch(/\bversion\?: string/u);
    // Amber, not the danger tier; the words give way before the dot does.
    expect(source).toMatch(/\.sidebar-footer-alert \{[^}]*color: var\(--status-waiting\);/u);
    expect(source).not.toMatch(/\.sidebar-footer-alert \{[^}]*--danger/u);
    expect(source).toMatch(/\.sidebar-footer-alert-dot \{[^}]*flex: none;/u);
    expect(source).toMatch(/@container sidebar-footer \(max-width: 240px\) \{[^}]*\.sidebar-footer-alert-label:not\(\.is-short\) \{ display: none; \}/u);
    expect(source).toMatch(/@container sidebar-footer \(max-width: 180px\) \{\s*\.sidebar-footer-alert-label,/u);
    // Without a notice the row is Settings alone.
    const quiet = renderToStaticMarkup(createElement(Sidebar, {
      groups: [], projectCount: 0, rateLimits: {}, hidden: { external: 0, trivial: 0 }, hideable: 0,
      onSelect: noop, onNew: noop, onNewInProject: noop, onArchive: noop, onToggleFilters: noop,
      onSettings: noop,
    }));
    expect(quiet.slice(quiet.indexOf('<footer class="sidebar-footer">'))).not.toContain("sidebar-footer-alert");
  });

  it("preserves grouped sessions and every lifecycle route", () => {
    expect(source).toContain("[groups, archivedProjects, protectedProjects, projectCount]");
    expect(source).toContain('listedGroups.map((g, i) =>');
    // One renderer, two places: inside the group, and lifted to the pinned
    // section. Both call the same function, so a lifted row keeps every action.
    expect(source).toContain("visibleGroup.rows.map(renderSessionRow)");
    expect(source).toContain("attentionRows.map(renderSessionRow)");
    expect(source).toContain('onSelect(r.key)');
    expect(source).toContain("onArchive(menuRow.key)");
    expect(source).toContain('lifecycle.onRename(row.key, title)');
    expect(source).toContain('lifecycle.onFork(menuRow.key)');
    expect(source).toContain("lifecycle.onDelete(key)");
    expect(source).toContain("lifecycle.onEngineArchive(key)");
  });

  it("keeps pane creation explicit and independent of provider lifecycle support", () => {
    expect(source).toContain('onOpenInNewPane?: (key: string) => void');
    expect(source).toContain("menuRow !== null && renameFor === null");
    expect(source).toContain("Open in new pane");
    expect(source).toContain("onOpenInNewPane(menuRow.key)");
    expect(source).toContain("openPane(r.key)");
    expect(source).toContain('aria-haspopup="menu"');
    expect(source).toContain("createPortal(menu, document.body)");
    expect(source).toContain("positionAppSelect");
    expect(controls).toContain('event.key === "ArrowDown"');
    expect(controls).toContain('event.key === "ArrowUp"');
    expect(source).toContain("restoreRowActionFocus");
    expect(source).toContain("<ConfirmDialog");
    expect(source).toContain("position: fixed");
    expect(source).toContain("box-shadow: var(--overlay-shadow)");
  });

  it("splits the row's actions by what the app can undo, not by how often used", () => {
    // WAS: one "…" owned every secondary action and local hide carried a
    // confirm like the other two. Archiving was then four interactions —
    // hover, open menu, pick, confirm — for the one action Interface can undo
    // by itself, which is why clearing a long list was a chore.
    //
    // NOW: archive is a button on the row, one press, no dialog. The two the
    // app CANNOT undo — archiving in the engine's own store, deleting the
    // transcript — keep both the menu and the confirm. The line is
    // reversibility; frequency is not a safety property.
    expect(source).toContain('className={menuOpen ? "session-menu is-open" : "session-menu"}');
    expect(source).toContain('className="session-archive"');
    expect(source).toContain("onArchive(r.key)");
    // The irreversible pair are still armed the slow way, behind a dialog.
    expect(source).toContain('setConfirmFor({ key: menuRow.key, kind: "engine" })');
    expect(source).toContain('setConfirmFor({ key: menuRow.key, kind: "delete" })');
    expect(source).toContain("sessionConfirmDialog");
    // The menu's own route to the SAME action carries the same name and the
    // same friction: no "…", no dialog, because it is the reversible one.
    expect(source).toContain("              Hide\n");
    expect(source).not.toContain("Hide from <ProductName />…");
    expect(source).not.toContain('kind: "local"');
    expect(source).toContain("can be restored from Library > Archived");
  });

  it("splits on modifier-click and replaces the current pane otherwise", () => {
    expect(sessionRowActivateAction({ metaKey: true }, true)).toBe("pane");
    expect(sessionRowActivateAction({ ctrlKey: true }, true)).toBe("pane");
    expect(sessionRowActivateAction({ metaKey: true }, false)).toBe("select");
    expect(sessionRowActivateAction({ metaKey: false, ctrlKey: false }, true)).toBe("select");
  });

  it("keeps its visual rules component-scoped and token driven", () => {
    const css = source.match(/const SIDEBAR_CSS = `([\s\S]*?)`;/)?.[1] ?? "";
    expect(css).toContain(".sidebar-nav-button");
    // The vertical-list selected look: a fill plus the strong text tier. The
    // accent hue is the horizontal tab strips' underline and nothing else.
    expect(css).toMatch(
      /\.sidebar-nav-button\.is-active \{[\s\S]*?color: var\(--fg-strong\);[\s\S]*?background: var\(--row-selected\);/u,
    );
    expect(css).toContain(
      ".sidebar-nav-button.is-active .sidebar-nav-icon { color: var(--fg-strong); }",
    );
    expect(css).toContain(".sidebar-nav-button:first-child:not(.is-active)");
    expect(css).not.toContain(".sidebar-nav-button:first-child { color:");
    expect(css).not.toContain("height: var(--titlebar-h)");
    expect(css).not.toContain("-webkit-app-region: drag");
    expect(css).toContain(".sidebar input { -webkit-app-region: no-drag; }");
    expect(css).toContain("var(--row-hover)");
    // Codex and Claude pips differ (owner, 2026-10-02): a working agent pane's
    // pip takes its provider colour, an idle one a faint wash of it, while
    // needs-you, done and failed keep their status colours by coming later.
    expect(css).toMatch(
      /\.workspace-tree-pips > i\.is-working\[data-provider\] \{\s*background: var\(--provider-accent, var\(--status-working\)\);/u,
    );
    // A shell has no provider; its working pip stays neutral so orange
    // never reads as Claude.
    expect(css).toContain(".workspace-tree-pips > i.is-working { background: var(--fg-muted); }");
    expect(css.indexOf(".workspace-tree-pips > i[data-provider]"))
      .toBeLessThan(css.indexOf(".workspace-tree-pips > i.is-waiting"));
    expect(css).not.toMatch(/#[0-9a-f]{3,8}\b/i);
    expect(source).toContain("<style>{SIDEBAR_CSS}</style>");
  });
});

/* The pane's core job: which agent is blocked on the user, said where the user
   already is, in a way that survives a collapsed group, no colour vision, and a
   screen reader. Before this, all three of those lost the signal entirely. */
describe("blocked on you, in the always-visible pane", () => {
  const blocked = (): SidebarGroup[] => [
    group("/work/interface", "Interface", { total: 3, waiting: 1, working: 1 }, [
      row("claude#a", "waiting", "Approve the migration"),
      row("claude#b", "working", "Rewrite the parser"),
      row("claude#c", "idle", "Old notes"),
    ]),
    group("/work/ledger", "ledger", { total: 2, waiting: 0, working: 0 }, [
      row("claude#d", "failed", "Nightly build"),
      row("claude#e", "idle", "Scratch run"),
    ]),
    group("/work/tracker", "tracker", { total: 1, waiting: 0, working: 1 }, [
      row("claude#f", "working", "Indexing the repo"),
    ]),
  ];

  it("carries the waiting count in the group header's accessible name", () => {
    const markup = sidebarMarkup(blocked());
    expect(markup).toContain(
      'aria-label="Interface, 3 sessions, 1 needs attention, 1 working"');
    expect(markup).toContain('aria-label="ledger, 2 sessions, 1 failed"');
  });

  it("keeps that name grammatical, because a collapsed group has nothing else", () => {
    expect(groupAccessibleName(group("/w/one", "one", { total: 1, waiting: 0, working: 0 })))
      .toBe("one, 1 session");
    expect(groupAccessibleName(group("/w/two", "two", { total: 9, waiting: 2, working: 3 },
      [row("k", "failed")])))
      .toBe("two, 9 sessions, 2 need attention, 1 failed, 3 working");
    // Two projects can both display as "web"; the name keeps what the header
    // shows, so the accessible name still contains the visible label.
    expect(groupAccessibleName(group("/a/web", "web"), "a")).toBe("web · a, 1 session");
  });

  it("lifts blocked and failed agents to a pinned section — once each, above Workspaces", () => {
    // Exactly the pair the sidebar's Needs attention section holds.
    expect(["waiting", "failed"].map(b => isAttentionRow(row("k", b as SidebarRow["badge"]))))
      .toEqual([true, true]);
    expect(["working", "idle"].map(b => isAttentionRow(row("k", b as SidebarRow["badge"]))))
      .toEqual([false, false]);
    const markup = sidebarMarkup(blocked());
    const pinned = markup.indexOf('class="attention"');
    expect(pinned).toBeGreaterThan(-1);
    // Under the header row, above the project list.
    expect(pinned).toBeGreaterThan(markup.indexOf(">Workspaces<"));
    expect(pinned).toBeLessThan(markup.indexOf('class="session-list"'));
    // Waiting before failed: the app's own band order, so the two surfaces
    // rank the same two states the same way.
    expect(markup.indexOf("Approve the migration"))
      .toBeLessThan(markup.indexOf("Nightly build"));
    // LIFTED, not copied. One row means one ⋯ trigger, so a blocked agent
    // keeps rename, fork, hide and delete instead of trading them for the pin.
    expect(markup.split('aria-label="Actions for Approve the migration"')).toHaveLength(2);
    expect(markup).toContain('aria-label="Actions for Nightly build"');
    // The quiet rows stay where they belong.
    expect(markup.indexOf("Old notes")).toBeGreaterThan(markup.indexOf('class="session-list"'));
    // The exceptional rows stay prominent, but the resting heading does not
    // repeat another aggregate beside the toolbar total.
    expect(markup).not.toContain('class="attention-count"');
  });

  it("says the words on the row, in every state, not only in a tooltip", () => {
    const markup = sidebarMarkup(blocked());
    // Every row is one line, so the pill is the word's one carrier. What must
    // never happen is the word disappearing, which is what this pins.
    expect(markup).toContain('class="session-flag session-flag-waiting">Needs attention<');
    expect(markup).toContain('class="session-flag session-flag-failed">Failed<');
    expect(markup).not.toContain("session-row-status");
    expect(markup).not.toContain("is-two-line");
    expect(markup).toContain('class="session-item is-waiting"');
    expect(markup).toContain('class="session-item is-failed"');
  });

  it("announces the state exactly once per row", () => {
    // The pill is text inside the row button, so it is already in the row's
    // accessible name; the glyph beside it is decoration and must stay
    // aria-hidden, or a blocked agent would be read out twice.
    const markup = sidebarMarkup(blocked());
    expect(markup).toContain(
      'class="codicon codicon-warning session-wait" aria-hidden="true" title="Needs attention"');
    expect(markup).toContain(
      'class="codicon codicon-error session-fail" aria-hidden="true" title="Failed"');
    expect(markup).not.toContain('role="img" aria-label="Needs attention"');
  });

  it("tells the states apart by shape before hue, on rows and on group headers", () => {
    const markup = sidebarMarkup(blocked());
    expect(markup).toContain("codicon-warning session-wait");
    expect(markup).toContain("codicon-error session-fail");
    expect(markup).toContain("codicon-warning group-attention-waiting");
    expect(markup).toContain("codicon-error group-attention-failed");
    // Project headers retain their working dot in both appearances.
    expect(markup).toContain('class="group-attention-working"');
  });

  /* THE ROW ACTION, PRESSED.
     Markup proves the button is drawn; only a press proves it archives. The
     failure this repo keeps shipping is a control that renders and does
     nothing, and `renderToStaticMarkup` never runs an onClick. */
  type Node = { type: unknown; props: Record<string, unknown> };
  const isNode = (v: unknown): v is Node =>
    typeof v === "object" && v !== null && "type" in v && "props" in v;
  function nodes(tree: unknown, out: Node[] = []): Node[] {
    if (Array.isArray(tree)) { for (const n of tree) nodes(n, out); return out; }
    if (!isNode(tree)) return out;
    // A memoized session row draws itself: walk what its function returns.
    if (tree.type === SidebarSessionRow) {
      const draw = (SidebarSessionRow as unknown as { type: (p: unknown) => unknown }).type;
      return nodes(draw(tree.props), out);
    }
    out.push(tree);
    nodes((tree.props as { children?: unknown }).children, out);
    return out;
  }

  function pressArchive(groups: SidebarGroup[], title: string): string[] {
    const archived: string[] = [];
    // The row action restores focus through rAF, which node has no global for.
    const saved = (globalThis as Record<string, unknown>).requestAnimationFrame;
    (globalThis as Record<string, unknown>).requestAnimationFrame =
      (cb: (t: number) => void): number => { cb(0); return 0; };
    try {
      let press: ((e: unknown) => void) | undefined;
      function Probe(): null {
        const tree = (Sidebar as unknown as (p: unknown) => unknown)({
          groups, projectCount: groups.length, rateLimits: {},
          hidden: { external: 0, trivial: 0 }, hideable: 0,
          onSelect: noop, onNew: noop, onNewInProject: noop, onToggleFilters: noop,
          onArchive: (key: string) => { archived.push(key); },
        });
        const button = nodes(tree).find(el => el.type === "button"
          && el.props["aria-label"] === `Hide ${title}`);
        press = button?.props.onClick as ((e: unknown) => void) | undefined;
        return null;
      }
      renderToStaticMarkup(createElement(Probe));
      // AFTER the render pass: the handler sets state, and React refuses a
      // state update raised while a component is still rendering.
      expect(press).toBeDefined();
      press?.({ stopPropagation: noop });
    } finally {
      (globalThis as Record<string, unknown>).requestAnimationFrame = saved;
    }
    return archived;
  }

  it("archives a chat in one press, from a button on its own row", () => {
    const groups = [group("/work/interface", "Interface",
      { total: 2, waiting: 0, working: 0 }, [
        row("claude#a", "idle", "Rewrite the parser"),
        row("codex#b", "idle", "Audit the sidebar"),
      ])];
    const markup = sidebarMarkup(groups);
    // Drawn once per row, named after the chat it acts on — a voice user has to
    // be able to say which one, and "Hide" alone is two identical controls.
    // eye-closed, not the archive box: that glyph is the engine-store action.
    expect(markup).toContain('aria-label="Hide Rewrite the parser"');
    expect(markup).toContain('aria-label="Hide Audit the sidebar"');
    expect(markup).toContain("codicon-eye-closed");
    expect(markup).not.toContain("codicon-archive");
    // And it reaches the caller, with the key of ITS OWN row.
    expect(pressArchive(groups, "Audit the sidebar")).toEqual(["codex#b"]);
    expect(pressArchive(groups, "Rewrite the parser")).toEqual(["claude#a"]);
  });

  it("spends one click only on the archive the app can undo itself", () => {
    // Local hide is restorable from Library > Archived, so it needs no dialog.
    // Engine archive and delete are not, so they stay behind the menu AND a
    // confirm — the split is reversibility, not how often each is used.
    const markup = sidebarMarkup([group("/work/i", "I",
      { total: 1, waiting: 0, working: 0 }, [row("claude#a", "idle", "One")])]);
    expect(markup).not.toContain("Delete");
    expect(markup).not.toContain("Archive in claude");
    // The row reserves room for BOTH buttons, or a long title runs under them.
    const css = source.match(/const SIDEBAR_CSS = `([\s\S]*?)`;/u)?.[1] ?? "";
    expect(css).toContain("padding-right: calc(var(--icon-size) * 2 + 16px);");
    expect(css).toContain(".session-archive { right: 28px; }");
    expect(css).toContain(".session-item:hover .session-archive");
  });

  it("paints a status dot on idle and working rows, and nothing but the title beside it", () => {
    const markup = sidebarMarkup([
      group("/work/interface", "Interface", { total: 2, waiting: 0, working: 1 }, [
        row("claude#a", "idle", "Local multi-model harness"),
        row("claude#b", "working", "Rewrite the parser"),
      ]),
    ]);
    expect(markup).toContain("dot dot-idle");
    expect(markup).toContain("dot dot-working dot-pulse");
    expect(markup).not.toContain("session-status-spacer");
    // The provider logo and the age left the row for the hover card; the dot
    // keeps the provider as data for the theme.
    expect(markup).not.toContain('data-provider-logo="claude"');
    expect(markup).not.toContain('class="engine-tag"');
    expect(markup).not.toContain('class="session-age"');
    expect(markup).toContain('data-provider="claude"');
    expect(markup).toContain("Local multi-model harness");
    // THE ROW NAMES THE WORK, NOT THE MODEL. Only Grok reported one in the
    // listing — measured, 18 of 18 Grok sessions against 0 of 95 Codex — so the
    // slot was in practice a per-engine tag repeating "grok-4.6" down the whole
    // list. The model is still on the tile header, which is where the session
    // being READ says what it is running.
    expect(markup).not.toContain("session-model");
  });

  it("draws the rail and the pill without spending contrast it does not have", () => {
    const css = source.match(/const SIDEBAR_CSS = `([\s\S]*?)`;/)?.[1] ?? "";
    // The rail is on the item, so the selected row's own accent rail survives.
    expect(css).toMatch(
      /\.session-item\.is-waiting::before[\s\S]*?background: var\(--status-waiting\)/);
    // The word is --fg-strong: --status-waiting as 11px text over a selected
    // row measures under AA in three shipped light themes.
    expect(css).toMatch(/\.session-flag \{[\s\S]*?color: var\(--fg-strong\)/);
    expect(css).toContain(".session-flag-waiting { border-color: var(--status-waiting); }");
  });

  it("keeps its two attention words in the shared status map", () => {
    expect(source).toMatch(/import \{[^}]*\bstatusWord\b[^}]*\} from "\.\/status-words"/u);
    expect(source).not.toContain('"Waiting for you"');
  });
});

/* M7: a loading pane must not answer a question it has not heard back on. */
describe("the sidebar before its first listing arrives", () => {
  it("draws row-shaped skeletons instead of claiming there are no sessions", () => {
    const markup = sidebarMarkup([], 0, { loading: true });
    expect(markup).toContain('class="session-skeleton"');
    expect(markup).toContain('aria-label="Loading sessions"');
    expect(markup).not.toContain("No agent sessions yet.");
    // The same 16px status column the real rows use — theme.css's own .dot.
    expect(markup).toContain('class="dot session-skeleton-dot"');
  });

  it("states the zero-state, not a skeleton, once the listing is in", () => {
    const markup = sidebarMarkup([], 0);
    expect(markup).not.toContain('class="session-skeleton"');
    expect(markup).toContain("No agent sessions yet.");
  });
});

describe("folding every project from the header", () => {
  function markupWithCollapsed(collapsed: readonly string[],
      groups: SidebarGroup[], over: Parameters<typeof sidebarMarkup>[2] = {}): string {
    vi.stubGlobal("window", {
      localStorage: {
        getItem: (key: string) => key === "interface.sidebar.collapsedProjects"
          ? JSON.stringify(collapsed) : null,
        setItem: noop,
      },
    });
    try {
      return sidebarMarkup(groups, groups.length, over);
    } finally {
      vi.unstubAllGlobals();
    }
  }
  const one = group("/work/one", "one");
  const two = group("/work/two", "two");

  it("collapses the listed projects and reopens only those", () => {
    expect(withAllProjectsCollapsed(["/gone"], ["/work/one", "/work/two"], true))
      .toEqual(["/gone", "/work/one", "/work/two"]);
    expect(withAllProjectsCollapsed(["/work/one"], ["/work/one", "/work/two"], true))
      .toEqual(["/work/one", "/work/two"]);
    expect(withAllProjectsCollapsed(["/gone", "/work/one", "/work/two"],
      ["/work/one", "/work/two"], false)).toEqual(["/gone"]);
  });

  it("offers Collapse all while any project is open and Expand all once every one is shut", () => {
    const open = markupWithCollapsed([], [one, two], { codeWorkspaces: true });
    expect(open).toContain('aria-label="Collapse all folders"');
    expect(open).toContain("codicon-collapse-all");
    const partly = markupWithCollapsed(["/work/one"], [one, two], { codeWorkspaces: true });
    expect(partly).toContain('aria-label="Collapse all folders"');
    const shut = markupWithCollapsed(["/work/one", "/work/two"], [one, two], { codeWorkspaces: true });
    expect(shut).toContain('aria-label="Expand all folders"');
    expect(shut).toContain("codicon-expand-all");
    expect(shut.match(/aria-expanded="false"/g)?.length).toBeGreaterThanOrEqual(2);
    expect(markupWithCollapsed([], [one, two])).toContain('aria-label="Collapse all projects"');
  });

  it("stays out of the header while there is nothing to fold", () => {
    expect(sidebarMarkup([], 0)).not.toMatch(/(Collapse|Expand) all/u);
    expect(sidebarMarkup([], 0, { loading: true })).not.toMatch(/(Collapse|Expand) all/u);
  });
});

describe("local project archive state", () => {
  it("fails closed for corrupt data and keeps unique, valid project records", () => {
    expect(parseArchivedProjects("not json")).toEqual([]);
    expect(parseArchivedProjects(JSON.stringify([
      { projectPath: "/work/one", projectName: "one" },
      { projectPath: "/work/one", projectName: "duplicate" },
      { projectPath: "", projectName: "missing path" },
      { projectPath: "/work/two", projectName: "two" },
      { projectPath: "/work/three", projectName: 3 },
    ]))).toEqual([
      { projectPath: "/work/one", projectName: "one" },
      { projectPath: "/work/two", projectName: "two" },
    ]);
  });

  it("moves archived projects to the front and restores by exact path", () => {
    const starting = [
      { projectPath: "/work/one", projectName: "old name" },
      { projectPath: "/work/two", projectName: "two" },
    ];
    const archived = withArchivedProject(starting,
      { projectPath: "/work/one", projectName: "one" });
    expect(archived).toEqual([
      { projectPath: "/work/one", projectName: "one" },
      { projectPath: "/work/two", projectName: "two" },
    ]);
    expect(withoutArchivedProject(archived, "/work/one")).toEqual([
      { projectPath: "/work/two", projectName: "two" },
    ]);
  });

  it("does not open the full archived-project browser after archiving one project", () => {
    expect(source).not.toContain("setArchivedProjectsOpen(true);");
  });

  it("allows idle terminals but restores an archive when real work starts", () => {
    const archived = [
      { projectPath: "/work/one", projectName: "one" },
      { projectPath: "/work/two", projectName: "two" },
    ];
    expect(withoutProtectedArchivedProjects(archived, new Set(["/work/one"]))).toEqual([
      { projectPath: "/work/two", projectName: "two" },
    ]);
    expect(source).toMatch(
      /useEffect\(\(\) => \{[\s\S]*?withoutProtectedArchivedProjects\([\s\S]*?writeArchivedProjects\(updated\);/u,
    );
    const project = group("/work/one", "one", { total: 2, waiting: 0, working: 0 });
    project.terminals = [terminalRow("idle", { activity: "idle" })];
    expect(projectArchiveBlocked(project)).toBe(false);
    project.terminals = [terminalRow("working", { activity: "working" })];
    expect(projectArchiveBlocked(project)).toBe(true);
    project.terminals = [terminalRow("waiting", { activity: "waiting" })];
    expect(projectArchiveBlocked(project)).toBe(true);
    project.terminals = [terminalRow("exited", { state: "exited", activity: "working" })];
    expect(projectArchiveBlocked(project)).toBe(false);
    expect(projectArchiveBlocked(project, new Set(["/work/one"]))).toBe(true);
    expect(projectArchiveBlocked(
      group("/work/one", "one", { total: 1, waiting: 1, working: 0 }),
    )).toBe(true);
    expect(projectArchiveBlocked(
      group("/work/one", "one", { total: 1, waiting: 0, working: 1 }),
    )).toBe(true);
  });
});

describe("one line per row", () => {
  const busy = (): SidebarGroup[] => {
    const working: SidebarRow = {
      ...row("claude#work", "working", "Steer verb polish"),
      worktree: "steer-polish-2",
    };
    return [group("/work/interface", "Interface",
      { total: 2, waiting: 0, working: 1 },
      [working, row("claude#quiet", "idle", "Icon exploration")])];
  };

  it("keeps every row one line: status, title, and nothing else", () => {
    const markup = sidebarMarkup(busy(), 1,
      { diffStats: { "claude#work": { added: 62, removed: 11 } } });
    expect(markup).not.toContain("session-row-meta");
    expect(markup).not.toContain("session-row-lines");
    expect(markup).not.toContain("is-two-line");
    expect(markup).not.toContain('class="session-age"');
    expect(markup).not.toContain('class="engine-tag"');
    expect(markup).not.toContain("+62");
    expect(markup).not.toContain("steer-polish-2");
    // The dot still announces the state, once.
    expect(markup).toContain('aria-label="Working"');
    expect(markup).toContain(">Steer verb polish<");
    // No two-line geometry survives in the sheet.
    expect(source).not.toContain(".session-row-meta");
    expect(source).not.toContain("min-height: calc(var(--row-h) + var(--text-meta) + 4px);");
  });

  it("hands the status, the diffstat, the age and the worktree to the hover card", () => {
    // The facts moved, they were not dropped: the peek card reads the same
    // view-model the row used to spell on its second line.
    expect(source).toContain("const meta = sidebarRowMeta(peekRow, diffStats[peekRow.key]);");
    expect(source).toMatch(/<SessionPeek[\s\S]*?status=\{meta\.status\}[\s\S]*?age=\{meta\.age\}[\s\S]*?diff=\{meta\.diff\}[\s\S]*?worktree=\{meta\.worktree\}/u);
  });

  it("leaves the bounded project list exactly as it was", () => {
    const rows = Array.from({ length: 14 }, (_, i) => row(`row-${i}`, "idle"));
    rows[3] = { ...rows[3], badge: "working", status: "working" };
    const visible = visibleSidebarProjectRows(rows, false, false);
    expect(visible.rows).toHaveLength(SIDEBAR_PROJECT_ROW_LIMIT);
    expect(visible.rows.map(r => r.key)).toContain("row-3");
  });
});

describe("the memoized session row", () => {
  const props = (over: Partial<SidebarSessionRowProps> = {}): SidebarSessionRowProps => ({
    row: row("claude#a", "working", "Rewrite the parser"), limited: undefined,
    menuOpen: false, renaming: false, renameDraft: "", peeking: false, showHeaders: true,
    uid: "u", menuTriggers: { current: new Map() }, menuAnchor: { current: null },
    onOpenInNewPane: undefined,
    actions: {} as SidebarSessionRowProps["actions"],
    ...over,
  });

  it("skips a rebuilt row object whose drawn fields are unchanged", () => {
    // buildSidebarView mints every row object again on each registry change,
    // and the 1 Hz activity rounds move lastActivity on every live session.
    const before = props();
    const after = { ...before, row: { ...before.row, lastActivity: "2026-08-11T10:00:01.000Z",
      status: "streaming" as SidebarRow["status"], external: true, worktree: "wt" } };
    expect(sameSidebarSessionRow(before, after)).toBe(true);
  });

  it("redraws for every field the row draws and every other prop", () => {
    const before = props();
    for (const change of [{ title: "Renamed" }, { badge: "waiting" as const },
      { selected: true }, { engine: "codex" as SidebarRow["engine"] }, { cloud: true }]) {
      expect(sameSidebarSessionRow(before, { ...before, row: { ...before.row, ...change } }))
        .toBe(false);
    }
    for (const change of [{ limited: "Rate limited until 5pm" }, { menuOpen: true },
      { renaming: true }, { renameDraft: "Rew" }, { peeking: true }, { showHeaders: false },
      { onOpenInNewPane: noop }, { actions: {} as SidebarSessionRowProps["actions"] }]) {
      expect(sameSidebarSessionRow(before, { ...before, ...change })).toBe(false);
    }
  });

  it("is the one renderer both row lists call", () => {
    expect(source).toContain("export const SidebarSessionRow = memo(function SidebarSessionRow(");
    expect(source).toContain("}, sameSidebarSessionRow);");
    // Only the renamed row carries the draft, so a keystroke redraws one row.
    expect(source).toContain('renameDraft={renameFor === r.key ? renameDraft : ""}');
    expect(source).toContain("const rowActions = useCommittedActions<SidebarRowActions>({");
  });
});

describe("the sidebar peek", () => {
  const hovered = (): SidebarGroup[] => [group("/work/interface", "Interface",
    { total: 1, waiting: 1, working: 0 },
    [row("claude#ask", "waiting", "Qwen catalog fix")])];

  it("opens on a considered rest and is instant once one card is up", () => {
    expect(SESSION_PEEK_HOVER_MS).toBe(350);
    expect(source).toContain("onPointerEnter={event => {");
    expect(source).toContain("openPeekFor(r.key, event.currentTarget)");
    expect(source).toContain("if (peekShowing.current) {");
    expect(source).toContain("}, SESSION_PEEK_HOVER_MS);");
    // The leave is deferred, or walking to the next row would end the run and
    // charge the full delay again.
    expect(source).toContain("}, SESSION_PEEK_LEAVE_MS);");
  });

  it("gives the keyboard the same gesture, and one way out", () => {
    expect(source).toContain('if (e.key === " ") {');
    expect(source).toContain("showPeek(r.key, e.currentTarget.getBoundingClientRect())");
    expect(source).toContain('if (e.key === "Escape" && peeking)');
    expect(source).toContain('if (event.key !== "Escape") return;');
    expect(source).toContain("aria-describedby={peeking ? peekId : undefined}");
  });

  it("cannot be dismissed by a close that was armed before it opened", () => {
    // Leaving a row schedules a close 120ms out. Opening a card with Space
    // inside that window — or at any time after an unrelated pointer-leave —
    // must disarm it, or the card the user just asked for vanishes on its own.
    // clearPeekTimer has to be the FIRST statement, before any state is set.
    expect(source).toMatch(
      /const showPeek = \(key: string, anchor: DOMRect\): void => \{(?:\s*\/\/[^\n]*\n)*\s*clearPeekTimer\(\);/u);
  });

  it("drops a pending open when the list scrolls out from under its anchor", () => {
    // The rect is captured at pointerenter and spent 350ms later. A listener
    // registered only while a card is open cannot see the scroll that happens
    // during that wait, so this one is always registered and clears the timer
    // too, for every scroll that can move a row: the sidebar's own lists and
    // its ancestors. A streaming transcript elsewhere must not cancel it.
    expect(source).toMatch(
      /const onScroll = \(event: Event\): void => \{\s*const target = event\.target;\s*const inSidebar = target instanceof Node && asideRef\.current\?\.contains\(target\) === true;\s*if \(!inSidebar && !scrollMovesAnchor\(target, asideRef\.current\)\) return;\s*clearPeekTimer\(\);\s*hidePeek\(\);\s*\};\s*window\.addEventListener\("scroll", onScroll, true\);\s*return \(\) => \{[^}]*\};\s*\}, \[\]\);/u);
  });

  it("renders from state the caller already holds and asks for nothing new", () => {
    // Presentational, as the whole pane is: every fact on the card arrives as
    // a prop, so there is no path from a hover to the main process.
    expect(source).not.toMatch(/^\s*(?:const|let|void|await)?\s*window\.iface\./mu);
    expect(source).toContain("facts={peeks[peekRow.key] ?? {}}");
    expect(source).toContain("sidebarRowMeta(peekRow, diffStats[peekRow.key])");
    // And the props themselves are plain data the caller already holds.
    expect(source).toContain("peeks?: Record<string, SessionPeekFacts>;");
    expect(source).toContain("diffStats?: Record<string, DiffStat>;");
  });

  it("draws nothing until a row is actually being previewed", () => {
    const markup = sidebarMarkup(hovered(), 1, {
      peeks: { "claude#ask": { activity: "Ran the probe", approval: "Allow Bash: rm" } },
    });
    expect(markup).not.toContain('class="session-peek"');
    expect(markup).not.toContain("Ran the probe");
    expect(markup).not.toContain("aria-describedby");
  });

  it("dismisses with every other row control, so Escape means one thing", () => {
    expect(source).toMatch(/const closeRowControls[\s\S]{0,240}hidePeek\(\);/u);
  });
});


describe("One Code workspace list", () => {
  it("keeps saved workspaces and conversations in their project without appearance categories", () => {
    const clean = row("codex#clean", "idle", "Clean task");
    const html = sidebarMarkup([group("/work/interface", "Interface", undefined, [clean])], 1, {
      codeWorkspaces: true, newLabel: "New workspace", terminalsActive: true,
      terminalWorkspaces: {
        records: [{ id: "terminal", name: "Shell task", openTerminalCount: 1,
          terminalIds: ["pty"], cwd: "/work/interface" }],
        activeId: "terminal", onSelect: noop, onCreate: noop, onRename: noop,
        onDelete: noop, busy: false, canCreate: true,
      },
    });
    expect(html).not.toContain('data-workspace-kind=');
    expect(html).not.toContain('aria-label="Clean workspace"');
    expect(html).not.toContain('aria-label="Terminal workspace"');
    expect(html.match(/aria-label="New workspace"/g)).toHaveLength(1);
    expect(html).not.toContain('aria-label="New terminal workspace"');
    const ordinary = sidebarMarkup([group("/work/interface", "Interface", undefined, [clean])]);
    expect(ordinary).not.toContain('data-workspace-kind=');
    expect(ordinary).not.toContain('workspace-kind-icon"');
  });
});


describe("pinned sidebar projects", () => {
  it("lifts the selected full path once while leaving an identically named project in Folders", () => {
    const first = group("/work/first/App", "App");
    const second = group("/work/second/App", "App");
    const html = sidebarMarkup([first, second], 2, {
      codeWorkspaces: true, pinnedProjects: [second.projectPath], onPinProject: noop,
    });
    const folders = html.indexOf('>Folders<');
    expect(html.indexOf('data-sidebar-project="/work/second/App"')).toBeLessThan(folders);
    expect(html.indexOf('data-sidebar-project="/work/first/App"')).toBeGreaterThan(folders);
    expect(html.match(/data-sidebar-project="\/work\/second\/App"/g)).toHaveLength(1);
    expect(html).toContain('aria-label="Pinned project. ');
  });
  it("retains an empty pinned project after all its workspaces close, with an actionable header", () => {
    const options = { codeWorkspaces: true, pinnedProjects: ["/work/Empty"], onPinProject: noop };
    const html = sidebarMarkup([], 0, options);
    expect(html).toContain('data-sidebar-project="/work/Empty"');
    expect(html).toContain('aria-label="Project actions for Empty at /work/Empty"');
    expect(sidebarMarkup([], 0, { ...options, query: "unmatched", searching: true }))
      .not.toContain('data-sidebar-project="/work/Empty"');
  });
});
