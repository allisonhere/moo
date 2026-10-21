#!/usr/bin/env bash
set -uo pipefail

readonly REPO="allisonhere/moo"
readonly BRANCH="master"
readonly TOOLCHAIN="1.96.1"
readonly SELF="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/$(basename "${BASH_SOURCE[0]}")"

bold=$'\033[1m'; cyan=$'\033[36m'; green=$'\033[32m'
yellow=$'\033[33m'; red=$'\033[31m'; reset=$'\033[0m'

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
        validate|commit) git merge-base --is-ancestor "origin/$BRANCH" HEAD || die "$BRANCH is behind or has diverged from origin/$BRANCH; pull first" ;;
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

# ── release console ──────────────────────────────────────────────────────────
# One screen: a four-step pipeline whose state is read from git, a live output
# panel, and a single "run the next step" key.

readonly SPIN=('⠋' '⠙' '⠹' '⠸' '⠼' '⠴' '⠦' '⠧' '⠇' '⠏')
readonly BUMPS=(patch minor major)
readonly STEPS=(check commit push release)
readonly STEP_HINT=("run the release checks" "bump the version and commit" "push $BRANCH to origin" "tag and publish the release")
readonly LOG_ROWS=7

dim=$'\033[2m'
ui_bump=0; ui_dry=0; ui_check=pending; ui_log=(); ui_note=""; ui_note_color=$dim; ui_spin=""

screen_leave() { printf '\033[?25h\033[?1049l'; }

cargo_version() { sed -n '/^\[package\]$/,/^\[/s/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1; }

# Version this run will publish: a version already committed past the latest tag
# wins, otherwise the chosen bump of the latest tag.
target_version() {
    local latest committed
    latest=$(latest_tag); committed=$(cargo_version)
    if [[ $(printf '%s\n%s\n' "${latest#v}" "$committed" | sort -V | tail -1) == "$committed" && "$committed" != "${latest#v}" ]]; then
        printf '%s\n' "$committed"
    else
        resolve_version "$latest" "${BUMPS[$ui_bump]}" 2>/dev/null || printf '?'
    fi
}

step_state() {
    local step=$1 target=$2
    case "$step" in
        check) printf '%s' "$ui_check" ;;
        commit) [[ $(cargo_version) == "$target" ]] && printf ok || printf pending ;;
        push)
            [[ $(cargo_version) == "$target" && $(git rev-parse HEAD) == $(git rev-parse "origin/$BRANCH" 2>/dev/null) ]] && printf ok || printf pending ;;
        release) git rev-parse --verify --quiet "refs/tags/v$target" >/dev/null && printf ok || printf pending ;;
    esac
}

# First unfinished step of commit → push → release; empty when all are done.
next_step() {
    local step
    for step in commit push release; do
        [[ $(step_state "$step" "$1") == ok ]] || { printf '%s' "$step"; return; }
    done
}

strip_ansi() { sed $'s/\033\\[[0-9;?]*[a-zA-Z]//g; s/\r//g'; }

load_log() {
    local width=$(($1 - 4)) line
    ui_log=()
    while IFS= read -r line; do ui_log+=("${line:0:$width}"); done < <(strip_ansi <"$2" | awk 'NF' | tail -n "$LOG_ROWS")
}

draw() {
    local cols width rule target current branch changed ignored step state glyph color i b next
    cols=$(tput cols 2>/dev/null || printf 80); width=$((cols < 72 ? cols - 2 : 70)); ((width < 40)) && width=40
    printf -v rule '%*s' "$((width - 2))" ''; rule=${rule// /─}
    current=$(latest_tag); target=$(target_version); next=$(next_step "$target")
    branch=$(git branch --show-current 2>/dev/null || printf '?')
    changed=$(git status --porcelain --untracked-files=no 2>/dev/null | wc -l)
    ignored=$(git ls-files --others --exclude-standard 2>/dev/null | wc -l)

    printf '\033[?2026h\033[H'
    printf '\n  %b◆ MOO RELEASE%b' "$bold$cyan" "$reset"
    ((ui_dry)) && printf '   %b dry run %b' $'\033[30;43m' "$reset"
    printf '\033[K\n  %b%s · %s · %s changed, %s untracked ignored%b\033[K\n\n' "$dim" "$REPO" "$branch" "$changed" "$ignored" "$reset"

    printf '  %b%s%b  →  %b%sv%s%b     ' "$dim" "$current" "$reset" "$bold$green" "" "$target" "$reset"
    for i in "${!BUMPS[@]}"; do
        b=${BUMPS[$i]}
        if ((i == ui_bump)); then printf '%b %s %b ' $'\033[7m' "$b" "$reset"; else printf '%b%s%b  ' "$dim" "$b" "$reset"; fi
    done
    printf '\033[K\n\n'

    for i in "${!STEPS[@]}"; do
        step=${STEPS[$i]}; state=$(step_state "$step" "$target")
        case "$state" in
            ok) glyph='✓'; color=$green ;;
            fail) glyph='✗'; color=$red ;;
            *) glyph='○'; color=$dim ;;
        esac
        [[ "$step" == "$next" && "$state" != fail ]] && { glyph='›'; color=$cyan; }
        printf '  %b%s%b %s  %b%-8s%b %b%s%b\033[K\n' "$color" "$glyph" "$reset" "$((i + 1))" "$bold" "${step^}" "$reset" "$dim" "${STEP_HINT[$i]}" "$reset"
    done

    printf '\n  %b%s%b\033[K\n' "$dim" "$rule" "$reset"
    for ((i = 0; i < LOG_ROWS; i++)); do printf '  %b%s%b\033[K\n' "$dim" "${ui_log[$i]:-}" "$reset"; done
    printf '  %b%s%b\033[K\n\n' "$dim" "$rule" "$reset"

    printf '  %b%s%s%b\033[K\n\n' "$ui_note_color" "$ui_spin" "$ui_note" "$reset"
    printf '  %benter%b %s   %ba%b all   %bc%b check   %b←/→%b bump   %bd%b dry run   %bq%b quit\033[K' \
        "$bold" "$reset" "${next:-done}" "$bold" "$reset" "$bold" "$reset" "$bold" "$reset" "$bold" "$reset" "$bold" "$reset"
    printf '\033[J\033[?2026l'
}

