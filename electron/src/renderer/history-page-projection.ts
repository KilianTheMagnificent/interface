import type { EngineEvent } from "../shared/engine";
import type { HistoryPage } from "../shared/ipc";
import { retainConversationItems } from "./conversation-projection";
import { conversationItems, type ConvItem } from "./conversation-reducer";
import { equalValue } from "./stable-values";

/**
 * One history page's conversation items. A page is the transcript's TAIL, and
 * "Load older" replaces it with a longer one that holds the same events behind
 * older ones, so generated ids count from the page's end: the rows already
 * drawn keep their ids, and so their keys, and items equal to the previous
 * page's keep their objects. Counted from the front, every older page renamed
 * every row.
 */
export function historyPageItems(events: EngineEvent[], previous: ConvItem[] = []): ConvItem[] {
  const last = events.length - 1;
  return retainConversationItems(previous,
    conversationItems(events, (_event, index) => last - index));
}

/** Reconcile deserialized pages with the previous immutable event identities,
 * allowing streaming tail updates to use the incremental transcript projection. */
export function retainHistoryPage(current: HistoryPage | null, next: HistoryPage): HistoryPage {
  if (!current) return next;
  const events = next.events.map((event, index) => equalValue(current.events[index], event)
    ? current.events[index] : event);
  const candidate = { ...next, events };
  return equalValue(current, candidate) ? current : candidate;
}
