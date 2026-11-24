import { readFileSync } from "node:fs";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import {
  advanceFresh, blockProse, caretInline, FRESH_HOLD_MS, FRESH_JOIN_MS, FRESH_MAX_CHARS, freshParts,
  graphemeStart, Markdown, MarkdownBlocks, NO_FRESH, parseMarkdown, parseMarkdownIncremental,
  type FreshState, type MarkdownBlock, type MarkdownParse,
} from "./markdown";
import { contrastRatio } from "./contrast";

/* ==========================================================================
   SETTLING IN (Threads, owner 2026-10-07): the words a paced paint adds fade
   in, once. These pin the bookkeeping (markdown.tsx, advanceFresh) and what
   it draws: only prose, only what the paint added, never on a block the
   paint left alone, never inside code, never when the paint was not paced —
   and the Markdown underneath exactly what it was without it.
   ========================================================================== */

const parse = (text: string): readonly MarkdownBlock[] => parseMarkdown(text);

/** Paint `texts` in order at `times`, each paced unless `settle` says otherwise. */
function paints(texts: readonly string[], times: readonly number[],
    settle: (index: number) => boolean = () => true): { blocks: readonly MarkdownBlock[]; state: FreshState }[] {
  let parsed: MarkdownParse | undefined;
  let state = NO_FRESH;
  return texts.map((text, index) => {
    parsed = parseMarkdownIncremental(text, parsed);
    state = advanceFresh(state, parsed.blocks, times[index], settle(index));
    return { blocks: parsed.blocks, state };
  });
}

const FRESH_SPAN = /<span class="conv-md-fresh">([^<]*)<\/span>/gu;
const draw = (blocks: readonly MarkdownBlock[], state: FreshState, caret = false): string =>
  renderToStaticMarkup(createElement(MarkdownBlocks,
    { blocks, fresh: state.fresh, caret: caret && caretInline(blocks) }));
const CARET = '<span class="conv-caret" aria-hidden="true">\u2060</span>';
/** Markup that holds `piece` inside an element of `tag`. */
const inside = (tag: string, piece: string): RegExp =>
  new RegExp(`<${tag}[ >](?:(?!</${tag}>)[\\s\\S])*${piece}`, "u");
const freshTexts = (html: string): string[] => [...html.matchAll(FRESH_SPAN)].map(([, text]) => text);

describe("settling in — what counts as prose", () => {
  it("reads text in the order it is drawn, and never code, annotations or rules", () => {
    const [heading, paragraph, code, list, table, quote, rule] = parse([
      "# A **b**", "",
      "c `code` [d](https://e.test) ~~f~~ :codex-annotation{index=\"1\"}", "",
      "```", "g", "```", "",
      "- h", "- i", "  j", "",
      "| k | l |", "| --- | --- |", "| m | `n` |", "",
      "> o", ">", "> p", "",
      "---",
    ].join("\n"));
    expect(blockProse(heading)).toBe("A b");
    expect(blockProse(paragraph)).toBe("c  d f ");
    expect(blockProse(code)).toBe("");
    expect(blockProse(list)).toBe("hij");
    expect(blockProse(table)).toBe("klm");
    expect(blockProse(quote)).toBe("op");
    expect(blockProse(rule)).toBe("");
  });

  it("cuts a text token at its ranges, keyed by where each fresh piece starts", () => {
    const ranges = [{ from: 2, to: 5, at: 0 }, { from: 5, to: 7, at: 20 }, { from: 9, to: 12, at: 40 }];
    expect(freshParts("abcdefgh", 0, ranges)).toEqual([
      { text: "ab" }, { text: "cde", from: 2 }, { text: "fg", from: 5 }, { text: "h" },
    ]);
    // A token that starts inside a range takes only its own share of it.
    expect(freshParts("xyz", 10, ranges)).toEqual([{ text: "xy", from: 10 }, { text: "z" }]);
    expect(freshParts("plain", 20, ranges)).toEqual([{ text: "plain" }]);
  });

  it("never splits a character between two spans", () => {
    const rocket = "go 🚀";
    expect(graphemeStart(rocket, rocket.length - 1)).toBe(rocket.length - 2);
    const family = "a 👩‍👩‍👧 b";
    for (let at = 3; at < 2 + "👩‍👩‍👧".length; at += 1) expect(graphemeStart(family, at)).toBe(2);
    expect(graphemeStart("café", 4)).toBe(3);
    expect(graphemeStart("plain", 3)).toBe(3);
  });
});

