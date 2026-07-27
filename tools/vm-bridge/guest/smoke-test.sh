#!/usr/bin/env bash
# Session Signals — end-to-end smoke test from inside the VM/WSL.
# Drives a throwaway session through every colour so you can confirm the whole
# path (mirrored loopback or bridge) before installing real hooks.
#
#   TOKEN=<host-token> ./smoke-test.sh
#
# Posts to 127.0.0.1:<PORT> — the same URL real hooks use — so a green run here
# means real sessions will report too.
set -euo pipefail

PORT="${PORT:-4317}"
TOKEN="${TOKEN:?set TOKEN=<host X-Beacon-Token>}"
GAP="${GAP:-3}"
ENDPOINT="http://127.0.0.1:${PORT}/hook"
SID="$(cat /proc/sys/kernel/random/uuid 2>/dev/null || uuidgen)"
# Friendly, non-hex basename so the engine's ignore rules don't hide the row.
CWD="/home/$(whoami)/vm-smoke-test"
BASE="\"session_id\":\"${SID}\",\"cwd\":\"${CWD}\",\"transcript_path\":\"/tmp/${SID}.jsonl\""

post() {
  local code
  code="$(curl -s -o /dev/null -w '%{http_code}' -m 3 -X POST "$ENDPOINT" \
    -H 'Content-Type: application/json' -H "X-Beacon-Token: ${TOKEN}" --data "$2" || echo 000)"
  printf '  %-24s -> HTTP %s\n' "$1" "$code"
  [[ "$code" == 200 ]] || echo "     (non-200: check token / mode / bridge)" >&2
}

echo "Smoke test ${SID:0:8} -> ${ENDPOINT} (watch the Windows tray)"
post "SessionStart (GREEN)"        "{\"hook_event_name\":\"SessionStart\",${BASE}}";        sleep "$GAP"
post "UserPromptSubmit (ORANGE)"   "{\"hook_event_name\":\"UserPromptSubmit\",${BASE}}";    sleep "$GAP"
post "Notification/perm (RED)"     "{\"hook_event_name\":\"Notification\",\"notification_type\":\"permission_prompt\",${BASE}}"; sleep "$GAP"
post "Stop (GREEN)"                "{\"hook_event_name\":\"Stop\",${BASE}}";                 sleep "$GAP"
post "SessionEnd (removed)"        "{\"hook_event_name\":\"SessionEnd\",${BASE}}"
echo "If every line is HTTP 200 and you saw a 'vm-smoke-test' row flip colours, you're set."
