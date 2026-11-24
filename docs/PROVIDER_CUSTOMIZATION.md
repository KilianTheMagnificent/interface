# Provider customization and the local runner

Settings → Agent customization selects Claude, Codex, Grok, Qwen or OpenCode.
The inventory shows files discovered in the selected project and that provider's
configured home. Every entry remains **loading unknown**: discovering a file is
not evidence of precedence, trust, enabled state or a running session loading it.
The legacy `listCustomization` request remains unchanged; `customizationControl`
is the additive protocol 49 surface.

The host supplies provider homes, including `CLAUDE_CONFIG_DIR`, `CODEX_HOME`,
`GROK_HOME`, `QWEN_HOME`, `OPENCODE_CONFIG_DIR` and `OPENCODE_CONFIG`. OpenCode
keeps its global directory (`XDG_CONFIG_HOME/opencode`, or `~/.config/opencode`)
when an extra config directory is supplied. Codex also discovers user skills
in `~/.agents/skills`, independently of `CODEX_HOME`. Codex TOML
is parsed with `toml`. Claude user/local MCP entries are read from `.claude.json`
as well as project `.mcp.json`; only the current project's local entries are
included. Configured argv, environments, headers, URL paths, tokens and skill
bodies never enter the result. Source reveals only a freshly discovered,
bounded, non-symlink file; it grants no generic filesystem access.

A server's Check starts its configured stdio process for at most ten seconds,
sends MCP initialize and ping, and stops the temporary process group. It calls no
tools. Stdout is bounded and never returned; stderr is discarded. Ready means
that temporary connection answered, not that an existing provider session loaded
it. Disabled servers are not started. HTTP/authenticated transports and
provider-specific substitution syntax remain explicitly unsupported: use that
provider's authenticated session to check them. Inventory does not resolve
managed configuration, inline runtime overrides, plugin discovery; those limits must not be represented as proof of absence.

Provider layout references: [Claude MCP scopes](https://code.claude.com/docs/en/mcp),
[OpenCode configuration](https://docs.opencode.ai/docs/config/), and
[Qwen configuration](https://qwenlm.github.io/qwen-code-docs/en/users/configuration/settings/).

The application menu's **Run with window closed** keeps the existing desktop
process, renderer and single Rust host alive after closing the window. Click the
Dock icon to return to the same window. Its preference is stored by Rust in the
leased `background.v1.json` sidecar. Disabling changes the *next close* into the
ordinary guarded Quit flow; it does not cancel current work immediately. Quit
still stops the runner, the Mac must stay awake, and this does not install a
login service or a second host. Existing routine test-before-enable, missed-run
policy and unknown-outcome recovery gates remain unchanged.
