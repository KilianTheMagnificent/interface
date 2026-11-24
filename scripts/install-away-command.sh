#!/bin/sh
# Installation only; all loop decisions and effects belong to the Rust binary.
set -eu
binary=${1:?Usage: install-away-command.sh /path/to/iface [--replace]}
replace=${2:-}
case "$replace" in ''|--replace) ;; *) echo 'Unknown option' >&2; exit 2;; esac
repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
skill="$repo/skills/away"
away_home=${AWAY_INSTALL_HOME:-$HOME}
install_root="$away_home/.local/lib/interface-away"
launcher="$away_home/.local/bin/iface-away"
test -x "$binary"
test -f "$skill/SKILL.md"
"$binary" away cycle --help >/dev/null
for target in "$install_root" "$launcher" "$away_home/.codex/skills/away" "$away_home/.claude/skills/away"; do
  if [ -e "$target" ] || [ -L "$target" ]; then
    if [ "$replace" != --replace ]; then
      echo "Existing Away installation at $target; inspect it before using --replace." >&2
      exit 1
    fi
    if [ -L "$target" ]; then echo "Refusing symlink installation target: $target" >&2; exit 1; fi
  fi
done
umask 077
mkdir -p "$install_root" "$away_home/.local/bin" "$away_home/.codex/skills/away/references" "$away_home/.claude/skills/away/references"
install -m 700 "$binary" "$install_root/iface.new"
mv "$install_root/iface.new" "$install_root/iface"
for target in "$away_home/.codex/skills/away" "$away_home/.claude/skills/away"; do
  install -m 600 "$skill/SKILL.md" "$target/SKILL.md"
  install -m 600 "$skill/references/command.md" "$target/references/command.md"
done
cat > "$launcher.new" <<'SH'
#!/bin/sh
away_bin=$(CDPATH= cd -- "$(dirname -- "$0")/../lib/interface-away" && pwd)
exec "$away_bin/iface" away cycle "$@"
SH
chmod 700 "$launcher.new"
mv "$launcher.new" "$launcher"
printf '%s\n' 'Installed iface-away, Codex $away and Claude Code /away.'
