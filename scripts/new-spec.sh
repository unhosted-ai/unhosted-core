#!/bin/bash
# Create the next numbered design doc from the template.
#
# Usage:
#   bash scripts/new-spec.sh <slug>
#   bash scripts/new-spec.sh memory-compaction
#
# Status options (set as second arg, default: Draft):
#   Draft    — spec only, no code yet
#   Hybrid   — spec and code landing together
#   Accepted — reviewed and merged

set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

SLUG="${1:-}"
STATUS="${2:-Draft}"

if [ -z "$SLUG" ]; then
  echo "usage: bash scripts/new-spec.sh <slug> [status]"
  echo ""
  echo "  bash scripts/new-spec.sh vram-pooling"
  echo "  bash scripts/new-spec.sh auth-refresh Hybrid"
  exit 1
fi

# Validate slug (lowercase, hyphens only)
if ! echo "$SLUG" | grep -qE '^[a-z0-9][a-z0-9-]*$'; then
  echo "error: slug must be lowercase letters, numbers, and hyphens only"
  exit 1
fi

[ -f "design/TEMPLATE.md" ] || { echo "error: design/TEMPLATE.md not found"; exit 1; }

# Find the next available number
LAST=$(ls design/[0-9]*.md 2>/dev/null \
  | grep -oE '[0-9]+' | sort -n | tail -1 || echo "0")
NEXT=$(printf "%04d" $((10#$LAST + 1)))

FILE="design/$NEXT-$SLUG.md"
DATE=$(date +%Y-%m-%d)

if [ -f "$FILE" ]; then
  echo "error: $FILE already exists"
  exit 1
fi

sed "s/{{NUMBER}}/$NEXT/g; s/{{SLUG}}/$SLUG/g; s/{{DATE}}/$DATE/g; s/Status: Draft/Status: $STATUS/" \
  design/TEMPLATE.md > "$FILE"

echo "created: $FILE"
echo ""
echo "next:"
echo "  1. fill in the spec"
echo "  2. add an entry to design/README.md"
echo "  3. set Status: Accepted when reviewed"
