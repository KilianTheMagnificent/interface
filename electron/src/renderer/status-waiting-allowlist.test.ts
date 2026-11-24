import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

/* ==========================================================================
   THE AMBER ALLOWLIST

   --status-waiting is the app's ONE sentence: an agent is blocked on you (and,
   by the same reading, your usage is about to block you). It works only while
   it is rare. Four amber marks in one pane meaning four different things is how
   a colour stops being read at all, and every one of those four arrives the
   same way — as a reasonable-looking line in a diff that nobody weighed against
   the sites already spending it.

   So every var(--status-waiting) in the renderer is written down here with the
   count it is allowed. A NEW site fails this test on purpose. The failure is
   not "you did something wrong"; it is "say why this one earns the app's
   loudest colour", answered in the same commit that adds it.

   This is a renderer-wide net. Individual panes pin their own amber rules in
   their own suites (Conversation.queue.test.ts spells out all three of the
   transcript's, by selector); this file exists to catch the site that lands
   somewhere nobody thought to look.
   ========================================================================== */

/** Where each file's amber goes, and why it is allowed to. */
const ALLOWED: Readonly<Record<string, { count: number; spent: string }>> = {
  "components/AttentionTray.tsx": {
    count: 2,
    spent: "the tray row's glyph for an agent or pane blocked on you — the one "
      + "row kind that is a request rather than a report — and the live count "
      + "beside the bell, which counts exactly those rows (owner, 2026-09-12)",
  },
  "theme.css": {
    count: 5,
    spent: "the shared status vocabulary — the waiting dot, the statusbar "
      + "warning, the near-limit usage bar, and a waiting Threads pane's head, "
      + "read once for its still amber line and its \"Needs you\" pill (owner, "
      + "2026-10-07; they replaced the head's amber dot); and the hidden "
      + "sidebar's toggle dot standing in for the footer's \"providers need "
      + "setup\" while the footer is out of sight (owner, 2026-11-15)",
  },
  "components/Conversation.css": {
    count: 3,
    spent: "the approval card's glyph, the pending card's border, and the jump "
      + "capsule saying the same thing where the card cannot be seen",
  },
  "components/Sidebar.tsx": {
    count: 8,
    spent: "the attention group and its glyph, the wait and limit meta, the "
      + "flag dot and its border, a workspace row's pip for the pane "
      + "blocked on you (owner, 2026-10-01), and the Settings row's \"N "
      + "providers need setup\": a provider you must set up before it can run "
      + "is blocked on you (owner, 2026-11-15, chosen over the red banner)",
  },
  "components/SessionPeek.tsx": {
    count: 3,
    spent: "the waiting age, and the peek card's border and heading when the "
      + "session it previews is blocked",
  },
  "components/SettingsPage.tsx": {
    count: 2,
    spent: "Settings' \"2 need setup\" beside Models & providers in its menu and "
      + "the \"Needs setup\" heading over those providers: setup the app cannot "
      + "do without you (owner, 2026-11-15, the approved Settings mockups)",
  },
  "components/SubagentPanel.tsx": {
    count: 1,
    spent: "the subagent action flag: this spawn is the one blocked on you",
  },
  "components/AgentFeed.tsx": {
    count: 1,
    spent: "the feed row's dot for an agent in `waiting` — the one state in "
      + "the feed that is a request rather than a status, and the feed exists "
      + "precisely to be read from another pane, where nothing else says it",
  },
  "terminal.css": {
    count: 5,
    spent: "the terminal tab and overview glyph when a terminal needs input, "
      + "the overview meter's needs-you segment and the needs-you card's edge "
      + "(owner, 2026-10-01), "
      + "and the pane's own strip under the screen and its word in the head "
      + "(owner, 2026-09-28: a 6px dot alone was lost among eight panes). "
      + "Every launch profile in this workspace is an agent harness, and the "
      + "bell is how Claude Code, Codex, Grok, Qwen and Antigravity say they are "
      + "blocked on you — the same sentence the sidebar's attention group "
      + "says, said in the pane where the agent actually runs. A plain "
      + "shell's own bell lights it too",
  },
  "components/AgentCanvas.tsx": {
    count: 1,
    spent: "the canvas node's status word when the agent's state is waiting — "
      + "the same 'Needs attention' sentence status-words.ts already gives it. "
      + "The in-flight delegation edge deliberately does NOT take amber: a "
      + "message in transit is pending, not blocked on you",
  },
  "components/AgentTask.tsx": {
    count: 1,
    spent: "the Agent surface task header's status word while its one visible "
      + "agent is blocked on the user",
  },
  "components/ThreadComposer.css": {
    count: 1,
    spent: "a Threads pane's live turn blocked on its owner (owner, 2026-10-07): "
      + "the request docked above the composer — its edge, tint, glyph and "
      + "arrival rings — and the composer's own waiting edge and glow while it "
      + "waits. Read once into a local; nothing merely pending or running there "
      + "takes it (the running comet is the project's hue)",
  },
  "components/ThreadTranscript.css": {
    count: 1,
    spent: "a Threads pane's one sentence that the run waits on you, read once into "
      + "the layout's --thread-waiting local (owner, 2026-10-06 redesign; live turn "
      + "2026-10-07): the pending approval card's edge and tint, and the live turn's "
      + "\"Waiting for you\", its waiting clock, the waiting step's ring and note, "
      + "and the live capsule's waiting line — one site, every mark the same meaning",
  },
  "components/ThreadInbox.tsx": {
    count: 1,
    spent: "a Threads inbox row that needs you (owner, 2026-10-07, \"Inbox "
      + "under load\"): its Needs you pill, the one filled pill in the inbox "
      + "(read once into a local for its fill, its ink and the two rings it "
      + "arrives with). What the thread asks reads in line two's dim tier "
      + "since the Threads look (owner, 2026-10-23); the folded caption and "
      + "shelf marks still say it through the sidebar's own .session-wait",
  },

};