note() { ui_note=$1; ui_note_color=${2:-$dim}; ui_spin=""; }

# Runs one action in a child process while animating the screen and tailing its output.
run_step() {
    local action=$1 tmp pid frame=0 started rc width
    width=$(tput cols 2>/dev/null || printf 80)
    tmp=$(mktemp "${TMPDIR:-/tmp}/moo-release-XXXXXX"); started=$SECONDS
    "$SELF" --internal "$action" "${BUMPS[$ui_bump]}" "$ui_dry" >"$tmp" 2>&1 & pid=$!
    while kill -0 "$pid" 2>/dev/null; do
        load_log "$width" "$tmp"
        ui_spin="${SPIN[$frame]} "; ui_note="${action^} · $((SECONDS - started))s"; ui_note_color=$cyan
        draw; frame=$(((frame + 1) % ${#SPIN[@]})); sleep 0.15
    done
    wait "$pid"; rc=$?
    load_log "$width" "$tmp"; rm -f "$tmp"
    [[ "$action" == check ]] && { ((rc == 0)) && ui_check=ok || ui_check=fail; }
    if ((rc == 0)); then note "✓ ${action^} finished in $((SECONDS - started))s" "$green"
    else note "✗ ${action^} failed (exit $rc), see output above" "$red"; fi
    return "$rc"
}

# Returns 0 on enter, 1 on anything else.
confirm() {
    local key
    note "$1 — enter to confirm, any other key cancels" "$yellow"; draw
    IFS= read -rsn1 key
    if [[ -z "$key" ]]; then return 0; fi
    note "cancelled" "$dim"; return 1
}

run_pipeline() {
    local target step
    target=$(target_version)
    confirm "Run every remaining step for v$target$( ((ui_dry)) && printf ' (dry run)')" || return
    for step in commit push release; do
        if ((ui_dry == 0)) && [[ $(step_state "$step" "$target") == ok ]]; then continue; fi
        run_step "$step" || return
    done
}

run_next() {
    local target step
    target=$(target_version); step=$(next_step "$target")
    [[ -n "$step" ]] || { note "v$target is already released — change the bump to start another" "$yellow"; return; }
    case "$step" in
        commit|release) confirm "${step^} v$target$( ((ui_dry)) && printf ' (dry run)')" || return ;;
    esac
    run_step "$step"
}

run_console() {
    [[ -t 0 && -t 1 ]] || die "a terminal is required; pass a version for direct mode"
    local key rest
    cd "$(repo_root)" || exit 1
    fetch_release_refs 2>/dev/null
    printf '\033[?1049h\033[?25l'; trap screen_leave EXIT INT TERM
    note "Ready." "$dim"
    while true; do
        draw; IFS= read -rsn1 key
        if [[ "$key" == $'\033' ]]; then IFS= read -rsn2 -t 0.05 rest || true; key+="$rest"; fi
        case "$key" in
            $'\033[D'|h) ui_bump=$(((ui_bump + 2) % 3)) ;;
            $'\033[C'|l) ui_bump=$(((ui_bump + 1) % 3)) ;;
            d) ui_dry=$((1 - ui_dry)) ;;
            c) run_step check ;;
            a) run_pipeline ;;
            '') run_next ;;
            q|$'\033') break ;;
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
