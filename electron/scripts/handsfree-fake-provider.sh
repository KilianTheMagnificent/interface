#!/bin/sh
# Stands in for every provider CLI in a hands-free session (HANDSFREE=1, see
# src/main/handsfree.ts): no account, network or provider state. It answers
# --version and, in a terminal pane, echoes each typed line until EOF or
# __HANDSFREE_EXIT__. Claude started for a structured session (stream-json in
# and out, the argument prefix crates/engine-claude/src/launch.rs builds) is
# the scripted fake Claude, handsfree-fake-claude.mjs, and Grok started over
# ACP is the scripted ACP agent, handsfree-fake-acp.mjs, each run on this
# checkout's Electron as Node. Anything else (other ACP agents, usage probes)
# ends at once.
case "${1:-}" in --version|-v|version) printf 'handsfree-fake-provider 1.0.0\n'; exit 0 ;; esac
# `claude auth status --json`: signed in, unless the session HOME holds
# .handsfree-signed-out (a test of Settings' Signed out state).
case "$*" in
  "auth status --json")
    if [ -f "$HOME/.handsfree-signed-out" ]; then printf '{"loggedIn": false, "authMethod": "none"}\n'; exit 1; fi
    printf '{"loggedIn": true, "authMethod": "claude.ai", "email": "handsfree@example.test"}\n'; exit 0 ;;
esac
# Runs one of the scripted fakes beside this script on this checkout's
# Electron as Node (else on `node`).
run_fake() {
  fake=$1; shift
  self=$0
  if [ -L "$self" ]; then
    target=$(readlink "$self")
    case "$target" in /*) self=$target ;; *) self=$(dirname "$self")/$target ;; esac
  fi
  scripts=$(dirname "$self")
  electron="$scripts/../node_modules/electron"
  binary="$electron/dist/$(cat "$electron/path.txt" 2>/dev/null)"
  if [ -f "$electron/path.txt" ] && [ -x "$binary" ]; then
    ELECTRON_RUN_AS_NODE=1 exec "$binary" "$scripts/$fake" "$@"
  fi
  exec node "$scripts/$fake" "$@"
}
case "$*" in
  "--output-format stream-json --verbose --input-format stream-json"*)
    run_fake handsfree-fake-claude.mjs "$@" ;;
  # A one-shot print call (crates/engine-claude/src/title.rs: a title, or
  # protocol 67's Write with AI) answers one JSON result.
  "-p --output-format json"*)
    run_fake handsfree-fake-claude.mjs "$@" ;;
esac
# Grok started over ACP (crates/engine-acp/src/adapter.rs: its flags, then
# `agent`, then `stdio` last) is the scripted ACP agent,
# handsfree-fake-acp.mjs: a second structured provider for hands-free runs.
for last in "$@"; do :; done
if [ "${last:-}" = stdio ]; then
  case " $* " in *" agent "*) run_fake handsfree-fake-acp.mjs "$@" ;; esac
fi
# `grok|agy mcp list|add|remove`: Settings' switch for Interface's tools
# (protocol 48). The session HOME keeps one marker file per CLI while its
# `iface` entry is on; each CLI lists it in its own format. The host runs the
# resolved script, so Grok is told apart by its own flags (`--json`,
# `--scope`), not by its name.
case "${1:-} ${2:-}" in
  "mcp list"|"mcp add"|"mcp remove")
    case "${3:-}" in --json|--scope) name=grok ;; *) name=agy ;; esac
    state="$HOME/.handsfree-mcp-$name"
    case "$2" in
      add) : > "$state"; exit 0 ;;
      remove) rm -f "$state"; exit 0 ;;
    esac
    if [ "$name" = grok ]; then
      if [ -f "$state" ]; then
        printf '[{"command":"/bin/sh","args":["-c","exec /usr/bin/nc -U \\"$INTERFACE_PANE_MCP_SOCKET\\""],"enabled":true,"name":"iface","scope":"user"}]\n'
      else printf '[]\n'; fi
    elif [ -f "$state" ]; then
      printf 'NAME   TYPE   STATUS   COMMAND/URL\niface  stdio  enabled  %s\n' \
        '/bin/sh -c exec /usr/bin/nc -U "$INTERFACE_PANE_MCP_SOCKET"'
    else printf 'No MCP servers configured.\n'; fi
    exit 0 ;;
esac
[ -t 0 ] || exit 0
printf 'handsfree fake provider: typed lines echo here\n> '
while IFS= read -r line; do
  [ "$line" = "__HANDSFREE_EXIT__" ] && exit 0
  printf 'fake-provider-echo:%s\n> ' "$line"
done
