import type { Session, WebContents } from "electron";

/**
 * The app window's network fence (T3 section 8, found in review 15 Nov).
 *
 * A visual reply's page runs inside the app window, in a sandboxed frame
 * whose CSP gives it no network. WebRTC is outside CSP (Electron 43 ignores
 * `webrtc 'block'`): a visual's script could open an `RTCPeerConnection` and
 * send STUN over UDP, or TURN over TCP, to any address it named. So the
 * window's own session gets a fence below CSP:
 *
 *  - `disable_non_proxied_udp`: WebRTC sends no UDP at all, and opens TCP only
 *    through the session's proxy;
 *  - that proxy leads nowhere (a refused loopback port), and loopback is not
 *    exempt from it (`<-loopback>`), so TCP has no route either.
 *
 * Nothing else in the window needs the network: the packaged renderer is a
 * `file:` page whose CSP allows `connect-src 'self'` only; visuals come from
 * `iface-visual:` (a protocol handler, not a network fetch); everything else
 * goes through the preload to the Rust host. The Browser pane has its own
 * partition and electron-updater its own (`electron-updater`), so neither is
 * touched. In `npm run dev` the Vite server's own host:port is exempt.
 */

/** A proxy no request can reach: a loopback port that refuses connections. */
export const DEAD_PROXY = "http://127.0.0.1:1";

/** The proxy bypass list: loopback goes through the dead proxy too, except
 * the dev server when there is one. */
export function fenceBypassRules(devServerUrl?: string): string {
  if (devServerUrl === undefined) return "<-loopback>";
  const url = new URL(devServerUrl);
  return `<-loopback>,${url.host}`;
}

export async function fenceWindowNetwork(contents: WebContents, target: Session,
    devServerUrl?: string): Promise<void> {
  contents.setWebRTCIPHandlingPolicy("disable_non_proxied_udp");
  await target.setProxy({
    mode: "fixed_servers",
    proxyRules: DEAD_PROXY,
    proxyBypassRules: fenceBypassRules(devServerUrl),
  });
}
