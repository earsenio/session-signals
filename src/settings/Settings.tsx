import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { DEFAULT_CONFIG, SOUNDS, type Config, type StateNotify } from "../state/config";
import { useTheme } from "../themes/useTheme";
import { THEME_LIST, type ThemePalette } from "../themes";
import { shapeForState, StateGlyph } from "../components/StateGlyph";
import "./Settings.css";

type StateKey = "needs_you" | "working" | "ready";

const STATE_META: Record<StateKey, { title: string; hint: string }> = {
  needs_you: { title: "Needs you", hint: "Alert when a session is blocked on you" },
  working: { title: "Working", hint: "Usually off — you don’t need pinging mid-run" },
  ready: { title: "Ready", hint: "Alert when a turn finishes and it’s your move" },
};

export default function Settings() {
  const theme = useTheme();
  const palette = theme.palette;
  const [cfg, setCfg] = useState<Config>(DEFAULT_CONFIG);
  const [portInput, setPortInput] = useState("4317");
  const [installed, setInstalled] = useState(false);
  const [endpoint, setEndpoint] = useState("");
  const [hookBlock, setHookBlock] = useState("");
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
  }, [refreshHooks]);

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
      {!installed && (
        <Onboarding hookBlock={hookBlock} onInstall={install} onCopy={copyBlock} palette={palette} />
      )}

      <Section label="Notifications">
        <div className="sCard">
          {(Object.keys(STATE_META) as StateKey[]).map((key, i) => (
            <StateRow
              key={key}
              first={i === 0}
              color={palette.states[key]}
              shape={shapeForState(key)}
              title={STATE_META[key].title}
              hint={STATE_META[key].hint}
              value={cfg[key]}
              onChange={(partial) => patchState(key, partial)}
            />
          ))}
        </div>
        <label className="sCheckRow">
          <Toggle
            checked={cfg.notify_idle}
            onChange={(v) => patch({ notify_idle: v })}
          />
          <span>Notify when a session goes idle (stale)</span>
        </label>
      </Section>

      <Section label="General">
        <div className="sCard">
          <div className="sRow">
            <div className="sRowText">
              <span className="sRowTitle">Listener port</span>
              <span className="sRowHint">Where the Claude Code hook sends events</span>
            </div>
            <div className="sChip">
              <span className="sChipPre">:</span>
              <input
                className="sChipInput"
                type="number"
                min={1024}
                max={65535}
                value={portInput}
                onChange={(e) => setPortInput(e.target.value)}
                onKeyDown={(e) => e.key === "Enter" && applyPort()}
                onBlur={() => portInput !== String(cfg.port) && applyPort()}
              />
            </div>
          </div>

          <div className="sRow">
            <div className="sRowText">
              <span className="sRowTitle">Stale timeout</span>
              <span className="sRowHint">Mark a silent session grey after</span>
            </div>
            <div className="sChip">
              <input
                className="sChipInput sChipInputWide"
                type="number"
                min={1}
                max={1440}
                value={cfg.stale_timeout_min}
                onChange={(e) => {
                  const v = parseInt(e.target.value, 10);
                  if (Number.isFinite(v) && v >= 1) patch({ stale_timeout_min: v });
                }}
              />
              <span className="sChipSuf">min</span>
            </div>
          </div>

          <div className="sRow">
            <div className="sRowText">
              <span className="sRowTitle">Launch at login</span>
              <span className="sRowHint">Start Beacon quietly in the tray</span>
            </div>
            <Toggle
              checked={cfg.launch_on_login}
              onChange={(v) => patch({ launch_on_login: v })}
            />
          </div>

          <div className="sRow">
            <div className="sRowText">
              <span className="sRowTitle">Theme</span>
              <span className="sRowHint">Shape set + color map</span>
            </div>
            <div className="sSegment" role="radiogroup" aria-label="Theme">
              {THEME_LIST.map((t) => (
                <button
                  key={t.id}
                  type="button"
                  role="radio"
                  aria-checked={cfg.theme === t.id}
                  className={`sSeg ${cfg.theme === t.id ? "on" : ""}`}
                  onClick={() => patch({ theme: t.id })}
                >
                  {t.name}
                </button>
              ))}
            </div>
          </div>
        </div>
      </Section>

      <Section label="Claude Code hooks">
        <div className="sCard sCardPad">
          <div className="sHookStatus">
            <StateGlyph
              shape={installed ? "check" : "ring"}
              color={installed ? palette.states.ready : palette.stale}
              size={16}
            />
            <span className="sHookLabel">{installed ? "Hook installed" : "Not installed"}</span>
            <span className="sHookPath">~/.claude/settings.json</span>
          </div>
          <div className="sHookBtns">
            <button className="sBtn" onClick={install}>
              {installed ? "Reinstall" : "Install"}
            </button>
            <button className="sBtn sBtnDanger" onClick={uninstall} disabled={!installed}>
              Uninstall
            </button>
            <button className="sBtn" onClick={copyBlock} disabled={!hookBlock}>
              Copy config
            </button>
          </div>
          <p className="sHookNote">
            Beacon detects sessions via hooks that POST to <code>{endpoint}</code>.
          </p>
          <pre className="sCode">{hookBlock}</pre>
        </div>
      </Section>

      {status && <div className={`sToast ${status.kind}`}>{status.msg}</div>}
    </main>
  );
}

