#!/usr/bin/env bash
set -uo pipefail

readonly REPO="allisonhere/moo"
readonly BRANCH="master"
readonly TOOLCHAIN="1.96.1"
readonly SELF="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/$(basename "${BASH_SOURCE[0]}")"
readonly STATUS_ROW=15

bold=$'\033[1m'; cyan=$'\033[36m'; green=$'\033[32m'
yellow=$'\033[33m'; red=$'\033[31m'; reset=$'\033[0m'
activity="Ready."

die() { printf 'error: %s\n' "$*" >&2; exit 1; }

usage() {
    cat <<'EOF'
Usage: ./deploy.sh
       ./deploy.sh [--dry-run] patch|minor|major|X.Y.Z

Without a version argument, opens the Moo release console. The direct form
runs Validate, Commit, Push, and Release in order.
EOF
}

repo_root() { git rev-parse --show-toplevel 2>/dev/null || die "run this inside the Moo repository"; }

require_origin() {
    local origin
    origin=$(git remote get-url origin 2>/dev/null) || die "origin is not configured"
    case "$origin" in
        git@github.com:"$REPO".git|https://github.com/"$REPO".git|https://github.com/"$REPO") ;;
        *) die "origin is $origin; expected github.com/$REPO" ;;
    esac
}

fetch_release_refs() { git fetch --quiet --prune origin "+refs/heads/$BRANCH:refs/remotes/origin/$BRANCH" --tags; }

latest_tag() {
    local tag
    tag=$(git tag --list 'v[0-9]*' --sort=-v:refname | head -1)
    printf '%s\n' "${tag:-v0.0.0}"
}

resolve_version() {
    python3 - "${1#v}" "$2" <<'PY'
import re, sys
current, requested = sys.argv[1:]
if not re.fullmatch(r"\d+\.\d+\.\d+", current): raise SystemExit(f"invalid current version: {current}")
major, minor, patch = map(int, current.split("."))
if requested == "patch": patch += 1
elif requested == "minor": minor, patch = minor + 1, 0
elif requested == "major": major, minor, patch = major + 1, 0, 0
elif re.fullmatch(r"\d+\.\d+\.\d+", requested): major, minor, patch = map(int, requested.split("."))
else: raise SystemExit(f"invalid version: {requested}")
candidate = (major, minor, patch)
if candidate <= tuple(map(int, current.split("."))): raise SystemExit("release version must be newer than the latest tag")
print(".".join(map(str, candidate)))
PY
}

preflight() {
    local action=$1
    [[ $(git branch --show-current) == "$BRANCH" ]] || die "releases must run from $BRANCH"
    require_origin
    fetch_release_refs || return
    case "$action" in
        validate|commit) [[ $(git rev-parse HEAD) == $(git rev-parse "origin/$BRANCH") ]] || die "start from the current origin/$BRANCH" ;;
        push)
            [[ -z $(git status --porcelain --untracked-files=no) ]] || die "tracked files remain; run Commit first"
            git merge-base --is-ancestor "origin/$BRANCH" HEAD || die "$BRANCH has diverged from origin/$BRANCH"
            ;;
        release)
            [[ -z $(git status --porcelain --untracked-files=no) ]] || die "tracked files remain; run Commit first"
            [[ $(git rev-parse HEAD) == $(git rev-parse "origin/$BRANCH") ]] || die "run Push first"
            ;;
    esac
}

action_validate() { preflight validate && env RUSTUP_TOOLCHAIN="$TOOLCHAIN" just ci; }

