/**
 * LIGHT SYNTAX HIGHLIGHTING for one line of a diff (owner, 2026-10-06, "hued
 * glow"). A diff draws lines out of context — a hunk can start inside a block
 * comment or a template string — so this reads each line on its own: no state
 * crosses a line, comments are the ones that start (and, for a `/* … *\/`,
 * end) on it, and a line it cannot place is plain text. Its only job is
 * colour; nothing here decides what a line means.
 *
 * Pure: a line in, its pieces out. The pieces always join back to the exact
 * line, so the diff can draw them as spans and copying a line copies the code.
 */

export type SyntaxKind =
  | "keyword" | "string" | "number" | "comment" | "function" | "type" | "literal"
  | "punctuation" | "plain";

export interface SyntaxToken { kind: SyntaxKind; text: string }

export type SyntaxLanguage = "ts" | "js" | "rust" | "python" | "go" | "css" | "json" | "markdown";

/** The languages read by the one scanner for code (the rest have their own). */
type CodeLanguage = Exclude<SyntaxLanguage, "css" | "json" | "markdown">;

/** A line longer than this is drawn plain: minified code, data, a lockfile. */
export const SYNTAX_MAX_LINE = 2_000;

const EXTENSIONS: Readonly<Record<string, SyntaxLanguage>> = {
  ts: "ts", tsx: "ts", mts: "ts", cts: "ts",
  js: "js", jsx: "js", mjs: "js", cjs: "js",
  rs: "rust", py: "python", go: "go", css: "css", json: "json", md: "markdown",
};

/** The language a path's extension names, or null for one this does not colour. */
export function syntaxLanguageFor(path: string): SyntaxLanguage | null {
  const name = path.split(/[\\/]/u).pop() ?? path;
  const dot = name.lastIndexOf(".");
  if (dot <= 0) return null;
  return EXTENSIONS[name.slice(dot + 1).toLowerCase()] ?? null;
}

interface CodeRules {
  keywords: ReadonlySet<string>;
  literals: ReadonlySet<string>;
  /** Built-in type names that are not capitalized (`string`, `u32`, `int`). */
  types: ReadonlySet<string>;
  /** What starts a comment that runs to the end of the line. */
  lineComment: "//" | "#";
  /** `/* … *\/` on one line, and a line that continues one (` * …`). */
  blockComments: boolean;
  /** Quote characters that open a string. */
  quotes: string;
  /** `$` is part of an identifier (JavaScript). */
  dollar: boolean;
  /** An identifier right after `:` is a type (TypeScript annotations). */
  colonTypes: boolean;
}

const words = (text: string): ReadonlySet<string> => new Set(text.split(/\s+/u).filter(Boolean));

const JS_KEYWORDS = "async await break case catch class const continue debugger default delete do else "
  + "export extends finally for from function get if import in instanceof let new of return set static "
  + "super switch this throw try typeof var void while with yield";
const JS_LITERALS = words("true false null undefined NaN Infinity");

const RULES: Readonly<Record<CodeLanguage, CodeRules>> = {
  js: {
    keywords: words(JS_KEYWORDS), literals: JS_LITERALS, types: words(""),
    lineComment: "//", blockComments: true, quotes: "\"'`", dollar: true, colonTypes: false,
  },
  ts: {
    keywords: words(`${JS_KEYWORDS} abstract as asserts declare enum implements infer interface is keyof `
      + "module namespace override private protected public readonly satisfies type unique"),
    literals: JS_LITERALS,
    types: words("any bigint boolean never number object string symbol unknown void"),
    lineComment: "//", blockComments: true, quotes: "\"'`", dollar: true, colonTypes: true,
  },
  rust: {
    keywords: words("as async await break const continue crate dyn else enum extern fn for if impl in let "
      + "loop match mod move mut pub ref return self Self static struct super trait type unsafe use where while"),
    literals: words("true false None"),
    types: words("bool char f32 f64 i8 i16 i32 i64 i128 isize str u8 u16 u32 u64 u128 usize"),
    lineComment: "//", blockComments: true, quotes: "\"'", dollar: false, colonTypes: false,
  },
  python: {
    keywords: words("and as assert async await break class continue def del elif else except finally for "
      + "from global if import in is lambda nonlocal not or pass raise return try while with yield match case"),
    literals: words("True False None"),
    types: words("bool bytes dict float int list object set str tuple"),
    lineComment: "#", blockComments: false, quotes: "\"'", dollar: false, colonTypes: false,
  },
  go: {
    keywords: words("break case chan const continue default defer else fallthrough for func go goto if "
      + "import interface map package range return select struct switch type var"),
    literals: words("true false nil iota"),
    types: words("any bool byte complex64 complex128 error float32 float64 int int8 int16 int32 int64 rune "
      + "string uint uint8 uint16 uint32 uint64 uintptr"),
    lineComment: "//", blockComments: true, quotes: "\"'`", dollar: false, colonTypes: false,
  },
};

