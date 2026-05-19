#!/bin/sh
# PostToolUse hook: warns when a new Rust source module is created without a spec.
# Receives Claude's tool-use JSON on stdin.

FILE=$(python3 -c "
import json, sys
d = json.load(sys.stdin)
print(d.get('tool_input', {}).get('file_path', ''))
" 2>/dev/null)

# Only check new Rust source files inside crates/
case "$FILE" in
  */crates/*/src/*.rs) ;;
  *) exit 0 ;;
esac

# Derive module name from filename
MODULE=$(basename "$FILE" .rs)

# Skip boilerplate files that never need specs
case "$MODULE" in
  main|lib|mod|error|errors|types|utils|config|prelude) exit 0 ;;
esac

# Look for a spec that mentions this module
REPO_ROOT=$(git rev-parse --show-toplevel 2>/dev/null || pwd)
if grep -rl --include="*.md" "$MODULE" "$REPO_ROOT/design/" >/dev/null 2>&1; then
  exit 0
fi

printf "\n  \033[33m!\033[0m  spec check: no design doc references \033[1m%s\033[0m\n" "$MODULE"
printf "     create one:  bash scripts/new-spec.sh %s\n" "$MODULE"
printf "     or hybrid:   bash scripts/new-spec.sh %s Hybrid\n\n" "$MODULE"
