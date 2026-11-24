import { flushSync } from "react-dom";
import type { Root } from "react-dom/client";
import { themeKind, type ResolvedTheme } from "../../shared/appearance";
import { CornerNotices, type CornerNoticeItem } from "../components/CornerNotices";
import { assert, frames, until } from "./fixture-support";

/**
 * The main area's corner notices (owner, 2026-11-15) inside a stand-in
 * `.workspace-content`: bottom-right, newest first, three and "N more",
 * rising above a thread composer that would sit under them, and errors that
 * leave by themselves while a status waits.
 */
export async function cornerNoticesFixture(root: Root, theme: ResolvedTheme, chrome: string) {
  document.documentElement.dataset.theme = theme;
  document.documentElement.dataset.chrome = chrome;
  document.documentElement.style.colorScheme = themeKind(theme);
  let items: CornerNoticeItem[] = [];
  let composer: "none" | "short" | "tall" = "none";
  let retried = 0;
  const drop = (id: string) => () => { items = items.filter(item => item.id !== id); render(); };
  const notice = (id: string, message: string, over: Partial<CornerNoticeItem> = {}): CornerNoticeItem => ({
    id, message, tone: "error", onExpire: drop(id), dismissLabel: "Dismiss refresh error", onDismiss: drop(id), ...over,
  });
  const render = (): void => {
    flushSync(() => root.render(
      <div className="workspace-content" style={{ position: "relative", width: 1000, height: 600 }}>
        {composer !== "none" && (
          <div className="thread-composer" style={{ position: "absolute", left: 250, right: 250, bottom: 24,
            height: composer === "tall" ? 220 : 120, background: "var(--surface)" }}>
            <button type="button" aria-label="Send" style={{ position: "absolute", right: 12, bottom: 12 }}>↑</button>
          </div>
        )}
        <CornerNotices items={items} />
      </div>));
  };
  const box = (selector: string) => document.querySelector(selector)!.getBoundingClientRect();
  const settle = () => frames(3);

  // One refresh failure: the mockup's card, at the corner, 20px in.
  items = [notice("refresh:1", "Codex could not refresh its sessions.", { onRetry: () => { retried++; } })];
  render(); await settle();
  const area = box(".workspace-content");
  let stack = box(".corner-notices");
  assert(Math.abs(area.right - stack.right - 20) <= 1 && Math.abs(area.bottom - stack.bottom - 20) <= 1,
    `Notice is not in the bottom-right corner: ${JSON.stringify({ area, stack })}`);
  const card = document.querySelector<HTMLElement>(".corner-notice")!;
  assert(card.getAttribute("role") === "alert" && card.getBoundingClientRect().height <= 48,
    `Notice is not one small alert line: ${card.outerHTML}`);
  const style = getComputedStyle(card);
  // Bare CLI draws no shadows; the edge alone sets the card off there.
  assert(style.backgroundColor !== "rgba(0, 0, 0, 0)" && parseFloat(style.borderTopWidth) > 0
    && (chrome === "cli" || style.boxShadow !== "none"),
    `Notice has no surface of its own: ${JSON.stringify([theme, chrome, style.backgroundColor, style.boxShadow])}`);
  (Array.from(card.querySelectorAll("button")).find(button => button.textContent === "Try again"))!.click();
  assert(retried === 1, "Try again did not re-run the refresh");

  // A composer under the corner lifts the stack over it, and follows it as it
  // grows; a composer that arrives later is noticed too.
  composer = "short"; render();
  await until(() => box(".corner-notices").bottom <= box(".thread-composer").top - 8,
    () => `Notice covers the composer: ${JSON.stringify({ stack: box(".corner-notices"), composer: box(".thread-composer") })}`);
  composer = "tall"; render();
  await until(() => box(".corner-notices").bottom <= box(".thread-composer").top - 8,
    () => `Notice did not follow a growing composer: ${JSON.stringify({ stack: box(".corner-notices"), composer: box(".thread-composer") })}`);
  composer = "none"; render();
  await until(() => Math.abs(box(".workspace-content").bottom - box(".corner-notices").bottom - 20) <= 1,
    "Notice did not return to the corner once the composer left");
  composer = "short"; render();

  // Five at once: newest first, three shown, "2 more" opens the rest.
  items = [
    ...items,
    notice("action:1", "Could not archive the thread."),
    notice("restore:1", "Restored 2 files.", { tone: "status", dismissLabel: "Dismiss restore result" }),
  ];
  render(); await settle();
  items = [...items, notice("refresh:2", "Interface could not check the providers."),
    notice("refresh:3", "Interface could not read what each provider supports.")];
  render(); await settle();
  const visible = () => Array.from(document.querySelectorAll<HTMLElement>(".corner-notice:not(.is-folded)"))
    .map(node => node.dataset.cornerNotice);
  assert(JSON.stringify(visible()) === JSON.stringify(["refresh:2", "refresh:3", "action:1"]),
    `Not newest first, three at most: ${JSON.stringify(visible())}`);
  const folded = Array.from(document.querySelectorAll<HTMLElement>(".corner-notice.is-folded"));
  assert(folded.length === 2 && folded.every(node => node.inert && node.getBoundingClientRect().width <= 1),
    "Folded notices are visible or reachable");
  const more = document.querySelector<HTMLButtonElement>(".corner-notice-more")!;
  assert(more.textContent === "2 more" && more.getAttribute("aria-expanded") === "false", `No "2 more": ${more?.outerHTML}`);
  assert(box(".corner-notices").bottom <= box(".thread-composer").top - 8, "A tall stack covers the composer");
  const nodes = Array.from(document.querySelectorAll(".corner-notice"));
  more.click(); await settle();
  assert(Array.from(document.querySelectorAll(".corner-notice")).every(node => nodes.includes(node)),
    "Unfolding remounted a notice (its countdown would restart)");
  assert(visible().length === 5 && (more.textContent as string) === "Show fewer", "N more did not show every notice");
  more.click(); await settle();

  // Errors leave by themselves, folded ones included; the status waits.
  for (const animation of document.getAnimations()) animation.finish();
  await until(() => items.length === 1 && items[0].id === "restore:1",
    () => `Errors did not clear themselves: ${JSON.stringify(items.map(item => item.id))}`);
  await settle();
  assert(document.querySelector(".corner-notice-more") === null
    && document.querySelector(".corner-notice")?.getAttribute("role") === "status", "The status notice did not stay");
  document.querySelector<HTMLButtonElement>(".corner-notice-dismiss")!.click(); await settle();
  assert(document.querySelector(".corner-notices") === null, "Dismiss left the stack behind");

  // Leave a full stack over a composer for the lane's screenshot.
  composer = "short";
  items = [notice("restore:2", "Restored 2 files.", { tone: "status" }),
    notice("action:2", "Could not archive the thread.")];
  render(); await settle();
  items = [...items, notice("refresh:4", "Codex could not refresh its sessions.", { onRetry: () => undefined }),
    notice("refresh:5", "Interface could not check the providers.", { onRetry: () => undefined })];
  render(); await settle();
  for (const animation of document.getAnimations()) animation.pause();
  return { theme, chrome, corner: true, lift: true, fold: true, expire: true };
}