action_commit() {
    local bump=$1 dry=$2 current version
    preflight commit || return
    current=$(latest_tag); version=$(resolve_version "$current" "$bump") || return
    if ((dry)); then printf 'would validate, bump to v%s, and commit tracked files\n' "$version"; return; fi
    env RUSTUP_TOOLCHAIN="$TOOLCHAIN" just ci || return
    python3 - "$version" <<'PY'
from pathlib import Path
import re, sys
p = Path("Cargo.toml"); text = p.read_text()
text, count = re.subn(r'(?m)^(\[package\]\n(?:.*\n)*?version = ")[^"]+("\n)', rf'\g<1>{sys.argv[1]}\2', text, count=1)
if count != 1: raise SystemExit("could not update Cargo.toml version")
p.write_text(text)
PY
    env RUSTUP_TOOLCHAIN="$TOOLCHAIN" cargo update -p herdr --offline || return
    git add -u && git add Cargo.toml Cargo.lock || return
    git diff --cached --quiet && die "nothing to commit"
    git commit -m "release: v$version"
}

action_push() {
    local dry=$1
    preflight push || return
    if ((dry)); then printf 'would push %s\n' "$BRANCH"; else git push origin "$BRANCH"; fi
}

action_release() {
    local bump=$1 dry=$2 current version tag cargo_version
    preflight release || return
    current=$(latest_tag); version=$(resolve_version "$current" "$bump") || return; tag="v$version"
    cargo_version=$(sed -n '/^\[package\]$/,/^\[/s/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1)
    [[ "$cargo_version" == "$version" ]] || die "Cargo.toml is $cargo_version; Commit v$version first"
    ! git rev-parse --verify --quiet "refs/tags/$tag" >/dev/null || die "$tag already exists"
    ! git ls-remote --exit-code --tags --refs origin "refs/tags/$tag" >/dev/null 2>&1 || die "$tag already exists on origin"
    if ((dry)); then printf 'would create and push %s\n' "$tag"; else git tag -a "$tag" -m "Moo $tag" && git push origin "refs/tags/$tag"; fi
}

run_internal() {
    local action=$1 bump=$2 dry=$3
    cd "$(repo_root)" || exit 1
    case "$action" in
        validate) action_validate ;; commit) action_commit "$bump" "$dry" ;;
        push) action_push "$dry" ;; release) action_release "$bump" "$dry" ;;
        *) die "unknown action: $action" ;;
    esac
}

screen_leave() { printf '\033[?25h\033[?1049l'; }
status_line() { printf '\033[%s;1H\033[2K  %b%s%b' "$STATUS_ROW" "${2:-$reset}" "$1" "$reset"; }

draw_screen() {
    local selected=$1 bump=$2 dry=$3 branch tracked untracked current next i marker
    branch=$(git branch --show-current 2>/dev/null || printf '?')
    tracked=$(git status --porcelain --untracked-files=no 2>/dev/null | wc -l)
    untracked=$(git ls-files --others --exclude-standard 2>/dev/null | wc -l)
    current=$(latest_tag); next=$(resolve_version "$current" "$bump" 2>/dev/null || printf '?')
    printf '\033[?2026h\033[H'
    printf '%b  ◆ MOO RELEASE%b\n' "$bold$cyan" "$reset"
    printf '  %b%s%b  ·  %s  ·  %s tracked  ·  %s untracked ignored\n\n' "$bold" "$REPO" "$reset" "$branch" "$tracked" "$untracked"
    printf '  Version  %b%s → v%s%b    bump: %b%s%b    dry run: %b%s%b\n\n' "$bold" "$current" "$next" "$reset" "$cyan" "$bump" "$reset" "$yellow" "$([[ "$dry" -eq 1 ]] && printf on || printf off)" "$reset"
    printf '  Release steps\n\n'
    local labels=("Validate" "Commit" "Push" "Tag & release" "Quit")
    local details=("run release checks" "commit tracked files + version" "push master" "publish immutable tag" "leave console")
    for i in "${!labels[@]}"; do
        marker=' '; [[ "$selected" -eq "$i" ]] && marker="${cyan}›${reset}"
        printf '  %b %-14s %s\n' "$marker" "${labels[$i]}" "${details[$i]}"
    done
    printf '\n  Activity\n  %s\n\n' "$activity"
    printf '  %b↑/↓%b move   %b←/→%b bump   %bd%b dry run   %benter%b select   %bq%b quit' "$bold" "$reset" "$bold" "$reset" "$bold" "$reset" "$bold" "$reset" "$bold" "$reset"
    printf '\033[J\033[?2026l'
}

