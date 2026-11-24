import { createContext, type ReactNode } from "react";

/**
 * Whether the conversation panes below are on screen. A covered pane stays
 * mounted, so its draft, disclosures and scroll position survive, but it has
 * nothing to paint: ProjectedConversation holds its streamed text and the reveal
 * stops pacing until the pane is shown again. Outside any surface a pane is
 * visible.
 */
export const PaneVisibleContext = createContext(true);

/** Cover, never remount, conversations when the user returns to terminals. */
export function ConversationPaneSurface({ visible, blocked = false, children }: {
  visible: boolean;
  blocked?: boolean;
  children: ReactNode;
}) {
  return <div className={`workspace-surface conversation-pane-surface${visible ? "" : " is-hidden"}`}
    inert={blocked || !visible || undefined} aria-hidden={blocked || !visible || undefined}>
    <PaneVisibleContext.Provider value={visible}>{children}</PaneVisibleContext.Provider>
  </div>;
}
