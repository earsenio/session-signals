# VM bridge — Linux VM / WSL sessions on the Windows traffic light

Surface Claude Code sessions running **inside Linux VMs / WSL** on the same
machine in the host's Session Signals tray and widget. Detection is unchanged —
it's the app's normal HTTP hooks; this only routes those hooks from the guest to
the host's loopback listener, and tags such rows as `VM` in the UI.

Two transports, same guest hooks. **Pick one:**

| Aspect | Mirrored (recommended for WSL2) | Bridge (full VMs, or NAT WSL) |
| --- | --- | --- |
| How the guest reaches the host | WSL shares the host's `127.0.0.1` | portproxy on a VM-facing NIC → host `127.0.0.1` |
| Host setup | none | `host/bridge-install.ps1` (self-healing) |
| Guest forwarder | none | `guest/forwarder.sh` (+ systemd unit) |
| Guest hooks | `guest/install-hooks.sh` | `guest/install-hooks.sh` (identical) |
| Network exposure | pure loopback | bridge port, private-subnet + token scoped |

In both cases the app's listener still sees a **loopback** peer (its non-loopback
`403` is untouched) and the **`X-Beacon-Token`** still gates every event.

```text
guest Claude Code ──▶ http://127.0.0.1:4317/hook (+X-Beacon-Token)
   mirrored: 127.0.0.1 == host loopback (shared)         ─┐
   bridge:   forwarder.sh ─▶ <gateway>:4318 ─▶ portproxy ─┴─▶ host 127.0.0.1:4317 (app)
```

---

## Prerequisites (host)

- Session Signals running. Its port is `config.port` (default **4317**) in
  `%APPDATA%\com.beacon.cc\beacon.json`.
- Your **token**: same file, `auth_token`. Every guest needs it.

---

## Path A — Mirrored networking (WSL2, Windows 11 22H2+) — recommended

No host scripts, no forwarder, no moving IPs. `127.0.0.1` is shared with the host.

**1. Host — `%USERPROFILE%\.wslconfig`** (create/merge), then `wsl --shutdown`:

```ini
[wsl2]
networkingMode=mirrored

[experimental]
hostAddressLoopback=true
```

`hostAddressLoopback=true` is what lets the guest reach host services on
`127.0.0.1`. `wsl --shutdown` (run from Windows) applies it.

**2. Guest — install hooks** (in each WSL distro):

```bash
cd tools/vm-bridge/guest
TOKEN=<auth_token from beacon.json> ./install-hooks.sh
# optional first: TOKEN=<...> ./smoke-test.sh   # drives all colours end-to-end
```

Done. New Claude Code sessions in that distro show up on the host, tagged `VM`.

---

## Path B — Bridge (full Hyper-V/VirtualBox VMs, or NAT WSL)

**1. Host — install the self-healing bridge** (elevated PowerShell):

```powershell
cd tools\vm-bridge\host
.\bridge-install.ps1
```

This opens a private-scoped firewall hole on `:4318`, registers a scheduled task
that re-points the portproxy to the current vEthernet IP(s) at every boot/logon,
runs it once, and prints the exact guest commands with your token.

**2. Guest — run the forwarder** (needs `socat`; resolves the host gateway at start):

```bash
cd tools/vm-bridge/guest
GATEWAY=<host-ip> PORT_BRIDGE=4318 ./forwarder.sh      # or omit GATEWAY on WSL to auto-detect
```

Make it persistent with the systemd unit (see the header of
`guest/session-signals-forwarder.service`).

**3. Guest — install hooks** (same as Path A):

```bash
TOKEN=<auth_token> PORT=4317 ./install-hooks.sh
```

**Uninstall host side:** `host\bridge-uninstall.ps1`.

---

## Config entries this creates

**Guest — `~/.claude/settings.json`** (one such group per event; `install-hooks.sh`
merges these non-destructively — your other hooks are preserved):

```json
{
  "hooks": {
    "SessionStart": [
      { "matcher": "", "hooks": [
        { "type": "http", "url": "http://127.0.0.1:4317/hook",
          "headers": { "X-Beacon-Token": "<host-token>" }, "timeout": 10 } ] }
    ]
  }
}
```

Events wired (matching the host app): `SessionStart`, `SessionEnd`,
`UserPromptSubmit`, `UserPromptExpansion`, `PreToolUse`, `SubagentStart`,
`PreCompact`, `PostToolUse`, `PostToolUseFailure`, `PostToolBatch`, `Stop`,
`StopFailure`, `SubagentStop`, `PostCompact`, `Notification`.

**Host (bridge only)** — created by `bridge-install.ps1`:
- portproxy entries: `<vEthernet-ip>:4318 → 127.0.0.1:4317` (one per VM adapter, refreshed on boot).
- firewall rule `SessionSignals VM bridge 4318`: inbound TCP 4318 from `172.16.0.0/12`.
- scheduled task `SessionSignals-BridgeRefresh`: runs `bridge-refresh.ps1` at
  startup (+20s), logon (+10s), and on a NetworkProfile connect/disconnect event
  (+8s settle). No overlapping runs; a 20s cooldown coalesces event bursts.

**Host (mirrored only)** — `.wslconfig` as above. No app or firewall changes.

---

## Uninstall

- Guest hooks: `guest/uninstall-hooks.sh`.
- Bridge host side: `host/bridge-uninstall.ps1`.
- Mirrored: remove the `.wslconfig` lines + `wsl --shutdown`.

---

## Design notes / decisions

- **VM rows are tagged `VM`.** The app patch (`engine.rs` `Origin`, widget badge)
  detects a POSIX-absolute cwd on a Windows host and marks the row remote, so two
  VMs whose project folders share a basename are distinguishable. It's a
  Windows-host-only heuristic (a POSIX host can't tell host from guest by path)
  and is display-only.
- **No git branch on VM rows.** The app resolves a session's branch by reading
  `<cwd>/.git/HEAD` on the *host* filesystem; a VM path doesn't exist there, so
  branch resolution falls back to none. State/colour are unaffected.
- **No click-to-focus for VM sessions.** Terminal capture is a Windows-host
  feature; `install-hooks.sh` deliberately omits the SessionStart capture hook.
- **Token rotation.** If you regenerate the token in the app, re-run
  `install-hooks.sh` (with the new token) in every VM. Re-running is idempotent.
- **Moving IPs (bridge).** Handled on both ends: the guest forwarder resolves the
  gateway at start; the host task re-points the portproxy at boot/logon. A
  mid-session `wsl --shutdown` that changes the IP needs a manual
  `bridge-refresh.ps1` (or a forwarder restart) until the next boot/logon.
- **Security.** Mirrored keeps everything on loopback. The bridge widens the
  surface to the private VM subnet only (firewall-scoped) and the token remains
  the integrity control — no session state can be spoofed without it.
- **Stale cleanup.** If a VM/forwarder dies without a `SessionEnd`, its rows go
  grey via the normal stale sweep (`stale_timeout_min`, default 20).

## WSL gotchas

- **systemd for the forwarder unit:** put `[boot]\nsystemd=true` in
  `/etc/wsl.conf`, then `wsl --shutdown`.
- **Hyper-V firewall:** recent WSL fronts the vSwitch with a separate Hyper-V
  firewall. If bridge traffic is blocked despite the inbound rule, allow it with
  `Set-NetFirewallHyperVVMSetting -Name '{...}' -DefaultInboundAction Allow` or a
  targeted `New-NetFirewallHyperVRule`. Mirrored mode avoids this entirely.
