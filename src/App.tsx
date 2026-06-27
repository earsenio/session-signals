import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  ROLLUP_COLOR,
  ROLLUP_LABEL,
  STATE_COLOR,
  STATE_LABEL,
  type SessionsPayload,
  type SessionView,
} from "./state/types";
import "./App.css";

function formatAge(seconds: number): string {
  if (seconds < 60) return `${seconds}s`;
  const m = Math.floor(seconds / 60);
  if (m < 60) return `${m}m`;
  return `${Math.floor(m / 60)}h ${m % 60}m`;
}

function SessionRow({ session }: { session: SessionView }) {
  const dot = session.stale ? "#9ca3af" : STATE_COLOR[session.state];
  const stateText = session.stale ? "Stale" : STATE_LABEL[session.state];
  return (
    <li className="row" style={{ opacity: session.stale ? 0.5 : 1 }}>
      <span className="dot" style={{ background: dot }} />
      <span className="label">{session.label}</span>
      <span className="stateText">{stateText}</span>
      <span className="age">{formatAge(session.seconds_in_state)}</span>
    </li>
  );
}

export default function App() {
  const [payload, setPayload] = useState<SessionsPayload>({
    rollup: "grey",
    sessions: [],
  });
  const [hookBlock, setHookBlock] = useState<string>("");
  const [endpoint, setEndpoint] = useState<string>("");
  const [toast, setToast] = useState<string>("");

  // Initial snapshot + static info.
  useEffect(() => {
    invoke<SessionsPayload>("get_snapshot").then(setPayload).catch(() => {});
    invoke<string>("hook_block").then(setHookBlock).catch(() => {});
    invoke<string>("endpoint").then(setEndpoint).catch(() => {});
  }, []);

  // Live updates from the engine.
  useEffect(() => {
    const sessionsUnlisten = listen<SessionsPayload>("beacon://sessions", (e) =>
      setPayload(e.payload),
    );
    const toastUnlisten = listen<string>("beacon://toast", (e) => setToast(e.payload));
    return () => {
      sessionsUnlisten.then((un) => un());
      toastUnlisten.then((un) => un());
    };
  }, []);

  const install = useCallback(async () => {
    try {
      const path = await invoke<string>("install_hooks");
      setToast(`Hooks installed in ${path}`);
    } catch (err) {
      setToast(`Install failed: ${String(err)}`);
    }
  }, []);

  const uninstall = useCallback(async () => {
    try {
      const path = await invoke<string>("uninstall_hooks");
      setToast(`Hooks removed from ${path}`);
    } catch (err) {
      setToast(`Uninstall failed: ${String(err)}`);
    }
  }, []);

  const copyBlock = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(hookBlock);
      setToast("Hook config copied to clipboard");
    } catch {
      setToast("Copy failed — select the text manually");
    }
  }, [hookBlock]);

  const { rollup, sessions } = payload;

  return (
    <main className="app">
      <header className="header">
        <span className="rollupDot" style={{ background: ROLLUP_COLOR[rollup] }} />
        <div>
          <h1>Beacon</h1>
          <p className="rollupText">{ROLLUP_LABEL[rollup]}</p>
        </div>
      </header>

      <section className="sessions">
        <h2>Sessions</h2>
        {sessions.length === 0 ? (
          <p className="empty">No sessions yet. Start a Claude Code session.</p>
        ) : (
          <ul className="list">
            {sessions.map((s) => (
              <SessionRow key={s.session_id} session={s} />
            ))}
          </ul>
        )}
      </section>

      <section className="hooks">
        <h2>Hooks</h2>
        <p className="muted">
          Beacon detects sessions via Claude Code hooks that POST to{" "}
          <code>{endpoint}</code>.
        </p>
        <div className="buttons">
          <button onClick={install}>Install hooks</button>
          <button onClick={uninstall}>Uninstall</button>
          <button onClick={copyBlock} disabled={!hookBlock}>
            Copy config
          </button>
        </div>
        <details>
          <summary>Copy-paste fallback</summary>
          <pre className="block">{hookBlock}</pre>
        </details>
      </section>

      {toast && <div className="toast">{toast}</div>}
    </main>
  );
}
