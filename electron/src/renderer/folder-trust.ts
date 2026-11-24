import type { ComposerLaunchForm } from "./composer-model";
import type {
  FolderTrustAnswer, ThreadCommand, ThreadControlResponse, ThreadFolderTrust,
} from "../shared/threads";

/**
 * Folder trust (protocol 72), the renderer's part: when to ask "Trust this
 * folder?", what each answer sends, and the line an untrusted thread shows.
 * Rust decides where a folder stands and keeps the answer; until a folder is
 * trusted, the Claude adapter already keeps a session there to the owner's
 * own settings, so nothing here protects the owner by itself, and a failed
 * question never blocks a send.
 */

/** The only provider whose threads ask. */
export const FOLDER_TRUST_ENGINE = "claude";

export const FOLDER_TRUST_TITLE = "Trust this folder?";
export const FOLDER_TRUST_CONFIRM = "Trust folder";
export const FOLDER_TRUST_CANCEL = "Not now";
export const FOLDER_TRUST_ROW_TEXT = "This folder's own settings and instructions are off.";
export const FOLDER_TRUST_ROW_ACTION = "Trust folder";

/** The question's body, naming the folder the answer covers. */
export function folderTrustBody(folder: string): string {
  return `${folder}\n\nIts own settings and instructions can run commands on your Mac and change `
    + "what an agent may do without asking. Trust it only if you know where it came from.\n\n"
    + "Until you trust it, Claude works here with your own settings only.";
}

/** How the owner left the question: a button, or Escape and closing it. */
export type FolderTrustChoice = "trust" | "notNow" | "dismiss";

/**
 * A new thread asks only in a project folder (No project is a plain chat
 * in Interface's own folder) and only when one of the threads it starts
 * runs on Claude.
 */
export function newThreadAsksFolderTrust(project: string | null,
    forms: readonly Pick<ComposerLaunchForm, "engine">[]): boolean {
  return project !== null && forms.some(form => form.engine === FOLDER_TRUST_ENGINE);
}

/**
 * What the new-thread card does with the host's answer to `folderTrust`:
 * ask when nobody has answered for the folder yet; otherwise send at once.
 * No answer (the host could not be asked) sends too.
 */
export function folderTrustStep(answer: FolderTrustAnswer | null | undefined):
    { kind: "ask"; folder: string } | { kind: "send" } {
  return answer?.state === "unasked" ? { kind: "ask", folder: answer.folder } : { kind: "send" };
}

/**
 * The command a choice sends: Trust folder trusts, Not now records the
 * decline; Escape or closing sends nothing.
 */
export function folderTrustCommand(choice: FolderTrustChoice, folder: string): ThreadCommand | null {
  if (choice === "dismiss") return null;
  return { kind: "trustFolder", folder, trust: choice === "trust" };
}

/**
 * Whether a send goes ahead after the question: Escape or closing keeps the
 * draft and sends nothing; either button sends.
 */
export function folderTrustSends(choice: FolderTrustChoice): boolean {
  return choice !== "dismiss";
}

/** The new-thread card's question, start to end; the card owns the dialog. */
export interface NewThreadFolderTrustPort {
  request: (command: ThreadCommand) => Promise<ThreadControlResponse>;
  ask: (folder: string) => Promise<FolderTrustChoice>;
}

/**
 * Asks before a new Claude thread's first prompt goes out. Answers whether
 * to send. The answer is recorded before the send; a host that cannot be
 * asked, or cannot record it, does not stop the send.
 */
export async function askBeforeFirstSend(port: NewThreadFolderTrustPort, project: string | null,
    forms: readonly Pick<ComposerLaunchForm, "engine">[]): Promise<boolean> {
  if (!newThreadAsksFolderTrust(project, forms) || project === null) return true;
  let answer: FolderTrustAnswer | undefined;
  try {
    answer = (await port.request({ kind: "folderTrust", folder: project })).folderTrust;
  } catch {
    return true;
  }
  const step = folderTrustStep(answer);
  if (step.kind === "send") return true;
  const choice = await port.ask(step.folder);
  const command = folderTrustCommand(choice, step.folder);
  if (command !== null) await port.request(command).catch(() => undefined);
  return folderTrustSends(choice);
}

/** The line an open thread shows while its folder's settings are off. */
export interface FolderTrustRow {
  folder: string;
  text: string;
  action: string;
}

export function folderTrustRow(trust: ThreadFolderTrust | undefined): FolderTrustRow | null {
  if (trust === undefined) return null;
  return { folder: trust.folder, text: FOLDER_TRUST_ROW_TEXT, action: FOLDER_TRUST_ROW_ACTION };
}

/**
 * Whether the open thread asks once by itself: a thread nobody asked about
 * (one a schedule, team tools or an outside agent started reaches the owner
 * this way), not yet asked in this window for that folder.
 */
export function openThreadAsksFolderTrust(trust: ThreadFolderTrust | undefined, open: boolean,
    alreadyAsked: ReadonlySet<string>): boolean {
  return open && trust !== undefined && !trust.asked && !alreadyAsked.has(trust.folder);
}
