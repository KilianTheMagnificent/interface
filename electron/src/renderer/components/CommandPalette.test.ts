import { readFileSync } from "node:fs";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import {
  COMMAND_PALETTE_MAX_RESULTS,
  COMMAND_PALETTE_AGENT_PREVIEW,
  COMMAND_PALETTE_CSS,
  CommandPalette,
  commandPaletteKeyAction,
  commandSearchText,
  displayCommandPalette,
  groupCommandPalette,
  paletteHighlightIndex,
  searchCommandPalette,
  type CommandPaletteCommand,
} from "./CommandPalette";
import {
  PALETTE_FILE_MAX_CANDIDATES,
  PALETTE_FILE_MAX_RESULTS,
  PALETTE_QUERY_MAX_CHARS,
  fuzzyPathMatchIndices,
  fuzzyPathScore,
  indexPaletteFiles,
  isClutterPath,
  rankPaletteFiles,
  rankPaletteIndex,
} from "./palette-match";
import type { ProjectFileEntry } from "../../shared/ipc";

const commands: CommandPaletteCommand[] = [
  { id: "disabled", label: "New window", group: "Window", disabled: true },
  { id: "files", label: "Go to file", group: "Navigation", shortcut: "⌘P",
    description: "Open a file by name", keywords: ["quick open"] },
  { id: "search", label: "Search project", group: "Navigation", shortcut: ["⌘", "⇧", "F"] },
  { id: "settings", label: "Open settings", group: "Preferences", keywords: ["configuration"] },
];

describe("searchCommandPalette", () => {
  it("matches literal terms case-insensitively across command metadata", () => {
    expect(searchCommandPalette(commands, "GO FILE").map(command => command.id))
      .toEqual(["files"]);
    expect(searchCommandPalette(commands, "navigation project").map(command => command.id))
      .toEqual(["search"]);
    expect(searchCommandPalette(commands, "quick").map(command => command.id))
      .toEqual(["files"]);
    expect(searchCommandPalette(commands, "CONFIGURATION").map(command => command.id))
      .toEqual(["settings"]);
  });

  it("retains registry order and never returns more than the hard cap", () => {
    const many = Array.from({ length: COMMAND_PALETTE_MAX_RESULTS + 25 }, (_, index) => ({
      id: String(index), label: `Command ${index}`,
    }));
    const result = searchCommandPalette(many, "command", COMMAND_PALETTE_MAX_RESULTS + 500);
    expect(result).toHaveLength(COMMAND_PALETTE_MAX_RESULTS);
    expect(result[0].id).toBe("0");
    expect(result.at(-1)?.id).toBe(String(COMMAND_PALETTE_MAX_RESULTS - 1));
  });

  it("honours a smaller caller limit and a zero limit", () => {
    expect(searchCommandPalette(commands, "", 2).map(command => command.id))
      .toEqual(["disabled", "files"]);
    expect(searchCommandPalette(commands, "", 0)).toEqual([]);
  });
});

describe("command search text", () => {
  it("is built once per command, not once per keystroke", () => {
    let reads = 0;
    const command: CommandPaletteCommand = { id: "agent:claude#a", group: "Agents",
      get label() { reads++; return "Fix the Parser"; } };
    expect(commandSearchText(command)).toBe("fix the parser agents");
    expect(searchCommandPalette([command], "parser")).toEqual([command]);
    expect(searchCommandPalette([command], "lexer")).toEqual([]);
    expect(reads).toBe(1);
  });
});

