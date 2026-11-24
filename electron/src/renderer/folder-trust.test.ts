import { describe, expect, it } from "vitest";
import type { FolderTrustState, ThreadCommand, ThreadControlResponse, ThreadsView } from "../shared/threads";
import {
  askBeforeFirstSend, FOLDER_TRUST_CANCEL, FOLDER_TRUST_CONFIRM, FOLDER_TRUST_ROW_ACTION,
  FOLDER_TRUST_ROW_TEXT, FOLDER_TRUST_TITLE, folderTrustBody, folderTrustCommand, folderTrustRow,
  folderTrustSends, folderTrustStep, newThreadAsksFolderTrust, openThreadAsksFolderTrust,
  type FolderTrustChoice,
} from "./folder-trust";

const FOLDER = "/Users/me/Downloads/cloned";
const VIEW: ThreadsView = { version: 1, revision: 1, now: 0, threads: [],
  settings: { autoSettleDays: 3, settleOnMerge: true, generatedTitles: false, trackPullRequests: true } };

function port(state: FolderTrustState | "error", choice: FolderTrustChoice = "trust",
    recordFails = false) {
  const sent: ThreadCommand[] = [];
  const asked: string[] = [];
  return {
    sent, asked,
    request: (command: ThreadCommand): Promise<ThreadControlResponse> => {
      sent.push(command);
      if (command.kind === "folderTrust" && state === "error") return Promise.reject(new Error("no host"));
      if (command.kind === "trustFolder" && recordFails) return Promise.reject(new Error("not saved"));
      if (state === "error") return Promise.reject(new Error("no host"));
      return Promise.resolve({ view: VIEW, folderTrust: { folder: FOLDER, state } });
    },
    ask: (folder: string): Promise<FolderTrustChoice> => {
      asked.push(folder);
      return Promise.resolve(choice);
    },
  };
}

describe("folder trust", () => {
  it("asks only for a new Claude thread in a project folder", () => {
    expect(newThreadAsksFolderTrust("/p", [{ engine: "claude" }])).toBe(true);
    expect(newThreadAsksFolderTrust("/p", [{ engine: "codex" }, { engine: "claude" }])).toBe(true);
    expect(newThreadAsksFolderTrust("/p", [{ engine: "codex" }])).toBe(false);
    expect(newThreadAsksFolderTrust("/p", [{ engine: "acp:grok" }])).toBe(false);
    expect(newThreadAsksFolderTrust(null, [{ engine: "claude" }])).toBe(false);
  });

  it("asks only when nobody has answered, and sends when the host cannot say", () => {
    expect(folderTrustStep({ folder: FOLDER, state: "unasked" })).toEqual({ kind: "ask", folder: FOLDER });
    expect(folderTrustStep({ folder: FOLDER, state: "trusted" })).toEqual({ kind: "send" });
    expect(folderTrustStep({ folder: FOLDER, state: "declined" })).toEqual({ kind: "send" });
    expect(folderTrustStep(undefined)).toEqual({ kind: "send" });
    expect(folderTrustStep(null)).toEqual({ kind: "send" });
  });

  it("records Trust folder and Not now, and Escape sends nothing and keeps the draft", () => {
    expect(folderTrustCommand("trust", FOLDER)).toEqual({ kind: "trustFolder", folder: FOLDER, trust: true });
    expect(folderTrustCommand("notNow", FOLDER)).toEqual({ kind: "trustFolder", folder: FOLDER, trust: false });
    expect(folderTrustCommand("dismiss", FOLDER)).toBeNull();
    expect(folderTrustSends("trust")).toBe(true);
    expect(folderTrustSends("notNow")).toBe(true);
    expect(folderTrustSends("dismiss")).toBe(false);
  });

  it("runs the whole question before a new thread's first prompt, answer first", async () => {
    const table: [Parameters<typeof port>[0], FolderTrustChoice, boolean, string[]][] = [
      ["unasked", "trust", true, ["folderTrust", "trustFolder"]],
      ["unasked", "notNow", true, ["folderTrust", "trustFolder"]],
      ["unasked", "dismiss", false, ["folderTrust"]],
      ["trusted", "trust", true, ["folderTrust"]],
      ["declined", "trust", true, ["folderTrust"]],
      ["error", "trust", true, ["folderTrust"]],
    ];
    for (const [state, choice, sends, kinds] of table) {
      const fake = port(state, choice);
      await expect(askBeforeFirstSend(fake, "/p", [{ engine: "claude" }]), `${state} ${choice}`)
        .resolves.toBe(sends);
      expect(fake.sent.map(command => command.kind), `${state} ${choice}`).toEqual(kinds);
      expect(fake.asked, `${state} ${choice}`).toEqual(state === "unasked" ? [FOLDER] : []);
    }
    // The question names the folder the host answered for, not the one asked about.
    const fake = port("unasked", "trust");
    await askBeforeFirstSend(fake, "/p/sub", [{ engine: "claude" }]);
    expect(fake.sent).toEqual([{ kind: "folderTrust", folder: "/p/sub" },
      { kind: "trustFolder", folder: FOLDER, trust: true }]);
    // Other providers' threads never ask.
    const codex = port("unasked");
    await expect(askBeforeFirstSend(codex, "/p", [{ engine: "codex" }])).resolves.toBe(true);
    expect(codex.sent).toEqual([]);
    // An answer that could not be saved still sends.
    await expect(askBeforeFirstSend(port("unasked", "trust", true), "/p", [{ engine: "claude" }]))
      .resolves.toBe(true);
  });

  it("shows the untrusted thread's row and asks an open thread once", () => {
    expect(folderTrustRow(undefined)).toBeNull();
    expect(folderTrustRow({ folder: FOLDER, asked: true }))
      .toEqual({ folder: FOLDER, text: FOLDER_TRUST_ROW_TEXT, action: FOLDER_TRUST_ROW_ACTION });
    expect(openThreadAsksFolderTrust({ folder: FOLDER, asked: false }, true, new Set())).toBe(true);
    expect(openThreadAsksFolderTrust({ folder: FOLDER, asked: false }, false, new Set())).toBe(false);
    expect(openThreadAsksFolderTrust({ folder: FOLDER, asked: true }, true, new Set())).toBe(false);
    expect(openThreadAsksFolderTrust({ folder: FOLDER, asked: false }, true, new Set([FOLDER]))).toBe(false);
    expect(openThreadAsksFolderTrust(undefined, true, new Set())).toBe(false);
  });

  it("words the question as the owner approved", () => {
    expect([FOLDER_TRUST_TITLE, FOLDER_TRUST_CONFIRM, FOLDER_TRUST_CANCEL])
      .toEqual(["Trust this folder?", "Trust folder", "Not now"]);
    expect(folderTrustBody(FOLDER)).toBe(`${FOLDER}\n\nIts own settings and instructions can run commands `
      + "on your Mac and change what an agent may do without asking. Trust it only if you know where it "
      + "came from.\n\nUntil you trust it, Claude works here with your own settings only.");
    expect(FOLDER_TRUST_ROW_TEXT).toBe("This folder's own settings and instructions are off.");
    expect(FOLDER_TRUST_ROW_ACTION).toBe("Trust folder");
  });
});
