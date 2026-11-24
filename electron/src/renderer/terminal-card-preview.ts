/**
 * A bounded display projection of xterm text; never changes its buffer or
 * title. `limit` is how many lines the caller shows: the workspace cards take
 * two, the agent overview's live cards up to three.
 */
export function terminalCardPreview(lines: readonly string[], cwd: string, profileId?: string,
    limit = 2): string[] {
  const agent = profileId !== undefined && ["claude", "codex", "grok"].includes(profileId);
  const homePath = cwd.replace(/^\/Users\/[^/]+(?=\/|$)/, "~");
  const result: string[] = [];
  for (const raw of lines.slice(-40)) {
    const line = raw.trim();
    if (!line || /^[\s─━═┄┈┼┬┴│┃╭╮╰╯┌┐└┘+\-]+$/.test(line)) continue;
    if (line === cwd || line === homePath) continue;
    if (profileId === "codex") {
      if (/^•?\s*You have \d+ usage limit resets? available\. Run \/usage to use one\.$/.test(line)) continue;
      if (/^│\s*(?:>_ OpenAI Codex|model:|directory:)/.test(line)) continue;
      if (/^Tip: Try the Desktop app\. Run 'codex app' or visit$/.test(line)
          || line === "https://chatgpt.com/codex?app-landing-page=true") continue;
    }
    if (agent) {
      // Claude Code's prompt box and mode line are chrome, not progress.
      if (/^│.*│$/.test(line) && /^[›>❯]?$/.test(line.slice(1, -1).trim())) continue;
      if (/^(?:⏵⏵|⏸)\s/.test(line) || line.includes("(shift+tab to cycle)")) continue;
      if (/^[›>❯]\s*(?:Ask Codex to do anything|Try ["“].*["”]|Type your message(?: here)?|What would you like to do\??)?\s*$/i.test(line)) continue;
      if (/^(?:gpt-|claude-|opus|sonnet|haiku|grok-|qwen)/i.test(line)
          && (line.includes(" · ") || line.includes("/model"))) continue;
      if (/^(?:\? for shortcuts|\d+(?:\.\d+)?% context left)\s*$/i.test(line)) continue;
    }
    // Provider UIs indent soft-wrapped continuation lines. Keep a reply
    // together so its last word does not become the card's whole summary.
    const previous = result.at(-1);
    if (agent && previous !== undefined && /^ {2,}\S/.test(raw)
        && !/^[•›>❯│]/.test(line) && !/^[›>❯]/.test(previous)) {
      result[result.length - 1] = `${previous} ${line}`;
    // Prose a provider wrapped at its own width without an indent (Codex's
    // answers): a lower-case line reads on from one that has not finished
    // its sentence. A capitalised line is a line of its own.
    } else if (agent && previous !== undefined && !/^[›>❯]/.test(previous)
        && !/[.!?:)]$/u.test(previous) && /^\p{Ll}/u.test(line)) {
      result[result.length - 1] = `${previous} ${line}`;
    } else if (previous !== line) result.push(line);
  }
  return result.slice(-Math.max(0, limit));
}
