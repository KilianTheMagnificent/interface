import { describe, expect, it } from "vitest";
import { isQuestionTool, parseAgentQuestions, questionSummary } from "./questions";

const input = {
  questions: [
    {
      question: "Which library should we use?",
      header: "Library",
      multiSelect: false,
      options: [
        { label: "date-fns", description: "Tree-shakeable" },
        { label: "luxon", description: "Timezone-first", preview: "DateTime.now()" },
      ],
    },
  ],
};

describe("parseAgentQuestions", () => {
  it("reads Claude AskUserQuestion payloads", () => {
    expect(parseAgentQuestions("AskUserQuestion", input)).toEqual([
      {
        question: "Which library should we use?",
        header: "Library",
        multiSelect: false,
        options: [
          { label: "date-fns", description: "Tree-shakeable" },
          { label: "luxon", description: "Timezone-first", preview: "DateTime.now()" },
        ],
      },
    ]);
  });

  it("accepts the same shape on Grok-style tool names", () => {
    expect(parseAgentQuestions("ask_user_question", input)?.[0]?.header).toBe("Library");
    expect(isQuestionTool("AskUserQuestion")).toBe(true);
    expect(isQuestionTool("Bash")).toBe(false);
  });

  it("ignores a normal approval payload", () => {
    expect(parseAgentQuestions("Bash", { command: "ls" })).toBeUndefined();
  });

  it("drops a questions array that is not actually questions", () => {
    expect(parseAgentQuestions("AskUserQuestion", { questions: [{ nope: true }] }))
      .toEqual([]);
  });
});

describe("questionSummary", () => {
  it("uses the first question, and counts when there are several", () => {
    const one = parseAgentQuestions("AskUserQuestion", input) ?? [];
    expect(questionSummary(one)).toBe("Which library should we use?");
    expect(questionSummary([...one, ...one])).toMatch(/^2 questions:/);
  });
});
