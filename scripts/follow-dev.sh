#!/bin/sh
# SessionStart hook (.claude/settings.json). A session reads AGENTS.md and
# .claude/settings.json from the checkout it starts in, so a main checkout left
# behind dev hands it stale rules. When the session starts in the main checkout
# and that checkout is detached, has no tracked edits and is behind origin/dev,
# move it to origin/dev. A branch, local edits, a linked worktree, another
# clone or a HEAD that is not an ancestor of origin/dev is left alone.
main="$HOME/Developer/Interface-Rust"
cd "${CLAUDE_PROJECT_DIR:-.}" 2>/dev/null || exit 0
top=$(git rev-parse --show-toplevel 2>/dev/null) || exit 0
[ "$top" = "$(cd "$main" 2>/dev/null && pwd -P)" ] || exit 0
git_dir=$(git rev-parse --path-format=absolute --git-dir 2>/dev/null) || exit 0
[ "$git_dir" = "$(git rev-parse --path-format=absolute --git-common-dir)" ] || exit 0
git symbolic-ref -q HEAD >/dev/null && exit 0
git diff --quiet 2>/dev/null && git diff --cached --quiet 2>/dev/null || exit 0
head=$(git rev-parse -q --verify HEAD) || exit 0
dev=$(git rev-parse -q --verify origin/dev) || exit 0
[ "$head" = "$dev" ] && exit 0
git merge-base --is-ancestor "$head" "$dev" 2>/dev/null || exit 0
git switch -q --detach "$dev" 2>/dev/null || exit 0
echo "This checkout moved from $(git rev-parse --short "$head") to origin/dev $(git rev-parse --short "$dev") after this session loaded AGENTS.md; reread it before relying on closeout or release rules."
exit 0
