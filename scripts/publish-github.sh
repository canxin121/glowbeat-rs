#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
repo_name="${1:-glowbeat-rs}"
visibility="${GLOWBEAT_REPO_VISIBILITY:-public}"

if ! command -v gh >/dev/null 2>&1; then
  echo "error: GitHub CLI (gh) is required" >&2
  exit 2
fi
gh auth status

case "$visibility" in
  public|private|internal) ;;
  *) echo "error: GLOWBEAT_REPO_VISIBILITY must be public, private, or internal" >&2; exit 2 ;;
esac

if [[ ! -d .git ]]; then
  git init -b main
fi
if ! git config user.name >/dev/null; then
  git config user.name "$(gh api user --jq .login)"
fi
if ! git config user.email >/dev/null; then
  login="$(gh api user --jq .login)"
  id="$(gh api user --jq .id)"
  git config user.email "${id}+${login}@users.noreply.github.com"
fi

git add -A
if ! git diff --cached --quiet; then
  git commit -m "Initial open-source release"
fi

if git remote get-url origin >/dev/null 2>&1; then
  git push -u origin HEAD:main
else
  gh repo create "$repo_name" "--$visibility" --source=. --remote=origin --push
fi

repo="$(gh repo view --json nameWithOwner --jq .nameWithOwner)"
sha="$(git rev-parse HEAD)"
echo "published: https://github.com/$repo"
echo "commit: $sha"

run_id=""
for _ in {1..30}; do
  run_id="$(gh run list --repo "$repo" --commit "$sha" --workflow ci.yml --limit 1 --json databaseId --jq '.[0].databaseId // empty')"
  [[ -n "$run_id" ]] && break
  sleep 2
done
if [[ -z "$run_id" ]]; then
  echo "error: CI workflow did not appear for commit $sha" >&2
  exit 3
fi

echo "watching GitHub Actions run $run_id"
gh run watch "$run_id" --repo "$repo" --exit-status
