#!/usr/bin/env bash
# PostToolUse hook: format the file Claude just edited and refresh the code map.
# Silent on success (zero tokens). Problems go to .claude/format.log.
f=$(jq -r '.tool_input.file_path // empty')
if [ -z "$f" ] || [ ! -f "$f" ]; then exit 0; fi
root=${CLAUDE_PROJECT_DIR:-.}
have() { command -v "$1" >/dev/null || { echo "$1 not installed"; return 1; }; }
edition() { rg -m1 -o --no-filename 'edition\s*=\s*"(\d+)"' -r '$1' "$root/Cargo.toml" 2>/dev/null || echo 2021; }
out=$(
case "$f" in
*.rs) have rustfmt && rustfmt --edition "$(edition)" "$f" ;;
esac 2>&1
) || printf '%s %s\n%s\n' "$(date '+%F %T')" "$f" "$out" >"$root/.claude/format.log"
case "$f" in
  *.rs|*.py|*.ts|*.tsx|*.js|*.jsx|*.mjs|*.go|*.java|*.kt|*.kts|*.cs|*.c|*.h|*.cc|*.cpp|*.hpp|*.dart|*.sh)
    "$HOME/.claude/bin/codemap.sh" --file "$f" "$root" >/dev/null 2>&1 ;;
esac
exit 0
