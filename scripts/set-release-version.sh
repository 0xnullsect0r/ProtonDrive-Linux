#!/usr/bin/env bash
# Rewrite the workspace Cargo.toml version AND every workspace-member
# entry in Cargo.lock so `cargo build --locked` keeps working after the
# bump. Idempotent.
#
# Usage: scripts/set-release-version.sh <X.Y.Z>
set -euo pipefail

V="${1:?usage: set-release-version.sh <X.Y.Z>}"
if [[ ! "$V" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "ERROR: '$V' is not in strict X.Y.Z form" >&2
    exit 1
fi

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

# Cargo.toml: only the FIRST `version = "..."` line (the [workspace.package]
# version). All crate manifests use `version.workspace = true`.
sed -i -E "0,/^version[[:space:]]*=.*/s//version = \"${V}\"/" Cargo.toml

# Cargo.lock: every [[package]] entry whose name starts with `protondrive-`
# gets its version line rewritten. Use awk (always present) so this
# works inside minimal containers that don't ship perl.
if [[ -f Cargo.lock ]]; then
    awk -v ver="$V" '
        /^name = "protondrive-/ { hit = 1; print; next }
        hit && /^version = "/    { sub(/"[^"]+"/, "\"" ver "\""); hit = 0 }
        { print }
    ' Cargo.lock > Cargo.lock.tmp && mv Cargo.lock.tmp Cargo.lock
fi

echo "Workspace version is now:"
grep -m1 '^version' Cargo.toml