describe("App's palette rows", () => {
  it("builds one row per session only while the palette is open", () => {
    const app = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
    const rows = app.slice(app.indexOf("const paletteSessionCommands = useMemo"),
      app.indexOf("const paletteAllCommands = useMemo"));
    expect(rows).toMatch(/=> !commandPaletteOpen\s*\? NO_PALETTE_COMMANDS : state\.sessions\.map\(/u);
    expect(app).toContain("commands={paletteAllCommands}");
  });
});

describe("groupCommandPalette", () => {
  it("consolidates groups in first-seen order without reordering their commands", () => {
    const grouped = groupCommandPalette([
      commands[1], commands[3], commands[2],
      { id: "plain", label: "Plain" },
    ]);
    expect(grouped.map(group => group.name))
      .toEqual(["Navigation", "Preferences", "Commands"]);
    expect(grouped[0].commands.map(command => command.id)).toEqual(["files", "search"]);
  });
});

describe("displayCommandPalette", () => {
  it("keeps core and recent commands ahead of a bounded agent preview", () => {
    const agents = Array.from({ length: 20 }, (_, index) => ({
      id: `agent:${index}`, label: `Agent ${index}`, group: "Open agent",
    }));
    const displayed = displayCommandPalette([...commands, ...agents], "", ["settings"]);
    expect(displayed[0]).toMatchObject({ id: "settings", group: "Recent" });
    expect(displayed.filter(command => command.id.startsWith("agent:")))
      .toHaveLength(COMMAND_PALETTE_AGENT_PREVIEW);
    expect(displayed.findIndex(command => command.id.startsWith("agent:")))
      .toBeGreaterThan(displayed.findIndex(command => command.id === "search"));
  });

  it("searches the complete agent registry instead of only the preview", () => {
    const agents = Array.from({ length: 20 }, (_, index) => ({
      id: `agent:${index}`, label: `Agent ${index}`, group: "Open agent",
    }));
    expect(displayCommandPalette([...commands, ...agents], "Agent 19").map(command => command.id))
      .toEqual(["agent:19"]);
  });
});

describe("commandPaletteKeyAction", () => {
  it("skips disabled commands, wraps, and supports Home and End", () => {
    expect(commandPaletteKeyAction(commands, 1, "ArrowDown"))
      .toEqual({ type: "move", index: 2 });
    expect(commandPaletteKeyAction(commands, 2, "ArrowDown"))
      .toEqual({ type: "move", index: 3 });
    expect(commandPaletteKeyAction(commands, 3, "ArrowDown"))
      .toEqual({ type: "move", index: 1 });
    expect(commandPaletteKeyAction(commands, 1, "ArrowUp"))
      .toEqual({ type: "move", index: 3 });
    expect(commandPaletteKeyAction(commands, 2, "Home"))
      .toEqual({ type: "move", index: 1 });
    expect(commandPaletteKeyAction(commands, 2, "End"))
      .toEqual({ type: "move", index: 3 });
  });

  it("starts from the appropriate edge when no enabled option is active", () => {
    expect(commandPaletteKeyAction(commands, 0, "ArrowDown"))
      .toEqual({ type: "move", index: 1 });
    expect(commandPaletteKeyAction(commands, -1, "ArrowUp"))
      .toEqual({ type: "move", index: 3 });
  });

  it("runs only an enabled active command and maps Escape to close", () => {
    expect(commandPaletteKeyAction(commands, 1, "Enter"))
      .toEqual({ type: "run", command: commands[1] });
    expect(commandPaletteKeyAction(commands, 0, "Enter")).toBeNull();
    expect(commandPaletteKeyAction(commands, 1, "Escape")).toEqual({ type: "close" });
    expect(commandPaletteKeyAction(commands, 1, "Tab")).toBeNull();
  });

  it("has no navigable action when every matching command is disabled", () => {
    const disabled = commands.filter(command => command.disabled);
    expect(commandPaletteKeyAction(disabled, -1, "ArrowDown")).toBeNull();
    expect(commandPaletteKeyAction(disabled, 0, "Enter")).toBeNull();
  });
});

describe("paletteHighlightIndex", () => {
  const agent = (id: string, disabled = false): CommandPaletteCommand =>
    ({ id: `agent:${id}`, label: id, group: "Agents", disabled });

  it("starts on the first enabled row until the user chooses one", () => {
    expect(paletteHighlightIndex(commands, null)).toBe(1);
    expect(paletteHighlightIndex([], null)).toBe(-1);
  });

  it("keeps the chosen command when a new list moves it, so Enter runs it", () => {
    const before = [agent("a"), agent("b"), agent("c")];
    const chosen = { id: "agent:c", index: 2 };
    expect(paletteHighlightIndex(before, chosen)).toBe(2);
    // A registry push: a new session sorts in above, another is renamed.
    const after = [agent("new"), { ...agent("a"), label: "renamed" }, agent("b"), agent("c")];
    const index = paletteHighlightIndex(after, chosen);
    expect(after[index].id).toBe("agent:c");
    expect(commandPaletteKeyAction(after, index, "Enter"))
      .toEqual({ type: "run", command: after[3] });
    // The same list rebuilt with equal rows keeps the row itself.
    expect(paletteHighlightIndex(before.map(command => ({ ...command })), chosen)).toBe(2);
  });

  it("stays on the chosen row when an id is listed twice", () => {
    const twice = [agent("a"), agent("b"), agent("a")];
    expect(paletteHighlightIndex(twice, { id: "agent:a", index: 2 })).toBe(2);
  });

  it("falls back to the first enabled row when the command is gone or disabled", () => {
    const list = [agent("x", true), agent("a"), agent("b")];
    expect(paletteHighlightIndex(list, { id: "agent:gone", index: 2 })).toBe(1);
    expect(paletteHighlightIndex([agent("x", true), agent("a"), agent("b", true)],
      { id: "agent:b", index: 2 })).toBe(1);
  });
});

const markup = (query: string, offered = commands): string => renderToStaticMarkup(
  createElement(CommandPalette, {
    open: true,
    query,
    commands: offered,
    onQuery: () => undefined,
    onRun: () => undefined,
    onClose: () => undefined,
  }),
);

describe("CommandPalette markup", () => {
  it("connects a labelled combobox to its listbox and active option", () => {
    const html = markup("");
    expect(html).toContain('role="dialog"');
    expect(html).toContain('aria-modal="true"');
    expect(html).toContain('aria-label="Command palette"');
    expect(html).toContain('role="combobox"');
    expect(html).toContain('aria-autocomplete="list"');
    expect(html).toMatch(/aria-controls="([^"]+)-listbox"/);
    expect(html).toMatch(/id="([^"]+)-listbox"[^>]*role="listbox"/);
    expect(html).toMatch(/aria-activedescendant="([^"]+)-option-1"/);
    expect(html).toMatch(/id="([^"]+)-option-1"[^>]*role="option"[^>]*aria-selected="true"/);
    expect(html).toContain("Search commands");
    expect(html).toContain('placeholder="Search commands…"');
  });

  it("renders named groups, disabled state, descriptions, and shortcut labels", () => {
    const html = markup("");
    expect(html.match(/role="group"/g)).toHaveLength(3);
    expect(html).toContain("Navigation");
    expect(html).toContain("Preferences");
    expect(html).toContain('aria-disabled="true"');
    expect(html).toContain("Open a file by name");
    expect(html).toContain("Unavailable right now");
    expect(html).toContain('aria-label="Shortcut: ⌘ then ⇧ then F"');
    expect(html).toContain("<kbd aria-hidden=\"true\">⌘</kbd>");
  });

  it("uses brief causal motion and keeps reduced motion explicit", () => {
    expect(COMMAND_PALETTE_CSS).toContain("command-palette-dialog-in");
    expect(COMMAND_PALETTE_CSS).toContain("var(--motion-slow)");
    expect(COMMAND_PALETTE_CSS).toContain("prefers-reduced-motion: reduce");
    expect(COMMAND_PALETTE_CSS).not.toContain("text-transform: uppercase");
  });

  it("keeps the empty view compact when many agents are available", () => {
    const offered = [...commands, ...Array.from({ length: 30 }, (_, index) => ({
      id: `agent:${index}`, label: `Agent ${index}`, group: "Open agent",
    }))];
    const html = markup("", offered);
    expect(html.match(/Agent \d+/g)).toHaveLength(COMMAND_PALETTE_AGENT_PREVIEW);
    expect(html).toContain("Search to find 22 more agents");
  });

  it("keeps an empty listbox and presents a useful empty state", () => {
    const html = markup("nothing matches this");
    expect(html).toContain('role="listbox"');
    expect(html).not.toContain('role="option"');
    expect(html).toContain('class="command-palette-empty" role="status"');
    expect(html).toContain("No matching commands.");
    expect(html).not.toContain("aria-activedescendant");
  });

  it("renders nothing while controlled closed", () => {
    const html = renderToStaticMarkup(createElement(CommandPalette, {
      open: false,
      query: "",
      commands,
      onQuery: () => undefined,
      onRun: () => undefined,
      onClose: () => undefined,
    }));
    expect(html).toBe("");
  });
});

