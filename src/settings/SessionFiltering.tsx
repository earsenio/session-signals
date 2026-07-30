import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Config, IgnoreMatcher } from "../state/config";
import type { FilterStatus } from "../state/filters";
import { STATE_LABEL } from "../state/types";
import { Section, Toggle } from "./Settings";
import ProposalCard from "./ProposalCard";
import RuleList from "./RuleList";
import "./SessionFiltering.css";

interface SessionFilteringProps {
  cfg: Config;
  patch: (partial: Partial<Config>) => void;
  flash: (msg: string, kind: "ok" | "err") => void;
}

/// Mirrors the backend's `MIN_PROPOSE_THRESHOLD` clamp (src-tauri/src/config.rs)
/// — shown as a hint only; the floor itself is enforced server-side.
const MIN_PROPOSE_THRESHOLD = 3;

function describeRule(rule: IgnoreMatcher): string {
  return rule.kind === "cwd_contains"
    ? `cwd contains "${rule.value}"`
    : `first prompt starts with "${rule.value}"`;
}

export default function SessionFiltering({ cfg, patch, flash }: SessionFilteringProps) {
  const [status, setStatus] = useState<FilterStatus | null>(null);
  const [builtinMarkers, setBuiltinMarkers] = useState<string[]>([]);
  const [auditExpanded, setAuditExpanded] = useState(false);
  // `null` until fetched: the warning below stays hidden rather than firing
  // against a wrong guess of the floor while this is in flight.
  const [minSampleLen, setMinSampleLen] = useState<number | null>(null);

  const refetchStatus = useCallback(() => {
    invoke<FilterStatus>("filter_status")
      .then(setStatus)
      .catch(() => {});
  }, []);

  useEffect(() => {
    refetchStatus();
    invoke<string[]>("markers_builtin")
      .then(setBuiltinMarkers)
      .catch(() => {});
    invoke<number>("min_propose_sample_len")
      .then(setMinSampleLen)
      .catch(() => {});
  }, [refetchStatus]);

  // Keep the audit view honest against anything that could move it: a
  // proposal accepted, a session appearing/vanishing, or a rule edited
  // (including from another window).
  useEffect(() => {
    let active = true;
    const unlistenProposals = listen("proposals-updated", () => active && refetchStatus());
    const unlistenSessions = listen("sessions-updated", () => active && refetchStatus());
    const unlistenConfig = listen("config-updated", () => active && refetchStatus());
    return () => {
      active = false;
      void unlistenProposals.then((un) => un());
      void unlistenSessions.then((un) => un());
      void unlistenConfig.then((un) => un());
    };
  }, [refetchStatus]);

  const clearObservations = useCallback(async () => {
    try {
      await invoke("clear_observations");
      flash("Observations cleared", "ok");
      refetchStatus();
    } catch (e) {
      flash(String(e), "err");
    }
  }, [flash, refetchStatus]);

  const hiddenCount = status?.hidden_count ?? 0;
  const revealCount = status?.reveal_count ?? 0;

  return (
    <Section label="Session filtering">
      <ProposalCard flash={flash} />

      <div className="sCard sCardPad">
        <div className="sAuditHead">
          <div className="sRowText">
            <span className="sRowTitle">Hidden right now — {hiddenCount} sessions</span>
            <span className="sRowHint">Revealed because they needed you: {revealCount}</span>
          </div>
          {hiddenCount > 0 && (
            <button type="button" className="sBtn" onClick={() => setAuditExpanded((v) => !v)}>
              {auditExpanded ? "hide" : "show"} ▾
            </button>
          )}
        </div>
        {auditExpanded && status && status.hidden.length > 0 && (
          <ul className="sAuditList">
            {status.hidden.map((h) => (
              <li key={h.session.session_id} className="sAuditRow">
                <span className="sAuditLabel">
                  {h.session.label} ({STATE_LABEL[h.session.state]})
                </span>
                <span className="sAuditRule">{describeRule(h.rule)}</span>
              </li>
            ))}
          </ul>
        )}
      </div>

      <RuleList
        label="Hide these sessions"
        hint="Sessions matching any rule below are hidden from the widget and tray"
        rules={cfg.ignore_rules}
        onChange={(next) => patch({ ignore_rules: next })}
      />

      <RuleList
        label="Never hide these (wins over the list above)"
        hint="A session matching both stays visible"
        rules={cfg.never_hide}
        onChange={(next) => patch({ never_hide: next })}
      />
      {minSampleLen !== null &&
        cfg.never_hide.some((r) => r.value.length > 0 && r.value.length < minSampleLen) && (
          <p className="sRuleWarn">
            A short entry above matches broadly — below {minSampleLen} characters, a prefix stopped
            reliably separating sessions on the measured corpus (see{" "}
            <code>docs/IGNORING_BOT_SPAWNED_SESSIONS.md</code>).
          </p>
        )}

      <div className="sCard sCardPad">
        <div className="sRowText">
          <span className="sRowTitle">Always treated as yours (built in)</span>
          <span className="sRowHint">Claude Code's own structural markers — not editable</span>
        </div>
        <div className="sMarkerList">
          {builtinMarkers.map((m) => (
            <code key={m} className="sMarkerChip">
              {m}
            </code>
          ))}
        </div>
      </div>

      <div className="sCard">
        <div className="sRow">
          <div className="sRowText">
            <span className="sRowTitle">Watch for repeating openings</span>
            <span className="sRowHint">Suggest a filter once a pattern repeats</span>
          </div>
          <Toggle checked={cfg.observe_enabled} onChange={(v) => patch({ observe_enabled: v })} />
        </div>
        <div className="sRow">
          <div className="sRowText">
            <span className="sRowTitle">Suggest after</span>
            <span className="sRowHint">
              Floored at {MIN_PROPOSE_THRESHOLD} sessions by the backend
            </span>
          </div>
          <div className="sChip">
            <input
              className="sChipInput"
              type="number"
              min={MIN_PROPOSE_THRESHOLD}
              value={cfg.propose_threshold}
              onChange={(e) => {
                const v = parseInt(e.target.value, 10);
                if (Number.isFinite(v) && v >= 1) patch({ propose_threshold: v });
              }}
            />
            <span className="sChipSuf">sessions</span>
          </div>
        </div>
        <div className="sRow">
          <div className="sRowText">
            <span className="sRowTitle">Keep observations for</span>
            <span className="sRowHint">Days before an unmatched opening is pruned</span>
          </div>
          <div className="sChip">
            <input
              className="sChipInput"
              type="number"
              min={1}
              value={cfg.observe_retain_days}
              onChange={(e) => {
                const v = parseInt(e.target.value, 10);
                if (Number.isFinite(v) && v >= 1) patch({ observe_retain_days: v });
              }}
            />
            <span className="sChipSuf">days</span>
          </div>
        </div>
        <div className="sRow">
          <div className="sRowText">
            <span className="sRowTitle">Clear observations</span>
            <span className="sRowHint">
              Wipes counted openings now. A hand-written never_hide entry only applies to new
              observations — use this to forget openings recorded before it existed.
            </span>
          </div>
          <button type="button" className="sBtn sBtnDanger" onClick={clearObservations}>
            Clear observations
          </button>
        </div>
      </div>
    </Section>
  );
}
