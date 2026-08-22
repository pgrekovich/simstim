#!/usr/bin/env bash
# Builds release notes for a tag out of the commit subjects since the previous
# tag, grouped by conventional-commit type.
#
# Run it locally to see exactly what a release will say before cutting it:
#   scripts/release-notes.sh v0.1.0
#
# Requires the full history and all tags, so CI must check out with
# fetch-depth: 0.
set -euo pipefail

TAG="${1:?usage: release-notes.sh <tag>}"

# The tag being released may or may not exist yet locally; fall back to HEAD so
# the script can preview notes before the tag is created.
if git rev-parse -q --verify "$TAG" >/dev/null; then
    END="$TAG"
else
    END="HEAD"
fi

PREV="$(git describe --tags --abbrev=0 "${END}^" 2>/dev/null || true)"
RANGE="${PREV:+${PREV}..}${END}"

subjects() {
    git log --no-merges --format='%s' "$RANGE"
}

section() {
    local type="$1" heading="$2" body
    body="$(subjects | sed -n "s/^${type}\(([^)]*)\)\{0,1\}: //p" | sed 's/^/- /' || true)"
    if [ -n "$body" ]; then
        printf '### %s\n\n%s\n\n' "$heading" "$body"
    fi
}

section feat "Features"
section fix "Fixes"
section docs "Documentation"

other="$(subjects | grep -vE '^(feat|fix|docs)(\([^)]*\))?: ' | sed 's/^/- /' || true)"
if [ -n "$other" ]; then
    printf '### Other\n\n%s\n\n' "$other"
fi

if [ -n "$PREV" ]; then
    printf '**Changes since %s.**\n' "$PREV"
else
    printf '**First release.**\n'
fi
