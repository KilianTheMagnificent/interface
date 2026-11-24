/** Presentation names only. Routing and saved provider IDs stay unchanged. */
export function providerLabel(engine: string, advertised?: string): string {
  switch (engine) {
    case "claude": return "Claude Code";
    case "codex": return "Codex";
    case "acp:grok": case "grok": return "Grok";
    case "acp:qwen": case "qwen": return "Qwen";
    case "acp:gemini": case "antigravity": return "Antigravity";
    case "acp:opencode": return "OpenCode";
    case "acp:muse": return "Muse Code";
    case "acp:kimi": return "Kimi Code";
    default: return advertised?.trim() || engine.replace(/^acp:/, "").trim() || "Custom provider";
  }
}
