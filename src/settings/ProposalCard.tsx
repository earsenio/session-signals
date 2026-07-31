import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Proposal } from "../state/proposals";
import { STATE_LABEL } from "../state/types";

interface ProposalCardProps {
  flash: (msg: string, kind: "ok" | "err") => void;
}

function formatWhen(unixSeconds: number): string {
  return new Date(unixSeconds * 1000).toLocaleString();
}

export default function ProposalCard({ flash }: ProposalCardProps) {
  const [proposal, setProposal] = useState<Proposal | null>(null);
  const [busy, setBusy] = useState(false);

  const refetch = useCallback(() => {
    invoke<Proposal[]>("list_proposals")
      .then((list) => setProposal(list[0] ?? null))
      .catch(() => {});
  }, []);

  useEffect(() => {
    refetch();
  }, [refetch]);

  // A proposal accepted/dismissed in another window, or a session ending
  // while this card is open, must never leave a stale card on screen.
  useEffect(() => {
    let active = true;
    const unlistenProposals = listen("proposals-updated", () => active && refetch());
    const unlistenSessions = listen("sessions-updated", () => active && refetch());
    return () => {
      active = false;
      void unlistenProposals.then((un) => un());
      void unlistenSessions.then((un) => un());
    };
  }, [refetch]);

  const run = useCallback(
    async (command: string, okMsg: string) => {
      if (!proposal) return;
      setBusy(true);
      try {
        await invoke(command, { fingerprint: proposal.fingerprint });
        flash(okMsg, "ok");
      } catch (e) {
        flash(String(e), "err");
      } finally {
        setBusy(false);
        refetch();
      }
    },
    [proposal, flash, refetch],
  );

  if (!proposal) return null;

  return (
    <div className="sCard sCardPad sProposal">
      <div className="sRowText">
        <span className="sRowTitle">Suggested filter</span>
      </div>
      <p className="sProposalCount">
        {proposal.count} session{proposal.count === 1 ? "" : "s"} have opened with:
      </p>
      <pre className="sCode">{proposal.sample}</pre>
      <p className="sProposalMeta">
        {proposal.len} characters · first seen {formatWhen(proposal.first_seen)} · last seen{" "}
        {formatWhen(proposal.last_seen)}
      </p>
      {proposal.matching.length > 0 ? (
        <div className="sProposalPreview">
          <p className="sProposalPreviewLabel">
            Hiding this would remove {proposal.matching.length} session
            {proposal.matching.length === 1 ? "" : "s"} visible right now:
          </p>
          <ul className="sProposalPreviewList">
            {proposal.matching.map((s) => (
              <li key={s.session_id}>
                {s.label} ({STATE_LABEL[s.state]})
              </li>
            ))}
          </ul>
        </div>
      ) : (
        <p className="sProposalPreviewLabel">No sessions matching this are visible right now.</p>
      )}
      <div className="sHookBtns">
        <button className="sBtn" disabled={busy} onClick={() => run("accept_proposal", "Hidden")}>
          Add to Hidden Sessions
        </button>
        <button
          className="sBtn"
          disabled={busy}
          onClick={() => run("dismiss_proposal", "Dismissed for now")}
        >
          Not now
        </button>
        <button
          className="sBtn sBtnDanger"
          disabled={busy}
          onClick={() => run("never_suggest_proposal", "Won't suggest this again")}
        >
          Never suggest this
        </button>
      </div>
    </div>
  );
}
