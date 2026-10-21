#!/usr/bin/env bash
set -euo pipefail

readonly REPO="allisonhere/moo"
readonly BRANCH="master"

bold='\033[1m'
cyan='\033[36m'
green='\033[32m'
yellow='\033[33m'
red='\033[31m'
reset='\033[0m'

info() { printf '%b•%b %s\n' "$cyan" "$reset" "$*"; }
ok() { printf '%b✓%b %s\n' "$green" "$reset" "$*"; }
warn() { printf '%b!%b %s\n' "$yellow" "$reset" "$*"; }
die() { printf '%b✗ %s%b\n' "$red" "$*" "$reset" >&2; exit 1; }

usage() {
    cat <<'EOF'
Usage: ./deploy.sh [--dry-run] [patch|minor|major|X.Y.Z]

Prepare and publish an immutable Moo release from clean, pushed master.
Without a version argument, the script prompts for the bump.
EOF
}

dry_run=0
requested=""
while (($#)); do
    case "$1" in
        --dry-run) dry_run=1 ;;
        -h|--help) usage; exit 0 ;;
        patch|minor|major) [[ -z "$requested" ]] || die "choose one version"; requested="$1" ;;
        [0-9]*.[0-9]*.[0-9]*) [[ -z "$requested" ]] || die "choose one version"; requested="$1" ;;
        *) die "unknown argument: $1" ;;
    esac
    shift
done

repo_root=$(git rev-parse --show-toplevel 2>/dev/null) || die "run this inside the Moo repository"
cd "$repo_root"

printf '\n%bMoo release%b\n\n' "$bold$cyan" "$reset"

[[ $(git branch --show-current) == "$BRANCH" ]] || die "releases must start on $BRANCH"
origin=$(git remote get-url origin)
case "$origin" in
    git@github.com:"$REPO".git|https://github.com/"$REPO".git|https://github.com/"$REPO") ;;
    *) die "origin is $origin; expected github.com/$REPO" ;;
esac
[[ -z $(git status --porcelain) ]] || die "working tree must be clean; commit or remove every change first"

info "fetching $BRANCH and release tags"
git fetch --prune origin "+refs/heads/$BRANCH:refs/remotes/origin/$BRANCH" --tags
[[ $(git rev-parse HEAD) == $(git rev-parse "origin/$BRANCH") ]] || die "local $BRANCH must exactly match origin/$BRANCH"

current=$(git tag --list 'v[0-9]*' --sort=-v:refname | head -1)
current=${current:-v0.0.0}
current_version=${current#v}

if [[ -z "$requested" ]]; then
    read -r major minor patch <<<"${current_version//./ }"
    printf 'Current release: %b%s%b\n' "$yellow" "$current" "$reset"
    printf '  1) patch  v%s.%s.%s\n' "$major" "$minor" "$((patch + 1))"
    printf '  2) minor  v%s.%s.0\n' "$major" "$((minor + 1))"
    printf '  3) major  v%s.0.0\n' "$((major + 1))"
    printf '  4) custom\n'
    read -r -p 'Choice [1]: ' choice
    case "${choice:-1}" in
        1) requested=patch ;;
        2) requested=minor ;;
        3) requested=major ;;
        4) read -r -p 'Version (without v): ' requested ;;
        *) die "invalid choice" ;;
    esac
fi

version=$(python3 - "$current_version" "$requested" <<'PY'
import re
import sys

current, requested = sys.argv[1:]
match = re.fullmatch(r"(\d+)\.(\d+)\.(\d+)", current)
if not match:
    raise SystemExit(f"invalid current version: {current}")
major, minor, patch = map(int, match.groups())
if requested == "patch":
    patch += 1
elif requested == "minor":
    minor, patch = minor + 1, 0
elif requested == "major":
    major, minor, patch = major + 1, 0, 0
elif re.fullmatch(r"\d+\.\d+\.\d+", requested):
    major, minor, patch = map(int, requested.split("."))
else:
    raise SystemExit(f"invalid version: {requested}")
candidate = (major, minor, patch)
if candidate <= tuple(map(int, current.split("."))):
    raise SystemExit(f"version {'.'.join(map(str, candidate))} must be newer than {current}")
print(".".join(map(str, candidate)))
PY
) || die "could not select a release version"
tag="v$version"

git rev-parse --verify --quiet "refs/tags/$tag" >/dev/null && die "$tag already exists locally"
git ls-remote --exit-code --tags --refs origin "refs/tags/$tag" >/dev/null 2>&1 && die "$tag already exists on origin"

printf '\nRelease %b%s%b from %s\n' "$bold" "$tag" "$reset" "$(git rev-parse --short=12 HEAD)"
if ((dry_run)); then
    warn "dry run: no files, commits, branches, or tags will change"
    info "would bump Cargo.toml and Cargo.lock"
    info "would run just check"
    info "would commit release: $tag and atomically push $BRANCH with an annotated tag"
    exit 0
fi

read -r -p "Continue? [y/N] " confirmed
[[ "$confirmed" =~ ^[Yy]$ ]] || die "release cancelled"

python3 - "$version" <<'PY'
from pathlib import Path
import re
import sys

path = Path("Cargo.toml")
text = path.read_text()
updated, count = re.subn(
    r'(?m)^(\[package\]\n(?:.*\n)*?version = ")[^"]+("\n)',
    rf'\g<1>{sys.argv[1]}\2',
    text,
    count=1,
)
if count != 1:
    raise SystemExit("could not update package version in Cargo.toml")
path.write_text(updated)
PY

release_committed=0
cleanup() {
    if ((release_committed == 0)); then
        git restore -- Cargo.toml Cargo.lock 2>/dev/null || true
    fi
}
trap cleanup EXIT

cargo update -p herdr --offline
info "running release checks"
just check

git add Cargo.toml Cargo.lock
git diff --cached --quiet && die "version files did not change"
git commit -m "release: $tag"
release_committed=1
trap - EXIT

git tag -a "$tag" -m "Moo $tag"
git push --atomic origin "$BRANCH" "refs/tags/$tag"
ok "$tag pushed; GitHub Actions is building the release"

if command -v gh >/dev/null 2>&1; then
    run_id=""
    for _ in {1..30}; do
        run_id=$(gh run list --repo "$REPO" --workflow moo-release.yml --branch "$tag" --limit 1 --json databaseId --jq '.[0].databaseId // empty' 2>/dev/null || true)
        [[ -n "$run_id" ]] && break
        sleep 2
    done
    if [[ -n "$run_id" ]]; then
        gh run watch "$run_id" --repo "$REPO" --exit-status
    else
        warn "release run has not appeared yet; monitor https://github.com/$REPO/actions"
    fi
fi

ok "release flow complete"
