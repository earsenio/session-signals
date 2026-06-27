import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  ROLLUP_COLOR,
  ROLLUP_LABEL,
  STATE_COLOR,
  type Rollup,
  type SessionsPayload,
} from "../state/types";
import { DEFAULT_CONFIG, SOUNDS, type Config, type StateNotify } from "../state/config";
import "./Settings.css";

type StateKey = "needs_you" | "working" | "ready";

const STATE_META: Record<StateKey, { title: string; hint: string }> = {
  needs_you: { title: "Needs you", hint: "Blocked on you — permission, choice, or answer" },
  working: { title: "Working", hint: "Actively running — don't interrupt" },
  ready: { title: "Ready", hint: "Finished its turn — okay to give new instructions" },
};

export default function Settings() {
  const [cfg, setCfg] = useState<Config>(DEFAULT_CONFIG);
  const [portInput, setPortInput] = useState("4317");
  const [installed, setInstalled] = useState(false);
  const [endpoint, setEndpoint] = useState("");
  const [hookBlock, setHookBlock] = useState("");
  const [rollup, setRollup] = useState<Rollup>("grey");
  const [sessionCount, setSessionCount] = useState(0);
  const [status, setStatus] = useState<{ msg: string; kind: "ok" | "err" } | null>(null);
  const flashTimer = useRef<number | undefined>(undefined);

  const flash = useCallback((msg: string, kind: "ok" | "err") => {
    setStatus({ msg, kind });
    if (flashTimer.current) window.clearTimeout(flashTimer.current);
    flashTimer.current = window.setTimeout(() => setStatus(null), 3000);
  }, []);

  const refreshHooks = useCallback(() => {
    invoke<boolean>("hooks_installed").then(setInstalled).catch(() => {});
    invoke<string>("endpoint").then(setEndpoint).catch(() => {});
    invoke<string>("hook_block").then(setHookBlock).catch(() => {});
  }, []);

  // Initial load.
  useEffect(() => {
    invoke<Config>("get_config")
      .then((c) => {
        setCfg(c);
        setPortInput(String(c.port));
      })
      .catch(() => {});
    refreshHooks();
    invoke<SessionsPayload>("get_snapshot")
      .then((p) => {
        setRollup(p.rollup);
        setSessionCount(p.sessions.length);
      })
      .catch(() => {});
  }, [refreshHooks]);

  // Live engine status.
  useEffect(() => {
    const un = listen<SessionsPayload>("sessions-updated", (e) => {
      setRollup(e.payload.rollup);
      setSessionCount(e.payload.sessions.length);
    });
    return () => {
      un.then((u) => u());
    };
  }, []);

  // Persist a full config and reflect backend errors.
  const persist = useCallback(
    async (next: Config) => {
      try {
        await invoke("set_config", { new: next });
        flash("Saved", "ok");
      } catch (e) {
        flash(String(e), "err");
        const fresh = await invoke<Config>("get_config").catch(() => next);
        setCfg(fresh);
        setPortInput(String(fresh.port));
      }
    },
    [flash],
  );

  // Merge a partial into config and persist (everything except port, which has
  // its own Apply so we don't rebind the listener on every keystroke).
  const patch = useCallback(
    (partial: Partial<Config>) => {
      setCfg((c) => {
        const next = { ...c, ...partial };
        void persist(next);
        return next;
      });
    },
    [persist],
  );

  const patchState = useCallback(
    (key: StateKey, partial: Partial<StateNotify>) => {
      setCfg((c) => {
        const next = { ...c, [key]: { ...c[key], ...partial } };
        void persist(next);
        return next;
      });
    },
    [persist],
  );

  const applyPort = useCallback(async () => {
    const p = parseInt(portInput, 10);
    if (!Number.isFinite(p) || p < 1024 || p > 65535) {
      flash("Port must be between 1024 and 65535", "err");
      return;
    }
    if (p === cfg.port) {
      flash("Port unchanged", "ok");
      return;
    }
    const next = { ...cfg, port: p };
    try {
      await invoke("set_config", { new: next });
      setCfg(next);
      flash(`Listening on 127.0.0.1:${p}`, "ok");
      refreshHooks();
    } catch (e) {
      flash(String(e), "err");
      const fresh = await invoke<Config>("get_config").catch(() => cfg);
      setCfg(fresh);
      setPortInput(String(fresh.port));
    }
  }, [portInput, cfg, flash, refreshHooks]);

  const install = useCallback(async () => {
    try {
      await invoke<string>("install_hooks");
      flash("Hooks installed", "ok");
    } catch (e) {
      flash(`Install failed: ${String(e)}`, "err");
    }
    refreshHooks();
  }, [flash, refreshHooks]);

  const uninstall = useCallback(async () => {
    try {
      await invoke<string>("uninstall_hooks");
      flash("Hooks removed", "ok");
    } catch (e) {
      flash(`Uninstall failed: ${String(e)}`, "err");
    }
    refreshHooks();
  }, [flash, refreshHooks]);

  const copyBlock = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(hookBlock);
      flash("Hook config copied", "ok");
    } catch {
      flash("Copy failed — select the text manually", "err");
    }
  }, [hookBlock, flash]);

  return (
    <main className="settings">
      <header className="sHeader">
        <span className="sRollup" style={{ background: ROLLUP_COLOR[rollup] }} />
        <div>
          <h1>Beacon</h1>
          <p className="sSub">
            {ROLLUP_LABEL[rollup]} · {sessionCount} session{sessionCount === 1 ? "" : "s"}
          </p>
        </div>
      </header>

      <section className="sCard">
        <h2>Notifications</h2>
        <p className="sMuted">Fired on state transitions only — never while idle.</p>
        {(Object.keys(STATE_META) as StateKey[]).map((key) => (
          <StateRow
            key={key}
            color={STATE_COLOR[key]}
            title={STATE_META[key].title}
            hint={STATE_META[key].hint}
            value={cfg[key]}
            onChange={(partial) => patchState(key, partial)}
          />
        ))}
        <label className="sCheckRow">
          <input
            type="checkbox"
            checked={cfg.notify_idle}
            onChange={(e) => patch({ notify_idle: e.target.checked })}
          />
          <span>Notify when a session goes idle (stale)</span>
        </label>
      </section>

      <section className="sCard">
        <h2>Listener</h2>
        <div className="sField">
          <label htmlFor="port">Port</label>
          <div className="sInline">
            <input
              id="port"
              className="sNum"
              type="number"
              min={1024}
              max={65535}
              value={portInput}
              onChange={(e) => setPortInput(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && applyPort()}
            />
            <button onClick={applyPort}>Apply</button>
          </div>
        </div>
        <p className="sMuted">
          Changing the port rebinds the local listener and updates{" "}
          <code>~/.claude/settings.json</code> if hooks are installed.
        </p>

        <div className="sField">
          <label htmlFor="stale">Stale timeout (min)</label>
          <input
            id="stale"
            className="sNum"
            type="number"
            min={1}
            max={1440}
            value={cfg.stale_timeout_min}
            onChange={(e) => {
              const v = parseInt(e.target.value, 10);
              if (Number.isFinite(v) && v >= 1) patch({ stale_timeout_min: v });
            }}
          />
        </div>

        <label className="sCheckRow">
          <input
            type="checkbox"
            checked={cfg.launch_on_login}
            onChange={(e) => patch({ launch_on_login: e.target.checked })}
          />
          <span>Launch Beacon on login</span>
        </label>
      </section>

      <section className="sCard">
        <h2>
          Hooks
          <span className={`sBadge ${installed ? "on" : "off"}`}>
            {installed ? "Installed" : "Not installed"}
          </span>
        </h2>
        <p className="sMuted">
          Beacon detects sessions via Claude Code hooks that POST to <code>{endpoint}</code>.
        </p>
        <div className="sButtons">
          <button onClick={install}>{installed ? "Reinstall" : "Install"}</button>
          <button onClick={uninstall} disabled={!installed}>
            Uninstall
          </button>
          <button onClick={copyBlock} disabled={!hookBlock}>
            Copy config
          </button>
        </div>
        <details>
          <summary>Copy-paste fallback</summary>
          <pre className="sBlock">{hookBlock}</pre>
        </details>
      </section>

      {status && <div className={`sToast ${status.kind}`}>{status.msg}</div>}
    </main>
  );
}

