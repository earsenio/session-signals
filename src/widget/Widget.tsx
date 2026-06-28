import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  ROLLUP_LABEL,
  type Rollup,
  type SessionState,
  type SessionsPayload,
  type SessionView,
  type WidgetPrefs,
} from "../state/types";
import { useTheme } from "../themes/useTheme";
import type { ThemePalette } from "../themes";
import { shapeForRollup, shapeForState, StateGlyph } from "../components/StateGlyph";
import "./Widget.css";

/// m:ss for the first hour, h:mm:ss beyond — matches the design's "0:08", "14:03".
function formatAge(seconds: number): string {
  const s = seconds % 60;
  const m = Math.floor(seconds / 60) % 60;
  const h = Math.floor(seconds / 3600);
  const ss = String(s).padStart(2, "0");
  if (h > 0) return `${h}:${String(m).padStart(2, "0")}:${ss}`;
  return `${m}:${ss}`;
}

/// The engine ships a combined label ("folder (branch)"); split it for the
/// two-tone row presentation. Pure presentation — the engine is unchanged.
function splitLabel(label: string): { folder: string; branch: string | null } {
  const m = label.match(/^(.*) \(([^)]+)\)$/);
  return m ? { folder: m[1], branch: m[2] } : { folder: label, branch: null };
}

/// Horizontal padding of `.wPill` (2 × --sp-4), added to the measured content
/// width when sizing the collapsed window so the pill hugs its glyphs.
const PILL_PAD_X = 26;

/// Row state text per the design (richer than the tray tooltips).
const ROW_STATE_TEXT: Record<SessionState, string> = {
  needs_you: "Needs your input",
  working: "Working",
  ready: "Ready for you",
};

function rowColor(palette: ThemePalette, s: LiveSession): string {
  return s.stale ? palette.stale : palette.states[s.state];
}

/// Subscribe to the engine's pushes and tick time-in-state locally between them.
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
    // The subagent sub-line's ticking timer reuses the SAME local tick — no
    // second timer. Only meaningful while at least one subagent is running.
    subLiveSeconds: s.subagent_count > 0 ? s.subagent_seconds + elapsed : 0,
  }));
  return { rollup: payload.rollup, sessions };
}

type LiveSession = SessionView & { liveSeconds: number; subLiveSeconds: number };

/// Short header summary derived from the rollup + live sessions (presentation).
function headerStatus(rollup: Rollup, sessions: LiveSession[]): string {
  const needs = sessions.filter((s) => s.state === "needs_you" && !s.stale).length;
  if (needs > 0) return `${needs} needs you`;
  switch (rollup) {
    case "orange":
      return "Working";
    case "green":
      return "Ready";
    default:
      return "idle";
  }
}

function ExpandedRow({ session, palette }: { session: LiveSession; palette: ThemePalette }) {
  const { folder, branch } = splitLabel(session.label);
  const color = rowColor(palette, session);
  const stateText = session.stale ? "No response" : ROW_STATE_TEXT[session.state];
  // Subagent activity is independent of the row's own state: a session can be
  // red (Needs you) or green (Ready) while subagents still run underneath.
  const busy = session.subagent_count > 0;
  const subLabel = `${session.subagent_count} ${
    session.subagent_count === 1 ? "agent" : "agents"
  } running`;

  // Click-to-focus: only offered when Beacon resolved the owning terminal
  // window (can_focus). A click raises it; if the window vanished since capture,
  // focus_session returns false and we flash a brief "can't focus" hint rather
  // than failing silently. Rows without a resolved window aren't clickable.
  const [focusFailed, setFocusFailed] = useState(false);
  const onFocus = useCallback(() => {
    if (!session.can_focus) return;
    invoke<boolean>("focus_session", { sessionId: session.session_id })
      .then((ok) => {
        if (!ok) {
          setFocusFailed(true);
          window.setTimeout(() => setFocusFailed(false), 1400);
        }
      })
      .catch(() => {
        setFocusFailed(true);
        window.setTimeout(() => setFocusFailed(false), 1400);
      });
  }, [session.can_focus, session.session_id]);

  return (
    <li
      className={`wRow${session.can_focus ? " wRowFocusable" : ""}`}
      style={{ opacity: session.stale ? 0.5 : 1 }}
      onClick={onFocus}
      title={session.can_focus ? "Click to focus this session’s terminal" : undefined}
    >
      <div className="wRowTop">
        <span className="wGlyphWrap">
          {/* Soft amber halo behind the glyph — always amber regardless of row
              state, so it reads as "busy" without a second competing marker. */}
          {busy && <span className="wBusyHalo" aria-hidden="true" />}
          <span className="wGlyphFront">
            <StateGlyph
              shape={session.stale ? "ring" : shapeForState(session.state)}
              color={color}
              size={22}
              pulse={session.state === "working" && !session.stale}
            />
          </span>
        </span>
        <div className="wRowMain">
          <div className="wRowLabel">
            <span className="wRowFolder">{folder}</span>
            {branch && (
              <span className="wRowBranch">
                <span className="wBranchIcon">⑃ </span>
                {branch}
              </span>
            )}
          </div>
          <div className="wRowState" style={{ color }}>
            {stateText} <span className="wRowAge">· {formatAge(session.liveSeconds)}</span>
          </div>
        </div>
        {focusFailed ? (
          <span className="wRowFocusFail" title="That terminal window couldn’t be focused">
            can’t focus
          </span>
        ) : (
          session.can_focus && <span className="wChevron">›</span>
        )}
      </div>
      {/* Quiet sub-line: pulsing dot + count + ticking elapsed. Rendered only
          while busy → no reserved height when the count is 0 (row reflows). */}
      {busy && (
        <div className="wSubline">
          <span className="wSubDot" />
          <span className="wSubLabel">{subLabel}</span>
          <span className="wSubTimer">{formatAge(session.subLiveSeconds)}</span>
        </div>
      )}
    </li>
  );
}