/* ---- Files: the group Quick Open became ---------------------------------- */

const file = (relativePath: string): ProjectFileEntry => ({
  path: `/workspace/${relativePath}`,
  relativePath,
});

const files = [
  file("src/renderer/App.tsx"),
  file("src/renderer/components/QuickOpen.tsx"),
  file("src/main/bridge.ts"),
  file("README.md"),
];

describe("fuzzyPathScore", () => {
  it("matches case-insensitive subsequences and every whitespace-separated term", () => {
    expect(fuzzyPathScore("src/components/QuickOpen.tsx", "QOPEN")).not.toBeNull();
    expect(fuzzyPathScore("src/components/QuickOpen.tsx", "src qkopen")).not.toBeNull();
    expect(fuzzyPathScore("src/components/QuickOpen.tsx", "missing")).toBeNull();
  });

  it("prefers a basename prefix and a shorter close match", () => {
    const direct = fuzzyPathScore("src/app.ts", "app")!;
    const directoryOnly = fuzzyPathScore("application/src/index.ts", "app")!;
    const long = fuzzyPathScore("very/long/generated/path/app.ts", "app")!;
    expect(direct).toBeGreaterThan(directoryOnly);
    expect(direct).toBeGreaterThan(long);
  });

  it("bounds an externally supplied query", () => {
    expect(fuzzyPathScore("a.ts", "a".repeat(PALETTE_QUERY_MAX_CHARS + 100)))
      .toBeNull();
  });
});

