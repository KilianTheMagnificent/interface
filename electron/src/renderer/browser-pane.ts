/** The Browser pane's session; main refuses a `<webview>` in any other. */
export const BROWSER_PANE_PARTITION = "persist:browser-pane";

/** Where a new Browser tab opens. */
export const BROWSER_HOME_URL = "https://www.google.com/";
const BROWSER_SEARCH_URL = "https://www.google.com/search?q=";

const LOOPBACK = /^(?:localhost|127(?:\.\d{1,3}){3}|0\.0\.0\.0|\[::1?\])(?::\d{1,5})?(?:[/?#]|$)/iu;
const DOMAIN = /^[a-z0-9-]+(?:\.[a-z0-9-]+)+(?::\d{1,5})?(?:[/?#]|$)/iu;

/** Main refuses longer addresses (`main/browser-pane.ts`). */
const MAX_BROWSER_URL_LENGTH = 8_192;

/** A web page with a host and no embedded credentials, or null. */
function webPage(raw: string): string | null {
  if (raw.length > MAX_BROWSER_URL_LENGTH) return null;
  let url: URL;
  try { url = new URL(raw); } catch { return null; }
  if (url.protocol !== "http:" && url.protocol !== "https:") return null;
  if (url.hostname.length === 0 || url.username.length > 0 || url.password.length > 0) return null;
  return url.href;
}

/** A saved or reported address the Browser may load, or null. */
export function browserPageUrl(raw: string): string | null {
  return webPage(raw);
}

/**
 * What the address bar loads for what was typed, or null for nothing or
 * another scheme: `5173`, `5173/admin` and `localhost:5173` are the local dev
 * server over http, a bare domain is https, anything else is a Google search,
 * and only http(s) is ever loaded.
 */
export function browserAddress(input: string): string | null {
  const text = input.trim();
  if (text.length === 0 || text.length > MAX_BROWSER_URL_LENGTH) return null;
  if (/\s/u.test(text)) return browserSearchUrl(text);
  const [, port, rest = ""] = /^:?(\d{2,5})([/?#].*)?$/u.exec(text) ?? [];
  if (port !== undefined && Number(port) <= 65_535) return webPage(`http://localhost:${port}${rest}`);
  if (/^[a-z][a-z0-9+.-]*:\/\//iu.test(text)) return webPage(text);
  if (/^(?:about|chrome|data|javascript|mailto|blob|view-source):/iu.test(text)) return null;
  if (LOOPBACK.test(text)) return webPage(`http://${text}`);
  return DOMAIN.test(text) ? webPage(`https://${text}`) : browserSearchUrl(text);
}

/** A Google search for `query`, or null when it is too long to send. */
export function browserSearchUrl(query: string): string | null {
  const text = query.trim();
  return text.length === 0 ? null : webPage(`${BROWSER_SEARCH_URL}${encodeURIComponent(text)}`);
}

/**
 * Loopback listeners the workspace's terminals hold, as `localhost` pages:
 * each listed once, in the order given (the source pane's terminal first).
 */
export function browserServerUrls(lists: readonly (readonly { readonly url: string }[])[]): string[] {
  const seen = new Set<string>();
  for (const list of lists) {
    for (const { url } of list) {
      const page = webPage(url.replace(/^http:\/\/(?:127\.0\.0\.1|\[::1?\])(?=:)/u, "http://localhost"));
      if (page !== null) seen.add(page);
    }
  }
  return [...seen];
}

/** `localhost:5173` for a local page, the full address otherwise. */
export function browserShortAddress(url: string): string {
  return url.replace(/^http:\/\/(localhost:\d+)\/$/u, "$1");
}

/** A Browser tab's label: the page's own title, else where it is, else "Browser". */
export function browserTabTitle(title: string | null | undefined, url: string | null | undefined): string {
  const named = title?.replace(/[\u0000-\u001f\u007f]/gu, "").trim() ?? "";
  if (named !== "") return named;
  if (url === null || url === undefined || url === "") return "Browser";
  try {
    return new URL(url).host || "Browser";
  } catch {
    return "Browser";
  }
}

/** No such host (DNS): a typed word with a dot may have been a search. */
export function browserHostNotFound(code: number): boolean {
  return code === -105 || code === -137;
}

/** Why the page did not load, in words; null for a cancelled load. */
export function browserLoadError(code: number, url: string): string | null {
  if (code === -3) return null;
  const where = browserShortAddress(url);
  if (code === -102) {
    return LOOPBACK.test(url.replace(/^https?:\/\//iu, "")) ? `Nothing is answering at ${where}. Start the dev server, then retry.`
      : `${where} refused the connection.`;
  }
  if (browserHostNotFound(code)) return `Could not find ${where}. Check the address.`;
  if (code === -106) return "This Mac is offline.";
  if (code <= -200 && code > -300) return `${where} has a certificate problem, so it was not opened.`;
  return `${where} could not be opened.`;
}
