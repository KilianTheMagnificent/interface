import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import {
  COLLABORATION_SOURCE_ENGINES, createDraftStore, draftWriteNeedsPublish,
  isCollaborationSourceKey, nextDraftRevision,
} from "./draft-store";

describe("the composer draft store", () => {
  it("keeps one draft per session key, and reads a missing one as empty", () => {
    const drafts = createDraftStore({ initial: { "claude#a": "Fix the parser", "claude#b": "" } });
    expect(drafts.get("claude#a")).toBe("Fix the parser");
    expect(drafts.get("claude#b")).toBe("");
    expect(drafts.get("codex#never")).toBe("");
    drafts.set("claude#b", "Review it");
    expect(drafts.get("claude#a")).toBe("Fix the parser");
    expect(drafts.get("claude#b")).toBe("Review it");
  });

  it("tells only that session's readers, and only when the text changed", () => {
    const drafts = createDraftStore();
    const heard: string[] = [];
    const stopA = drafts.subscribe("claude#a", () => { heard.push(`a:${drafts.get("claude#a")}`); });
    drafts.subscribe("claude#a", () => { heard.push("a2"); });
    drafts.subscribe("claude#b", () => { heard.push("b"); });
    drafts.set("claude#a", "F");
    drafts.set("claude#a", "F");
    drafts.set("claude#a", "Fi");
    expect(heard).toEqual(["a:F", "a2", "a:Fi", "a2"]);
    stopA();
    stopA();
    drafts.set("claude#a", "");
    expect(heard).toEqual(["a:F", "a2", "a:Fi", "a2", "a2"]);
  });

  it("reports every write to its owner, including one that changes nothing", () => {
    // App publishes each write as the advisory collaboration draft state, as it
    // did for every change when drafts were App state.
    const writes: [string, string][] = [];
    const drafts = createDraftStore({ onWrite: (key, text) => { writes.push([key, text]); } });
    drafts.set("claude#a", "Run");
    drafts.set("claude#a", "Run");
    drafts.set("claude#a", "");
    expect(writes).toEqual([["claude#a", "Run"], ["claude#a", "Run"], ["claude#a", ""]]);
  });

  it("forgets a closed session's draft without reporting a write", () => {
    const writes: string[] = [];
    const drafts = createDraftStore({
      initial: { "claude#a": "Half typed" },
      onWrite: key => { writes.push(key); },
    });
    const heard: string[] = [];
    drafts.subscribe("claude#a", () => { heard.push(drafts.get("claude#a")); });
    drafts.delete("claude#a");
    drafts.delete("claude#a");
    expect(drafts.get("claude#a")).toBe("");
    expect(heard).toEqual([""]);
    expect(writes).toEqual([]);
  });
});

describe("the collaboration draft revision", () => {
  // The host's rule: a session's revision must exceed every one it accepted
  // earlier in its lifetime, across renderer reloads and listing re-entries.
  function hostAccepts() {
    const accepted = new Map<string, number>();
    return (key: string, revision: number): boolean => {
      const current = accepted.get(key);
      if (current !== undefined && revision <= current) return false;
      accepted.set(key, revision);
      return true;
    };
  }

  it("stays above the host's mark after a reload forgets the counter", () => {
    const accept = hostAccepts();
    let now = 1_790_000_000_000;
    const before = new Map<string, number>();
    const write = (): void => {
      const revision = nextDraftRevision(before.get("claude#a"), now);
      before.set("claude#a", revision);
      expect(accept("claude#a", revision)).toBe(true);
    };
    for (let keystroke = 0; keystroke < 500; keystroke += 1) {
      write();
      now += 40;
    }
    // A send clears the draft and publishes it again within the same tick.
    write();
    write();
    write();
    // A reload forgets the counter; counting from 1 again fell below the host's mark.
    now += 250;
    const after = new Map<string, number>();
    const revision = nextDraftRevision(after.get("claude#a"), now);
    expect(revision).toBeGreaterThan(before.get("claude#a")!);
    expect(accept("claude#a", revision)).toBe(true);
  });

  it("stays above the host's mark when an archived session is restored", () => {
    const accept = hostAccepts();
    const counters = new Map<string, number>();
    const publish = (now: number): boolean => {
      const revision = nextDraftRevision(counters.get("codex#b"), now);
      counters.set("codex#b", revision);
      return accept("codex#b", revision);
    };
    expect(publish(1_790_000_000_000)).toBe(true);
    expect(publish(1_790_000_000_400)).toBe(true);
    counters.delete("codex#b");
    expect(publish(1_790_000_009_000)).toBe(true);
  });

  it("increases strictly within one document even when the clock does not", () => {
    let previous: number | undefined;
    const seen: number[] = [];
    for (const now of [1_000, 1_000, 1_000, 999, 1_001]) {
      previous = nextDraftRevision(previous, now);
      seen.push(previous);
    }
    expect(seen).toEqual([1_000, 1_001, 1_002, 1_003, 1_004]);
    expect(Number.isSafeInteger(nextDraftRevision(undefined, Date.now()))).toBe(true);
  });
});