describe("palette path presentation", () => {
  it("filters generated and system clutter without hiding ordinary source", () => {
    expect(isClutterPath(".DS_Store")).toBe(true);
    expect(isClutterPath("dist-main/index.js")).toBe(true);
    expect(isClutterPath("src/build.ts")).toBe(false);
  });

  it("returns the fuzzy character positions used for visual highlighting", () => {
    expect(fuzzyPathMatchIndices("src/QuickOpen.tsx", "qopen"))
      .toEqual([4, 9, 10, 11, 12]);
    expect(fuzzyPathMatchIndices("src/QuickOpen.tsx", "missing")).toEqual([]);
  });
});

describe("rankPaletteFiles", () => {
  it("does not mutate the IPC-owned array it ranks", () => {
    const original = [...files];
    expect(rankPaletteFiles(files, "")).toHaveLength(files.length);
    expect(files).toEqual(original);
  });

  it("filters nonmatches, sorts by score, and caps rendered results", () => {
    expect(rankPaletteFiles(files, "qopen").map(entry => entry.file.relativePath))
      .toEqual(["src/renderer/components/QuickOpen.tsx"]);
    const many = Array.from({ length: PALETTE_FILE_MAX_RESULTS + 25 }, (_, index) =>
      file(`src/file-${String(index).padStart(3, "0")}.ts`));
    expect(rankPaletteFiles(many, "file", PALETTE_FILE_MAX_RESULTS + 100))
      .toHaveLength(PALETTE_FILE_MAX_RESULTS);
  });

  it("defensively removes generated entries supplied by an older index", () => {
    const ranked = rankPaletteFiles([
      file(".DS_Store"), file("dist-main/bundle.js"), file("src/app.ts"),
    ], "");
    expect(ranked.map(entry => entry.file.relativePath)).toEqual(["src/app.ts"]);
  });

  it("does not score candidates beyond the filesystem API ceiling", () => {
    const many = Array.from({ length: PALETTE_FILE_MAX_CANDIDATES + 1 }, (_, index) =>
      file(index === PALETTE_FILE_MAX_CANDIDATES ? "only-match.ts" : `ordinary-${index}.ts`));
    expect(rankPaletteFiles(many, "onlymatch")).toEqual([]);
  });

  it("normalizes Windows separators once while ranking", () => {
    const mixed = [{ path: "C:\\repo\\App.tsx", relativePath: "src\\App.tsx" }];
    expect(rankPaletteFiles(mixed, "app")[0]?.file).toBe(mixed[0]);
  });

  it("ranks an index exactly as sorting every match did, on a 10k-path project", () => {
    // The ranker this replaced: normalize and filter all candidates on every
    // keystroke, then locale-sort every match and slice.
    const sortEveryMatch = (list: readonly ProjectFileEntry[], query: string,
        limit = PALETTE_FILE_MAX_RESULTS) => {
      const bounded = Math.max(0, Math.min(PALETTE_FILE_MAX_RESULTS,
        Number.isFinite(limit) ? Math.floor(limit) : PALETTE_FILE_MAX_RESULTS));
      if (bounded === 0) return [];
      const ranked: { file: ProjectFileEntry; score: number }[] = [];
      for (const entry of list.slice(0, PALETTE_FILE_MAX_CANDIDATES)) {
        if (isClutterPath(entry.relativePath)) continue;
        const score = fuzzyPathScore(entry.relativePath, query);
        if (score !== null) ranked.push({ file: entry, score });
      }
      ranked.sort((a, b) => b.score - a.score
        || a.file.relativePath.localeCompare(b.file.relativePath));
      return ranked.slice(0, bounded);
    };
    let seed = 20260927;
    const next = (): number => { seed = (seed * 1103515245 + 12345) % 2147483648; return seed; };
    const pick = <T,>(items: readonly T[]): T => items[next() % items.length];
    const directories = ["src", "Src", "src/renderer", "src/renderer/components", "docs",
      "node_modules/pkg", "dist", "crates/terminal", "tests", "Écoles", "über", "日本",
      "a-b_c.d", ".", "build", "src\\win"];
    const names = ["App", "app", "Sidebar", "sidebar", "QuickOpen", "index", "README",
      "readme", "CommandPalette", "palette-match", "é", "e", "Z", "z", "10", "9", "a_b", "a-b"];
    const extensions = [".ts", ".tsx", ".md", "", ".TS", ".json"];
    const project: ProjectFileEntry[] = Array.from({ length: PALETTE_FILE_MAX_CANDIDATES + 40 },
      (_, at) => {
        const relativePath = `${next() % 9 === 0 ? "./" : ""}${pick(directories)}/${pick(names)}${pick(extensions)}`;
        // Equal paths are frequent, so ties must keep the index's order.
        return { path: `/repo/${at}/${relativePath}`, relativePath };
      });
    project.push(file(".DS_Store"), file("dist-main/bundle.js"));
    const index = indexPaletteFiles(project);
    const paths = (ranked: readonly { file: ProjectFileEntry }[]): string[] =>
      ranked.map(entry => entry.file.path);
    for (const query of ["", "a", "src", "comp tsx", "É", "readme md", "s/r", "日本",
      "z9", "no-such-path-here", "  ", "a".repeat(PALETTE_QUERY_MAX_CHARS + 20)]) {
      for (const limit of [PALETTE_FILE_MAX_RESULTS, 1, 7, 250, 0, Number.NaN]) {
        const expected = paths(sortEveryMatch(project, query, limit));
        expect(paths(rankPaletteIndex(index, query, limit))).toEqual(expected);
        expect(paths(rankPaletteFiles(project, query, limit))).toEqual(expected);
      }
    }
    // The empty query is answered once per index; each caller gets its own list.
    const first = rankPaletteIndex(index, "");
    const second = rankPaletteIndex(index, "");
    expect(second).not.toBe(first);
    expect(paths(second)).toEqual(paths(first));
  });
});

