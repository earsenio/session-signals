#!/usr/bin/env bash
# Session Signals — remove our hooks from a Linux VM/WSL ~/.claude/settings.json.
# Non-destructive: strips only Session Signals' http hooks, leaves everything else,
# and drops event keys that become empty.
set -euo pipefail

SETTINGS="${CLAUDE_SETTINGS:-$HOME/.claude/settings.json}"
[[ -f "$SETTINGS" ]] || { echo "No $SETTINGS — nothing to do."; exit 0; }
command -v python3 >/dev/null 2>&1 || { echo "python3 required." >&2; exit 1; }
cp "$SETTINGS" "$SETTINGS.bak.$(date +%s)"

SETTINGS="$SETTINGS" python3 - <<'PY'
import json, os, sys
settings = os.environ["SETTINGS"]
HEADER = "X-Beacon-Token"
with open(settings) as f:
    data = json.load(f)

def is_ours(h):
    return (isinstance(h, dict) and h.get("type") == "http"
            and str(h.get("url", "")).endswith("/hook")
            and isinstance(h.get("headers"), dict) and HEADER in h["headers"])

hooks = data.get("hooks", {})
for ev in list(hooks.keys()):
    groups = hooks[ev]
    if not isinstance(groups, list):
        continue
    kept = []
    for g in groups:
        if isinstance(g, dict) and isinstance(g.get("hooks"), list):
            g = dict(g); g["hooks"] = [h for h in g["hooks"] if not is_ours(h)]
            if not g["hooks"] and set(g.keys()) <= {"matcher", "hooks"}:
                continue
        kept.append(g)
    if kept:
        hooks[ev] = kept
    else:
        del hooks[ev]
if not hooks:
    data.pop("hooks", None)

with open(settings, "w") as f:
    json.dump(data, f, indent=2); f.write("\n")
print(f"Removed Session Signals hooks from {settings}")
PY
