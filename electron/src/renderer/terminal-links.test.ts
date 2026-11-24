import { describe, expect, it, vi } from "vitest";
import {
  detectTerminalLineLinks,
  provideTerminalBufferLinks,
  resolveTerminalLinkTarget,
} from "./terminal-links";

describe("terminal-links", () => {
  it("resolves OSC 8 file:// URLs with query or line fragments to clean local file paths", () => {
    expect(
      resolveTerminalLinkTarget("file:///Users/exampleuser/project/docs/report.md?line=42&col=2"),
    ).toEqual({
      kind: "file",
      path: "/Users/exampleuser/project/docs/report.md",
    });
    expect(
      resolveTerminalLinkTarget("file://localhost/Users/exampleuser/project/src/app.tsx#L10-L20"),
    ).toEqual({
      kind: "file",
      path: "/Users/exampleuser/project/src/app.tsx",
    });
    expect(
      resolveTerminalLinkTarget("file:///Users/exampleuser/my%20docs/diagram.png"),
    ).toEqual({
      kind: "file",
      path: "/Users/exampleuser/my docs/diagram.png",
    });
  });

  it("resolves absolute and cwd-relative file paths while rejecting traversal or unsafe schemes", () => {
    expect(
      resolveTerminalLinkTarget("/Users/exampleuser/project/src/main.rs:120:4"),
    ).toEqual({
      kind: "file",
      path: "/Users/exampleuser/project/src/main.rs",
    });
    expect(
      resolveTerminalLinkTarget("docs/TODO.md#L15", "/Users/exampleuser/project"),
    ).toEqual({
      kind: "file",
      path: "/Users/exampleuser/project/docs/TODO.md",
    });
    expect(
      resolveTerminalLinkTarget("./artifacts/preview.pdf", "/Users/exampleuser/project/"),
    ).toEqual({
      kind: "file",
      path: "/Users/exampleuser/project/artifacts/preview.pdf",
    });
    expect(resolveTerminalLinkTarget("../secret.txt", "/Users/exampleuser/project")).toBeNull();
    expect(resolveTerminalLinkTarget("file://remote-host/etc/passwd")).toBeNull();
    expect(resolveTerminalLinkTarget("javascript:alert(1)", "/Users/exampleuser/project")).toBeNull();
  });

  it("resolves http and https URLs", () => {
    expect(resolveTerminalLinkTarget("https://example.com/docs?q=1")).toEqual({
      kind: "url",
      url: "https://example.com/docs?q=1",
    });
  });

  it("detects markdown links, file:// URLs, and plain file paths in terminal lines", () => {
    const cwd = "/Users/exampleuser/project";
    const line =
      "Attached [report.md](file:///Users/exampleuser/project/docs/report.md#L12) and wrote docs/summary.md plus /tmp/chart.png.";
    const links = detectTerminalLineLinks(line, cwd);
    expect(links.map(link => ({ text: link.text, resolved: link.resolved }))).toEqual([
      {
        text: "[report.md](file:///Users/exampleuser/project/docs/report.md#L12)",
        resolved: { kind: "file", path: "/Users/exampleuser/project/docs/report.md" },
      },
      {
        text: "docs/summary.md",
        resolved: { kind: "file", path: "/Users/exampleuser/project/docs/summary.md" },
      },
      {
        text: "/tmp/chart.png",
        resolved: { kind: "file", path: "/tmp/chart.png" },
      },
    ]);
  });

  it("ignores version numbers and does not split URLs into fake relative paths", () => {
    const cwd = "/Users/exampleuser/project";
    const line = "Interface 0.1.169 see https://example.com/docs/guide.md or user@example.com";
    const links = detectTerminalLineLinks(line, cwd);
    expect(links.map(link => ({ text: link.text, resolved: link.resolved }))).toEqual([
      {
        text: "https://example.com/docs/guide.md",
        resolved: { kind: "url", url: "https://example.com/docs/guide.md" },
      },
    ]);
  });

  it("provides 1-based xterm buffer link ranges across wrapped terminal rows", () => {
    const activated: string[] = [];
    const onOpenLink = vi.fn((link: string) => {
      activated.push(link);
    });
    const rows = [
      { text: "Saved artifact to /Users/exampleuser/project/docs/very-long-", isWrapped: false },
      { text: "report-name.md for review.", isWrapped: true },
    ];
    const buffer = {
      length: rows.length,
      getLine(y: number) {
        const entry = rows[y];
        if (!entry) return undefined;
        return {
          isWrapped: entry.isWrapped,
          translateToString: () => entry.text,
        };
      },
    };

    const row1Links = provideTerminalBufferLinks(buffer, 1, "/Users/exampleuser/project", onOpenLink);
    const row2Links = provideTerminalBufferLinks(buffer, 2, "/Users/exampleuser/project", onOpenLink);
    expect(row1Links).toHaveLength(1);
    expect(row1Links?.[0]?.range).toEqual({
      start: { x: 19, y: 1 },
      end: { x: 14, y: 2 },
    });
    expect(row2Links?.[0]?.range).toEqual({
      start: { x: 19, y: 1 },
      end: { x: 14, y: 2 },
    });
    row1Links?.[0]?.activate();
    expect(activated).toEqual(["/Users/exampleuser/project/docs/very-long-report-name.md"]);
  });
});