const filesMarkup = (query: string, over: Record<string, unknown> = {}): string =>
  renderToStaticMarkup(createElement(CommandPalette, {
    open: true,
    query,
    commands,
    mode: "files" as const,
    files,
    onQuery: () => undefined,
    onRun: () => undefined,
    onOpenFile: () => undefined,
    onClose: () => undefined,
    ...over,
  }));

describe("CommandPalette files mode", () => {
  it("is the same dialog with one dataset, named for what a pick does", () => {
    const html = filesMarkup("");
    // Not "Go to file": picking one ADDS it to the prompt's context. This
    // renderer has no editor to navigate to.
    expect(html).toContain('aria-label="Add a file to the prompt"');
    expect(html).toContain('placeholder="Search files by name…"');
    expect(html).toContain('aria-label="Matching project files"');
    expect(html).toContain(`maxLength="${PALETTE_QUERY_MAX_CHARS}"`);
    expect(html).toContain("Files");
    expect(html).toContain("QuickOpen.tsx");
    expect(html).toContain("src/renderer/components");
    expect(html).toContain("command-palette-file-icon");
    // Commands are not competing for the same hundred rows here.
    expect(html).not.toContain("Open settings");
    // Quick Open's footer line, kept.
    expect(html).toContain("4 files indexed");
  });

  it("marks the characters a fuzzy match actually matched", () => {
    const html = filesMarkup("qopen");
    expect(html).toContain('<mark class="command-palette-match">Open</mark>');
    expect(html).not.toContain('<mark class="command-palette-match">O</mark>');
  });

  it("announces controlled loading, error and truncated states", () => {
    expect(filesMarkup("", { files: [], filesLoading: true }))
      .toContain('role="status">Loading project files…');
    const failed = filesMarkup("", { files: [], filesError: "That project is unavailable." });
    expect(failed).toContain('class="command-palette-empty command-palette-error" role="alert"');
    expect(failed).toContain("That project is unavailable.");
    // A loading or failed index has no footer to be honest about.
    expect(failed).not.toContain("files indexed");
    expect(filesMarkup("", { filesTruncated: true })).toContain("index incomplete");
    expect(filesMarkup("no-such-file")).toContain("No matching files.");
  });

  it("shows no files in all mode, typed query or not", () => {
    const many = Array.from({ length: 60 }, (_, index) =>
      file(`src/thing-${String(index).padStart(3, "0")}.ts`));
    const all = (query: string): string => renderToStaticMarkup(
      createElement(CommandPalette, {
        open: true, query, commands, files: many,
        onQuery: () => undefined, onRun: () => undefined,
        onOpenFile: () => undefined, onClose: () => undefined,
      }));
    // Files are ⌘P's dataset. In all mode App hands over an empty list, so a
    // tail here could only ever have been dead code.
    for (const markup of [all(""), all("thing")]) {
      expect(markup).not.toContain('class="codicon codicon-file command-palette-file-icon"');
      expect((markup.match(/command-palette-option-file/g) ?? []).length)
        .toBe(1);   // the CSS rule itself
      expect(markup).not.toContain(">Files<");
    }
  });

  /**
   * One open-state source. `quickOpen.open` IS files mode: a second
   * `paletteMode` state let a reset lower one and not the other, leaving the
   * picker open over a session whose index had gone.
   */
  it("derives files mode from the file store alone", () => {
    const source = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
    expect(source).not.toContain("paletteMode");
    expect(source).toContain('mode={quickOpen.open ? "files" : "all"}');
    expect(source).toContain("const resetComposerContext = useCallback(() => {\n"
      + "    contextRequestEpoch.current += 1;\n    closeQuickOpen();");
    // Every other dismissal goes through the same reset.
    expect(source).not.toContain("setQuickOpen(QUICK_OPEN_CLOSED);\n    setContextValueTarget");
    const store = readFileSync(new URL("../use-quick-open.ts", import.meta.url), "utf8");
    expect((`${source}\n${store}`.match(/setQuickOpen\(QUICK_OPEN_CLOSED\)/g) ?? []).length).toBe(1);
  });

  it("takes its focus lifecycle from the shared modal hook", () => {
    const source = readFileSync(new URL("./CommandPalette.tsx", import.meta.url), "utf8");
    expect(source).toContain('import { useModalFocus } from "./ModalBehavior"');
    expect(source).toContain("const handleModalKey = useModalFocus<HTMLElement>({");
    expect(source).not.toContain("previousFocus");
  });
});