describe("settling in — the bookkeeping", () => {
  it("takes its measure on the first paced paint, so what is on screen stays as it is", () => {
    const [first] = paints(["Hello there"], [0]);
    expect(first.state.fresh).toEqual([]);
    expect(first.state.blocks).toBe(first.blocks);
    // Not pacing, from the start: nothing is measured or kept at all.
    expect(advanceFresh(NO_FRESH, parse("Hello"), 0, false)).toBe(NO_FRESH);
  });

  it("adds what each paced paint extended, joining paints closer than the join window", () => {
    const steps = paints(["Hello", "Hello wor", "Hello world", "Hello world, and"],
      [0, 16, 16 + FRESH_JOIN_MS - 1, 16 + FRESH_JOIN_MS + 16]);
    expect(steps[1].state.fresh).toEqual([{ block: 0, ranges: [{ from: 5, to: 9, at: 16 }] }]);
    expect(steps[2].state.fresh).toEqual([{ block: 0, ranges: [{ from: 5, to: 11, at: 16 }] }]);
    expect(steps[3].state.fresh).toEqual([{ block: 0, ranges: [
      { from: 5, to: 11, at: 16 }, { from: 11, to: 16, at: 16 + FRESH_JOIN_MS + 16 },
    ] }]);
    expect(freshTexts(draw(steps[3].blocks, steps[3].state))).toEqual([" world", ", and"]);
  });

  it("lets each range go once its fade is over, keeping the state when nothing changed", () => {
    const [, painted] = paints(["Hello", "Hello world"], [0, 10]);
    const later = advanceFresh(painted.state, painted.blocks, 10 + FRESH_HOLD_MS - 1, true);
    expect(later).toBe(painted.state);
    const over = advanceFresh(painted.state, painted.blocks, 10 + FRESH_HOLD_MS, true);
    expect(over.fresh).toEqual([]);
    expect(over.blocks).toBe(painted.blocks);
    // Once the reveal stops pacing and nothing is fading, there is nothing to hold.
    expect(advanceFresh(over, painted.blocks, 10 + FRESH_HOLD_MS, false)).toBe(NO_FRESH);
  });

  it("finishes a fade a paint that is not paced lands on, and adds nothing for it", () => {
    const [, paced, whole] = paints(["Hello", "Hello world", "Hello world, all at once"],
      [0, 10, 20], index => index !== 2);
    expect(whole.state.fresh).toEqual(paced.state.fresh);
    expect(freshTexts(draw(whole.blocks, whole.state))).toEqual([" world"]);
  });

  it("paints more than the cap as it is", () => {
    const start = "Intro.";
    const [, flood] = paints([start, start + " word".repeat(FRESH_MAX_CHARS / 5 + 1)], [0, 16]);
    expect(flood.state.fresh).toEqual([]);
    const [, typed] = paints([start, start + " word".repeat(FRESH_MAX_CHARS / 5)], [0, 16]);
    expect(typed.state.fresh[0].ranges[0].to - typed.state.fresh[0].ranges[0].from).toBe(FRESH_MAX_CHARS);
  });

  it("leaves a settled block alone and settles a new one in from its first word", () => {
    const steps = paints(["First paragraph.\n\nSecond", "First paragraph.\n\nSecond one.\n\nThi"],
      [0, 16]);
    const [, after] = steps;
    // The first paragraph kept its parsed object: nothing about it is measured.
    expect(after.blocks[0]).toBe(steps[0].blocks[0]);
    expect(after.state.fresh).toEqual([
      { block: 1, ranges: [{ from: 6, to: 11, at: 16 }] },
      { block: 2, ranges: [{ from: 0, to: 3, at: 16 }] },
    ]);
    const html = draw(after.blocks, after.state);
    expect(html.startsWith('<p class="conv-md-p">First paragraph.</p>')).toBe(true);
    expect(freshTexts(html)).toEqual([" one.", "Thi"]);
  });

  it("paints text laid out again as it now is, never fading it a second time", () => {
    // `**bo` is literal asterisks until the pair closes; then the same words
    // are a different shape. They were already faded in once.
    const steps = paints(["Look at **bo", "Look at **bold", "Look at **bold**"], [0, 16, 32]);
    expect(steps[1].state.fresh).toEqual([{ block: 0, ranges: [{ from: 12, to: 14, at: 16 }] }]);
    expect(steps[2].state.fresh).toEqual([]);
    expect(draw(steps[2].blocks, steps[2].state)).toBe('<p class="conv-md-p">Look at <strong>bold</strong></p>');
    // A paragraph that becomes a table is painted, not faded in whole.
    const head = "| a | b |\n| --- | --- |\n|";
    const table = paints([head, `${head} 1`, `${head} 1 | 2`], [0, 16, 32]);
    expect(table[1].blocks[0].kind).toBe("paragraph");
    expect(freshTexts(draw(table[1].blocks, table[1].state))).toEqual([" 1"]);
    expect(table[2].blocks[0].kind).toBe("table");
    expect(table[2].state.fresh).toEqual([]);
  });

  it("never measures code: a growing fence adds nothing, and prose after it settles in", () => {
    const steps = paints(["Run:\n\n```sh\nnpm", "Run:\n\n```sh\nnpm test", "Run:\n\n```sh\nnpm test\n```\n\nDone"],
      [0, 16, 32]);
    expect(steps[1].state.fresh).toEqual([]);
    expect(steps[2].state.fresh).toEqual([{ block: 2, ranges: [{ from: 0, to: 4, at: 32 }] }]);
    expect(draw(steps[2].blocks, steps[2].state)).toContain("<code>npm test</code>");
  });

  it("starts a range at a character the last paint split, never inside it", () => {
    const rocket = "Ship it 🚀";
    const steps = paints(["Ship it", rocket.slice(0, -1), rocket], [0, 16, 16 + FRESH_JOIN_MS]);
    expect(steps[1].state.fresh[0].ranges).toEqual([{ from: 7, to: 9, at: 16 }]);
    expect(steps[2].state.fresh[0].ranges).toEqual([
      { from: 7, to: 8, at: 16 }, { from: 8, to: 10, at: 16 + FRESH_JOIN_MS },
    ]);
    expect(freshTexts(draw(steps[2].blocks, steps[2].state))).toEqual([" ", "🚀"]);
  });
});

