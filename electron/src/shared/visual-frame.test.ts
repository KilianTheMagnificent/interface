import { describe, expect, it } from "vitest";
import {
  MAX_WRAPPED_CHARS, parseVisualFrameRequest, svgVisualPage, VISUAL_FRAME_CSP,
  VISUAL_FRAME_RESPONSE_CSP, visualFrameToken, visualFrameUrl, wrapVisualDocument,
} from "./visual-frame";

const TOKEN = "0123456789abcdef0123456789abcdef";

describe("visual frame wrapper (T3 section 8)", () => {
  it("puts the CSP meta first, then the app's scheme, then the page", () => {
    const wrapped = wrapVisualDocument("<!doctype html><p>hi</p>", "dark");
    const head = wrapped.slice(0, wrapped.indexOf("</head>"));
    expect(wrapped.startsWith('<!doctype html><html data-theme="dark"><head>'
      + `<meta http-equiv="Content-Security-Policy" content="${VISUAL_FRAME_CSP}">`)).toBe(true);
    expect(head.indexOf("Content-Security-Policy")).toBeLessThan(head.indexOf("<meta charset"));
    expect(head).toContain('<meta name="color-scheme" content="dark">');
    expect(head).toContain("<style>:root{color-scheme:dark}</style>");
    // Its own <base> has no href (base-uri 'none'): a plain link click opens
    // a popup the sandbox drops, so the frame never navigates to a blank page.
    expect(head).toContain('<base target="_blank">');
    expect(head).not.toMatch(/<base[^>]*href/i);
    expect(head).not.toContain("<script");
    expect(wrapped.endsWith("<!doctype html><p>hi</p>")).toBe(true);
    expect(wrapVisualDocument("<p>x</p>", "light")).toContain('data-theme="light"');
  });

  it("gives the page no network, storage or forms: only inline code and data: media", () => {
    expect(VISUAL_FRAME_CSP.split("; ")).toEqual([
      "default-src 'none'", "img-src data:", "style-src 'unsafe-inline'",
      "script-src 'unsafe-inline'", "font-src data:", "media-src data:",
      "base-uri 'none'", "form-action 'none'",
    ]);
    expect(VISUAL_FRAME_CSP).not.toMatch(/connect-src|frame-src|'self'|https?:|\*/);
    expect(VISUAL_FRAME_RESPONSE_CSP).toBe(`${VISUAL_FRAME_CSP}; sandbox allow-scripts`);
  });

  it("keeps hostile markup inside the page: </iframe>, quotes, <base> and refresh tags", () => {
    const hostile = '</iframe></html><iframe srcdoc="&quot;><script>parent.x=1</script>">'
      + '<BASE href="https://evil.example/" target="_top"><base target=_blank>'
      + '<meta http-equiv="refresh" content="0;url=https://evil.example">'
      + "<META HTTP-EQUIV=Refresh CONTENT='1'><meta http-equiv='refresh'content=0>"
      + '<meta http-equiv="Content-Security-Policy" content="default-src *">';
    const wrapped = wrapVisualDocument(hostile, "light");
    const head = wrapped.slice(0, wrapped.indexOf("</head>") + "</head>".length);
    // The app's head is whole and first: nothing in the page lands before it.
    expect(head).toBe('<!doctype html><html data-theme="light"><head>'
      + `<meta http-equiv="Content-Security-Policy" content="${VISUAL_FRAME_CSP}">`
      + '<meta charset="utf-8"><base target="_blank"><meta name="color-scheme" content="light">'
      + "<style>:root{color-scheme:light}</style></head>");
    const page = wrapped.slice(head.length);
    expect(page).not.toMatch(/<base\b/i);
    expect(page).toContain('<x-base href="https://evil.example/"');
    expect(page).not.toMatch(/http-equiv\s*=\s*["']?refresh/i);
    // A page's own CSP can only narrow the app's (both apply); it stays.
    expect(page).toContain('content="default-src *"');
    // The page is a document body, never an attribute value: no quoting to escape.
    expect(page).toContain("</iframe></html><iframe srcdoc=");
  });

  it("wraps an SVG file in a minimal page without its XML prolog", () => {
    expect(svgVisualPage('﻿<?xml version="1.0" encoding="UTF-8"?>\n<svg width="2"/>'))
      .toBe("<!doctype html><html><head><style>html,body{margin:0}svg{max-width:100%;height:auto}</style></head><body>"
        + '\n<svg width="2"/></body></html>');
  });

  it("names frames by an unguessable token and reads only well-formed requests", () => {
    expect(visualFrameUrl(TOKEN)).toBe(`iface-visual://frame/${TOKEN}`);
    expect(() => visualFrameUrl("../x")).toThrow();
    expect(visualFrameToken(`iface-visual://frame/${TOKEN}`)).toBe(TOKEN);
    for (const url of [`iface-visual://frame/${TOKEN}/x`, `iface-visual://other/${TOKEN}`,
      `https://frame/${TOKEN}`, "iface-visual://frame/ABC", ""]) {
      expect(visualFrameToken(url)).toBeNull();
    }
    expect(parseVisualFrameRequest({ kind: "register", html: "<p>x</p>" }))
      .toEqual({ kind: "register", html: "<p>x</p>" });
    expect(parseVisualFrameRequest({ kind: "release", url: `iface-visual://frame/${TOKEN}` }))
      .toEqual({ kind: "release", url: `iface-visual://frame/${TOKEN}` });
    for (const bad of [null, "x", {}, { kind: "register" }, { kind: "register", html: 1 },
      { kind: "register", html: "x", extra: true }, { kind: "release", url: "https://x" },
      { kind: "register", html: "x".repeat(MAX_WRAPPED_CHARS + 1) }]) {
      expect(parseVisualFrameRequest(bad)).toBeNull();
    }
  });
});
