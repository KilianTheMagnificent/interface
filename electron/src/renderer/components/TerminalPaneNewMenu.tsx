import { useState } from "react";
import { createPortal } from "react-dom";
import type { TerminalLaunchProfile, TerminalLaunchProfileId } from "../../shared/ipc";
import { homeRelativePath } from "../display-paths";
import { TerminalProfileIcon, useTerminalMenuPopover } from "./TerminalMenu";
import type { WorkspaceTool } from "./TerminalPane";
import "../terminal.css";

export interface TerminalPaneNewMenuProps {
  readonly open: boolean;
  readonly profiles: readonly TerminalLaunchProfile[];
  readonly loading?: boolean;
  readonly onOpenChange: (open: boolean) => void;
  /** Refreshes the profile list; the host owns discovery, not this menu. */
  readonly onLoad?: () => void;
  readonly onChoose: (profileId: TerminalLaunchProfileId) => void;
  readonly cwd?: string;
  readonly onOpenTool?: (tool: WorkspaceTool) => void;
  readonly onSplitDown?: () => void;
  readonly onNewTab?: () => void;
  readonly onNewSession?: () => void;
}

/**
 * The launcher rows shared by every menu that offers a choice of engines — the
 * pane's `+` and the `···` menu's Pass off page. Presentation only: the caller
 * owns what choosing one means, including closing its own menu.
 */
export function TerminalLaunchProfileList({ profiles, loading, activeIndex, onFocusIndex,
    onChoose }: {
  readonly profiles: readonly TerminalLaunchProfile[];
  readonly loading: boolean;
  /** Which row holds the menu's one tab stop, counted in this list's own order. */
  readonly activeIndex: number;
  /** Same order back out, so a host with rows above these can add its offset. */
  readonly onFocusIndex: (index: number) => void;
  readonly onChoose: (profileId: TerminalLaunchProfileId) => void;
}) {
  return (
    <>
      {profiles.length === 0 && (
        <div className="terminal-menu-empty" role="none">
          {loading ? "Loading…" : "No launchers available."}
        </div>
      )}
      {profiles.map((profile, index) => (
        <button key={profile.id} type="button" className="terminal-menu-launch" role="menuitem"
          data-terminal-launch-profile={profile.id}
          disabled={!profile.available}
          tabIndex={index === activeIndex ? 0 : -1}
          onFocus={() => { onFocusIndex(index); }}
          title={profile.available
            ? undefined : profile.unavailableReason ?? `${profile.label} is unavailable`}
          onClick={() => { onChoose(profile.id); }}>
          <TerminalProfileIcon profileId={profile.id} providerLabel={profile.label} size={16} />
          <span>{profile.label}</span>
        </button>
      ))}
    </>
  );
}

const TOOLS: readonly { id: WorkspaceTool; label: string; icon: string }[] = [
  { id: "files", label: "Files", icon: "folder" },
  { id: "browser", label: "Browser", icon: "globe" },
  { id: "ios-simulator", label: "iOS Simulator", icon: "device-mobile" },
];

/**
 * The pane header's `+`: a new pane split off this one — a shell, a workspace
 * tool or a launcher — then the pane's own Split down, New tab and New Session.
 * Only chords that exist are printed beside a row.
 */
export function TerminalPaneNewMenu({ open, profiles, loading = false, onOpenChange, onLoad,
    onChoose, cwd, onOpenTool, onSplitDown, onNewTab, onNewSession }: TerminalPaneNewMenuProps) {
  // Roving tabindex: one tab stop for the whole menu, so Tab leaves it and the
  // arrow keys move inside it.
  const [activeIndex, setActiveIndex] = useState(0);
  const { triggerRef, menuRef, box, close, onMenuKey } = useTerminalMenuPopover({
    open, onOpenChange, minWidth: 250, maxHeight: 800, placeOn: profiles,
  });
  const agents = profiles.filter(profile => profile.id !== "shell");
  const tools = onOpenTool === undefined ? [] : TOOLS;
  const agentStart = 1 + tools.length;
  let next = agentStart + agents.length;
  const slot = (index: number) => ({
    tabIndex: activeIndex === index ? 0 : -1,
    onFocus: () => { setActiveIndex(index); },
  });
  const row = (label: string, icon: string, onClick: () => void, shortcut?: [string, string]) => (
    <button type="button" className="terminal-menu-launch" role="menuitem" {...slot(next++)}
      aria-keyshortcuts={shortcut?.[1]} onClick={() => { close(false); onClick(); }}>
      <span className={`codicon codicon-${icon}`} aria-hidden="true" /><span>{label}</span>
      {shortcut && <span className="terminal-menu-shortcut" aria-hidden="true">{shortcut[0]}</span>}
    </button>
  );

  const panel = open ? (
    <div ref={menuRef} className="terminal-menu-panel is-compact is-floating terminal-pane-new-menu"
      role="menu" aria-label="Open in a new pane" onKeyDown={onMenuKey}
      style={box === null
        ? { visibility: "hidden", top: 0, left: 0 }
        : { top: box.top, left: box.left, width: box.width, maxHeight: box.maxHeight }}>
      <div className="terminal-pane-menu-heading" role="presentation">
        <strong>New pane</strong>{cwd && <span title={cwd}>{homeRelativePath(cwd)}</span>}
      </div>
      <button type="button" className="terminal-menu-launch" role="menuitem" {...slot(0)}
        aria-keyshortcuts="Meta+D"
        disabled={profiles.find(profile => profile.id === "shell")?.available !== true}
        onClick={() => { close(false); onChoose("shell"); }}>
        <span className="codicon codicon-terminal" aria-hidden="true" /><span>Terminal</span>
        <span className="terminal-menu-shortcut" aria-hidden="true">⌘D</span>
      </button>
      {tools.map((tool, index) => <button key={tool.id} type="button"
        className="terminal-menu-launch" role="menuitem" {...slot(index + 1)}
        onClick={() => { close(false); triggerRef.current?.focus(); onOpenTool?.(tool.id); }}>
        <span className={`codicon codicon-${tool.icon}`} aria-hidden="true" /><span>{tool.label}</span>
      </button>)}
      <div className="terminal-pane-menu-divider" role="separator" />
      <TerminalLaunchProfileList profiles={agents} loading={loading}
        activeIndex={activeIndex - agentStart}
        onFocusIndex={index => { setActiveIndex(index + agentStart); }}
        onChoose={profileId => { close(false); onChoose(profileId); }} />
      {(onSplitDown ?? onNewTab ?? onNewSession) && <div className="terminal-pane-menu-divider" role="separator" />}
      {onSplitDown && row("Split down", "split-vertical", onSplitDown,
        ["⌘⇧D", "Meta+Shift+D"])}
      {onNewTab && row("New tab", "add", onNewTab)}
      {onNewSession && row("New session…", "folder-opened", onNewSession)}
    </div>
  ) : null;

  return (
    <>
      <button ref={triggerRef} type="button" className="terminal-pane-action"
        aria-label="New terminal in this pane" title="New pane"
        aria-haspopup="menu" aria-expanded={open}
        onClick={() => {
          if (!open) onLoad?.();
          onOpenChange(!open);
        }}>
        <span className="codicon codicon-sm codicon-add" aria-hidden="true" />
      </button>
      {panel !== null && (typeof document === "undefined" ? panel : createPortal(panel, document.body))}
    </>
  );
}
