import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import {
  ACP_PROFILES_CSS, AcpProfilesEditor, validateAcpProfileDraft,
} from "./AcpProfilesEditor";

describe("AcpProfilesEditor", () => {
  it("states exact argv, no-shell, and credential boundaries", () => {
    const html = renderToStaticMarkup(createElement(AcpProfilesEditor, {
      profiles: [{ slug: "gemini", displayName: "Gemini", executablePath: "/bin/gemini",
        args: ["agent", "stdio"] }],
      onSave: () => undefined,
    }));
    expect(html).toContain("Custom ACP providers");
    expect(html).toContain('data-voice-guard="settings"');
    expect(html).toContain("exact argument array");
    expect(html).toContain("never invokes a shell");
    expect(html).toContain("do not put tokens or secrets in arguments");
    expect(html).toContain("[&quot;agent&quot;,&quot;stdio&quot;]");
    // A profile whose slug names a known brand gets its mark, not initials.
    expect(html).toContain('data-provider-logo="gemini"');
    expect(html).toContain('role="status"');
    expect(html).toContain('aria-live="polite"');
  });

  it("shows restart activation and backend errors explicitly", () => {
    const html = renderToStaticMarkup(createElement(AcpProfilesEditor, {
      profiles: [], restartRequired: true, error: "Profile path is invalid.",
      onSave: () => undefined,
    }));
    expect(html).toContain("Restart Interface");
    expect(html).toContain('role="alert"');
    expect(html).toContain("Profile path is invalid.");
  });

  it("mirrors backend field validation before a roster is submitted", () => {
    const valid = validateAcpProfileDraft({
      slug: "local-agent", displayName: "  Local Agent  ",
      executablePath: "/opt/homebrew/bin/local-agent", argsJson: '["acp","stdio"]',
    });
    expect(valid.errors).toEqual({});
    expect(valid.profile).toEqual({
      slug: "local-agent", displayName: "Local Agent",
      executablePath: "/opt/homebrew/bin/local-agent", args: ["acp", "stdio"],
    });

    const invalid = validateAcpProfileDraft({
      slug: "grok", displayName: "", executablePath: "/bin/sh",
      argsJson: '["--api-key","placeholder"]',
    }, true);
    expect(invalid.profile).toBeNull();
    expect(invalid.errors.slug).toMatch(/reserved/);
    expect(validateAcpProfileDraft({
      slug: "qwen", displayName: "Qwen", executablePath: "/opt/homebrew/bin/qwen",
      argsJson: '["--acp"]',
    }).errors.slug).toMatch(/reserved/);
    expect(invalid.errors.displayName).toMatch(/visible/);
    expect(invalid.errors.executablePath).toMatch(/non-shell/);
    expect(invalid.errors.argsJson).toMatch(/credential/);
  });

  it("reports malformed JSON, duplicate slugs, and pending saves explicitly", () => {
    expect(validateAcpProfileDraft({
      slug: "local-agent", displayName: "Local Agent",
      executablePath: "/bin/local-agent", argsJson: "not-json",
    }).errors.argsJson).toMatch(/valid JSON/);
    expect(validateAcpProfileDraft({
      slug: "local-agent", displayName: "Local Agent",
      executablePath: "/bin/local-agent", argsJson: "[]",
    }, true).errors.slug).toMatch(/unique/);

    const html = renderToStaticMarkup(createElement(AcpProfilesEditor, {
      profiles: [], pending: true, onSave: () => undefined,
    }));
    expect(html).toContain('aria-busy="true"');
    expect(html).toContain("Saving provider roster…");
    expect(html).toContain("disabled");
  });

  it("prevents adding beyond the backend roster limit", () => {
    const html = renderToStaticMarkup(createElement(AcpProfilesEditor, {
      profiles: Array.from({ length: 64 }, (_, index) => ({
        slug: `provider-${index}`, displayName: `Provider ${index}`,
        executablePath: `/bin/provider-${index}`, args: [],
      })),
      onSave: () => undefined,
    }));
    expect(html).toMatch(/<button[^>]+disabled=""[^>]+title="A roster can contain at most 64 providers\./);
  });

  it("stacks fields and actions for narrow settings windows", () => {
    expect(ACP_PROFILES_CSS).toContain("@media (max-width:480px)");
    expect(ACP_PROFILES_CSS).toContain("grid-template-columns:1fr");
    expect(ACP_PROFILES_CSS).toContain(':hover:not(:disabled)');
    expect(ACP_PROFILES_CSS).toContain('aria-invalid="true"');
  });

  it("does not spend the attention hue on a settled roster", () => {
    // The status line's usual message is "Provider roster is saved." — amber
    // there claimed an agent was waiting on the user for a state that is done.
    expect(ACP_PROFILES_CSS).not.toContain("var(--status-waiting)");
    expect(ACP_PROFILES_CSS).toContain(
      ".acp-profiles-status { flex:1 1 auto; color:var(--fg-muted); font-size:var(--text-meta); }");
    expect(ACP_PROFILES_CSS).toContain("color:var(--danger-fg); background:var(--danger-bg);");
  });
});
