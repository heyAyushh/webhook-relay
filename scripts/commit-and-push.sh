#!/usr/bin/env bash
# Commit and push current branch (non-interactive).
# Branch policy: direct pushes to main/master are disallowed (adjust per project).
# Commit message: use env MSG or first argument. See .cursor/rules/commit-message-format.mdc if present.
# Preconditions: modified files; remote origin configured.

set -e

MSG="${MSG:-$1}"
if [ -z "$MSG" ]; then
  echo "Usage: MSG=\"<Prefix>: <summary>\" $0   OR   $0 \"<Prefix>: <summary>\""
  exit 1
fi

# 1) Check branch — prevent direct pushes to main/master
BRANCH=$(git branch --show-current)
if [ "$BRANCH" = "main" ] || [ "$BRANCH" = "master" ]; then
  echo "⚠️ Direct pushes to main/master are not allowed"
  exit 1
fi

# 2) Optional quality checks (uncomment and adjust per project)
# ./scripts/lint.sh && ./scripts/test.sh && ./scripts/build.sh || exit 1

# 3) Stage changes
git add -A

# 4) Commit
git commit -m "$MSG"

# 5) Push
git push -u origin "$BRANCH"
