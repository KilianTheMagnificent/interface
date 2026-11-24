import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import {
  Markdown, blockProse, parseMarkdown, parseMarkdownDirective, parseMarkdownInline, safeMarkdownFileTarget,
  safeMarkdownHref, splitMarkdownTableRow,
} from "./markdown";

function paint(text: string): string {
  return renderToStaticMarkup(createElement(Markdown, { text }));
}

describe("assistant Markdown — block structure", () => {
  it("turns heading hashes, list markers, quotes and fences into semantic elements", () => {
    const html = paint([
      "# Result",
      "",
      "- first",
      "- second with **weight**",
      "",
      "> A useful note",
      "",
      "```ts",
      "const answer = 42;",
      "```",
    ].join("\n"));

    expect(html).toContain('<h1 class="conv-md-heading conv-md-h1">Result</h1>');
    expect(html).not.toContain("# Result");
    expect(html).toContain('<ul class="conv-md-list"><li>first');
    expect(html).toContain("<strong>weight</strong>");
    expect(html).toContain('<blockquote class="conv-md-quote">');
    // The language label now shares its row with the copy control, so it is a
    // span inside the header rather than the header's whole text.
    expect(html).toContain('<div class="conv-md-code-language"><span>ts</span>');
    expect(html).toContain("<code>const answer = 42;</code></pre>");
  });

  it("keeps a 4-space indented fence from closing the open block", () => {
    const html = paint("```\nconst answer = 42;\n    ```\nstill");
    expect(html).toContain("<code>const answer = 42;\n    ```\nstill</code></pre>");
    expect(html.match(/<pre[ >]/gu)).toHaveLength(1);
  });

  it("makes a code block keyboard-reachable and gives it one copy control", () => {
    // The pre is what scrolls, so the pre is what a keyboard has to be able to
    // reach (WCAG 2.1.1) — the table wrapper's treatment, on the element that
    // actually has the overflow.
    const html = paint("```ts\nconst answer = 42;\n```");
    expect(html).toContain('<pre tabindex="0" role="region" aria-label="Scrollable code">');
    expect(html).toContain(
      '<button type="button" class="conv-md-code-copy" aria-label="Copy code">Copy</button>');
    // A fence with no info string still gets the control: the header row is the
    // control's home, not the language's.
    expect(paint("```\nplain\n```")).toContain('class="conv-md-code-copy"');
  });

  it("keeps ordered-list starts and line breaks inside an item", () => {
    const parsed = parseMarkdown("3. one\n   continued\n4. two");
    expect(parsed).toMatchObject([{
      kind: "list", ordered: true, start: 3,
      items: [[[{ kind: "text", text: "one" }], [{ kind: "text", text: "continued" }]],
        [[{ kind: "text", text: "two" }]]],
    }]);
    const html = paint("3. one\n   continued\n4. two");
    expect(html).toContain('<ol class="conv-md-list" start="3">');
    expect(html).toContain("one<br/>continued");
  });

  it("draws task items as read-only boxes and nests indented lists under their item", () => {
    const source = [
      "- [x] shipped",
      "- [ ] **pending**",
      "- plain",
      "",
      "1. one",
      "2. two",
      "   - under two",
      "     - deeper",
      "3. three",
    ].join("\n");
    const [tasks, numbered] = parseMarkdown(source);
    expect(tasks).toMatchObject({ kind: "list", tasks: [true, false, null],
      items: [[[{ kind: "text", text: "shipped" }]], [[{ kind: "strong" }]], [[{ kind: "text", text: "plain" }]]] });
    expect(numbered).toMatchObject({ kind: "list", ordered: true, items: [[], [], []].map(() => expect.anything()),
      nested: [[], [{ kind: "list", ordered: false, items: [[[{ kind: "text", text: "under two" }]]],
        nested: [[{ kind: "list", items: [[[{ kind: "text", text: "deeper" }]]] }]] }], []] });
    expect(blockProse(numbered)).toBe("onetwounder twodeeperthree");

    const html = paint(source);
    expect(html).toContain('<li class="conv-md-task"><span class="conv-md-task-box" role="checkbox" aria-checked="true"'
      + ' aria-disabled="true" aria-label="Done"><span class="codicon codicon-check" aria-hidden="true"></span></span>shipped</li>');
    expect(html).toContain('aria-checked="false" aria-disabled="true" aria-label="Not done"></span><strong>pending</strong>');
    expect(html).not.toContain("[x]");
    expect(html).toContain('<li>two<ul class="conv-md-list"><li>under two<ul class="conv-md-list"><li>deeper</li></ul></li></ul></li><li>three</li>');
    // Items at the marker's own depth stay siblings, and a lone "[x]" is text.
    expect(parseMarkdown("  - a\n  - b")).toMatchObject([{ kind: "list", items: [expect.anything(), expect.anything()] }]);
    expect(parseMarkdown("  - a\n  - b")[0]).not.toHaveProperty("nested");
    expect(parseMarkdown("- [x]")[0]).not.toHaveProperty("tasks");
  });

  it("renders inline code, emphasis, strong text, strikethrough and links", () => {
    const html = paint("Use `npm test`, *then* **ship** ~~later~~ [docs](https://example.com/a).");
    expect(html).toContain('<code class="conv-md-inline-code">npm test</code>');
    expect(html).toContain("<em>then</em>");
    expect(html).toContain("<strong>ship</strong>");
    expect(html).toContain("<del>later</del>");
    expect(html).toContain('href="https://example.com/a"');
    expect(html).toContain('target="_blank"');
    expect(html).toContain('rel="noreferrer noopener"');
  });

  it("leaves intraword underscores and spaced asterisks as the characters they are", () => {
    const plain = (text: string): void => {
      expect(parseMarkdownInline(text), text).toEqual([{ kind: "text", text }]);
    };
    plain("set max_tokens and top_p");
    plain("renderer_cells_builder");
    plain("my__init__py and x__init__.py");
    plain("2 * 3 * 4");
    plain("a ** b ** c");
    plain("_ not emphasis _");
    plain("~~ not struck ~~");
    plain("*".repeat(10_000));
    expect(paint("Rename `a` to snake_case_name.")).toContain("to snake_case_name.");
    const table = paint("| Field | Default |\n| --- | --- |\n| max_tokens | 1_024 |");
    expect(table).toContain(">max_tokens<");
    expect(table).toContain(">1_024<");
    expect(table).not.toContain("<em>");
  });

  it("still emphasises delimiters that open and close a phrase", () => {
    expect(parseMarkdownInline("*a* and _a_")).toEqual([
      { kind: "emphasis", children: [{ kind: "text", text: "a" }] },
      { kind: "text", text: " and " },
      { kind: "emphasis", children: [{ kind: "text", text: "a" }] },
    ]);
    expect(parseMarkdownInline("say __bold__ now")).toEqual([
      { kind: "text", text: "say " },
      { kind: "strong", children: [{ kind: "text", text: "bold" }] },
      { kind: "text", text: " now" },
    ]);
    // CommonMark: a closing run followed by punctuation closes, so this is
    // strong `init` exactly as GitHub draws it.
    expect(parseMarkdownInline("__init__.py")).toEqual([
      { kind: "strong", children: [{ kind: "text", text: "init" }] },
      { kind: "text", text: ".py" },
    ]);
    expect(parseMarkdownInline("*a * b* and _foo_bar_ and foo*bar*baz")).toEqual([
      { kind: "emphasis", children: [{ kind: "text", text: "a * b" }] },
      { kind: "text", text: " and " },
      { kind: "emphasis", children: [{ kind: "text", text: "foo_bar" }] },
      { kind: "text", text: " and foo" },
      { kind: "emphasis", children: [{ kind: "text", text: "bar" }] },
      { kind: "text", text: "baz" },
    ]);
  });

  it("keeps inline code as a class, not a boxed element of its own", () => {
    const html = paint("`npm test`");
    expect(html).toContain('<code class="conv-md-inline-code">npm test</code>');
    expect(html).not.toMatch(/conv-md-inline-code"[^>]*style=/u);
  });

  it("renders complete tables with semantic structure and alignment", () => {
    const html = paint([
      "| Area | State | Score |",
      "| :--- | :---: | ---: |",
      "| Chat | **Ready** | 9 |",
      "| Review | Waiting | 7 |",
    ].join("\n"));

    expect(html).toContain('class="conv-md-table-wrap" role="region"');
    expect(html).toContain("<table");
    expect(html).toContain("<thead><tr>");
    expect(html).toContain('<th scope="col" class="is-align-left">');
    expect(html).toContain('<th scope="col" class="is-align-center">');
    expect(html).toContain('<th scope="col" class="is-align-right">');
    expect(html).toContain("<tbody><tr>");
    expect(html).toContain("<strong>Ready</strong>");
  });

  it("does not split escaped pipes or pipes inside inline code", () => {
    expect(splitMarkdownTableRow("| one \\| two | `a | b` | three |")).toEqual([
      "one \\| two", "`a | b`", "three",
    ]);
    expect(splitMarkdownTableRow("left | right \\|")).toEqual(["left", "right \\|"]);
    const html = paint([
      "| Label | Value |",
      "| --- | --- |",
      "| one \\| two | `a | b` |",
    ].join("\n"));
    expect(html).toContain("one | two");
    expect(html).toContain('<code class="conv-md-inline-code">a | b</code>');
  });

  it("keeps incomplete or malformed streaming tables as ordinary prose", () => {
    const incomplete = paint("| Area | State |\n| --- | --- |");
    expect(incomplete).not.toContain("<table");
    expect(incomplete).toContain("| Area | State |");

    const malformed = paint("| Area | State |\n| -- | --- |\n| Chat | Ready |");
    expect(malformed).not.toContain("<table");
  });

  it("keeps HTML in table cells escaped", () => {
    const html = paint([
      "| Area | State |",
      "| --- | --- |",
      "| Chat | <img src=x onerror=alert(1)> |",
    ].join("\n"));
    expect(html).toContain("<table");
    expect(html).not.toContain("<img");
    expect(html).toContain("&lt;img src=x onerror=alert(1)&gt;");
  });
});

describe("assistant Markdown — Interface directives", () => {
  it("parses escaped directive attributes without eval or JSON coercion", () => {
    expect(parseMarkdownDirective(
      '::code-comment{title="[P2] Quote \\"safe\\"" body="line 1\\nline 2" start=7}',
    )).toEqual({
      name: "code-comment",
      attributes: { title: '[P2] Quote "safe"', body: "line 1\nline 2", start: "7" },
    });
  });

  it("turns a codex annotation marker into a readable inline badge", () => {
    const html = paint('This is addressed. :codex-annotation{index="12"}');
    expect(html).toContain("This is addressed.");
    expect(html).toContain('<span class="conv-md-annotation" title="Response annotation">Annotation 12</span>');
    expect(html).not.toContain("codex-annotation");
  });

  it("turns a code-comment directive into a readable review card", () => {
    const html = paint(
      '::code-comment{title="[P1] Guard this call" body="This can **throw**." ' +
      'file="/repo/src/a.ts" start=10 end=12 priority=1}',
    );
    expect(html).toContain('aria-label="Code comment"');
    expect(html).toContain("[P1] Guard this call");
    expect(html).toContain("This can ");
    expect(html).toContain("<strong>throw</strong>");
    expect(html).toContain("/repo/src/a.ts:10–12");
    expect(html).toContain('<span class="conv-md-priority">P1</span>');
    expect(html).not.toContain("::code-comment");
  });
});

describe("assistant Markdown — hostile or incomplete model output", () => {
  it("never interprets model-authored HTML", () => {
    const html = paint('<script>alert("x")</script> <img src=x onerror=alert(1)>');
    expect(html).not.toContain("<script>");
    expect(html).not.toContain("<img");
    expect(html).toContain("&lt;script&gt;");
    expect(html).toContain("&lt;img src=x onerror=alert(1)&gt;");
  });

  it("removes navigation from javascript, data, file and relative links", () => {
    for (const href of ["javascript:alert(1)", "data:text/html,x", "file:///tmp/a", "./a"] ) {
      const html = paint(`[still readable](${href})`);
      expect(html).toContain("still readable");
      expect(html).not.toContain("href=");
      expect(html).toContain("Unsafe or local link omitted");
    }
  });

  it("allows only explicit web and mail schemes", () => {
    expect(safeMarkdownHref("https://example.com/x")).toBe("https://example.com/x");
    expect(safeMarkdownHref("http://example.com")).toBe("http://example.com/");
    expect(safeMarkdownHref("mailto:hello@example.com")).toBe("mailto:hello@example.com");
    expect(safeMarkdownHref("javascript:alert(1)")).toBeUndefined();
    expect(safeMarkdownHref("/local/file")).toBeUndefined();
  });

  it("keeps unfinished streaming markers visible instead of dropping prose", () => {
    expect(parseMarkdownInline("Working on **the fix")).toEqual([
      { kind: "text", text: "Working on **the fix" },
    ]);
    expect(paint("Working on **the fix")).toContain("Working on **the fix");
  });

  it("bounds recursive formatting while retaining all text", () => {
    const source = "*".repeat(20) + "deep" + "*".repeat(20);
    const html = paint(source);
    expect(html.length).toBeLessThan(10_000);
    expect(html).toContain("deep");
  });

  it("preserves large incomplete markup with a bounded plain-text fallback", () => {
    for (const marker of [":codex-annotation{", "[", "<"]) {
      const source = marker.repeat(Math.ceil(65_536 / marker.length)).slice(0, 65_536);
      const tokens = parseMarkdownInline(source);
      expect(tokens).toEqual([{ kind: "text", text: source }]);
    }
    expect(parseMarkdownInline('Before :codex-annotation{index="7"} **after**')).toEqual([
      { kind: "text", text: "Before " }, { kind: "annotation", index: "7" },
      { kind: "text", text: " " }, { kind: "strong", children: [{ kind: "text", text: "after" }] },
    ]);
  });

  it("turns local file links into preview actions when an onOpenFile handler is provided", () => {
    expect(safeMarkdownFileTarget("file:///Users/me/repo/docs/spec.md#L12-L20"))
      .toBe("/Users/me/repo/docs/spec.md");
    expect(safeMarkdownFileTarget("file://localhost/tmp/out%20put/report.pdf?line=4"))
      .toBe("/tmp/out put/report.pdf");
    expect(safeMarkdownFileTarget("/repo/src/main.rs:42:8"))
      .toBe("/repo/src/main.rs");
    expect(safeMarkdownFileTarget("./docs/notes.md#summary"))
      .toBe("docs/notes.md");
    expect(safeMarkdownFileTarget("javascript:alert(1)")).toBeUndefined();
    expect(safeMarkdownFileTarget("data:text/html,x")).toBeUndefined();
    expect(safeMarkdownFileTarget("../secret.txt")).toBeUndefined();

    const html = renderToStaticMarkup(createElement(Markdown, {
      text: "See [report.md](file:///repo/docs/report.md#L10) and [evil](javascript:alert(1)).",
      onOpenFile: () => undefined,
    }));
    expect(html).toContain('class="conv-md-link" href="#preview-file" title="Preview /repo/docs/report.md">report.md</a>');
    expect(html).toContain('<span class="conv-md-link-disabled" title="Unsafe or local link omitted">evil</span>');
  });
});