const RULE = "Amber says ONE thing: an agent is blocked on you, or your usage "
  + "is about to block you. It is not for warnings, not for rate-limited-but-"
  + "running, not for modified files (--vcs-modified), not for failures "
  + "(--status-failed), and not for anything merely pending. If this site says "
  + "that one sentence, add it to ALLOWED in this file with the reason. If it "
  + "does not, take the colour off it.";

const RENDERER = fileURLToPath(new URL("./", import.meta.url));
const SOURCE = /\.(?:ts|tsx|css)$/u;
const SUITE = /\.test\.(?:ts|tsx)$/u;
const AMBER = /var\(--status-waiting\)/gu;

/** Every renderer source file, tests excluded, as paths relative to src/renderer. */
function sourceFiles(dir: string, prefix = ""): string[] {
  const found: string[] = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const relative = prefix === "" ? entry.name : `${prefix}/${entry.name}`;
    if (entry.isDirectory()) {
      found.push(...sourceFiles(join(dir, entry.name), relative));
      continue;
    }
    if (!SOURCE.test(entry.name) || SUITE.test(entry.name)) continue;
    found.push(relative);
  }
  return found.sort();
}

function amberCounts(): Map<string, number> {
  const counts = new Map<string, number>();
  for (const file of sourceFiles(RENDERER)) {
    const text = readFileSync(join(RENDERER, file), "utf8");
    const hits = (text.match(AMBER) ?? []).length;
    if (hits > 0) counts.set(file, hits);
  }
  return counts;
}

