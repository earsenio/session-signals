import { useEffect, useRef, useState } from "react";
import type { IgnoreMatcher } from "../state/config";

const KIND_LABELS: Record<IgnoreMatcher["kind"], string> = {
  first_prompt_prefix: "first prompt starts",
  cwd_contains: "cwd contains",
};

const KIND_ORDER: IgnoreMatcher["kind"][] = ["first_prompt_prefix", "cwd_contains"];

/// Saving the whole config (and swapping the engine's live rules) on every
/// keystroke would be jank and log spam — debounce the persist rather than
/// firing `onChange` per character.
const PERSIST_DEBOUNCE_MS = 400;

interface RuleListProps {
  label: string;
  hint: string;
  rules: IgnoreMatcher[];
  onChange: (next: IgnoreMatcher[]) => void;
}

export default function RuleList({ label, hint, rules, onChange }: RuleListProps) {
  const [draft, setDraft] = useState<IgnoreMatcher[]>(rules);
  // Adjusting state during render (not via an effect) on a prop-identity
  // change — the recommended way to reflect an external update (another
  // window saved, or a proposal accept appended a rule) without an extra
  // render pass, and without clobbering an in-progress edit's debounce timer.
  const [prevRules, setPrevRules] = useState(rules);
  if (rules !== prevRules) {
    setPrevRules(rules);
    setDraft(rules);
  }
  // A single list-level timer, not one per row index: indices shift on
  // delete, so a per-index timer map (keyed by position) can fire against a
  // stale `next` after a different row is removed, reviving a deleted rule
  // (review finding H2). One timer for the whole list sidesteps the index
  // problem entirely — there is only ever one thing to debounce.
  const timer = useRef<number | undefined>(undefined);

  useEffect(() => () => window.clearTimeout(timer.current), []);

  const schedule = (next: IgnoreMatcher[]) => {
    setDraft(next);
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => onChange(next), PERSIST_DEBOUNCE_MS);
  };

  const flushNow = (next: IgnoreMatcher[]) => {
    setDraft(next);
    window.clearTimeout(timer.current);
    onChange(next);
  };

  const setKind = (index: number, kind: IgnoreMatcher["kind"]) => {
    const next = draft.map((r, i) => (i === index ? { kind, value: r.value } : r));
    flushNow(next);
  };

  const setValue = (index: number, value: string) => {
    const next = draft.map((r, i) => (i === index ? { ...r, value } : r));
    schedule(next);
  };

  const flushPending = () => {
    window.clearTimeout(timer.current);
    onChange(draft);
  };

  const remove = (index: number) => {
    const next = draft.filter((_, i) => i !== index);
    flushNow(next);
  };

  const add = () => {
    const next: IgnoreMatcher[] = [...draft, { kind: "first_prompt_prefix", value: "" }];
    flushNow(next);
  };

  return (
    <div className="sRuleList">
      <div className="sRuleListHead">
        <div className="sRowText">
          <span className="sRowTitle">{label}</span>
          <span className="sRowHint">{hint}</span>
        </div>
        <button type="button" className="sBtn" onClick={add}>
          + Add rule
        </button>
      </div>
      {draft.map((rule, index) => (
        // Index as key: values are duplicable and mutate as you type, so the
        // value itself can't be a stable key.
        <div className="sRuleRow" key={index}>
          <select
            className="sSelect sRuleKind"
            value={rule.kind}
            onChange={(e) => setKind(index, e.target.value as IgnoreMatcher["kind"])}
          >
            {KIND_ORDER.map((k) => (
              <option key={k} value={k}>
                {KIND_LABELS[k]}
              </option>
            ))}
          </select>
          <input
            className="sChipInput sRuleValue"
            type="text"
            value={rule.value}
            placeholder="…"
            onChange={(e) => setValue(index, e.target.value)}
            onBlur={flushPending}
          />
          <button
            type="button"
            className="sRuleRemove"
            onClick={() => remove(index)}
            aria-label="Remove rule"
          >
            ×
          </button>
        </div>
      ))}
    </div>
  );
}
