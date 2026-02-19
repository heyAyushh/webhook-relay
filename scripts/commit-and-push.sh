#!/usr/bin/env bash
# Commit all changes and push to origin on the current branch.
# Prevents direct pushes to main/master. Use MSG for commit message.
#
# Usage:
#   MSG="fix: remove unnecessary debug log" ./scripts/commit-and-push.sh
#   ./scripts/commit-and-push.sh "feat: add new endpoint"
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

# Commit message: env MSG or first argument
MSG="${MSG:-${1:-}}"
if [ -z "$MSG" ]; then
  echo "Usage: MSG=\"<Prefix>: <summary>\" $0" >&2
  echo "   or: $0 \"<Prefix>: <summary>\"" >&2
  exit 1
fi

BRANCH=$(git -C "$REPO_ROOT" branch --show-current)
if [ "$BRANCH" = "main" ] || [ "$BRANCH" = "master" ]; then
  echo "⚠️ Direct pushes to main/master are not allowed"
  exit 1
fi

# Optional quality checks (uncomment and adjust per project)
# ./scripts/lint.sh && ./scripts/test.sh && ./scripts/build.sh || exit 1

git -C "$REPO_ROOT" add -A
git -C "$REPO_ROOT" commit -m "$MSG"
git -C "$REPO_ROOT" push -u origin "$BRANCH"
