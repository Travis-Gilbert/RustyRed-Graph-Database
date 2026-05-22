#!/usr/bin/env bash
#
# Sync the in-tree vendored proto from the theorem-protos submodule.
#
# The vendored copy at vendor/proto/rustyred/v1/rustyred.proto is what
# crates/rustyred-server/build.rs compiles in Docker / Railway builds,
# where the submodule is not initialized. This script keeps the vendored
# copy byte-identical to the submodule HEAD and records the source
# commit in vendor/proto/SOURCE_COMMIT.
#
# Usage:
#   scripts/sync-vendored-proto.sh           # sync; fail if submodule missing
#   scripts/sync-vendored-proto.sh --check   # fail with diff if out of sync
#
# Exit codes:
#   0 - in sync (after sync, or already in sync under --check)
#   1 - submodule not initialized or other prerequisite failure
#   2 - --check mode and vendored is out of sync with submodule

set -euo pipefail
IFS=$'\n\t'

readonly SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
readonly REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
readonly SUBMODULE_PROTO="$REPO_ROOT/proto/rustyred/v1/rustyred.proto"
readonly VENDORED_PROTO="$REPO_ROOT/vendor/proto/rustyred/v1/rustyred.proto"
readonly SOURCE_COMMIT_FILE="$REPO_ROOT/vendor/proto/SOURCE_COMMIT"

mode="sync"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --check)
            mode="check"
            shift
            ;;
        -h|--help)
            sed -n '2,18p' "$0"
            exit 0
            ;;
        *)
            echo "error: unknown argument: $1" >&2
            exit 1
            ;;
    esac
done

if [[ ! -f "$SUBMODULE_PROTO" ]]; then
    echo "error: $SUBMODULE_PROTO not found" >&2
    echo "       run: git submodule update --init" >&2
    exit 1
fi

submodule_commit="$(git -C "$REPO_ROOT/proto" rev-parse HEAD)"

if [[ "$mode" == "check" ]]; then
    if [[ ! -f "$VENDORED_PROTO" ]]; then
        echo "error: $VENDORED_PROTO is missing" >&2
        echo "       run: scripts/sync-vendored-proto.sh" >&2
        exit 2
    fi
    if ! diff -q "$SUBMODULE_PROTO" "$VENDORED_PROTO" >/dev/null; then
        echo "error: vendored proto is out of sync with submodule" >&2
        echo "       submodule HEAD: $submodule_commit" >&2
        echo "       diff:" >&2
        diff "$SUBMODULE_PROTO" "$VENDORED_PROTO" >&2 || true
        echo "       fix: scripts/sync-vendored-proto.sh" >&2
        exit 2
    fi
    if [[ ! -f "$SOURCE_COMMIT_FILE" ]]; then
        echo "error: $SOURCE_COMMIT_FILE is missing" >&2
        echo "       the provenance anchor must be present alongside the vendored proto" >&2
        echo "       fix: scripts/sync-vendored-proto.sh" >&2
        exit 2
    fi
    recorded_commit="$(head -n1 "$SOURCE_COMMIT_FILE")"
    if [[ "$recorded_commit" != "$submodule_commit" ]]; then
        echo "error: vendor/proto/SOURCE_COMMIT does not match submodule HEAD" >&2
        echo "       recorded:  $recorded_commit" >&2
        echo "       submodule: $submodule_commit" >&2
        echo "       fix: scripts/sync-vendored-proto.sh" >&2
        exit 2
    fi
    echo "ok: vendored proto matches submodule HEAD ($submodule_commit)"
    exit 0
fi

mkdir -p "$(dirname "$VENDORED_PROTO")"
cp "$SUBMODULE_PROTO" "$VENDORED_PROTO"

cat > "$SOURCE_COMMIT_FILE" <<EOF
$submodule_commit

Source: https://github.com/Travis-Gilbert/theorem-protos
File:   proto/rustyred/v1/rustyred.proto
EOF

echo "synced: $VENDORED_PROTO"
echo "        from submodule HEAD $submodule_commit"