run_with_progress() {
    local action=$1 bump=$2 dry=$3 tmp pid frame=0 started result message
    local spin=('⠋' '⠙' '⠹' '⠸' '⠼' '⠴' '⠦' '⠧' '⠇' '⠏')
    tmp=$(mktemp "${TMPDIR:-/tmp}/moo-release-XXXXXX")
    "$SELF" --internal "$action" "$bump" "$dry" >"$tmp" 2>&1 & pid=$!; started=$SECONDS
    while kill -0 "$pid" 2>/dev/null; do
        status_line "${spin[$frame]} ${action^}… $((SECONDS - started))s" "$cyan"
        frame=$(((frame + 1) % ${#spin[@]})); sleep 0.2
    done
    if wait "$pid"; then
        activity="✓ ${action^} completed in $((SECONDS - started))s"
        status_line "$activity" "$green"
    else
        result=$?; message=$(sed $'s/\033\[[0-9;]*m//g' "$tmp" | awk 'NF { line=$0 } END { print line }'); message=${message:-"exit $result"}
        activity="✗ ${action^}: ${message:0:100}"
        status_line "$activity" "$red"
    fi
    rm -f "$tmp"
}

confirm_action() {
    local key
    status_line "$1? y/N" "$yellow"; IFS= read -rsn1 key
    [[ "$key" == y || "$key" == Y ]]
}

run_console() {
    [[ -t 0 && -t 1 ]] || die "a terminal is required; pass a version for direct mode"
    local bumps=(patch minor major) bump_index=0 selected=0 dry=0 key rest
    cd "$(repo_root)" || exit 1
    printf '\033[?1049h\033[?25l'; trap screen_leave EXIT INT TERM
    while true; do
        draw_screen "$selected" "${bumps[$bump_index]}" "$dry"; IFS= read -rsn1 key
        if [[ "$key" == $'\033' ]]; then IFS= read -rsn2 -t 0.05 rest || true; key+="$rest"; fi
        case "$key" in
            $'\033[A'|k) selected=$(((selected + 4) % 5)) ;; $'\033[B'|j) selected=$(((selected + 1) % 5)) ;;
            $'\033[D'|h) bump_index=$(((bump_index + 2) % 3)) ;; $'\033[C'|l) bump_index=$(((bump_index + 1) % 3)) ;;
            d) dry=$((1 - dry)) ;; q|$'\033') break ;;
            '')
                case "$selected" in
                    0) run_with_progress validate "${bumps[$bump_index]}" "$dry" ;;
                    1) confirm_action Commit && run_with_progress commit "${bumps[$bump_index]}" "$dry" ;;
                    2) run_with_progress push "${bumps[$bump_index]}" "$dry" ;;
                    3) confirm_action Release && run_with_progress release "${bumps[$bump_index]}" "$dry" ;;
                    4) break ;;
                esac
                ;;
        esac
    done
}

run_direct() {
    local bump=$1 dry=$2 answer
    printf 'Release %s (%s)? [y/N] ' "$bump" "$([[ "$dry" -eq 1 ]] && printf 'dry run' || printf live)"; read -r answer
    [[ "$answer" =~ ^[Yy]$ ]] || exit 1
    run_internal validate "$bump" "$dry" && run_internal commit "$bump" "$dry" && run_internal push "$bump" "$dry" && run_internal release "$bump" "$dry"
}

dry=0
if [[ ${1:-} == --internal ]]; then [[ $# -eq 4 ]] || die "invalid internal invocation"; run_internal "$2" "$3" "$4"; exit; fi
if [[ ${1:-} == --dry-run ]]; then dry=1; shift; fi
case ${1:-} in
    '') run_console ;; -h|--help) usage ;;
    patch|minor|major|[0-9]*.[0-9]*.[0-9]*) run_direct "$1" "$dry" ;;
    *) usage >&2; exit 2 ;;
esac