/// The collapsed view: a headerless pill of just the state glyphs (one per
/// session) + a count. Per the design it "drags anywhere, stays on top, click
/// to expand". Drag and click share the same surface, so we only begin an OS
/// window drag once the pointer actually moves past a small threshold — a
/// stationary press is treated as a click and expands. (Hence no
/// `data-tauri-drag-region`, which would start dragging on every mousedown and
/// swallow the click.)
function CompactPill({
  sessions,
  palette,
  onExpand,
}: {
  sessions: LiveSession[];
  palette: ThemePalette;
  onExpand: () => void;
}) {
  // Tracks the press in flight: where it began and whether it became a drag.
  const press = useRef<{ x: number; y: number; dragging: boolean } | null>(null);
  // Measures the pill's natural content width so the window can hug it.
  const innerRef = useRef<HTMLDivElement>(null);

  // Whenever the content's width changes (sessions added/removed, fonts settle)
  // ask the shell to resize the collapsed window to fit. `.wPillInner` is
  // `width: max-content`, so its measured width is the true content width
  // regardless of the current window size — no resize feedback loop.
  useLayoutEffect(() => {
    const el = innerRef.current;
    if (!el) return;
    const fit = () => {
      const w = Math.ceil(el.getBoundingClientRect().width) + PILL_PAD_X;
      invoke("widget_set_compact_width", { width: w }).catch(() => {});
    };
    fit();
    const ro = new ResizeObserver(fit);
    ro.observe(el);
    return () => ro.disconnect();
  }, [sessions.length]);

  const onMouseDown = useCallback((e: React.MouseEvent) => {
    if (e.button !== 0) return;
    press.current = { x: e.screenX, y: e.screenY, dragging: false };

    const move = (me: MouseEvent) => {
      const p = press.current;
      if (!p || p.dragging) return;
      if (Math.abs(me.screenX - p.x) > 4 || Math.abs(me.screenY - p.y) > 4) {
        // Real drag: hand off to the OS and stop listening (the drag loop
        // takes over; the trailing click, if any, is ignored below).
        p.dragging = true;
        document.removeEventListener("mousemove", move);
        document.removeEventListener("mouseup", up);
        getCurrentWindow().startDragging().catch(() => {});
      }
    };
    const up = () => {
      document.removeEventListener("mousemove", move);
      document.removeEventListener("mouseup", up);
    };
    document.addEventListener("mousemove", move);
    document.addEventListener("mouseup", up);
  }, []);

  const onClick = useCallback(() => {
    const dragged = press.current?.dragging ?? false;
    press.current = null;
    if (!dragged) onExpand();
  }, [onExpand]);

  return (
    <div
      className="wPill"
      onMouseDown={onMouseDown}
      onClick={onClick}
      title="Click to expand"
    >
      <div className="wPillInner" ref={innerRef}>
        <svg className="wGrip" width="7" height="16" viewBox="0 0 7 16" aria-hidden="true">
          <circle cx="1.5" cy="3" r="1.4" />
          <circle cx="5.5" cy="3" r="1.4" />
          <circle cx="1.5" cy="8" r="1.4" />
          <circle cx="5.5" cy="8" r="1.4" />
          <circle cx="1.5" cy="13" r="1.4" />
          <circle cx="5.5" cy="13" r="1.4" />
        </svg>
        {sessions.length === 0 ? (
          <span className="wStripEmpty">idle</span>
        ) : (
          <>
            {sessions.map((s) => (
              <span
                key={s.session_id}
                className="wStripGlyph"
                style={{ opacity: s.stale ? 0.5 : 1 }}
                title={`${s.label} — ${s.stale ? "No response" : ROW_STATE_TEXT[s.state]}`}
              >
                <StateGlyph
                  shape={s.stale ? "ring" : shapeForState(s.state)}
                  color={rowColor(palette, s)}
                  size={17}
                  pulse={s.state === "working" && !s.stale}
                />
              </span>
            ))}
            <span className="wStripDivider" />
            <span className="wStripCount">{sessions.length}</span>
          </>
        )}
      </div>
    </div>
  );
}

