/**
 * Offering to move a freshly downloaded app into Applications.
 *
 * macOS runs an app opened straight from Downloads from a hidden read-only
 * copy, and an app anywhere outside Applications may sit where it cannot
 * replace itself. Either way Restart to update can never install. The offer
 * is the standard fix: one question at launch, and on Move the app relaunches
 * from /Applications.
 */
export interface ApplicationsFolderSituation {
  readonly isPackaged: boolean;
  readonly platform: NodeJS.Platform;
  readonly inApplicationsFolder: boolean;
  /** The packaged smoke and hands-free sessions run the app where it was built. */
  readonly unattended: boolean;
}

export function shouldOfferApplicationsMove(situation: ApplicationsFolderSituation): boolean {
  return situation.isPackaged
    && situation.platform === "darwin"
    && !situation.unattended
    && !situation.inApplicationsFolder;
}

export const APPLICATIONS_MOVE_QUESTION = {
  type: "question",
  message: "Move Interface to your Applications folder?",
  detail: "Interface can only update itself from the Applications folder. "
    + "It will reopen from there.",
  buttons: ["Move to Applications", "Not Now"],
  defaultId: 0,
  cancelId: 1,
} as const;

export interface ApplicationsMoveEffects {
  ask(): number;
  move(): boolean;
}

/**
 * Asks, and moves on yes. `true` means the app is relaunching from
 * Applications and this launch must go no further. A move macOS refuses
 * leaves the app running where it is.
 */
export function offerApplicationsMove(
  situation: ApplicationsFolderSituation,
  effects: ApplicationsMoveEffects,
): boolean {
  if (!shouldOfferApplicationsMove(situation)) return false;
  if (effects.ask() !== APPLICATIONS_MOVE_QUESTION.defaultId) return false;
  try {
    return effects.move();
  } catch {
    return false;
  }
}