describe("the amber allowlist", () => {
  it("scans real renderer sources, so a silent no-op cannot pass it", () => {
    const files = sourceFiles(RENDERER);
    expect(files.length).toBeGreaterThan(40);
    expect(files).toContain("theme.css");
    expect(files).toContain("components/Conversation.tsx");
    // Its own suites are not sources: a test file quoting the token would
    // otherwise register as a site spending it.
    expect(files.some(file => SUITE.test(file))).toBe(false);
  });

  it("spends --status-waiting on the written-down sites and nowhere else", () => {
    const counts = amberCounts();
    const problems: string[] = [];

    for (const [file, hits] of [...counts].sort()) {
      const allowed = ALLOWED[file];
      if (allowed === undefined) {
        problems.push(`NEW AMBER SITE — ${file} spends var(--status-waiting) `
          + `${hits} time(s) and is not on the allowlist. ${RULE}`);
        continue;
      }
      if (allowed.count !== hits) {
        problems.push(`AMBER COUNT CHANGED — ${file} now spends `
          + `var(--status-waiting) ${hits} time(s); the allowlist says `
          + `${allowed.count} (${allowed.spent}). If a site moved or was `
          + `removed, update the number here in the same commit. If one was `
          + `ADDED: ${RULE}`);
      }
    }

    for (const [file, allowed] of Object.entries(ALLOWED)) {
      if (counts.has(file)) continue;
      problems.push(`AMBER GONE — ${file} no longer spends `
        + `var(--status-waiting) at all, but the allowlist still claims `
        + `${allowed.count} (${allowed.spent}). Drop the entry.`);
    }

    expect(problems).toEqual([]);
  });

  it("keeps the whole renderer's amber to a number a reader can hold", () => {
    // The point of the colour is scarcity. This is not a budget anyone should
    // spend down to — it is the line past which "rare" stopped being true and
    // the rule needs re-arguing rather than the number raising.
    const total = [...amberCounts().values()].reduce((sum, hits) => sum + hits, 0);
    // 28 -> 29, argued rather than raised: the agent feed is the only surface
    // that reports a blocked agent while the human is looking at a different
    // pane. Its amber is the same sentence the canvas node says, said where
    // the canvas cannot be seen.
    // 29 -> 30, argued the same way: the integrated terminals run the same
    // harnesses, and until this mark the one pane where an agent can ask for
    // you was the one pane that never said so in the app's colour.
    //
    // 30 -> 31, argued rather than raised: every other site above points AT a
    // pane from somewhere else — a sidebar row, a card, a feed line. The pane
    // header's dot is that same sentence said ON the pane, which is where a
    // four-pane workspace actually leaves the eye. It renders the data-status
    // the tile already carries, so it adds a mark, not a state.
    //
    // 31 -> 32: AgentTask is the Agent surface's one task and has no sibling
    // list or tile header to say that its agent is waiting. Its status word is
    // the same meaning in the only visible place that surface owns.
    //
    // 32 -> 27: the sessions board was removed with the Plans surface, giving
    // back the five marks Dashboard.tsx spent. Nothing was argued up.
    // 27 -> 26: the sidebar row's second line went, and the waiting status
    // word on it with it; the row's pill and rail still say the same thing.
    // 26 -> 23: the toolbar's "N need you" pill (four amber marks in App.tsx)
    // became the attention tray's bell, which spends amber once, on the row
    // glyph of a blocked agent or pane inside the tray.
    // 23 -> 24: the bell's number now counts live needs only and says so in
    // amber (owner, 2026-09-12); finished turns wait behind a neutral dot.
    // 24 -> 27: Clean, Direction B. The pane head grew a status pill and the
    // transcript grew an approval card of its own, and both say the sentence
    // amber is for — in the pane the reader is looking at. The third is the
    // answer to that card: an edit the reader declined, worded and coloured as
    // their decision rather than as a failure. Nothing else in the new surface
    // takes it — the working indicator is the accent, and a command that blew
    // up is danger.
    // 27 -> 24: retiring Clean removes its three dedicated amber marks.
    // 24 -> 23: the toolbar's usage chips were no longer rendered, and their
    // near-limit rule went with the rest of that dead CSS.
    // 23 -> 25: a waiting pane lights an amber strip in its frame and says
    // "Needs attention" in its head (owner, 2026-09-28), the Porcelain
    // design's needs-you strip.
    // 25 -> 28, argued rather than raised (owner, 2026-10-01): the dense
    // overview's meter segment and card edge, and the sidebar's per-pane pip,
    // each say "this pane is blocked on you" where only a count or a grey
    // card said it before. They are the same sentence the pane's own strip
    // says, said from the two places that point at a pane from outside it.
    //
    // 28 -> 29, argued rather than raised (owner, 2026-10-06): the Threads
    // layout restyles the transcript's own pending approval card as an amber
    // tinted card. It is not a new site saying a new thing — it is the card
    // .conv-approval-pending already marks, in the one layout that draws it as
    // a filled block, and it reads the colour once for its edge and its tint.
    //
    // 29 -> 32, argued rather than raised (owner, 2026-10-07, "a live turn"
    // and "Inbox under load"): a working Threads row wore an orange pill that
    // read like the amber Needs you beside it, so Working lost its pill and
    // Needs you became the inbox's one filled amber pill, with the row's second
    // line saying what the thread asks in an amber ink (+2). A Threads pane
    // docks the request it is blocked on directly above its composer and turns
    // the composer amber while it waits — one local read for both, the same
    // sentence said where the owner's hands are (+1). The head's amber dot
    // became its waiting line and "Needs you" pill, read once in the same
    // rule, so theme.css stays at four.
    //
    // 32 -> 31 (owner, 2026-10-23, "Threads look"): the inbox row's second
    // line no longer says what a thread asks in amber; the approved canvas
    // sets it in the line's dim tier beside the row's Needs you pill, which
    // already says the sentence. Nothing was argued up.
    //
    // 31 -> 33 (owner, 2026-11-15, the shell redesign): "N providers need
    // setup" left its red banner for one amber line in the Settings row, and
    // the hidden sidebar's toggle carries its dot while the row is out of
    // sight. A provider that must be set up before it can run anything is
    // blocked on you; the two marks are one notice, never shown together.
    //
    // 33 -> 35, argued rather than raised (owner, 2026-11-15, the approved
    // Settings mockups): "2 need setup" in Settings' menu and the "Needs
    // setup" heading over those providers, the same notice said inside
    // Settings, where the sidebar's row is covered. The health dots stay red
    // and grey.
    expect(total).toBe(35);
  });
});