function StateRow({
  color,
  title,
  hint,
  value,
  onChange,
}: {
  color: string;
  title: string;
  hint: string;
  value: StateNotify;
  onChange: (partial: Partial<StateNotify>) => void;
}) {
  return (
    <div className="sStateRow">
      <span className="sStateDot" style={{ background: color }} />
      <div className="sStateText">
        <span className="sStateTitle">{title}</span>
        <span className="sStateHint">{hint}</span>
      </div>
      <div className="sStateControls">
        <label className="sToggle" title="Notify">
          <input
            type="checkbox"
            checked={value.enabled}
            onChange={(e) => onChange({ enabled: e.target.checked })}
          />
          <span>Notify</span>
        </label>
        <label className="sToggle" title="Play a sound">
          <input
            type="checkbox"
            checked={value.sound}
            disabled={!value.enabled}
            onChange={(e) => onChange({ sound: e.target.checked })}
          />
          <span>Sound</span>
        </label>
        <select
          className="sSelect"
          value={value.sound_name}
          disabled={!value.enabled || !value.sound}
          onChange={(e) => onChange({ sound_name: e.target.value })}
        >
          {SOUNDS.map((s) => (
            <option key={s} value={s}>
              {s}
            </option>
          ))}
        </select>
      </div>
    </div>
  );
}
