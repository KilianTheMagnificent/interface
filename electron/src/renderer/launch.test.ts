import { describe, it, expect } from "vitest";
import { buildLaunchOptions, DEFAULT_ENGINE, type ComposerForm, type LaunchRequest,
} from "./launch";

const base: ComposerForm = {
  engine: "claude",
  cwd: "/tmp/scratch",
  prompt: "Say READY and stop.",
  model: "claude-sonnet-5",
  effort: "default",
  permissionMode: "acceptEdits",
};

function ok(form: Partial<ComposerForm>): LaunchRequest {
  const result = buildLaunchOptions({ ...base, ...form });
  if ("error" in result) throw new Error(`expected options, got error: ${result.error}`);
  return result;
}

function err(form: Partial<ComposerForm>): string {
  const result = buildLaunchOptions({ ...base, ...form });
  if (!("error" in result)) throw new Error("expected an error, got options");
  return result.error;
}

describe("buildLaunchOptions", () => {
  it("rejects an empty prompt", () => {
    expect(err({ prompt: "" })).toMatch(/prompt/i);
  });

  it("rejects a whitespace-only prompt", () => {
    expect(err({ prompt: "   \n\t " })).toMatch(/prompt/i);
  });

  it("rejects an empty cwd", () => {
    expect(err({ cwd: "" })).toMatch(/director/i);
  });

  it("rejects a whitespace-only cwd", () => {
    expect(err({ cwd: "   " })).toMatch(/director/i);
  });

  it("reports the missing directory first when both are empty", () => {
    expect(err({ cwd: "", prompt: "" })).toMatch(/director/i);
  });

  it("trims the prompt", () => {
    expect(ok({ prompt: "  hello  \n" }).prompt).toBe("hello");
  });

  it("sends a form's name as the session title, and none when it has none", () => {
    expect(ok({ name: " Fix the login · Opus 5 " }).name).toBe("Fix the login · Opus 5");
    expect("name" in ok({})).toBe(false);
    expect("name" in ok({ name: "  " })).toBe(false);
  });

  it("trims the cwd", () => {
    expect(ok({ cwd: "  /tmp/scratch  " }).cwd).toBe("/tmp/scratch");
  });

  it("requires a concrete model", () => {
    expect(err({ model: "" })).toMatch(/choose a model/i);
    expect(err({ model: "default" })).toMatch(/choose a model/i);
  });

  it("passes an explicit model through", () => {
    expect(ok({ model: "claude-opus-5" }).model).toBe("claude-opus-5");
  });

  it("passes permissionMode through", () => {
    expect(ok({ permissionMode: "plan" }).permissionMode).toBe("plan");
  });

  // Same rule as model, and for a sharper reason: the SDK's EffortLevel union
  // has no "default" member, so the word must never leave this function.
  it("maps effort 'default' to undefined", () => {
    expect(ok({ effort: "default" }).effort).toBeUndefined();
  });

  it("maps an empty effort to undefined", () => {
    expect(ok({ effort: "" }).effort).toBeUndefined();
    expect(ok({ effort: "   " }).effort).toBeUndefined();
  });

  it("passes an explicit effort through, including the session-scoped max", () => {
    expect(ok({ effort: "high" }).effort).toBe("high");
    expect(ok({ effort: "max" }).effort).toBe("max");
  });

  it("builds the full options object for a valid form", () => {
    expect(buildLaunchOptions({
      engine: "claude",
      cwd: "/work",
      prompt: " build it ",
      model: "claude-sonnet-5",
      effort: "xhigh",
      permissionMode: "default",
    })).toEqual({
      engine: "claude",
      cwd: "/work",
      prompt: "build it",
      model: "claude-sonnet-5",
      effort: "xhigh",
      permissionMode: "default",
    });
  });

  it("omits effort while retaining the explicitly chosen model", () => {
    expect(buildLaunchOptions({
      engine: "claude", cwd: "/work", prompt: "go", model: "claude-sonnet-5",
      effort: "default", permissionMode: "default",
    })).toEqual({ engine: "claude", cwd: "/work", prompt: "go",
      model: "claude-sonnet-5",
      permissionMode: "default" });
  });
});

// The engine is what the bridge routes on: it picks the adapter that will make
// the session, and it becomes the `engine#id` prefix of every key that session
// is ever addressed by. It must survive this function verbatim.
describe("buildLaunchOptions engine", () => {
  it("passes the selected engine through", () => {
    expect(ok({ engine: "codex" }).engine).toBe("codex");
    expect(ok({ engine: "acp:gemini" }).engine).toBe("acp:gemini");
  });

  it("keeps the engine out of the fields the adapter reads", () => {
    // Belt and braces on the split: whatever else the request carries, `engine`
    // must not have leaked into a field an adapter interprets.
    const opts = ok({ engine: "codex", model: "gpt-5", effort: "high" });
    expect(opts).toMatchObject({ engine: "codex", model: "gpt-5", effort: "high" });
    expect(opts.cwd).toBe("/tmp/scratch");
  });

  it("routes the same raw model id independently for each provider", () => {
    expect(ok({ engine: "claude", model: "shared-model" })).toMatchObject({
      engine: "claude", model: "shared-model",
    });
    expect(ok({ engine: "codex", model: "shared-model" })).toMatchObject({
      engine: "codex", model: "shared-model",
    });
  });

  it("keeps each provider route with an explicit model", () => {
    expect(ok({ engine: "claude", model: "claude-opus-5" }))
      .toMatchObject({ engine: "claude", model: "claude-opus-5" });
    expect(ok({ engine: "codex", model: "gpt-5.6-sol" }))
      .toMatchObject({ engine: "codex", model: "gpt-5.6-sol" });
    expect(ok({ engine: "acp:grok", model: "grok-build" }))
      .toMatchObject({ engine: "acp:grok", model: "grok-build" });
  });

  it("trims the engine", () => {
    expect(ok({ engine: "  codex  " }).engine).toBe("codex");
  });

  // A blank field is the composer having nothing to offer yet (the probe has
  // not answered) — not a reason to refuse the launch.
  it("defaults a blank engine rather than rejecting it", () => {
    expect(ok({ engine: "" }).engine).toBe(DEFAULT_ENGINE);
    expect(ok({ engine: "   " }).engine).toBe(DEFAULT_ENGINE);
    expect(DEFAULT_ENGINE).toBe("claude");
  });

  // Field order is unchanged by the new field: a blank engine plus a blank cwd
  // still reports the directory, because the engine has no failure of its own.
  it("still reports the directory first when the engine is blank too", () => {
    expect(err({ engine: "", cwd: "" })).toMatch(/director/i);
  });
});

// Which surface is launching. "code" is what every session was before the
// field existed, so it stays off the wire; the other two must survive.
describe("buildLaunchOptions kind", () => {
  it("does not require a working directory for a chat", () => {
    const r = ok({ cwd: "", kind: "chat" });
    expect(r.cwd).toBe("");
    expect(r.kind).toBe("chat");
  });

  it("still requires one for code and agent launches", () => {
    expect(err({ cwd: "" })).toBe("Working directory is required.");
    expect(err({ cwd: "   ", kind: "agent" })).toBe("Working directory is required.");
    expect(ok({ kind: "agent" }).kind).toBe("agent");
  });

  it("leaves kind off the wire for code", () => {
    expect("kind" in ok({})).toBe(false);
    expect("kind" in ok({ kind: "code" })).toBe(false);
  });
});
