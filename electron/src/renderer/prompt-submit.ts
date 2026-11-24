export type PromptSubmitKeyAction = "submit" | "newline" | "ignore";

export interface PromptSubmitKeyEvent {
  key: string;
  shiftKey: boolean;
  altKey?: boolean;
  metaKey?: boolean;
  ctrlKey?: boolean;
  isComposing?: boolean;
  nativeEvent?: { isComposing?: boolean; keyCode?: number };
}

/** Enter sends. Shift+Enter and Alt+Enter insert a newline. IME confirmation
 *  never submits, and a modifier is not required. */
export function promptSubmitKeyAction(event: PromptSubmitKeyEvent): PromptSubmitKeyAction {
  if (event.key !== "Enter") return "ignore";
  if (event.isComposing === true || event.nativeEvent?.isComposing === true) return "ignore";
  if (event.nativeEvent?.keyCode === 229) return "ignore";
  if (event.shiftKey || event.altKey === true) return "newline";
  return "submit";
}
