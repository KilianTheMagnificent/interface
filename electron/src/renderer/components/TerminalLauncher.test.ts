import { readFileSync } from "node:fs";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { TerminalLaunchProfile } from "../../shared/ipc";
import {
  TERMINAL_LAUNCH_COUNTS,
  TerminalLauncher,
  terminalLauncherActionLabel, terminalLauncherAdversarialPair,
  terminalLauncherCanTakeFocus,
  terminalLauncherChosenCwd,
  terminalLauncherCountForKey,
  terminalLauncherDefaultCwd,
  terminalLauncherFolds,
  terminalLauncherInstallHint,
  terminalLauncherIsLaunchKey,
  terminalLauncherIsolationNote,
  terminalLauncherPickedDirectory,
  terminalLauncherProfileForKey,
} from "./TerminalLauncher";
import {
  adversarialReviewLineup, AdversarialReviewDialog,
} from "./AdversarialReviewDialog";
import { terminalLauncherShowing } from "./TerminalWorkspace";
import {
  EMPTY_TERMINAL_WORKSPACE_STATE, openTerminalTab,
} from "../terminal-workspace";

const profiles: TerminalLaunchProfile[] = [
  { id: "shell", label: "Terminal", available: true },
  { id: "claude", engine: "claude", label: "Claude Code", available: true },
  { id: "codex", engine: "codex", label: "Codex", available: true },
  { id: "grok", engine: "acp:grok", label: "Grok", available: true },
  { id: "omp", engine: "acp:omp", label: "OMP", available: true },
  { id: "opencode", engine: "acp:opencode", label: "OpenCode", available: true },
  { id: "gemini", engine: "acp:gemini", label: "Antigravity", available: false,
    unavailableReason: "Antigravity CLI was not found." },
];
const terminalCss = readFileSync(new URL("../terminal.css", import.meta.url), "utf8");
const codiconCss = readFileSync(new URL("../codicons.css", import.meta.url), "utf8");
const launcherSource = readFileSync(new URL("./TerminalLauncher.tsx", import.meta.url), "utf8");
const workspaceSource = readFileSync(new URL("./TerminalWorkspace.tsx", import.meta.url), "utf8");
const reviewDialogSource = readFileSync(
  new URL("./AdversarialReviewDialog.tsx", import.meta.url), "utf8");
const centerSource = readFileSync(
  new URL("./IntegratedTerminalCenter.tsx", import.meta.url), "utf8");

