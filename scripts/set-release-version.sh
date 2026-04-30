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
# gets its version line rewritten. Tiny perl state machine so we only
# touch the line immediately after a matching `name = ...`.
if [[ -f Cargo.lock ]]; then
    V="$V" perl -i -pe '
        BEGIN { $hit = 0 }
        if (/^name = "protondrive-/) { $hit = 1 }
        elsif ($hit && /^version = "/) {
            s/"[^"]+"/"$ENV{V}"/;
            $hit = 0;
        }
    ' Cargo.lock
fi

echo "Workspace version is now:"
grep -m1 '^version' Cargo.toml