const PUNCTUATION = /[{}[\]()<>;:,.=+\-*/%!&|^~?@#\\]/u;
const SPACE = /\s/u;
const DIGIT = /[0-9]/u;
const NUMBER = /^(?:0[xX][0-9a-fA-F_]+|0[bB][01_]+|0[oO][0-7_]+|[0-9][0-9_]*(?:\.[0-9][0-9_]*)?(?:[eE][+-]?[0-9]+)?|\.[0-9][0-9_]*(?:[eE][+-]?[0-9]+)?)[A-Za-z0-9_]*/u;

function identifierAt(text: string, from: number, dollar: boolean): string {
  const pattern = dollar ? /^[\p{L}_$][\p{L}\p{N}_$]*/u : /^[\p{L}_][\p{L}\p{N}_]*/u;
  return pattern.exec(text.slice(from))?.[0] ?? "";
}

/** Where a string that opens at `start` with `quote` ends (exclusive): its closing quote, or the line's end. */
function stringEnd(text: string, start: number, quote: string, escapes: boolean): number {
  const triple = quote.repeat(3);
  const closer = (quote === "\"" || quote === "'") && text.startsWith(triple, start) ? triple : quote;
  let index = start + closer.length;
  while (index < text.length) {
    if (escapes && text[index] === "\\") { index += 2; continue; }
    if (text.startsWith(closer, index)) return index + closer.length;
    index += 1;
  }
  return text.length;
}

/** The next non-space character at or after `from`. */
function nextVisible(text: string, from: number): string {
  let index = from;
  while (index < text.length && SPACE.test(text[index])) index += 1;
  return text[index] ?? "";
}

/** Appends a piece, joining it to the last one when they are the same kind. */
function push(tokens: SyntaxToken[], kind: SyntaxKind, text: string): void {
  if (text === "") return;
  const last = tokens.at(-1);
  if (last !== undefined && last.kind === kind) last.text += text;
  else tokens.push({ kind, text });
}

function tokenizeCode(text: string, language: CodeLanguage): SyntaxToken[] {
  const rules = RULES[language];
  const tokens: SyntaxToken[] = [];
  // A line that continues a block comment: ` * …` or the closing ` */`.
  if (rules.blockComments && /^\s*\*(?:\s|\/|$)/u.test(text)) {
    const indent = /^\s*/u.exec(text)?.[0] ?? "";
    push(tokens, "plain", indent);
    push(tokens, "comment", text.slice(indent.length));
    return tokens;
  }
  /** The kind of the last piece that was not white space. */
  let previous: SyntaxKind | null = null;
  let previousText = "";
  let index = 0;
  while (index < text.length) {
    const char = text[index];
    const rest = text.slice(index);
    if (SPACE.test(char)) {
      const run = /^\s+/u.exec(rest)![0];
      push(tokens, "plain", run);
      index += run.length;
      continue;
    }
    let kind: SyntaxKind;
    let end: number;
    if (rest.startsWith(rules.lineComment)) {
      kind = "comment";
      end = text.length;
    } else if (rules.blockComments && rest.startsWith("/*")) {
      kind = "comment";
      const close = text.indexOf("*/", index + 2);
      end = close < 0 ? text.length : close + 2;
    } else if (language === "rust" && char === "'" && text[index + 1] !== "\\" && text[index + 2] !== "'"
        && /[\p{L}_]/u.test(text[index + 1] ?? "")) {
      // A lifetime ('a, 'static), not a char literal ('a').
      kind = "keyword";
      end = index + 1 + identifierAt(text, index + 1, false).length;
    } else if (rules.quotes.includes(char)) {
      kind = "string";
      // Go's backquoted strings are raw: a backslash is just a backslash.
      end = stringEnd(text, index, char, !(language === "go" && char === "`"));
    } else if (DIGIT.test(char) || (char === "." && DIGIT.test(text[index + 1] ?? ""))) {
      kind = "number";
      end = index + (NUMBER.exec(rest)?.[0].length ?? 1);
    } else {
      const word = identifierAt(text, index, rules.dollar);
      if (word !== "") {
        end = index + word.length;
        const after = text[end] ?? "";
        // A string prefix: r"…", b'…', f"…" (Python), b"…" / r"…" (Rust).
        if (rules.quotes.includes(after) && after !== "`"
            && ((language === "python" && /^(?:[rRbBuUfF]|[rR][bBfF]|[bBfF][rR])$/u.test(word))
              || (language === "rust" && /^(?:b|r|br)$/u.test(word)))) {
          kind = "string";
          end = stringEnd(text, end, after, !/[rR]/u.test(word));
        } else if (rules.literals.has(word)) kind = "literal";
        else if (rules.keywords.has(word)) kind = "keyword";
        else if (rules.types.has(word)) kind = "type";
        else if (nextVisible(text, end) === "(" || (language === "rust" && after === "!")) kind = "function";
        else if (/^\p{Lu}/u.test(word)) kind = "type";
        else if (rules.colonTypes && previous === "punctuation" && previousText.endsWith(":")
            && !previousText.endsWith("::")) kind = "type";
        else kind = "plain";
      } else if (PUNCTUATION.test(char)) {
        kind = "punctuation";
        end = index + 1;
      } else {
        kind = "plain";
        end = index + 1;
      }
    }
    const piece = text.slice(index, end);
    push(tokens, kind, piece);
    previous = kind;
    previousText = kind === "punctuation" && tokens.at(-1)?.kind === "punctuation" ? tokens.at(-1)!.text : piece;
    index = end;
  }
  return tokens;
}

function tokenizeCss(text: string): SyntaxToken[] {
  const tokens: SyntaxToken[] = [];
  if (/^\s*\*(?:\s|\/|$)/u.test(text)) {
    const indent = /^\s*/u.exec(text)?.[0] ?? "";
    push(tokens, "plain", indent);
    push(tokens, "comment", text.slice(indent.length));
    return tokens;
  }
  let index = 0;
  while (index < text.length) {
    const char = text[index];
    const rest = text.slice(index);
    let kind: SyntaxKind = "plain";
    let end = index + 1;
    if (SPACE.test(char)) {
      end = index + /^\s+/u.exec(rest)![0].length;
    } else if (rest.startsWith("/*")) {
      kind = "comment";
      const close = text.indexOf("*/", index + 2);
      end = close < 0 ? text.length : close + 2;
    } else if (char === "\"" || char === "'") {
      kind = "string";
      end = stringEnd(text, index, char, true);
    } else if (char === "@" && /[A-Za-z]/u.test(text[index + 1] ?? "")) {
      kind = "keyword";
      end = index + 1 + /^[\w-]+/u.exec(text.slice(index + 1))![0].length;
    } else if (rest.startsWith("!important")) {
      kind = "keyword";
      end = index + "!important".length;
    } else if (char === "#" && /^#[0-9a-fA-F]{3,8}\b/u.test(rest)) {
      kind = "number";
      end = index + /^#[0-9a-fA-F]{3,8}/u.exec(rest)![0].length;
    } else if (DIGIT.test(char) || (char === "." && DIGIT.test(text[index + 1] ?? ""))) {
      kind = "number";
      end = index + /^[0-9]*\.?[0-9]+(?:[eE][+-]?[0-9]+)?(?:%|[A-Za-z]+)?/u.exec(rest)![0].length;
    } else if (rest.startsWith("--") && /[\w]/u.test(text[index + 2] ?? "")) {
      // A custom property: a token the theme names.
      kind = "type";
      end = index + 2 + /^[\w-]+/u.exec(text.slice(index + 2))![0].length;
    } else if (/[A-Za-z_-]/u.test(char)) {
      const word = /^-?[A-Za-z_][\w-]*/u.exec(rest)?.[0] ?? char;
      end = index + word.length;
      kind = text[end] === "(" ? "function" : "plain";
    } else if (PUNCTUATION.test(char)) {
      kind = "punctuation";
    }
    push(tokens, kind, text.slice(index, end));
    index = end;
  }
  return tokens;
}

function tokenizeJson(text: string): SyntaxToken[] {
  const tokens: SyntaxToken[] = [];
  let index = 0;
  while (index < text.length) {
    const char = text[index];
    const rest = text.slice(index);
    let kind: SyntaxKind = "plain";
    let end = index + 1;
    if (SPACE.test(char)) {
      end = index + /^\s+/u.exec(rest)![0].length;
    } else if (char === "\"") {
      kind = "string";
      end = stringEnd(text, index, char, true);
    } else if (char === "-" || DIGIT.test(char)) {
      const number = /^-?[0-9]+(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?/u.exec(rest)?.[0];
      if (number !== undefined) { kind = "number"; end = index + number.length; } else kind = "punctuation";
    } else if (/^(?:true|false|null)\b/u.test(rest)) {
      kind = "literal";
      end = index + /^(?:true|false|null)/u.exec(rest)![0].length;
    } else if (PUNCTUATION.test(char)) {
      kind = "punctuation";
    }
    push(tokens, kind, text.slice(index, end));
    index = end;
  }
  return tokens;
}

/** Markdown: a heading, list and quote markers, and inline code. */
function tokenizeMarkdown(text: string): SyntaxToken[] {
  const tokens: SyntaxToken[] = [];
  if (/^\s{0,3}#{1,6}(?:\s|$)/u.test(text)) {
    push(tokens, "keyword", text);
    return tokens;
  }
  const marker = /^(\s*)((?:[-*+]|[0-9]+[.)]|>)(?=\s|$))/u.exec(text);
  let index = 0;
  if (marker !== null) {
    push(tokens, "plain", marker[1]);
    push(tokens, "punctuation", marker[2]);
    index = marker[0].length;
  }
  while (index < text.length) {
    const open = text.indexOf("`", index);
    if (open < 0) { push(tokens, "plain", text.slice(index)); break; }
    const ticks = /^`+/u.exec(text.slice(open))![0];
    const close = text.indexOf(ticks, open + ticks.length);
    if (close < 0) { push(tokens, "plain", text.slice(index)); break; }
    push(tokens, "plain", text.slice(index, open));
    push(tokens, "function", text.slice(open, close + ticks.length));
    index = close + ticks.length;
  }
  return tokens;
}

/**
 * One line's pieces, by kind. Unknown languages, empty lines and lines over
 * SYNTAX_MAX_LINE characters come back as one plain piece (none for "").
 * The pieces' text always joins back to `text`.
 */
export function tokenizeLine(text: string, language: SyntaxLanguage | null): SyntaxToken[] {
  if (text === "") return [];
  if (language === null || text.length > SYNTAX_MAX_LINE) return [{ kind: "plain", text }];
  switch (language) {
    case "css": return tokenizeCss(text);
    case "json": return tokenizeJson(text);
    case "markdown": return tokenizeMarkdown(text);
    default: return tokenizeCode(text, language);
  }
}
