#!/bin/sh
# Installs the interface-theme skill for Codex and Claude Code, with the theme
# guide, the scene embedder, the battery checker, the example looks and TN
# Code's shaders it reads.
set -eu
replace=${1:-}
case "$replace" in ''|--replace) ;; *) echo 'Usage: install-theme-skill.sh [--replace]' >&2; exit 2;; esac
repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
skill="$repo/skills/interface-theme"
examples="$repo/docs/theme-examples"
skill_home=${THEME_SKILL_INSTALL_HOME:-$HOME}
test -f "$skill/SKILL.md"
test -f "$repo/docs/THEMES.md"
test -f "$examples/embed-scene.py"
test -f "$examples/check-theme.mjs"
test -f "$examples/tn-code/shaders/picture-dither.frag"
for target in "$skill_home/.codex/skills/interface-theme" "$skill_home/.claude/skills/interface-theme"; do
  if [ -L "$target" ]; then echo "Refusing symlink installation target: $target" >&2; exit 1; fi
  if [ -e "$target" ] && [ "$replace" != --replace ]; then
    echo "Existing installation at $target; inspect it before using --replace." >&2
    exit 1
  fi
done
for target in "$skill_home/.codex/skills/interface-theme" "$skill_home/.claude/skills/interface-theme"; do
  mkdir -p "$target/references/scenes" "$target/references/shaders"
  install -m 644 "$skill/SKILL.md" "$target/SKILL.md"
  install -m 644 "$repo/docs/THEMES.md" "$target/references/themes.md"
  install -m 755 "$examples/embed-scene.py" "$target/references/embed-scene.py"
  install -m 755 "$examples/check-theme.mjs" "$target/references/check-theme.mjs"
  for theme in "$examples"/*.css; do install -m 644 "$theme" "$target/references/"; done
  for scene in "$examples"/scenes/*.svg; do install -m 644 "$scene" "$target/references/scenes/"; done
  for shader in "$examples"/tn-code/shaders/*.frag; do install -m 644 "$shader" "$target/references/shaders/"; done
done
printf '%s\n' 'Installed Codex $interface-theme and Claude Code /interface-theme.'
