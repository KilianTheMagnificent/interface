/** xterm limits announcements by line feeds, which cursor-only animations do
 * not emit. Keep this DOM projection bounded independently of PTY/scrollback. */
export const TERMINAL_ANNOUNCEMENT_LIMIT = 2_048;
export const TERMINAL_ANNOUNCEMENT_NOTICE = 'Earlier output omitted. Navigate terminal rows to read more.\n';

export function boundedTerminalAnnouncement(text: string): string {
  if (text.length <= TERMINAL_ANNOUNCEMENT_LIMIT) return text;
  let start = text.length - (TERMINAL_ANNOUNCEMENT_LIMIT - TERMINAL_ANNOUNCEMENT_NOTICE.length);
  // Never leave half an astral character at the retained tail's boundary.
  const code = text.charCodeAt(start);
  if (code >= 0xdc00 && code <= 0xdfff) start++;
  return TERMINAL_ANNOUNCEMENT_NOTICE + text.slice(start);
}

/** Observe only the announcement text and replacement of xterm's accessibility
 * container. Mutation observers run before the next layout/paint; an animation
 * frame or timer here would leave the oversized node available for forced
 * layout. Uses DOM hooks, not xterm private fields or provider bytes. */
export function limitTerminalAnnouncements(element: HTMLElement): () => void {
  let region: HTMLElement | null = null;
  const trim = (): void => {
    if (region === null) return;
    const text = region.textContent ?? '';
    const bounded = boundedTerminalAnnouncement(text);
    if (text !== bounded) region.textContent = bounded;
  };
  const content = new MutationObserver(trim);
  const bind = (): void => {
    const next = element.querySelector<HTMLElement>('.xterm-accessibility .live-region');
    if (next === region) return;
    content.disconnect();
    region = next;
    if (region !== null) {
      content.observe(region, { childList: true, characterData: true, subtree: true });
      trim();
    }
  };
  const containers = new MutationObserver(bind);
  containers.observe(element, { childList: true });
  bind();
  return () => {
    containers.disconnect();
    content.disconnect();
    region = null;
  };
}