/* --------------------------------------------------------------------------
   A STREAM, PAINT BY PAINT. Everything a message is drawn as while it
   streams, grown in uneven steps at 60fps through the incremental parse.
   -------------------------------------------------------------------------- */

const STREAMED = [
  "# Findings", "", "The panes keep **their own** drafts. Code stays `distinct`:", "",
  "```ts", "const pane = panes.get(key);", "", "if (!pane) return;", "```", "",
  "| Area | State |", "| :--- | ---: |", "| Chat | Ready |", "| Review | `a | b` |", "",
  "- first [link](https://example.test)", "- second", "  continued", "",
  "> A note", ">", "> in two parts", "", "Done 🚀 with *emphasis* :codex-annotation{index=\"2\"}",
].join("\n");

function streamed(settle: boolean): { text: string; blocks: readonly MarkdownBlock[];
    before: FreshState; state: FreshState; html: string }[] {
  let parsed: MarkdownParse | undefined;
  let state = NO_FRESH;
  const out = [];
  for (let end = 0, step = 0; end < STREAMED.length; step += 1) {
    end = Math.min(STREAMED.length, end + 1 + step % 6);
    const text = STREAMED.slice(0, end);
    parsed = parseMarkdownIncremental(text, parsed);
    const before = state;
    state = advanceFresh(state, parsed.blocks, step * (1000 / 60), settle);
    out.push({ text, blocks: parsed.blocks, before, state, html: draw(parsed.blocks, state, true) });
  }
  return out;
}

