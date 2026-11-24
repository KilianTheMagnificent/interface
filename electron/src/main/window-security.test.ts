import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { productionCspHtml } from "../../vite.config";
import {
  contentSecurityPolicy, isAllowedRendererNavigation, isAuthorizedIpcSender,
  resolveRendererLocation,
  validatedExternalUrl,
} from "./window-security.js";

describe("window security", () => {
  it("keeps production CSP free of dev script and connection allowances", () => {
    const production = contentSecurityPolicy(undefined);
    expect(production).toContain("script-src 'self'");
    expect(production).toContain("connect-src 'self'");
    expect(production).not.toContain("unsafe-eval");
    expect(production).toContain("worker-src 'self'");
    for (const directive of ["object-src", "base-uri", "form-action", "frame-ancestors"]) {
      expect(production).toContain(`${directive} 'none'`);
    }
    // T3 section 8: frames come only from the visual scheme, in both
    // builds, and nothing else in the policy loosens.
    for (const policy of [production, contentSecurityPolicy("http://127.0.0.1:5173")]) {
      expect(policy.split("; ").filter(directive => directive.startsWith("frame-src")))
        .toEqual(["frame-src iface-visual:"]);
    }
    const page = readFileSync(new URL("../../index.html", import.meta.url), "utf8");
    const meta = /http-equiv="Content-Security-Policy" content="([^"]*)"/.exec(page)?.[1] ?? "";
    expect(meta.split("; ").filter(directive => directive.startsWith("frame-src")))
      .toEqual(["frame-src iface-visual:"]);
    expect(meta).toContain("object-src 'none'");
    expect(meta).toContain("base-uri 'none'");

    const development = contentSecurityPolicy("http://127.0.0.1:5173");
    expect(development).toContain("script-src 'self' 'unsafe-inline'");
    expect(development).toContain("http://127.0.0.1:5173");
    expect(development).toContain("ws://127.0.0.1:5173");

    // Packaged file:// loads rely on the built meta tag: every meta-legal directive
    // in built index.html must match contentSecurityPolicy(undefined).
    const builtPage = productionCspHtml(page);
    const builtMeta = /http-equiv="Content-Security-Policy" content="([^"]*)"/.exec(builtPage)?.[1] ?? "";
    expect(builtMeta.split("; ")).toEqual(
      production.split("; ").filter(directive => !directive.startsWith("frame-ancestors ")),
    );
    expect(builtMeta).not.toContain("'unsafe-inline');");
    expect(builtMeta).toContain("script-src 'self';");
    expect(builtMeta).toContain("connect-src 'self';");

    const remotePage = readFileSync(new URL("../../remote.html", import.meta.url), "utf8");
    expect(remotePage).toContain("<head>");
    const remoteMeta = /http-equiv="Content-Security-Policy" content="([^"]*)"/.exec(remotePage)?.[1] ?? "";
    expect(remoteMeta).toBe(
      "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; connect-src 'self' ws: wss:; img-src 'self' data:; font-src 'self' data:; manifest-src 'self'; base-uri 'none'; form-action 'self'",
    );
  });

  it("ignores a development URL in packaged builds", () => {
    expect(resolveRendererLocation({
      isPackaged: true,
      allowPackagedDevServer: false,
      devServerUrl: "https://attacker.example",
      packagedEntryUrl: "file:///Applications/Interface.app/index.html",
    })).toEqual({ packagedEntryUrl: "file:///Applications/Interface.app/index.html" });
  });

  it("lets only the supervised packaged Dev launcher use an exact loopback Vite root", () => {
    const packagedEntryUrl = "file:///Applications/Interface Dev.app/index.html";
    expect(resolveRendererLocation({
      isPackaged: true,
      allowPackagedDevServer: true,
      devServerUrl: "http://127.0.0.1:5199",
      packagedEntryUrl,
    })).toEqual({ devServerUrl: "http://127.0.0.1:5199", packagedEntryUrl });
    for (const devServerUrl of [
      "https://127.0.0.1:5199",
      "http://localhost:5199",
      "http://127.0.0.1:5199/nested",
      "http://attacker.example:5199",
    ]) {
      expect(resolveRendererLocation({
        isPackaged: true,
        allowPackagedDevServer: true,
        devServerUrl,
        packagedEntryUrl,
      })).toEqual({ packagedEntryUrl });
    }
  });

  it("accepts only the configured renderer trust root", () => {
    const packaged = { packagedEntryUrl: "file:///Applications/Interface.app/index.html" };
    expect(isAllowedRendererNavigation("file:///Applications/Interface.app/index.html#chat", packaged))
      .toBe(true);
    expect(isAllowedRendererNavigation("file:///Applications/Interface.app/other.html", packaged))
      .toBe(false);
    const dev = { devServerUrl: "http://127.0.0.1:5173", packagedEntryUrl: "file:///unused" };
    expect(isAllowedRendererNavigation("http://127.0.0.1:5173/chat", dev)).toBe(true);
    expect(isAllowedRendererNavigation("https://127.0.0.1:5173/chat", dev)).toBe(false);
  });

  it("allows narrow external links and rejects privileged or credentialed URLs", () => {
    expect(validatedExternalUrl("https://example.com/docs")).toBe("https://example.com/docs");
    expect(validatedExternalUrl("mailto:person@example.com")).toBe("mailto:person@example.com");
    expect(validatedExternalUrl("file:///etc/passwd")).toBeNull();
    expect(validatedExternalUrl("https://user:pass@example.com")).toBeNull();
    expect(validatedExternalUrl(" javascript:alert(1)")).toBeNull();
  });

  it("authorizes only the trusted WebContents main frame", () => {
    const mainFrame = {};
    const contents = { mainFrame };
    expect(isAuthorizedIpcSender({ sender: contents, senderFrame: mainFrame }, contents)).toBe(true);
    expect(isAuthorizedIpcSender({ sender: contents, senderFrame: {} }, contents)).toBe(false);
    expect(isAuthorizedIpcSender({ sender: {}, senderFrame: mainFrame }, contents)).toBe(false);
    expect(isAuthorizedIpcSender({ sender: contents, senderFrame: mainFrame }, null)).toBe(false);
  });
});
