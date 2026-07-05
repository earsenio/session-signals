#!/usr/bin/env bash
# Surface 9 (subagent activity) UI test driver.
#
# Drives Session Signals' local hook listener directly so you can watch the widget react
# without needing real Claude Code subagents. Open and EXPAND the Session Signals widget
# first — the sub-line only renders in expanded rows, not the compact pill.
#
# Usage:  bash scripts/test-subagents.sh [port]   (default port 4317)
set -euo pipefail
PORT="${1:-4317}"
URL="http://127.0.0.1:${PORT}/hook"
SID="beacon-demo-fanout"
CWD="/tmp/beacon-demo/my-project"   # not a git repo → row label is "my-project"

hook() { # hook <EventName> [notification_type]
  local body="{\"hook_event_name\":\"$1\",\"session_id\":\"$SID\",\"cwd\":\"$CWD\""
  [ -n "${2:-}" ] && body="$body,\"notification_type\":\"$2\""
  curl -s -m 2 -X POST "$URL" -H 'Content-Type: application/json' -d "$body}" >/dev/null
}
state() { curl -s "http://127.0.0.1:${PORT}/state" \
  | python3 -c "import sys,json;[print(f\"   {s['label']:<14} {s['state']:<10} agents={s['subagent_count']} t={s['subagent_seconds']}s\") for s in json.load(sys.stdin)['sessions'] if s['session_id']=='$SID']"; }
pause() { echo "   ⏸  $1"; sleep "${2:-3}"; }

if ! curl -s -m 2 "http://127.0.0.1:${PORT}/state" >/dev/null; then
  echo "No Session Signals listener on 127.0.0.1:${PORT}. Start Session Signals (npm run tauri dev) first." >&2
  exit 1
fi

echo "▶ Scenario A — fan out 0→3 then drain to 0 (watch plural/singular, timer, cleanup)"
hook SessionStart; hook UserPromptSubmit
pause "row should be ORANGE 'Working', NO sub-line yet" 2; state
hook SubagentStart
pause "sub-line appears: pulsing dot · '1 agent' (singular) · timer + amber halo" 4; state
hook SubagentStart
pause "'2 agents running'; timer keeps climbing from the FIRST start" 4; state
hook SubagentStart
pause "'3 agents running'" 3; state
hook SubagentStop; hook SubagentStop
pause "back to '1 agent' — timer did NOT reset" 3; state
hook SubagentStop
pause "count 0 → sub-line AND halo vanish, row reflows with no leftover gap" 3; state

echo "▶ Scenario B — red 'Needs you' row WITH 2 agents (they must coexist)"
hook SubagentStart; hook SubagentStart
hook Notification permission_prompt
pause "row glyph RED + amber halo behind it + amber '2 agents running' sub-line" 5; state
hook SubagentStop; hook SubagentStop
pause "sub-line gone; row stays red" 2; state

echo "▶ Scenario C — clamp + cleanup"
hook SubagentStop; hook SubagentStop   # extra stops must NOT go negative
pause "still agents=0 (clamped, no underflow)" 2; state
hook SessionEnd
echo "   ✓ demo session removed"
echo "Done. (Concurrent-session isolation is covered by the Rust unit tests.)"