describe("the collaboration draft publish", () => {
  // App's wiring in miniature: the store's writes go through
  // draftWriteNeedsPublish, the outbound lane's publish after a send does not.
  function harness() {
    const published: { key: string; empty: boolean }[] = [];
    const lastEmpty = new Map<string, boolean>();
    const publish = (key: string, text: string): void => {
      if (!isCollaborationSourceKey(key)) return;
      lastEmpty.set(key, text.length === 0);
      published.push({ key, empty: text.length === 0 });
    };
    const drafts = createDraftStore({
      onWrite: (key, text) => {
        if (draftWriteNeedsPublish(lastEmpty.get(key), text)) publish(key, text);
      },
    });
    const publishDraft = (key: string): void => { publish(key, drafts.get(key)); };
    return { drafts, published, publishDraft };
  }

  it("publishes a typed word once, its clearing once, and every send's republish", () => {
    const { drafts, published, publishDraft } = harness();
    for (const text of ["F", "Fi", "Fix", "Fix it"]) drafts.set("claude#a", text);
    expect(published).toEqual([{ key: "claude#a", empty: false }]);
    drafts.set("claude#a", "");
    expect(published).toHaveLength(2);
    expect(published[1]).toEqual({ key: "claude#a", empty: true });
    // The host forgets the draft at every send; the republish always goes,
    // empty or not, and resets what later keystrokes compare against.
    publishDraft("claude#a");
    drafts.set("claude#a", "Next");
    publishDraft("claude#a");
    drafts.set("claude#a", "Next one");
    expect(published.map(entry => entry.empty)).toEqual([false, true, true, false, false]);
    // Empties always go, even twice in a row.
    drafts.set("claude#a", "");
    drafts.set("claude#a", "");
    expect(published.slice(-2).map(entry => entry.empty)).toEqual([true, true]);
  });

  it("keeps each session's last publish to itself", () => {
    const { drafts, published } = harness();
    drafts.set("claude#a", "One");
    drafts.set("codex#b", "Two");
    drafts.set("claude#a", "One more");
    drafts.set("codex#b", "Two more");
    expect(published).toEqual([
      { key: "claude#a", empty: false }, { key: "codex#b", empty: false },
    ]);
  });

  it("sends nothing for a session the host cannot take as a source", () => {
    const { drafts, published, publishDraft } = harness();
    drafts.set("acp:qwen#q", "Hello");
    drafts.set("acp:qwen#q", "");
    publishDraft("opencode#o");
    expect(published).toEqual([]);
    for (const key of ["claude#1", "codex#2", "acp:grok#3"]) {
      expect(isCollaborationSourceKey(key)).toBe(true);
    }
    for (const key of ["acp:qwen#1", "grok#2", "claude", "#claude", "acp:grok"]) {
      expect(isCollaborationSourceKey(key)).toBe(false);
    }
  });

  it("names exactly the host's collaboration engines", () => {
    const rust = readFileSync(fileURLToPath(new URL(
      "../../../crates/interface-collaboration/src/state/mod.rs", import.meta.url)), "utf8");
    const start = rust.indexOf("pub enum CollaborationEngineId {");
    const body = rust.slice(start, rust.indexOf("\n}", start));
    const wire = [...body.matchAll(/#\[serde\(rename = "([^"]+)"\)\]/gu)].map(match => match[1]);
    expect(start).toBeGreaterThan(-1);
    expect(wire).toEqual([...COLLABORATION_SOURCE_ENGINES]);
  });
});