describe("settling in — a whole stream", () => {
  const steps = streamed(true);

  it("draws exactly the Markdown it would without settling in or a caret, at every paint", () => {
    for (const { text, html } of steps) {
      expect(html.replace(FRESH_SPAN, "$1").replace(CARET, ""), text).toBe(draw(parse(text), NO_FRESH));
    }
    expect(steps.some(({ html }) => html.includes("conv-md-fresh"))).toBe(true);
    expect(steps.some(({ html }) => html.includes(CARET))).toBe(true);
  });

  it("never wraps code, fenced or inline, and never puts the caret in code or a table", () => {
    for (const { text, html } of steps) {
      for (const tag of ["code", "pre"]) expect(html, text).not.toMatch(inside(tag, "conv-md-fresh"));
      for (const tag of ["code", "pre", "table"]) expect(html, text).not.toMatch(inside(tag, "conv-caret"));
    }
  });

  it("adds nothing to a block the paint left alone, and keeps its ranges by identity", () => {
    for (let index = 1; index < steps.length; index += 1) {
      const { blocks, before, state } = steps[index];
      const previous = steps[index - 1].blocks;
      for (const entry of state.fresh) {
        if (previous[entry.block] !== blocks[entry.block]) continue;
        const was = before.fresh.find(other => other.block === entry.block)?.ranges ?? [];
        for (const range of entry.ranges) expect(was).toContain(range);
      }
    }
  });

  it("keeps the span count bounded and every range inside its block's prose", () => {
    const most = Math.ceil(FRESH_HOLD_MS / FRESH_JOIN_MS) + 1;
    for (const { blocks, state } of steps) {
      for (const entry of state.fresh) {
        expect(entry.ranges.length).toBeLessThanOrEqual(most);
        for (const range of entry.ranges) {
          expect(range.from).toBeLessThan(range.to);
          expect(range.to).toBeLessThanOrEqual(blockProse(blocks[entry.block]).length);
        }
      }
    }
  });

  it("wraps nothing at all when no paint is paced", () => {
    for (const { html, state } of streamed(false)) {
      expect(html).not.toContain("conv-md-fresh");
      expect(state).toBe(NO_FRESH);
    }
  });
});

describe("the Threads caret", () => {
  const paint = (text: string, caret?: boolean): string =>
    renderToStaticMarkup(createElement(Markdown, caret === undefined ? { text } : { text, caret }));

  it("follows the last painted word inside a prose block's final line", () => {
    expect(paint("One\n\nTwo **words**", true))
      .toBe(`<div class="conv-markdown"><p class="conv-md-p">One</p><p class="conv-md-p">Two <strong>words</strong>${CARET}</p></div>`);
    expect(paint("Line one\nline two", true)).toContain(`line two${CARET}</p>`);
    expect(paint("## Heading", true)).toContain(`Heading${CARET}</h2>`);
    expect(paint("- a\n- b\n  more", true)).toContain(`<li>b<br/>more${CARET}</li></ul>`);
    expect(paint("> quoted\n>\n> last", true)).toContain(`last${CARET}</p></blockquote>`);
  });

  it("stands after the block, as every pane's does, when the last block is not prose", () => {
    for (const text of ["Run:\n\n```sh\nnpm test", "| a | b |\n| --- | --- |\n| 1 | 2 |", "Done\n\n---",
      "> quoted\n>\n> ```\n> code", ""]) {
      const html = paint(text, true);
      expect(html, text).toMatch(new RegExp(`</div>${CARET}$`, "u"));
      expect(html.split("conv-caret").length, text).toBe(2);
    }
  });

  it("is one hidden mark, and the Markdown is unchanged around it at every prefix", () => {
    for (let end = 0; end <= STREAMED.length; end += 1) {
      const text = STREAMED.slice(0, end);
      const html = paint(text, true);
      expect(html.split("conv-caret").length, text).toBe(2);
      expect(html.replace(CARET, ""), text).toBe(paint(text));
      for (const tag of ["code", "pre", "table"]) expect(html, text).not.toMatch(inside(tag, "conv-caret"));
    }
    // Not streaming, and in every other pane, there is no caret here at all.
    expect(paint("Done.", false)).toBe(paint("Done."));
    expect(paint("Done.")).not.toContain("conv-caret");
  });
});

