import { describe, expect, it } from "vitest";
import { THREAD_STEP_TARGET_MAX, threadStepLine, threadStepWords } from "./thread-live";

const ROOT = "/Users/ann/Developer/atlas-web";

describe("a live turn's words for one tool call", () => {
  it("says a file step project-relative, in the state's tense, never the absolute path", () => {
    const edit = { tool: "Edit", summary: `${ROOT}/src/routes/login.ts`,
      input: { file_path: `${ROOT}/src/routes/login.ts`, old_string: "a", new_string: "b" } };
    expect(threadStepWords(edit, "running", ROOT)).toEqual({
      verb: "Editing", target: "src/routes/login.ts", short: "login.ts", mono: true });
    expect(threadStepLine(threadStepWords(edit, "done", ROOT))).toBe("Edited src/routes/login.ts");
    expect(threadStepLine(threadStepWords(edit, "waiting", ROOT), true)).toBe("Edit login.ts");
    const write = { tool: "Write", summary: "", input: { file_path: `${ROOT}/test/a.test.ts`, content: "x" } };
    expect(threadStepLine(threadStepWords(write, "running", ROOT))).toBe("Writing test/a.test.ts");
    expect(threadStepLine(threadStepWords(write, "done", ROOT))).toBe("Wrote test/a.test.ts");
    const read = { tool: "Read", summary: `${ROOT}/README.md`, input: { file_path: `${ROOT}/README.md` } };
    expect(threadStepLine(threadStepWords(read, "running", ROOT))).toBe("Reading README.md");
  });

  it("runs a command by its first line, and searches by the pattern Grep leaves out of its summary", () => {
    const bash = { tool: "Bash", summary: "Run the login tests",
      input: { command: "npm test -- test/login.test.ts\necho done", description: "Run the login tests" } };
    expect(threadStepWords(bash, "running", ROOT)).toEqual({
      verb: "Running", target: "npm test -- test/login.test.ts",
      short: "npm test -- test/login.test.ts", mono: true });
    expect(threadStepLine(threadStepWords(bash, "waiting", ROOT))).toBe("Run npm test -- test/login.test.ts");
    const codex = { tool: "commandExecution", summary: "cargo test -p voice", input: {} };
    expect(threadStepLine(threadStepWords(codex, "done", ROOT))).toBe("Ran cargo test -p voice");
    const grep = { tool: "Grep", summary: "Grep", input: { pattern: "session.verify" } };
    expect(threadStepLine(threadStepWords(grep, "done", ROOT))).toBe('Searched "session.verify"');
  });

  it("falls back to the tool's own name, and to a shortened summary only when it says more", () => {
    expect(threadStepLine(threadStepWords({ tool: "mcp__linear__create_issue", summary: "create_issue", input: {} },
      "running", ROOT))).toBe("Create issue");
    expect(threadStepLine(threadStepWords({ tool: "SomeTool", summary: "SomeTool", input: null },
      "running", ROOT))).toBe("Some tool");
    expect(threadStepLine(threadStepWords({ tool: "TodoWrite", summary: "3 todos", input: {} },
      "running", ROOT))).toBe("Updating the plan");
    expect(threadStepLine(threadStepWords({ tool: "Task", summary: "Find callers",
      input: { description: "Find callers of verify" } }, "running", ROOT)))
      .toBe("Delegating Find callers of verify");
  });

  it("keeps a live line to one bounded line", () => {
    const long = "x".repeat(THREAD_STEP_TARGET_MAX * 2);
    const words = threadStepWords({ tool: "Bash", summary: "", input: { command: long } }, "running", ROOT);
    expect(words.target.length).toBe(THREAD_STEP_TARGET_MAX);
    expect(words.target.endsWith("…")).toBe(true);
  });
});
