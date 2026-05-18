#!/usr/bin/env bash
set -euo pipefail
IFS=$'\n\t'

readonly SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
readonly SOURCE_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

usage() {
    cat <<'USAGE'
Usage:
  scripts/sync-downstream-subtree.sh --downstream PATH --prefix PATH [options]

Options:
  --downstream PATH  Downstream git repository checkout.
  --prefix PATH      Path inside downstream repo that owns the Rusty Red subtree.
  --remote URL       Upstream Rusty Red git remote. Defaults to this repo's origin.
  --ref REF          Upstream ref to sync. Defaults to main.
  --mode MODE        pull or add. Defaults to pull.
  -h, --help         Show this help.

Examples:
  scripts/sync-downstream-subtree.sh --downstream ../product --prefix vendor/rusty-red
  scripts/sync-downstream-subtree.sh --downstream ../product --prefix vendor/rusty-red --mode add
USAGE
}

die() {
    echo "error: $*" >&2
    exit 1
}

require_command() {
    local command_name=$1
    if ! command -v "$command_name" >/dev/null 2>&1; then
        die "$command_name is required"
    fi
}

default_remote() {
    git -C "$SOURCE_ROOT" config --get remote.origin.url 2>/dev/null \
        || echo "https://github.com/Travis-Gilbert/RustyRed-Graph-Database.git"
}

downstream_path=""
prefix=""
remote="$(default_remote)"
ref="main"
mode="pull"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --downstream)
            downstream_path=${2:-}
            shift 2
            ;;
        --prefix)
            prefix=${2:-}
            shift 2
            ;;
        --remote)
            remote=${2:-}
            shift 2
            ;;
        --ref)
            ref=${2:-}
            shift 2
            ;;
        --mode)
            mode=${2:-}
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            die "unknown argument: $1"
            ;;
    esac
done

require_command git

[[ -n "$downstream_path" ]] || die "--downstream is required"
[[ -n "$prefix" ]] || die "--prefix is required"
[[ -n "$remote" ]] || die "--remote must not be empty"
[[ -n "$ref" ]] || die "--ref must not be empty"
[[ "$mode" == "pull" || "$mode" == "add" ]] || die "--mode must be pull or add"

[[ -d "$downstream_path" ]] || die "downstream path does not exist: $downstream_path"
git -C "$downstream_path" rev-parse --is-inside-work-tree >/dev/null 2>&1 \
    || die "downstream path is not a git repository: $downstream_path"

subtree_help="$(git subtree -h 2>&1 || true)"
if [[ "$subtree_help" != *"usage: git subtree"* ]]; then
    die "git subtree is required"
fi

if [[ -n "$(git -C "$downstream_path" status --porcelain)" ]]; then
    die "downstream worktree must be clean before subtree sync"
fi

case "$mode" in
    add)
        if [[ -e "$downstream_path/$prefix" ]]; then
            die "cannot add subtree because prefix already exists: $prefix"
        fi
        git -C "$downstream_path" subtree add \
            --prefix="$prefix" \
            "$remote" \
            "$ref" \
            --squash \
            -m "chore(rusty-red): import upstream $ref"
        ;;
    pull)
        if [[ ! -e "$downstream_path/$prefix" ]]; then
            die "cannot pull subtree because prefix does not exist: $prefix"
        fi
        git -C "$downstream_path" subtree pull \
            --prefix="$prefix" \
            "$remote" \
            "$ref" \
            --squash \
            -m "chore(rusty-red): sync upstream $ref"
        ;;
esac
