import type { ConvItem } from "../conversation-reducer";
import { lastSegment } from "../display-paths";
import { toolActivityKind } from "../tool-activity";
import type { ThreadGlyphName } from "./ThreadGlyph";

/**
 * How the Threads pieces draw (owner, 2026-10-23, the Threads look): the
 * glyph a step wears on the trail's rail, and a changed file's path split so
 * its directory can be dimmed. Pure and total; ThreadLiveTurn.tsx and
 * ThreadTurnParts.tsx only draw what these say.
 */

const key = (tool: string): string => tool.trim().toLowerCase().replace(/[\s_-]+/gu, "");

/**
 * The glyph for one tool name, by the same reading thread-live.ts gives the
 * step its verb, so "Wrote" wears the new-file glyph and "Ran" the terminal.
 * A tool nobody taught this module (an MCP call, a skill) is the generic tool.
 */
export function threadToolGlyph(tool: string): ThreadGlyphName {
  const name = key(tool);
  if (name === "write" || name === "writefile") return "write";
  if (name === "webfetch" || name === "fetch" || name === "websearch") return "web";
  if (name === "task" || name === "agent") return "agent";
  if (name === "todowrite" || name === "updateplan" || name === "exitplanmode") return "plan";
  switch (toolActivityKind(tool)) {
    case "read": return "read";
    case "edit": return "edit";
    case "search":
    case "list": return "search";
    case "command": return "run";
    default: return "tool";
  }
}

/**
 * The glyph a trail step wears on the rail, from the item it stands for: its
 * call (or the call an approval asks for) by tool name, a subagent as an
 * agent, a file change with no call of its own as an edit.
 */
export function threadStepGlyph(item: ConvItem): ThreadGlyphName {
  switch (item.kind) {
    case "tool":
    case "approval":
      return threadToolGlyph(item.tool);
    case "subagent":
      return "agent";
    case "diff":
      return "edit";
    default:
      return "tool";
  }
}

/**
 * A shown path as its directory, separator included, and its name, so a row
 * can dim the one and keep the other in the text colour. Joined, the two are
 * always the path as given; a bare name has no directory.
 */
export function threadPathParts(shown: string): { dir: string; name: string } {
  const name = lastSegment(shown);
  const end = name === "" ? -1 : shown.lastIndexOf(name);
  return end <= 0 ? { dir: "", name: shown } : { dir: shown.slice(0, end), name: shown.slice(end) };
}