describe("TerminalLauncher choices", () => {
  it("offers Muse Code with its own mark beside the other agents", () => {
    const html = renderToStaticMarkup(createElement(TerminalLauncher, {
      profiles: [...profiles, { id: "muse", engine: "acp:muse", label: "Muse Code", available: true }],
      profilesLoading: false,
      cwd: "/work/interface",
      launching: false,
      notice: null,
      onLaunch: () => undefined,
      onBrowse: async () => null,
      onRetryProfiles: () => undefined,
    }));
    expect(html).toContain('data-terminal-launch-profile="muse" data-provider="muse"');
    expect(html).toContain('data-provider-logo="muse"');
    expect(html).toContain("Muse Code");
  });

  it("offers Kimi Code with its own mark beside the other agents", () => {
    const html = renderToStaticMarkup(createElement(TerminalLauncher, {
      profiles: [...profiles, { id: "kimi", engine: "acp:kimi", label: "Kimi Code", available: true }],
      profilesLoading: false,
      cwd: "/work/interface",
      launching: false,
      notice: null,
      onLaunch: () => undefined,
      onBrowse: async () => null,
      onRetryProfiles: () => undefined,
    }));
    expect(html).toContain('data-terminal-launch-profile="kimi" data-provider="kimi"');
    expect(html).toContain('data-provider-logo="kimi"');
    expect(html).toContain("Kimi Code");
  });

  it("renders only the owner's launch roster with compact provider rows", () => {
    const html = renderToStaticMarkup(createElement(TerminalLauncher, {
      profiles,
      profilesLoading: false,
      cwd: "/work/interface",
      launching: false,
      notice: null,
      onLaunch: () => undefined,
      onBrowse: async () => null,
      onRetryProfiles: () => undefined,
    }));
    expect(html).toContain('aria-labelledby="');
    expect(html).toContain('role="radiogroup"');
    expect(html).toContain('data-terminal-launch-profile="claude"');
    expect(html).toContain('data-terminal-launch-profile="claude" data-provider="claude"');
    expect(html).toContain('data-terminal-launch-profile="codex" data-provider="codex"');
    expect(html).toContain('data-terminal-launch-profile="grok" data-provider="grok"');
    // Not installed and not one of Claude Code, Codex or Terminal: folded
    // under More with its reason, not a tile (review C4).
    expect(html).not.toContain('data-terminal-launch-profile="gemini"');
    expect(html).toContain('data-terminal-launch-folded="gemini"');
    expect(html).toContain('data-provider-logo="claude"');
    expect(html).toContain('data-provider-logo="codex"');
    expect(html).toContain('data-provider-logo="grok"');
    expect(html).toContain('data-terminal-launch-profile="omp" data-provider="omp"');
    expect(html).toContain('data-provider-logo="omp"');
    expect(html).toContain('data-terminal-launch-profile="opencode" data-provider="opencode"');
    expect(html).toContain('data-provider-logo="opencode"');
    expect(html).toContain('data-provider-logo="antigravity"');
    expect(html).not.toContain('data-terminal-launch-profile="qwen"');
    expect(html).not.toContain('data-provider-logo="qwen"');
    expect(html).toContain("Antigravity CLI was not found.");
    expect(html).toContain('class="terminal-launcher-profile is-shell"');
    expect(html).not.toMatch(/data-terminal-launch-profile="shell"[^>]*data-provider=/u);
    expect(html).toContain('class="terminal-launcher-profile-check codicon codicon-check"');
    expect(html.match(/data-terminal-launch-profile=/g)).toHaveLength(6);
    expect(html).toContain('title="/work/interface"');
    expect(html).toContain('aria-label="Change working folder"');
    expect(html).toContain("Change…");
    expect(html).toContain("New workspace");
    // The reference lineup names each session, while the action retains its exact launch label.
    expect(html).toContain("Launch 1 Claude Code terminal");
    expect(html).not.toContain("1 × Claude Code");
    expect(html).not.toContain("terminal-launcher-summary");
    expect(html).not.toContain("terminal-launcher-actions-summary");
    expect(html).toContain("Will launch");
    expect(html).toContain("Isolation");
    // One worktree vocabulary: Local checkout / New worktree (review C2).
    expect(html).toContain("Local checkout");
    expect(html).not.toContain("Shared folder");
    // Refine 1 (owner, 2026-10-01): no intro line, a visible setup state, one
    // vocabulary for what launches, and the folder named before its path.
    expect(html).not.toContain("Pick an agent and give it room to work.");
    // The visible setup state belongs to the tiles that never fold.
    expect(html).not.toContain("is-unavailable");
    expect(html).not.toContain("Needs setup");
    expect(html).toContain("The terminal works in the folder below.");
    expect(html).not.toMatch(/\bsessions?\b/u);
    expect(html).toContain('<span class="terminal-launcher-cwd-name">interface</span>');
    expect(html).toContain('<span class="terminal-launcher-lineup-folder">in interface</span>');
    expect(html).toContain('<kbd class="terminal-launcher-shortcut" aria-hidden="true">⌘↵</kbd>');
    expect(html).toContain('aria-keyshortcuts="Meta+Enter Control+Enter"');
  });

  it("folds tools that are not installed under More, each with its install hint", () => {
    // The shared Rust fixture: Codex and OMP are missing, each with Rust's hint.
    const fixture = JSON.parse(readFileSync(
      new URL("../../fixtures/terminal-tools-v43.json", import.meta.url), "utf8")) as {
      profiles: TerminalLaunchProfile[];
    };
    const roster: TerminalLaunchProfile[] = [...fixture.profiles,
      { id: "grok", engine: "acp:grok", label: "Grok", available: true },
      { id: "opencode", engine: "acp:opencode", label: "OpenCode", available: false,
        unavailableReason: "OpenCode CLI was not found." }];
    const { tiles, folded } = terminalLauncherFolds(roster);
    // Claude Code, Codex and Terminal stay tiles even when missing.
    expect(tiles.map(profile => profile.id)).toEqual(["claude", "codex", "grok", "shell"]);
    expect(folded.map(profile => profile.id)).toEqual(["omp", "opencode"]);
    expect(terminalLauncherInstallHint(folded[0]!))
      .toBe("Install OMP so `omp` runs in your login shell, then press Refresh.");
    expect(terminalLauncherInstallHint(folded[1]!)).toBe("OpenCode CLI was not found.");
    expect(terminalLauncherInstallHint({ id: "muse", engine: "acp:muse", label: "Muse Code",
      available: false })).toBe("Install Muse Code, then press Refresh.");
    const html = renderToStaticMarkup(createElement(TerminalLauncher, {
      profiles: roster, profilesLoading: false, cwd: "/work/interface", launching: false,
      notice: null, onLaunch: () => undefined, onBrowse: async () => null,
      onRetryProfiles: () => undefined,
    }));
    expect(html.match(/data-terminal-launch-profile=/g)).toHaveLength(4);
    // A missing Codex keeps its tile and its visible setup state.
    expect(html).toMatch(
      /class="terminal-launcher-profile is-unavailable"[^>]*data-terminal-launch-profile="codex"/u);
    expect(html.match(/Needs setup/g)).toHaveLength(1);
    // One closed disclosure below the grid, outside the radio group.
    expect(html).toContain('<details class="terminal-launcher-more">');
    expect(html).not.toMatch(/<details[^>]* open/u);
    expect(html).toContain("More agents");
    expect(html).toContain("2 not installed");
    expect(html).toMatch(/<\/div><\/div><details class="terminal-launcher-more">/u);
    expect(html).toContain('data-terminal-launch-folded="omp"');
    expect(html).toContain('data-terminal-launch-folded="opencode"');
    expect(html).toContain("Install OMP so <code>omp</code> runs in your login shell, then press Refresh.");
    expect(html).toContain("OpenCode CLI was not found.");
    expect(html).toMatch(/class="btn terminal-launcher-more-refresh">[\s\S]*?Refresh<\/button>/u);
    // Keyboard movement walks the runnable tiles only.
    expect(terminalLauncherProfileForKey(roster, "claude", "ArrowRight")).toBe("grok");
    expect(terminalLauncherProfileForKey(roster, "grok", "ArrowRight")).toBe("shell");
    // Everything installed: nothing folds and no More appears.
    const installed = renderToStaticMarkup(createElement(TerminalLauncher, {
      profiles: roster.map(profile => ({ id: profile.id, label: profile.label,
        ...(profile.engine === undefined ? {} : { engine: profile.engine }), available: true })),
      profilesLoading: false, cwd: "/work/interface", launching: false, notice: null,
      onLaunch: () => undefined, onBrowse: async () => null, onRetryProfiles: () => undefined,
    }));
    expect(installed).not.toContain("terminal-launcher-more");
    expect(installed.match(/data-terminal-launch-profile=/g)).toHaveLength(6);
  });

  it("shows a home folder the way a terminal prints it, keeping the absolute path", () => {
    const html = renderToStaticMarkup(createElement(TerminalLauncher, {
      profiles,
      profilesLoading: false,
      cwd: "/Users/someone/Developer/Interface-Rust",
      launching: false,
      notice: null,
      onLaunch: () => undefined,
      onBrowse: async () => null,
      onRetryProfiles: () => undefined,
    }));
    expect(html).toContain('<span class="terminal-launcher-cwd-name">Interface-Rust</span>');
    expect(html).toMatch(
      /class="terminal-launcher-cwd-path" title="\/Users\/someone\/Developer\/Interface-Rust">~\/Developer\/Interface-Rust</u);
  });

  it("counts the shared folder note and launches on ⌘↵ only", () => {
    expect(terminalLauncherIsolationNote(1)).toBe("The terminal works in the folder below.");
    expect(terminalLauncherIsolationNote(6)).toBe("All 6 terminals work in the folder below.");
    expect(terminalLauncherIsLaunchKey({ key: "Enter", metaKey: true, ctrlKey: false })).toBe(true);
    expect(terminalLauncherIsLaunchKey({ key: "Enter", metaKey: false, ctrlKey: true })).toBe(true);
    expect(terminalLauncherIsLaunchKey({ key: "Enter", metaKey: false, ctrlKey: false })).toBe(false);
    expect(terminalLauncherIsLaunchKey({ key: "k", metaKey: true, ctrlKey: false })).toBe(false);
  });

  it("offers review as one link, never as a second meaning for the count row", () => {
    const html = renderToStaticMarkup(createElement(TerminalLauncher, {
      profiles,
      profilesLoading: false,
      cwd: "/work/interface",
      launching: false,
      notice: null,
      onLaunch: () => undefined,
      onOpenReview: () => undefined,
      onBrowse: async () => null,
      onRetryProfiles: () => undefined,
    }));
    // Clicking a number used to switch the mode back to independent SILENTLY,
    // discarding a typed objective. There is no mode left to discard.
    expect(html).not.toContain("data-terminal-launch-mode");
    expect(html).not.toContain("Session type");
    expect(html).not.toContain("Independent terminals");
    expect(html).not.toContain("What should the builder complete before review?");
    expect(html).toContain("Builder and reviewer instead");
    expect(html).toContain('title="Start an adversarial review: one agent builds, another reviews"');
    expect(html).toContain('class="terminal-launcher-review-link"');
    expect(html).not.toContain("data-terminal-launch-preset");
  });

  it("shows no review link at all where the workflow is not connected", () => {
    // A visible-but-refusing entry point explained itself only after you
    // pressed it. Without the host workflow there is nothing to link to.
    const html = renderToStaticMarkup(createElement(TerminalLauncher, {
      profiles,
      profilesLoading: false,
      cwd: "/work/interface",
      launching: false,
      notice: null,
      onLaunch: () => undefined,
      onBrowse: async () => null,
      onRetryProfiles: () => undefined,
    }));
    expect(html).not.toContain("terminal-launcher-review-link");
    expect(html).not.toContain("adversarial review");
    expect(html).not.toContain("Adversarial review is not connected in this build.");
  });

  it("keeps color semantic across selection and provider identity", () => {
    expect(terminalCss).toMatch(
      /\.terminal-launcher-count\.is-selected \{[\s\S]*?color: var\(--selection-accent\);[\s\S]*?border-color: var\(--selection-accent\);/u,
    );
    expect(terminalCss).toMatch(
      /\.terminal-launcher-profile \{[\s\S]*?background: var\(--surface-raised\);/u,
    );
    expect(terminalCss).toMatch(
      /\.terminal-launcher-profile\.is-selected \{[\s\S]*?border-color: var\(--selection-accent\);/u,
    );
    expect(terminalCss).not.toContain("var(--provider-accent)");
    expect(reviewDialogSource).toContain(
      ".adversarial-review-lineup > .adversarial-review-icon { color:var(--review-accent); }");
  });

  it("names a fixed cross-provider lineup and blocks a half-installed pair", () => {
    expect(adversarialReviewLineup(profiles))
      .toMatchObject({ builder: { id: "claude" }, reviewer: { id: "codex" } });
    // Both providers or neither: each is the other's reviewer, so losing either
    // one leaves no lineup and the dialog says so instead of naming a builder
    // that is not installed. There is no "swap to the other builder" case.
    const withoutClaude = profiles.map(profile =>
      profile.id === "claude" ? { ...profile, available: false } : profile);
    expect(adversarialReviewLineup(withoutClaude)).toBeNull();
    expect(adversarialReviewLineup(profiles.filter(profile => profile.id !== "claude")))
      .toBeNull();
    expect(adversarialReviewLineup(profiles.map(profile =>
      profile.id === "codex" ? { ...profile, available: false } : profile))).toBeNull();
    const html = renderToStaticMarkup(createElement(AdversarialReviewDialog, {
      open: true, profiles, cwd: "/work/interface", launching: false, notice: null,
      onStart: () => undefined, onCancel: () => undefined,
    }));
    expect(html).toContain("Adversarial review");
    // F2's finding, said before the click rather than discovered after it.
    expect(html).toContain("Opens two agent chat panes on the Threads surface.");
    expect(html).toContain("Claude Code builds, then Codex reviews");
    expect(html).toContain('data-adversarial-review-icon="opposed-verdict"');
    expect(html).toContain("What should the builder complete before review?");
    expect(html).toContain("/work/interface");
    expect(html).toContain("Start review");
    expect(html).toContain('class="adversarial-review-dialog is-floating"');
    expect(html).toContain('role="dialog"');
    expect(html).toContain('aria-modal="true"');
    // Nothing to start yet: no objective, so the submit is off and says why.
    expect(html).toMatch(/class="adversarial-review-button adversarial-review-submit" disabled=""/u);
    const blocked = renderToStaticMarkup(createElement(AdversarialReviewDialog, {
      open: true, profiles: profiles.filter(profile => profile.id !== "codex"),
      cwd: "/work/interface", launching: false, notice: null,
      onStart: () => undefined, onCancel: () => undefined,
    }));
    expect(blocked).toContain("Adversarial review needs both Claude Code and Codex.");
    const noFolder = renderToStaticMarkup(createElement(AdversarialReviewDialog, {
      open: true, profiles, cwd: null, launching: false, notice: null,
      onStart: () => undefined, onCancel: () => undefined,
    }));
    expect(noFolder).toContain("Choose a working folder in the launcher before starting review.");
    expect(renderToStaticMarkup(createElement(AdversarialReviewDialog, {
      open: false, profiles, cwd: "/work/interface", launching: false, notice: null,
      onStart: () => undefined, onCancel: () => undefined,
    }))).toBe("");
  });

  it("does not throw away a typed objective on a backdrop press", () => {
    // The dialog is a textarea in a small box: a press that lands beside it is
    // a miss far more often than a dismissal. Escape and Cancel stay the ways
    // out, and both work with an objective typed.
    expect(reviewDialogSource).toContain(
      "if (!launching && trimmed === \"\" && event.target === event.currentTarget) onCancel();");
    expect(reviewDialogSource).toContain("onEscape: launching ? () => undefined : onCancel,");
    expect(reviewDialogSource).toContain(
      "<button type=\"button\" className=\"adversarial-review-button\" disabled={launching}");
  });

  it("uses only glyph names the bundled codicon sheet actually declares", () => {
    const names = new Set([...launcherSource.matchAll(/\bcodicon-([a-z0-9-]+)/gu)]
      .map(([, name]) => name));
    expect(names.size).toBeGreaterThan(0);
    for (const name of names) {
      expect(codiconCss, `missing .codicon-${name}`).toContain(`.codicon-${name}::before`);
    }
  });

  it("renders exactly the simple count buttons 1 through 8", () => {
    const html = renderToStaticMarkup(createElement(TerminalLauncher, {
      profiles,
      profilesLoading: false,
      cwd: "/work/interface",
      launching: false,
      notice: null,
      onLaunch: () => undefined,
      onBrowse: async () => null,
      onRetryProfiles: () => undefined,
    }));
    expect(TERMINAL_LAUNCH_COUNTS).toEqual([1, 2, 3, 4, 5, 6, 7, 8]);
    expect(html.match(/class="terminal-launcher-count(?: is-selected)?"/g)).toHaveLength(8);
    for (const count of TERMINAL_LAUNCH_COUNTS) {
      expect(html).toContain(`aria-label="${count.toString()} terminal${count === 1 ? "" : "s"}"`);
    }
  });

  it("locks every interactive launcher control while a batch is in flight", () => {
    const html = renderToStaticMarkup(createElement(TerminalLauncher, {
      profiles,
      profilesLoading: false,
      cwd: null,
      launching: true,
      notice: { kind: "partial", message: "3 of 4 Claude terminals started. No retry." },
      onLaunch: () => undefined,
      onBrowse: async () => null,
      onRetryProfiles: () => undefined,
      onCancel: () => undefined,
    }));
    expect(html).toContain('aria-busy="true"');
    expect(html).toContain("Launching…");
    expect(html).toContain("Choose a working folder");
    expect(html).toContain('class="terminal-launcher-notice is-partial" role="status"');
    expect(html).toContain("3 of 4 Claude terminals started. No retry.");
    expect(html).toContain('aria-label="Back to terminal workspace"');
    expect(html.match(/data-terminal-launch-profile="[^"]+"[^>]*disabled=""/gu))
      .toHaveLength(6);
    expect(html).toMatch(/class="btn terminal-launcher-more-refresh" disabled=""/u);
    expect(html.match(/class="terminal-launcher-count[^>]*disabled=""/gu))
      .toHaveLength(8);
    expect(html).toMatch(/class="terminal-launcher-close"[^>]*disabled=""/u);
    expect(html).toMatch(/class="terminal-launcher-cwd-browse"[^>]*disabled=""/u);
    expect(html).toMatch(/class="btn btn-primary" disabled=""/u);
  });

  it("takes focus once and closes on Escape, like every other overlay here", () => {
    // Static rendering has no live DOM, so the contract is read off the source:
    // one guarded focus of the opening card, and Escape routed to the same
    // onCancel the exit button uses — never a focus trap, because the toolbar
    // above the launcher stays live.
    expect(launcherSource).toContain("const focusedForOpen = useRef(false);");
    expect(launcherSource).toContain("focusedForOpen.current = true;");
    expect(launcherSource).toContain("card?.focus();");
    expect(launcherSource).toMatch(
      /if \(event\.key !== "Escape" \|\| onCancel === undefined \|\| launching\) return;[\s\S]*?onCancel\(\);/u,
    );
    // An empty workspace has nothing to go back to: onClose only lowers
    // `launcher.open`, which showLauncher overrides while there are no panes.
    expect(workspaceSource).toContain(
      "onCancel={state.panes.length > 0 ? launcher.onClose : undefined}");
  });

  it("keeps the native folder chooser available when no project is selected", () => {
    const html = renderToStaticMarkup(createElement(TerminalLauncher, {
      profiles,
      profilesLoading: false,
      cwd: null,
      launching: false,
      notice: null,
      onLaunch: () => undefined,
      onBrowse: async () => "/work/chosen",
      onRetryProfiles: () => undefined,
    }));
    expect(html).toContain('aria-label="Choose working folder"');
    expect(html).toContain("Choose…");
    expect(html).toMatch(/class="terminal-launcher-cwd-browse"[^>]*>/u);
    expect(html).toMatch(/class="btn btn-primary" disabled=""/u);
  });
});

describe("TerminalLauncher keyboard and copy helpers", () => {
  it("keeps Rust's roster stable and skips unavailable profiles for keyboard movement", () => {
    expect(profiles.map(profile => profile.id))
      .toEqual(["shell", "claude", "codex", "grok", "omp", "opencode", "gemini"]);
    expect(terminalLauncherProfileForKey(profiles, "claude", "ArrowRight")).toBe("codex");
    expect(terminalLauncherProfileForKey(profiles, "codex", "ArrowRight")).toBe("grok");
    expect(terminalLauncherProfileForKey(profiles, "grok", "ArrowRight")).toBe("omp");
    expect(terminalLauncherProfileForKey(profiles, "omp", "ArrowRight")).toBe("opencode");
    expect(terminalLauncherProfileForKey(profiles, "opencode", "ArrowRight")).toBe("shell");
    expect(terminalLauncherProfileForKey(profiles, "shell", "ArrowRight")).toBe("claude");
    expect(terminalLauncherProfileForKey(profiles, "claude", "ArrowLeft")).toBe("shell");
    expect(terminalLauncherProfileForKey(profiles, "gemini", "Home")).toBe("claude");
    expect(terminalLauncherProfileForKey(profiles, "claude", "Escape")).toBeNull();
  });

  it("wraps the 1 through 8 count radio group", () => {
    expect(terminalLauncherCountForKey(1, "ArrowLeft")).toBe(8);
    expect(terminalLauncherCountForKey(8, "ArrowRight")).toBe(1);
    expect(terminalLauncherCountForKey(5, "Home")).toBe(1);
    expect(terminalLauncherCountForKey(5, "End")).toBe(8);
    expect(terminalLauncherCountForKey(5, "Enter")).toBeNull();
  });

  it("uses dynamic count, provider, shell, and plural copy", () => {
    expect(terminalLauncherActionLabel(profiles[1], 1)).toBe("Launch 1 Claude Code terminal");
    expect(terminalLauncherActionLabel(profiles[2], 6)).toBe("Launch 6 Codex terminals");
    expect(terminalLauncherActionLabel(profiles[0], 2)).toBe("Launch 2 shell terminals");
    expect(terminalLauncherActionLabel(null, 4)).toBe("Choose an agent");
  });

  it("pairs Codex and Claude in opposite builder/reviewer roles only when both are available", () => {
    expect(terminalLauncherAdversarialPair(profiles, "claude"))
      .toMatchObject({ builder: { id: "claude" }, reviewer: { id: "codex" } });
    expect(terminalLauncherAdversarialPair(profiles, "codex"))
      .toMatchObject({ builder: { id: "codex" }, reviewer: { id: "claude" } });
    expect(terminalLauncherAdversarialPair(profiles, "shell")).toBeNull();
    expect(terminalLauncherAdversarialPair(
      profiles.map(profile => profile.id === "codex" ? { ...profile, available: false } : profile),
      "claude",
    )).toBeNull();
  });

  it("opens on the agent and count the last launch used", () => {
    const html = renderToStaticMarkup(createElement(TerminalLauncher, {
      profiles,
      profilesLoading: false,
      cwd: "/work/interface",
      launching: false,
      notice: null,
      defaultProfileId: "codex" as const,
      defaultCount: 3,
      onLaunch: () => undefined,
      onBrowse: async () => null,
      onRetryProfiles: () => undefined,
    }));
    expect(html).toContain("Launch 3 Codex terminals");
    expect(html).toMatch(
      /class="terminal-launcher-profile is-selected"[^>]*data-terminal-launch-profile="codex"/u);
    expect(html).toMatch(/class="terminal-launcher-count is-selected"[^>]*>3</u);
    // The launcher unmounts on close, so the memory lives with the folder, in
    // the surface that outlives it.
    expect(centerSource).toContain("setLastProfileId(profileId);");
    expect(centerSource).toContain("setLastCount(count);");
    expect(centerSource).toContain("defaultProfileId: lastProfileId,");
    expect(centerSource).toContain("defaultCount: lastCount,");
  });

  it("launches review in the folder the launcher shows, not the focused pane's", () => {
    // `terminalLauncherDefaultCwd` prefers the FOCUSED terminal's folder, so a
    // folder chosen with Change… reached the launcher's own state and nothing
    // else: with a pane open in /a you could browse to /b, read /b in the
    // launcher, open the review link and start a review in /a.
    expect(terminalLauncherChosenCwd("/work/browsed", "/work/focused"))
      .toBe("/work/browsed");
    expect(terminalLauncherChosenCwd(null, "/work/focused")).toBe("/work/focused");
    expect(terminalLauncherChosenCwd("   ", "/work/focused")).toBe("/work/focused");
    expect(terminalLauncherChosenCwd(null, null)).toBeNull();
    // One value, read by the launcher and the dialog alike, owned by the centre
    // because the launcher unmounts on close.
    expect(centerSource).toContain(
      "const launcherCwd = terminalLauncherChosenCwd(browsedCwd, contextualCwd);");
    expect(centerSource).toContain("cwd: launcherCwd,");
    expect(centerSource).toContain("onCwdChange: setBrowsedCwd,");
    expect(centerSource).toMatch(/<AdversarialReviewDialog[\s\S]*?cwd=\{launcherCwd\}/u);
    expect(centerSource).toContain("useEffect(() => { setBrowsedCwd(null); }, [contextualCwd]);");
    expect(workspaceSource).toContain("cwd={launcher.cwd}");
    expect(workspaceSource).toContain("onCwdChange={launcher.onCwdChange}");
    expect(launcherSource).toContain("if (chosen !== workingDirectory) onCwdChange?.(chosen);");
    // And the picker opens where the launcher is, not where the pane is.
    expect(centerSource).toContain("preferredPath || launcherCwd || undefined,");
    // Handed the same folder, both surfaces print it.
    const dialog = renderToStaticMarkup(createElement(AdversarialReviewDialog, {
      open: true, profiles, cwd: "/work/browsed", launching: false, notice: null,
      onStart: () => undefined, onCancel: () => undefined,
    }));
    const launcher = renderToStaticMarkup(createElement(TerminalLauncher, {
      profiles, profilesLoading: false, cwd: "/work/browsed", launching: false,
      notice: null, onLaunch: () => undefined, onBrowse: async () => null,
      onRetryProfiles: () => undefined,
    }));
    expect(dialog).toContain("/work/browsed");
    expect(launcher).toContain("/work/browsed");
  });

  it("forgets a browsed folder when the launcher or review session ends", () => {
    // The launcher used to unmount on close and take its folder with it. Now
    // the folder lives in a component that NEVER unmounts, and the effect keyed
    // on contextualCwd cannot see the boundary: a focused terminal holds that
    // value byte-identical, so browsing to /project-B from a pane in
    // /project-A and closing left /project-B waiting for the next, unrelated
    // launcher. Every boundary says so by hand.
    const boundaries = [
      // Open: a new session starts on the contextual folder, not the last one.
      "    setLauncherOpen(true);\n    setLauncherNotice(null);\n    setBrowsedCwd(null);\n",
      // Close → reopen with the same terminal focused: back to its folder.
      "          if (launchPending.current) return;\n"
      + "          setLauncherOpen(false);\n          setBrowsedCwd(null);\n",
      // A launch consumed the folder; the next session starts fresh.
      "        setLauncherOpen(false);\n        setBrowsedCwd(null);\n",
      // A started review, likewise.
      "      setReviewOpen(false);\n      setLauncherOpen(false);\n      setBrowsedCwd(null);\n",
      // A saved-workspace switch is a session boundary too.
      '    setWorkspaceView("split");\n    setLauncherOpen(false);\n    setBrowsedCwd(null);\n',
      // A project-menu launch replaces the folder left in the previous form.
      "    adoptCwd(projectPath, false);\n    setBrowsedCwd(null);\n",
      // Cancel → reopen for the dialog, but ONLY when the dialog was the
      // session: over a still-open launcher it would pull the folder out from
      // under a form the user is still looking at.
      "        if (!terminalLauncherShowing(launcherOpen, layout)) setBrowsedCwd(null);",
      // Going to an agent from the overview uncovers its pane.
      "    if (row.workspaceId !== activeWorkspaceIdRef.current) switchWorkspace(row.workspaceId);\n"
        + "    setLauncherOpen(false);\n    setBrowsedCwd(null);\n",
    ];
    for (const boundary of boundaries) expect(centerSource).toContain(boundary);
    // Eight boundaries plus the contextual-move effect, and nothing else touches
    // it. A ninth way to end a session has to come here and say so.
    expect(centerSource.match(/setBrowsedCwd\(null\)/gu)).toHaveLength(9);
    // The guard the dialog's cancel reads, as its own decision.
    const empty = EMPTY_TERMINAL_WORKSPACE_STATE;
    const filled = openTerminalTab(empty, "pane-a", "terminal-a");
    expect(terminalLauncherShowing(true, filled)).toBe(true);
    expect(terminalLauncherShowing(false, filled)).toBe(false);
    // An empty workspace forces it open, so the dialog is never the whole session.
    expect(terminalLauncherShowing(false, empty)).toBe(true);
    // And the contract the contextual folder is still resolved by is unchanged:
    // a focused terminal outranks the last folder the user picked.
    expect(terminalLauncherDefaultCwd("/work/focused", "/work/last", "/work/fallback"))
      .toBe("/work/focused");
  });

  it("waits for a card that can actually take focus before burning the guard", () => {
    // The terminal centre is mounted from app start behind `display:none`, and
    // an empty workspace forces the launcher open inside it. A one-shot guard
    // that burned on that hidden render meant the launcher never took focus at
    // all — the exact case the focus was added for.
    expect(terminalLauncherCanTakeFocus({ isConnected: true, offsetParent: {} })).toBe(true);
    // display:none anywhere above it: no layout box, no offsetParent.
    expect(terminalLauncherCanTakeFocus({ isConnected: true, offsetParent: null })).toBe(false);
    expect(terminalLauncherCanTakeFocus({ isConnected: false, offsetParent: {} })).toBe(false);
    expect(terminalLauncherCanTakeFocus(null)).toBe(false);
    expect(terminalLauncherCanTakeFocus(undefined)).toBe(false);
    // The surface becoming visible changes no other prop the effect reads, so
    // it is the retry trigger and has to be a dependency.
    expect(launcherSource).toContain(
      "if (focusedForOpen.current || !visible || rovingProfileId === null) return;");
    expect(launcherSource).toContain("if (!terminalLauncherCanTakeFocus(card)) return;");
    expect(launcherSource).toContain("}, [choices, rovingProfileId, visible]);");
    expect(centerSource).toContain("visible: visible && !interactionBlocked,");
    expect(workspaceSource).toContain("visible={launcher.visible}");
  });

  it("keeps the current folder on cancel and preserves an exact picked path", () => {
    expect(terminalLauncherPickedDirectory("/work/current", null)).toBe("/work/current");
    expect(terminalLauncherPickedDirectory("/work/current", "   ")).toBe("/work/current");
    expect(terminalLauncherPickedDirectory("/work/current", "/work/folder "))
      .toBe("/work/folder ");
  });

  it("defaults an empty workspace to the last selected directory", () => {
    expect(terminalLauncherDefaultCwd("/work/focused", "/work/last", "/work/fallback"))
      .toBe("/work/focused");
    expect(terminalLauncherDefaultCwd(null, "/work/last", "/work/fallback"))
      .toBe("/work/last");
    expect(terminalLauncherDefaultCwd("  ", "/work/last", "/work/fallback"))
      .toBe("/work/last");
    expect(terminalLauncherDefaultCwd(undefined, null, "/work/fallback"))
      .toBe("/work/fallback");
    expect(terminalLauncherDefaultCwd(undefined, undefined, undefined)).toBeNull();
  });
});

describe("Terminal launcher integration contract", () => {
  const center = readFileSync(new URL("./IntegratedTerminalCenter.tsx", import.meta.url), "utf8");
  const workspace = readFileSync(new URL("./TerminalWorkspace.tsx", import.meta.url), "utf8");
  const app = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");

  it("discovers profiles only from Rust and never types an agent command into a shell", () => {
    // Try again asks Rust to look the CLIs up again first (protocol 41).
    expect(center).toContain("api.listTerminalLaunchProfiles(refresh ? { refresh: true } : undefined)");
    expect(center).toContain("onRetryProfiles: retryLaunchProfiles,");
    expect(center).toContain("launchProfilesRef.current.find");
    expect(center).toContain("const launch: TerminalLaunchRequestV1 = { version: 1, profileId }");
    expect(center).not.toContain("claude --");
    expect(center).not.toContain("codex --");
  });

  it("pre-mints the batch, confirms exact ids, and explicitly avoids replay", () => {
    expect(center).toContain("const ids: string[] = Array.from({ length: count }, () => crypto.randomUUID())");
    expect(center).toContain("Promise.allSettled(requests)");
    expect(center).toContain("snapshot = await api.listTerminals()");
    expect(center).toContain("requested.has(result.value.id)");
    expect(center).toContain("const fulfilledById = new Map");
    expect(center).toContain("fulfilledById.get(id)");
    expect(center).toContain("file(terminal, bindings[ids.indexOf(terminal.id)], launchWorkspaceId)");
    expect(center).toContain("const launchWorkspaceId = activeWorkspaceIdRef.current");
    expect(center).toContain("attachTerminalsBalanced(");
    expect(center).toContain("Interface did not retry");
  });

  it("keeps clipboard image paste native but stores a pathless dropped screenshot", () => {
    const view = readFileSync(new URL("./TerminalView.tsx", import.meta.url), "utf8");
    expect(view).toContain("promptPasteAction(event.clipboardData");
    expect(view).toContain("receive(images)");
    expect(center).toContain("api.savePastedImages(bytes)");
    // An agent pane gets the paste keystroke only for an actual clipboard paste.
    expect(center).toMatch(
      /terminalImagePasteInput\(terminal\.launch\?\.profileId, source\)[\s\S]*?sendBytes\(terminal, textEncoder\.encode\(pasteInput\)\)/u,
    );
    expect(center).toContain("splitDroppedFiles(drop.files");
    expect(center).toContain('void insertPaths(terminal, paths, images, "drop")');
    expect(center).toContain(
      'onPasteFiles={files => { void insertPaths(terminal, [], files, "clipboard"); }}',
    );
    expect(center).toContain('void insertPaths(terminal, [], images, "clipboard")');
    expect(center).toContain("terminalPathInsertPrefix(line)");
    expect(center).toContain("currentLine()");
    expect(center).toContain("handle?.paste(`${shellQuote(path)} `)");
  });

  it("uses the approved folder chosen in the launcher for the whole batch", () => {
    expect(center).toContain("count: number, cwd: string");
    expect(center).toContain('if (cwd.trim() === "")');
    expect(center).toContain("const root = cwd;");
    expect(center).toContain("cwd: root");
    expect(center).toContain("const sessionKey = root === contextualCwd ? contextualSessionKey : null");
    expect(center).toContain("api.pickDirectory(");
    // The launcher's own folder, not the focused pane's: browsing twice in a
    // row used to reopen at the pane's directory rather than where you were.
    expect(center).toContain("preferredPath || launcherCwd || undefined");
    expect(center).toContain("adoptCwd(picked, true)");
    expect(workspace).toContain("onBrowse={launcher.onBrowse}");
  });

  it("routes review through the Rust workflow and never approximates it with two terminals", () => {
    expect(center).toContain("api.startAdversarialReview({");
    expect(center).toContain("builderEngineId,");
    expect(center).toContain("reviewerEngineId,");
    expect(center).toContain("if (!reserveAgentSessionPanes(2))");
    expect(center).toContain("if (!onOpenAgentSessions(");
    expect(center.indexOf("if (!reserveAgentSessionPanes(2))"))
      .toBeLessThan(center.indexOf("api.startAdversarialReview({"));
    const reviewLaunch = center.match(
      /const launchAdversarialReview = useCallback\([\s\S]*?\}, \[adoptCwd, onOpenAgentSessions, releaseAgentSessionPanes, reserveAgentSessionPanes\]\);/u,
    )?.[0] ?? "";
    expect(reviewLaunch).not.toContain("createTerminal");
    // The whole dialog path, end to end: the dialog itself only shapes a
    // request, the launcher's link only opens the dialog, and the palette
    // reaches the same opener through the center's one imperative handle.
    expect(reviewDialogSource).not.toContain("createTerminal");
    expect(reviewDialogSource).not.toContain("window.iface");
    expect(launcherSource).not.toContain("startAdversarialReview");
    expect(launcherSource).toMatch(/onOpenReview !== undefined && \([\s\S]*?onClick=\{onOpenReview\}/u);
    expect(center).toContain("onStart={launchAdversarialReview}");
    expect(center).toContain(
      "openReviewDialog: openReview,");
    expect(center).toContain("focusTerminal: (terminalId: string) => {");
    expect(readFileSync(new URL("../palette-commands.ts", import.meta.url), "utf8"))
      .toContain('{ id: "terminals.review", label: "Start adversarial review…"');
    expect(app).toContain(
      'case "terminals.review": terminalCenter.current?.openReviewDialog(); break;');
    expect(app).toContain("historyPaging.current.markStartedHere(builder)");
    expect(app).toContain("historyPaging.current.markStartedHere(reviewer)");
    expect(app).toContain("appendConversationEcho(builder, launchPendingEcho(objective, []))");
    expect(app).toContain("appendConversationEcho(reviewer, launchPendingEcho(");
    expect(app).toContain("workspacePaneReservation.current = total");
    expect(app).toContain("workspacePaneReservation.current -= 2");
    expect(app).toContain("mayOpenWorkspaceKeyWithReservation(");
  });

  it("routes every new-terminal entry point through the reviewed launcher", () => {
    // The surface itself has no new-terminal entry point left: the bar's New
    // button and the terminals menu went with the bar, and the card grid's
    // dashed tile with the unreachable overview. The sidebar's + and every
    // pane's + are the ways in, and both route through the launcher.
    expect(workspace).not.toContain("openCreate");
    expect(workspace).not.toContain('aria-label="New terminal"');
    expect(workspace).not.toContain("TerminalOverview");
  });

  it("disables and refuses terminal input after its generation stops running", () => {
    expect(center).toContain('current.state !== "running"');
    expect(center).toContain("const TERMINAL_OPTIONS_STOPPED = { disableStdin: true };");
    expect(center).toContain('options={terminal.state === "running"\n'
      + "      ? TERMINAL_OPTIONS_RUNNING : TERMINAL_OPTIONS_STOPPED}");
  });

  it("confirms a workspace delete, stops its terminals, and keeps a destination", () => {
    expect(center).toContain("onDelete: requestDeleteWorkspace");
    expect(center).toContain("forgetWorkspace(");
    expect(center).toContain("neighboringWorkspaceId(");
    expect(center).toContain("workspaceDeleteConfirm(");
    expect(center).toContain("retirePending.current.add(terminal.id)");
    expect(center).toContain("api.closeTerminal({");
    expect(center).toContain("remaining.length === 0");
    expect(center).toContain("createWorkspace()");
    expect(center).toContain("confirmLabel=\"Delete workspace\"");
    expect(center).toContain("tone=\"danger\"");
    expect(app).toContain("onDelete: terminalSwitcher.onDelete");
  });

  it("publishes the workspace switcher outward and draws no workspace bar itself", () => {
    // The switcher belongs to the sidebar now, so the center surface owns the
    // model and the left navigation owns the chrome.
    expect(center).toContain("readonly onSwitcherChange?: (switcher: TerminalWorkspaceSwitcher | null) => void");
    expect(center).toContain("onSwitcherChange?.(switcher)");
    expect(center).not.toContain("switcher: {");
    // The registry stays whole. What narrows is the VIEW: only the open
    // workspace's terminals are drawn. The sidebar receives the whole registry
    // so inactive workspaces remain categorized under their projects.
    expect(center).toContain(
      "      : withTerminalActivity(registry.terminals, activityByStream, completedTurnsByStream, doneStreams);");
    expect(center).toContain("onTerminalsChange?.(next);");
    expect(center).toContain("const known = new Set(visibleTerminals.map(terminal => terminal.id))");
    expect(center).toContain("visibleWorkspaceTerminals(");
    expect(center).toContain("terminalIdsInWorkspaceState({ panes: layoutPanes })");
    // The active workspace lists its open panes from its live layout;
    // terminalSwitcherRecords reads the others from their saved layouts and
    // keeps equal rows.
    expect(center).toContain("const activeOpenIds = useMemo(() => [...layoutTerminalIds], [layoutTerminalIds]);");
    expect(center).toContain("activeWorkspaceId, activeOpenIds);");
    expect(center).toContain("records: switcherRecords,");
    expect(center).toMatch(/\} : null, \[activeWorkspaceId,[^\]]*switcherRecords,[^\]]*workspacesLoaded\]\);/u);
    expect(center).toContain("const reconciliationWorkspaceId = activeWorkspaceId");
    expect(center).toContain("reconcileActiveTerminalWorkspaceState(");
    expect(center).toContain("activeWorkspaceIdRef.current = workspaceId");
    expect(center).toContain("activeWorkspaceIdRef.current = record.id");
    expect(center).toContain("const contextualCwd = paneStartDirectory(");
    expect(center).toContain("focusedTerminal?.cwd,");
    expect(center).toContain("terminalLauncherDefaultCwd(null, lastSelectedCwd, activeCwd)");
    expect(center).toContain("const LAST_CWD_KEY = \"iface.lastCwd\"");
    expect(center).toContain("adoptCwd(focusedTerminal?.cwd, false)");
    expect(center).toContain("adoptCwd(cwd, true)");
    // Selecting a workspace in the sidebar has to reveal the terminal surface;
    // switching the record alone would leave the user looking at Files.
    expect(app).toContain("terminalSwitcher?.onSelect(workspaceId)");
    expect(app).toContain("showTerminals()");
  });

  it("writes membership immediately and flushes a pending layout save on hide", () => {
    expect(center).toContain('persist: "now" | "soon" = "soon"');
    expect(center).toContain('updateWorkspaces(current => [record, ...current], "now")');
    expect(center).toContain(
      "current, workspaceId, terminal, binding, stamp, bindingAuthority,",
    );
    expect(center).toContain("workspacesRef.current = next;\n      setWorkspaces(next);");
    expect(center).toContain('window.addEventListener("pagehide", onPageHide)');
    expect(center).toContain(
      "(migrating && registryRef.current.terminals.length > 0)",
    );
    const reconcileBeforeLoaded = center.indexOf(
      "const reconciled = reconcileLiveTerminalWorkspaceBindings(",
    );
    expect(reconcileBeforeLoaded).toBeGreaterThan(-1);
    expect(reconcileBeforeLoaded).toBeLessThan(
      center.indexOf("workspacesLoadedRef.current = true"),
    );
    expect(center).toContain("|| reconciledProviderBinding");
    // Tabs an older build closed leave the roster before anything reopens,
    // and that pruning is saved.
    const forgetBeforeLoaded = center.indexOf(
      "const records = forgetClosedTabs(reconciled, registryRef.current.terminals, stamp);",
    );
    expect(forgetBeforeLoaded).toBeGreaterThan(reconcileBeforeLoaded);
    expect(forgetBeforeLoaded).toBeLessThan(center.indexOf("workspacesLoadedRef.current = true"));
    expect(center).toContain("|| reconciledProviderBinding || records !== reconciled");
  });
});