describe("settling in — the Threads stylesheet", () => {
  const read = (path: string): string => readFileSync(new URL(path, import.meta.url), "utf8");
  const css = read("./components/Conversation.css").replace(/\/\*[\s\S]*?\*\//gu, "");
  const rules = [...css.matchAll(/([^{}]+)\{([^{}]*)\}/gu)]
    .map(([, selector, body]) => ({ selector: selector.trim(), body }));

  it("fades and draws the bar only in a Threads pane, in the accent and never a hue", () => {
    for (const rule of rules) {
      if (!/conv-md-fresh|conv-caret/u.test(rule.selector) || rule.selector === ".conv-caret") continue;
      expect(rule.selector).toMatch(/\.conv\.is-thread-layout \.conv-(?:md-fresh|caret(?:::after)?)$/u);
      // The Threads look (owner, 2026-10-23): the project's hue no longer tints it.
      expect(rule.body, rule.selector).not.toContain("--project-h");
    }
    expect(css).toContain(".conv.is-thread-layout .conv-md-fresh { animation:conv-settle-in 240ms ease-out; }");
    expect(css).toContain("@keyframes conv-settle-in { from { opacity:0; filter:blur(3px); } }");
    expect(css).toContain(".conv.is-thread-layout .conv-caret { animation:conv-blink 1s steps(1) infinite; }");
    expect(css).toMatch(/\.conv\.is-thread-layout \.conv-caret::after \{[^}]*width:2px;[^}]*background:var\(--accent\);[^}]*border-radius:var\(--radius-pill\);/u);
    // The fade is off outright for a reader who asked for less motion, and the
    // caret keeps the class theme.css's guard holds at full opacity.
    expect(css).toMatch(/prefers-reduced-motion: reduce\) \{\s*\.conv\.is-thread-layout \.conv-md-fresh \{ animation:none; \}/u);
    expect(read("./theme.css")).toMatch(/prefers-reduced-motion: reduce[\s\S]*\.conv-caret \{ opacity: 1; \}/u);
  });

  it("draws the accent caret at 3:1 against the pane in every palette", () => {
    const theme = read("./theme.css");
    const rootBlock = theme.match(/:root \{([\s\S]*?)\n\}/u)?.[1] ?? "";
    const blocks = [rootBlock, ...[...theme.matchAll(/:root\[data-theme="(?!custom-)[^"]+"\] \{([\s\S]*?)\n\}/gu)]
      .map(([, body]) => body)];
    expect(blocks.length).toBeGreaterThanOrEqual(8);
    const hex = (body: string, name: string): string =>
      (body.match(new RegExp(`--${name}:\\s*(#[0-9a-f]{6})`, "u"))
        ?? rootBlock.match(new RegExp(`--${name}:\\s*(#[0-9a-f]{6})`, "u")))![1];
    const failures: string[] = [];
    for (const body of blocks) {
      const ratio = contrastRatio(hex(body, "accent"), hex(body, "editor-bg"));
      if (ratio < 3) failures.push(`${hex(body, "accent")} on ${hex(body, "editor-bg")}: ${ratio.toFixed(2)}`);
    }
    expect(failures).toEqual([]);
  });
});
