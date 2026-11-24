import { VISUAL_FRAME_SCHEME } from "../shared/visual-frame.js";
const MAX_EXTERNAL_URL_LENGTH = 8_192;
const CONTROL_CHARACTER = /[\u0000-\u001f\u007f]/;

export interface RendererLocation {
  devServerUrl?: string;
  packagedEntryUrl: string;
}

export function resolveRendererLocation(input: {
  isPackaged: boolean;
  allowPackagedDevServer: boolean;
  devServerUrl: string | undefined;
  packagedEntryUrl: string;
}): RendererLocation {
  const trimmed = input.devServerUrl?.trim();
  const packagedDevServerAllowed = input.allowPackagedDevServer
    && trustedPackagedDevServerUrl(input.devServerUrl);
  const devServerUrl = (input.isPackaged && !packagedDevServerAllowed)
      || trimmed === undefined || trimmed.length === 0
    ? undefined : input.devServerUrl;
  return { devServerUrl, packagedEntryUrl: input.packagedEntryUrl };
}

function trustedPackagedDevServerUrl(raw: string | undefined): boolean {
  if (raw === undefined || raw !== raw.trim() || raw.length === 0) return false;
  let url: URL;
  try { url = new URL(raw); } catch { return false; }
  return url.protocol === "http:"
    && url.hostname === "127.0.0.1"
    && url.port.length > 0
    && url.username.length === 0
    && url.password.length === 0
    && url.pathname === "/"
    && url.search.length === 0
    && url.hash.length === 0;
}

export function validatedExternalUrl(raw: string): string | null {
  if (raw.length === 0 || raw.length > MAX_EXTERNAL_URL_LENGTH
      || raw !== raw.trim() || CONTROL_CHARACTER.test(raw)) return null;
  let url: URL;
  try { url = new URL(raw); } catch { return null; }
  if (url.protocol === "http:" || url.protocol === "https:") {
    if (url.hostname.length === 0 || url.username.length > 0 || url.password.length > 0) {
      return null;
    }
    return url.href;
  }
  if (url.protocol === "mailto:") {
    if (url.host.length > 0 || url.pathname.length === 0 || url.hash.length > 0) return null;
    return url.href;
  }
  return null;
}

export function isAllowedRendererNavigation(raw: string, location: RendererLocation): boolean {
  let candidate: URL;
  try { candidate = new URL(raw); } catch { return false; }
  if (candidate.username.length > 0 || candidate.password.length > 0) return false;
  if (location.devServerUrl !== undefined) {
    try {
      const dev = new URL(location.devServerUrl);
      return (dev.protocol === "http:" || dev.protocol === "https:")
        && dev.username.length === 0 && dev.password.length === 0
        && candidate.protocol === dev.protocol
        && candidate.origin === dev.origin;
    } catch { return false; }
  }
  try {
    const entry = new URL(location.packagedEntryUrl);
    candidate.hash = "";
    entry.hash = "";
    return entry.protocol === "file:" && candidate.href === entry.href;
  } catch { return false; }
}

export interface TrustedWebContents {
  mainFrame: object;
}

export interface InvokeSender {
  sender: object;
  senderFrame?: object | null;
}

export function isAuthorizedIpcSender(
  event: InvokeSender,
  trusted: TrustedWebContents | null,
): boolean {
  return trusted !== null
    && event.sender === trusted
    && event.senderFrame === trusted.mainFrame;
}

/** Strict renderer policy; development adds only Vite's script/HMR allowances. */
export function contentSecurityPolicy(devServerUrl: string | undefined): string {
  const script = devServerUrl === undefined ? "'self'" : "'self' 'unsafe-inline'";
  const connect = devServerUrl === undefined
    ? "'self'"
    : `'self' ${devServerUrl} ${devServerUrl.replace(/^http/, "ws")}`;
  return [
    "default-src 'self'",
    `script-src ${script}`,
    "style-src 'self' 'unsafe-inline'",
    "worker-src 'self'",
    "img-src 'self' data:",
    "font-src 'self' data:",
    `connect-src ${connect}`,
    // Not covered by default-src: nothing here embeds plugins, a <base> or a
    // submitting form, so injected markup gets none of them. The one frame is
    // a visual reply's, served sandboxed from its own scheme (T3 section 8,
    // shared/visual-frame.ts); nothing else can be framed.
    "object-src 'none'",
    `frame-src ${VISUAL_FRAME_SCHEME}:`,
    "base-uri 'none'",
    "form-action 'none'",
    "frame-ancestors 'none'",
  ].join("; ");
}
