import { describe, expect, it } from "vitest";
import {
  SYNTAX_MAX_LINE, syntaxLanguageFor, tokenizeLine, type SyntaxKind, type SyntaxLanguage,
} from "./syntax-tokens";

/* ==========================================================================
   Light syntax highlighting for one diff line at a time. The pieces always
   join back to the exact line; strings own whatever looks like a comment
   inside them; nothing carries from one line to the next.
   ========================================================================== */

/** The pieces as "kind:text" for the kinds that are coloured. */
function coloured(text: string, language: SyntaxLanguage | null): string[] {
  return tokenizeLine(text, language).filter(token => token.kind !== "plain")
    .map(token => `${token.kind}:${token.text}`);
}

function kindOf(text: string, language: SyntaxLanguage, piece: string): SyntaxKind | undefined {
  return tokenizeLine(text, language).find(token => token.text === piece)?.kind;
}

describe("syntaxLanguageFor", () => {
  it("reads the language from the extension", () => {
    expect(["a.ts", "a.tsx", "a.js", "a.jsx", "a.rs", "a.py", "a.go", "a.css", "a.json", "a.md"]
      .map(syntaxLanguageFor)).toEqual(["ts", "ts", "js", "js", "rust", "python", "go", "css", "json", "markdown"]);
    expect(syntaxLanguageFor("src/auth/backoff.ts → src/auth/retry.TS")).toBe("ts");
    expect(syntaxLanguageFor("C:\\repo\\main.rs")).toBe("rust");
  });

  it("colours nothing it does not know", () => {
    expect(syntaxLanguageFor("notes.txt")).toBeNull();
    expect(syntaxLanguageFor("Makefile")).toBeNull();
    expect(syntaxLanguageFor(".env")).toBeNull();
    expect(tokenizeLine("if (x) { y(); }", null)).toEqual([{ kind: "plain", text: "if (x) { y(); }" }]);
  });
});