function Section({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <section className="sSection">
      <div className="sSectionLabel">{label}</div>
      {children}
    </section>
  );
}

function Toggle({
  checked,
  disabled,
  onChange,
}: {
  checked: boolean;
  disabled?: boolean;
  onChange: (v: boolean) => void;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      disabled={disabled}
      className={`sToggle ${checked ? "on" : ""}`}
      onClick={() => onChange(!checked)}
    >
      <span className="sToggleKnob" />
    </button>
  );
}

function SoundIcon({ on }: { on: boolean }) {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor" stroke="currentColor" strokeWidth="1.6" strokeLinejoin="round" aria-hidden="true">
      <path d="M5 9 H8.5 L12.5 5 V19 L8.5 15 H5 Z" />
      {on ? (
        <path d="M16.5 9.5 a4 4 0 0 1 0 5" fill="none" strokeLinecap="round" />
      ) : (
        <>
          <line x1="16" y1="9.5" x2="20.5" y2="14.5" strokeLinecap="round" />
          <line x1="20.5" y1="9.5" x2="16" y2="14.5" strokeLinecap="round" />
        </>
      )}
    </svg>
  );
}

function StateRow({
  first,
  color,
  shape,
  title,
  hint,
  value,
  onChange,
}: {
  first: boolean;
  color: string;
  shape: "square" | "dot" | "check" | "ring";
  title: string;
  hint: string;
  value: StateNotify;
  onChange: (partial: Partial<StateNotify>) => void;
}) {
  return (
    <div className={`sStateRow ${first ? "first" : ""}`}>
      <StateGlyph shape={shape} color={color} size={18} />
      <div className="sStateText">
        <span className="sStateTitle">{title}</span>
        <span className="sStateHint">{hint}</span>
      </div>
      <div className="sStateControls">
        {value.enabled && value.sound && (
          <select
            className="sSelect"
            value={value.sound_name}
            onChange={(e) => onChange({ sound_name: e.target.value })}
            title="Notification sound"
          >
            {SOUNDS.map((s) => (
              <option key={s} value={s}>
                {s}
              </option>
            ))}
          </select>
        )}
        <button
          type="button"
          className={`sSoundBtn ${value.sound ? "on" : ""}`}
          disabled={!value.enabled}
          onClick={() => onChange({ sound: !value.sound })}
          title={value.sound ? "Sound on" : "Sound off"}
        >
          <SoundIcon on={value.sound} />
        </button>
        <Toggle checked={value.enabled} onChange={(v) => onChange({ enabled: v })} />
      </div>
    </div>
  );
}

function Onboarding({
  hookBlock,
  onInstall,
  onCopy,
  palette,
}: {
  hookBlock: string;
  onInstall: () => void;
  onCopy: () => void;
  palette: ThemePalette;
}) {
  return (
    <section className="sOnboard">
      <div className="sOnboardGlyphs">
        <StateGlyph shape="square" color={palette.states.needs_you} size={22} />
        <StateGlyph shape="dot" color={palette.states.working} size={22} />
        <StateGlyph shape="check" color={palette.states.ready} size={22} />
        <StateGlyph shape="ring" color={palette.stale} size={22} />
      </div>
      <h1 className="sOnboardTitle">One quick setup</h1>
      <p className="sOnboardDesc">
        Beacon watches your Claude Code sessions through a small hook in its config. Add it
        once and Beacon will know the moment a session needs you, starts working, or finishes
        its turn.
      </p>
      <button className="sOnboardBtn" onClick={onInstall}>
        Set up automatically
      </button>
      <button className="sOnboardLink" onClick={onCopy} disabled={!hookBlock}>
        Copy the snippet instead ›
      </button>
      <pre className="sCode sOnboardCode">{hookBlock}</pre>
      <p className="sOnboardFoot">
        Beacon only appends its hook · reversible anytime below · no code leaves your machine
      </p>
    </section>
  );
}