function EmptyBody({ palette }: { palette: ThemePalette }) {
  return (
    <div className="wEmpty">
      <span className="wEmptyGlyph">
        <StateGlyph shape="ring" color={palette.stale} size={30} />
      </span>
      <div className="wEmptyTitle">No active sessions</div>
      <div className="wEmptyHint">
        Start a Claude Code session in your terminal and it’ll appear here.
      </div>
    </div>
  );
}

export default function Widget() {
  const { rollup, sessions } = useEngineState();
  const theme = useTheme();
  const palette = theme.palette;
  const [compact, setCompact] = useState(false);
  const [opacity, setOpacity] = useState(0.95);
  const [port, setPort] = useState<number | null>(null);

  useEffect(() => {
    invoke<WidgetPrefs>("widget_prefs")
      .then((p) => {
        setCompact(p.compact);
        setOpacity(p.opacity);
      })
      .catch(() => {});
    // For the footer status strip; read-only, no engine change.
    invoke<{ port: number }>("get_config")
      .then((c) => setPort(c.port))
      .catch(() => {});
  }, []);

  const toggleCompact = useCallback(() => {
    setCompact((prev) => {
      const next = !prev;
      invoke("widget_set_compact", { compact: next }).catch(() => {});
      return next;
    });
  }, []);

  const hide = useCallback(() => {
    invoke("widget_hide").catch(() => {});
  }, []);

  const rollupShape = shapeForRollup(rollup);
  const rollupColor = rollup === "grey" ? palette.stale : palette.rollups[rollup];

  // Collapsed: nothing but the draggable, click-to-expand pill.
  if (compact) {
    return (
      <div className="widget widgetCompact" style={{ opacity }}>
        <CompactPill sessions={sessions} palette={palette} onExpand={toggleCompact} />
      </div>
    );
  }

  return (
    <div className="widget" style={{ opacity }}>
      <header className="wHeader" data-tauri-drag-region>
        <span className="wHeaderGlyph" data-tauri-drag-region>
          <StateGlyph
            shape={rollupShape}
            color={rollupColor}
            size={13}
            pulse={rollup === "orange"}
          />
        </span>
        <span className="wTitle" data-tauri-drag-region>
          Beacon
        </span>
        <span className="wHeaderStatus" data-tauri-drag-region title={ROLLUP_LABEL[rollup]}>
          {headerStatus(rollup, sessions)}
        </span>
        <span className="wHeaderSpacer" data-tauri-drag-region />
        <button
          className="wIconBtn"
          onClick={toggleCompact}
          title={compact ? "Expand" : "Compact"}
        >
          <svg width="14" height="14" viewBox="0 0 24 24" aria-hidden="true">
            <circle cx="5" cy="12" r="2" />
            <circle cx="12" cy="12" r="2" />
            <circle cx="19" cy="12" r="2" />
          </svg>
        </button>
        <button className="wIconBtn" onClick={hide} title="Hide widget">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" aria-hidden="true">
            <line x1="5" y1="6" x2="19" y2="6" />
            <line x1="5" y1="12" x2="19" y2="12" />
            <line x1="5" y1="18" x2="19" y2="18" />
          </svg>
        </button>
      </header>

      <div className="wDivider" />

      {sessions.length === 0 ? (
        <EmptyBody palette={palette} />
      ) : (
        <ul className="wList">
          {sessions.map((s) => (
            <ExpandedRow key={s.session_id} session={s} palette={palette} />
          ))}
        </ul>
      )}

      <footer className="wFooter">
        <span className="wFootItem">LISTENING · :{port ?? "—"}</span>
        <span className="wFootItem">
          {sessions.length} SESSION{sessions.length === 1 ? "" : "S"}
        </span>
      </footer>
    </div>
  );
}