describe("tokenizeLine", () => {
  const samples: [string, SyntaxLanguage][] = [
    ["export function login(user: string) {", "ts"],
    ["  return users.find(u => u.name === user); // first match", "ts"],
    ["const s = \"say \\\"hi\\\" // still a string\"; // a comment", "ts"],
    ["const t = `a ${b} // c`;", "js"],
    ["let n = 0x1F + 1_000 + 3.14e-2 + 10n;", "ts"],
    ["fn main<'a>(x: &'a str) -> Option<u32> { println!(\"{}\", 'c'); }", "rust"],
    ["def area(r): return 3.14 * r ** 2  # \"not a string\"", "python"],
    ["x = f'{y}' + r\"\\d\" + b'\\x00' if z is None else True", "python"],
    ["func (r *Reader) Read(p []byte) (n int, err error) { return 0, nil }", "go"],
    ["s := `raw \\ string` // done", "go"],
    ["  color: var(--fg-dim); /* the dim one */ margin: -2px 0.5em #fff !important;", "css"],
    ["@media (max-width: 620px) {", "css"],
    ["  \"name\": \"interface\", \"count\": -12.5e3, \"on\": true, \"none\": null,", "json"],
    ["## Auth", "markdown"],
    ["- `login(user)` trims the name and starts a session.", "markdown"],
    ["\t\t  ", "ts"],
    ["\"unterminated // string", "ts"],
    ["/* unterminated block", "ts"],
    [" * a JSDoc line", "ts"],
    ["const café = naïve(ünïcode);", "js"],
  ];

  it("joins back to the exact line, whatever the language", () => {
    for (const [line, language] of samples) {
      expect(tokenizeLine(line, language).map(token => token.text).join(""), line).toBe(line);
    }
  });

  it("never draws an empty piece or two plain pieces in a row", () => {
    for (const [line, language] of samples) {
      const tokens = tokenizeLine(line, language);
      expect(tokens.every(token => token.text !== "")).toBe(true);
      expect(tokens.some((token, index) => index > 0 && token.kind === tokens[index - 1].kind)).toBe(false);
    }
  });

  it("colours a TypeScript line: keywords, a function, a type after a colon, punctuation", () => {
    expect(coloured("export function login(user: string) {", "ts")).toEqual([
      "keyword:export", "keyword:function", "function:login", "punctuation:(",
      "punctuation::", "type:string", "punctuation:)", "punctuation:{",
    ]);
    expect(kindOf("const found: Session | null = users.find(u => u);", "ts", "Session")).toBe("type");
    expect(kindOf("let draft: draftText;", "ts", "draftText")).toBe("type");
    // Only TypeScript reads the word after a colon as a type.
    expect(coloured("const v = a ? b : other;", "js")).not.toContain("type:other");
  });

  it("calls an identifier before ( a function and a capitalized one a type", () => {
    expect(kindOf("session.clear();", "ts", "clear")).toBe("function");
    expect(kindOf("await sleep (wait);", "ts", "sleep")).toBe("function");
    expect(kindOf("const m = new Map<string, Thread>();", "ts", "Thread")).toBe("type");
    expect(kindOf("if (x) return;", "ts", "if")).toBe("keyword");
  });

  it("keeps // inside a string a string, and a comment after it a comment", () => {
    expect(coloured("const url = \"https://example.com/a\"; // the home page", "ts")).toEqual([
      "keyword:const", "punctuation:=", "string:\"https://example.com/a\"", "punctuation:;",
      "comment:// the home page",
    ]);
    expect(coloured("const t = `a // b`;", "js")).toContain("string:`a // b`");
    expect(coloured("x = 'a # b'  # note", "python")).toEqual(["punctuation:=", "string:'a # b'", "comment:# note"]);
  });

  it("reads escaped quotes inside a string", () => {
    expect(coloured("say(\"a \\\"quoted\\\" word\", 'it\\'s');", "ts")).toEqual([
      "function:say", "punctuation:(", "string:\"a \\\"quoted\\\" word\"", "punctuation:,",
      "string:'it\\'s'", "punctuation:);",
    ]);
    // A trailing backslash escapes the quote: the string runs to the end of the line.
    expect(coloured("const p = \"C:\\\\\"; done();", "ts")).toContain("string:\"C:\\\\\"");
    expect(coloured("const q = \"a\\\" // b", "ts")).toEqual(["keyword:const", "punctuation:=", "string:\"a\\\" // b"]);
  });

  it("ends a line comment's reach at the line: no state crosses lines", () => {
    expect(coloured("/* opens here", "ts")).toEqual(["comment:/* opens here"]);
    expect(coloured("still inside a comment up there", "ts")).toEqual([]);
    expect(coloured("a = 1; /* note */ b = 2;", "ts")).toEqual([
      "punctuation:=", "number:1", "punctuation:;", "comment:/* note */", "punctuation:=", "number:2", "punctuation:;",
    ]);
    // A line that continues a block comment reads as one.
    expect(coloured("   * Returns the session.", "ts")).toEqual(["comment:* Returns the session."]);
    expect(coloured(" */", "rust")).toEqual(["comment:*/"]);
  });

  it("reads numbers and literals", () => {
    expect(coloured("let n = 0x1F + 1_000 + 3.14e-2 + 10n;", "ts").filter(piece => piece.startsWith("number")))
      .toEqual(["number:0x1F", "number:1_000", "number:3.14e-2", "number:10n"]);
    expect(coloured("[true, false, null, undefined]", "ts").filter(piece => piece.startsWith("literal")))
      .toEqual(["literal:true", "literal:false", "literal:null", "literal:undefined"]);
    expect(kindOf("if x is None:", "python", "None")).toBe("literal");
    expect(kindOf("return nil", "go", "nil")).toBe("literal");
    expect(kindOf("let r = 1..10;", "rust", "1")).toBe("number");
  });

  it("reads Rust: macros, lifetimes, char literals and built-in types", () => {
    const line = "fn main<'a>(x: &'a str) -> Option<u32> { println!(\"{}\", 'c'); }";
    expect(kindOf(line, "rust", "println")).toBe("function");
    expect(kindOf(line, "rust", "'a")).toBe("keyword");
    expect(kindOf(line, "rust", "'c'")).toBe("string");
    expect(kindOf(line, "rust", "u32")).toBe("type");
    expect(kindOf(line, "rust", "Option")).toBe("type");
    expect(kindOf("let b = b\"bytes\";", "rust", "b\"bytes\"")).toBe("string");
  });

  it("reads Python's comments, string prefixes and keywords", () => {
    expect(coloured("def area(r): return 3.14  # in cm", "python")).toEqual([
      "keyword:def", "function:area", "punctuation:(", "punctuation:):", "keyword:return",
      "number:3.14", "comment:# in cm",
    ]);
    expect(kindOf("x = f'{y}'", "python", "f'{y}'")).toBe("string");
    expect(kindOf("x = r\"\\d\"", "python", "r\"\\d\"")).toBe("string");
  });

  it("reads Go's raw strings without escapes", () => {
    expect(coloured("s := `raw \\` // tail", "go")).toEqual([
      "punctuation::=", "string:`raw \\`", "comment:// tail",
    ]);
    expect(kindOf("func (r *Reader) Read(p []byte) error {", "go", "byte")).toBe("type");
    expect(kindOf("func (r *Reader) Read(p []byte) error {", "go", "Read")).toBe("function");
  });

  it("reads CSS: at-rules, functions, custom properties, numbers and comments", () => {
    expect(coloured("  color: var(--fg-dim); /* dim */ margin: -2px 0.5em #fff !important;", "css")).toEqual([
      "punctuation::", "function:var", "punctuation:(", "type:--fg-dim", "punctuation:);",
      "comment:/* dim */", "punctuation::", "number:2px", "number:0.5em", "number:#fff",
      "keyword:!important", "punctuation:;",
    ]);
    expect(coloured("@media (max-width: 620px) {", "css")).toEqual([
      "keyword:@media", "punctuation:(", "punctuation::", "number:620px", "punctuation:)", "punctuation:{",
    ]);
  });

  it("reads JSON's strings, numbers and literals", () => {
    expect(coloured("  \"count\": -12.5e3, \"on\": true, \"none\": null", "json")).toEqual([
      "string:\"count\"", "punctuation::", "number:-12.5e3", "punctuation:,", "string:\"on\"",
      "punctuation::", "literal:true", "punctuation:,", "string:\"none\"", "punctuation::", "literal:null",
    ]);
  });

  it("reads Markdown's headings, list markers and inline code", () => {
    expect(coloured("## Auth", "markdown")).toEqual(["keyword:## Auth"]);
    expect(coloured("- `login(user)` trims the name.", "markdown"))
      .toEqual(["punctuation:-", "function:`login(user)`"]);
    expect(coloured("an `unclosed tick", "markdown")).toEqual([]);
    expect(coloured("#hashtag is not a heading", "markdown")).toEqual([]);
  });

  it("draws an empty line as nothing and a very long one plain", () => {
    expect(tokenizeLine("", "ts")).toEqual([]);
    const long = `const x = "${"a".repeat(SYNTAX_MAX_LINE)}";`;
    expect(tokenizeLine(long, "ts")).toEqual([{ kind: "plain", text: long }]);
    const edge = "x".repeat(SYNTAX_MAX_LINE - 3) + "();";
    expect(tokenizeLine(edge, "ts")[0].kind).toBe("function");
  });
});
