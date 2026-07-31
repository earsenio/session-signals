#!/usr/bin/env bash
# Session Signals — install hooks into a Linux VM/WSL Claude Code (guest side).
#
# Merges the same HTTP hook block Session Signals installs on the host into this
# machine's ~/.claude/settings.json, so real Claude Code sessions here report
# their traffic-light state to the Windows host app. NON-DESTRUCTIVE: your other
# hooks are preserved; re-running is idempotent (our entries are replaced, not
# duplicated), which also refreshes a rotated token.
#
# Every event posts to http://127.0.0.1:<port>/hook — identical to the host's
# own hooks. That URL means:
#   * mirrored networking  -> 127.0.0.1 IS the host loopback (shared). Direct.
#   * NAT + bridge         -> the guest forwarder (forwarder.sh) intercepts
#                             127.0.0.1:<port> and relays it to the host.
# So this installer is the same for both modes; only the transport differs.
#
# The Windows-only SessionStart terminal-capture command hook is intentionally
# omitted — click-to-focus can't reach a terminal inside the VM.
#
# Usage:
#   TOKEN=<host X-Beacon-Token> ./install-hooks.sh
#   TOKEN=<...> PORT=4317 ./install-hooks.sh     # PORT must match the host app
#
# Find the token on the host in %APPDATA%\com.beacon.cc\beacon.json -> auth_token.
set -euo pipefail

PORT="${PORT:-4317}"
TOKEN="${TOKEN:?set TOKEN=<host X-Beacon-Token> (from beacon.json on the host)}"
SETTINGS="${CLAUDE_SETTINGS:-$HOME/.claude/settings.json}"
URL="http://127.0.0.1:${PORT}/hook"

# The exact event set Session Signals wires (see src-tauri/src/hooks.rs EVENTS).
EVENTS=(
  SessionStart SessionEnd
  UserPromptSubmit UserPromptExpansion PreToolUse SubagentStart PreCompact
  PostToolUse PostToolUseFailure PostToolBatch
  Stop StopFailure SubagentStop PostCompact
  Notification
)

command -v python3 >/dev/null 2>&1 || {
  echo "python3 is required (used for a safe JSON merge)." >&2
  echo "Install it, or paste the block from 'print-block.sh' into $SETTINGS manually." >&2
  exit 1
}

mkdir -p "$(dirname "$SETTINGS")"
[[ -f "$SETTINGS" ]] && cp "$SETTINGS" "$SETTINGS.bak.$(date +%s)"

URL="$URL" TOKEN="$TOKEN" EVENTS="${EVENTS[*]}" SETTINGS="$SETTINGS" python3 - <<'PY'
import json, os, sys

settings = os.environ["SETTINGS"]
url      = os.environ["URL"]
token    = os.environ["TOKEN"]
events   = os.environ["EVENTS"].split()
HEADER   = "X-Beacon-Token"

try:
    with open(settings) as f:
        data = json.load(f)
    if not isinstance(data, dict):
        raise ValueError("settings.json is not a JSON object")
except FileNotFoundError:
    data = {}
except (ValueError, json.JSONDecodeError) as e:
    sys.exit(f"refusing to touch malformed {settings}: {e}")

def is_ours(hook):
    # Our shape: an http hook to a /hook endpoint carrying our token header.
    return (
        isinstance(hook, dict)
        and hook.get("type") == "http"
        and str(hook.get("url", "")).endswith("/hook")
        and isinstance(hook.get("headers"), dict)
        and HEADER in hook["headers"]
    )

our_group = {
    "matcher": "",
    "hooks": [{"type": "http", "url": url, "headers": {HEADER: token}, "timeout": 10}],
}

hooks = data.setdefault("hooks", {})
if not isinstance(hooks, dict):
    sys.exit('refusing to run: "hooks" in settings.json is not an object')

for ev in events:
    groups = hooks.get(ev, [])
    if not isinstance(groups, list):
        groups = []
    # Drop any prior Session Signals entries (idempotent + token refresh), keep the rest.
    cleaned = []
    for g in groups:
        if isinstance(g, dict) and isinstance(g.get("hooks"), list):
            g = dict(g)
            g["hooks"] = [h for h in g["hooks"] if not is_ours(h)]
            if not g["hooks"] and set(g.keys()) <= {"matcher", "hooks"}:
                continue  # group was purely ours
        cleaned.append(g)
    cleaned.append(json.loads(json.dumps(our_group)))
    hooks[ev] = cleaned

with open(settings, "w") as f:
    json.dump(data, f, indent=2)
    f.write("\n")

print(f"Installed {len(events)} hook events -> {url}")
print(f"Wrote {settings}")
PY

echo "Done. Start a Claude Code session here; it should appear on the Windows traffic light."
