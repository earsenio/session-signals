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
  type WidgetPrefs,
} from "../state/types";
import "./Widget.css";

const STALE_COLOR = "#9ca3af";

function formatAge(seconds: number): string {
  if (seconds < 60) return `${seconds}s`;
  const m = Math.floor(seconds / 60);
  if (m < 60) return `${m}m`;
  return `${Math.floor(m / 60)}h ${m % 60}m`;
}

/// Subscribe to the engine's pushes and tick time-in-state locally between them.
/// The engine only re-emits when something changes, so a Ready session sitting
/// idle still needs the clock to advance client-side — we add the seconds
/// elapsed since the last payload to each session's `seconds_in_state`.
function useEngineState() {
  const [payload, setPayload] = useState<SessionsPayload>({ rollup: "grey", sessions: [] });
  const [baseAt, setBaseAt] = useState<number>(() => Date.now());
  const [now, setNow] = useState<number>(() => Date.now());

  useEffect(() => {
    invoke<SessionsPayload>("get_snapshot")
      .then((p) => {
        setPayload(p);
        setBaseAt(Date.now());
      })
      .catch(() => {});
    const unlisten = listen<SessionsPayload>("sessions-updated", (e) => {
      setPayload(e.payload);
      setBaseAt(Date.now());
    });
    return () => {
      unlisten.then((un) => un());
    };
  }, []);

  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, []);

  const elapsed = Math.max(0, Math.floor((now - baseAt) / 1000));
  const sessions = payload.sessions.map((s) => ({
    ...s,
    liveSeconds: s.seconds_in_state + elapsed,
  }));
  return { rollup: payload.rollup, sessions };
}

type LiveSession = SessionView & { liveSeconds: number };

function ExpandedRow({ session }: { session: LiveSession }) {
  const dot = session.stale ? STALE_COLOR : STATE_COLOR[session.state];
  const stateText = session.stale ? "Stale" : STATE_LABEL[session.state];
  return (
    <li className="wRow" style={{ opacity: session.stale ? 0.5 : 1 }}>
      <span className="wRowDot" style={{ background: dot }} />
      <span className="wRowLabel">{session.label}</span>
      <span className="wRowState">{stateText}</span>
      <span className="wRowAge">{formatAge(session.liveSeconds)}</span>
    </li>
  );
}

function CompactStrip({ sessions }: { sessions: LiveSession[] }) {
  if (sessions.length === 0) {
    return (
      <div className="wStrip" data-tauri-drag-region>
        <span className="wStripEmpty" data-tauri-drag-region>
          idle
        </span>
      </div>
    );
  }
  return (
    <div className="wStrip" data-tauri-drag-region>
      {sessions.map((s) => (
        <span
          key={s.session_id}
          className="wStripDot"
          title={`${s.label} — ${s.stale ? "Stale" : STATE_LABEL[s.state]}`}
          style={{
            background: s.stale ? STALE_COLOR : STATE_COLOR[s.state],
            opacity: s.stale ? 0.5 : 1,
          }}
        />
      ))}
    </div>
  );
}

export default function Widget() {
  const { rollup, sessions } = useEngineState();
  const [compact, setCompact] = useState(false);
  const [opacity, setOpacity] = useState(0.95);

  useEffect(() => {
    invoke<WidgetPrefs>("widget_prefs")
      .then((p) => {
        setCompact(p.compact);
        setOpacity(p.opacity);
      })
      .catch(() => {});
  }, []);

  const toggleCompact = useCallback(() => {
    setCompact((prev) => {
      const next = !prev;
      invoke("widget_set_compact", { compact: next }).catch(() => {});
      return next;
    });
  }, []);

  const changeOpacity = useCallback((value: number) => {
    setOpacity(value);
    invoke("widget_set_opacity", { opacity: value }).catch(() => {});
  }, []);

  const hide = useCallback(() => {
    invoke("widget_hide").catch(() => {});
  }, []);

  return (
    <div className="widget" style={{ opacity }}>
      <header className="wHeader" data-tauri-drag-region>
        <span
          className="wHeaderDot"
          data-tauri-drag-region
          style={{ background: ROLLUP_COLOR[rollup] }}
          title={ROLLUP_LABEL[rollup]}
        />
        <span className="wHeaderTitle" data-tauri-drag-region>
          Beacon
        </span>
        <button
          className="wIconBtn"
          onClick={toggleCompact}
          title={compact ? "Expand" : "Compact"}
        >
          {compact ? "▦" : "▤"}
        </button>
        <button className="wIconBtn" onClick={hide} title="Hide widget">
          ×
        </button>
      </header>

      {compact ? (
        <CompactStrip sessions={sessions} />
      ) : sessions.length === 0 ? (
        <p className="wEmpty">No live sessions.</p>
      ) : (
        <ul className="wList">
          {sessions.map((s) => (
            <ExpandedRow key={s.session_id} session={s} />
          ))}
        </ul>
      )}

      <footer className="wFooter">
        <span className="wFooterIcon" title="Opacity">
          ◐
        </span>
        <input
          className="wOpacity"
          type="range"
          min={0.3}
          max={1}
          step={0.05}
          value={opacity}
          onChange={(e) => changeOpacity(parseFloat(e.target.value))}
          title="Opacity"
        />
      </footer>
    </div>
  );
}
