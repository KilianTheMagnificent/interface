import { describe, expect, it } from "vitest";
import { providerLabel } from "./provider-labels";

describe("provider display names", () => {
  it("uses the launcher names even when a built-in probe advertises a different name", () => {
    expect(providerLabel("claude", "Anthropic Claude")).toBe("Claude Code");
    expect(providerLabel("codex", "OpenAI Codex")).toBe("Codex");
    expect(providerLabel("acp:grok")).toBe("Grok");
    expect(providerLabel("acp:qwen")).toBe("Qwen");
    expect(providerLabel("acp:gemini", "Gemini (Antigravity)")).toBe("Antigravity");
    expect(providerLabel("acp:opencode", "OpenCode Go")).toBe("OpenCode");
  });
  it("names Meta's terminal agent Muse Code", () => {
    expect(providerLabel("acp:muse", "Muse")).toBe("Muse Code");
  });
  it("names Moonshot's terminal agent Kimi Code", () => {
    expect(providerLabel("acp:kimi", "Kimi")).toBe("Kimi Code");
  });
  it("preserves custom provider names and useful unknown adapter names", () => {
    expect(providerLabel("acp:studio", " Studio Agent ")).toBe("Studio Agent");
    expect(providerLabel("acp:studio")).toBe("studio");
    expect(providerLabel("", " ")).toBe("Custom provider");
  });
  it("names the usage menu's engines, bare aliases included", () => {
    expect(providerLabel("claude")).toBe("Claude Code");
    expect(providerLabel("codex")).toBe("Codex");
    expect(providerLabel("grok")).toBe("Grok");
    expect(providerLabel("acp:gemini")).toBe("Antigravity");
    expect(providerLabel("acp:local-agent", "Local Agent")).toBe("Local Agent");
    expect(providerLabel("acp:local-agent")).toBe("local-agent");
  });
});
