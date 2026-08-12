/**
 * RunningChip — header indicator that an agent turn is in flight.
 *
 * Compact: a pulsing green dot + elapsed time when busy, a static gray
 * dot when idle (always present so the agent's alive/ready state is
 * visible). Session id and the last `[i/N] subject — verdict` progress
 * line move to the hover tooltip so the chip stays narrow (issue #171).
 * Click while busy to attach to the running session (sends `/load <id>`
 * through the normal shell input path).
 *
 * Companion to `useBusyState` — see dev-plan/36.
 */
import { useEffect, useState } from "react";
import { send } from "../hooks/useIPC";
import { useBusyState } from "../hooks/useBusyState";

function fmtElapsed(startedAtMs: number): string {
  const s = Math.max(0, Math.floor((Date.now() - startedAtMs) / 1000));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  const rem = s % 60;
  if (m < 60) return `${m}m${rem}s`;
  const h = Math.floor(m / 60);
  return `${h}h${m % 60}m`;
}

export function RunningChip() {
  const { busy, sessionId, startedAtMs, lastProgress } = useBusyState();
  // Force a re-render every second so the elapsed-time label ticks
  // without polling the engine. Only mounts the interval when busy.
  const [, setTick] = useState(0);
  useEffect(() => {
    if (!busy) return;
    const id = window.setInterval(() => setTick((t) => t + 1), 1000);
    return () => window.clearInterval(id);
  }, [busy]);

  // Compact, near-fixed-width indicator (issue #171): a status dot +
  // elapsed when busy, a static gray dot when idle. Full details
  // (session, progress) move to the hover tooltip so the chip never
  // pushes other header items off-screen. Always rendered — a persistent
  // dot confirms the agent is alive/ready.
  const elapsed = busy && startedAtMs ? fmtElapsed(startedAtMs) : "";
  const onClick = () => {
    if (busy && sessionId) {
      send({ type: "shell_input", text: `/load ${sessionId}` });
    }
  };

  const title = busy
    ? [
        sessionId ? `Running session ${sessionId}` : "Agent running",
        elapsed && `elapsed ${elapsed}`,
        lastProgress || null,
        sessionId && "click to attach",
      ]
        .filter(Boolean)
        .join(" — ")
    : "Idle";

  return (
    <button
      onClick={onClick}
      title={title}
      className={`${busy ? "running-chip " : ""}flex items-center justify-center gap-1.5 mr-2 rounded text-xs font-medium`}
      style={{
        padding: "1px 6px",
        minWidth: 24,
        background: busy ? "rgba(95, 179, 179, 0.15)" : "transparent",
        color: busy ? "var(--accent, #5fb3b3)" : "var(--text-secondary)",
        border: busy
          ? "1px solid rgba(95, 179, 179, 0.45)"
          : "1px solid transparent",
        cursor: busy && sessionId ? "pointer" : "default",
      }}
    >
      <span
        className="running-chip-dot"
        style={{
          display: "inline-block",
          width: 7,
          height: 7,
          borderRadius: "50%",
          background: busy ? "currentColor" : "var(--text-secondary, #888)",
          opacity: busy ? 1 : 0.5,
        }}
      />
      {busy && elapsed && <span style={{ opacity: 0.85 }}>{elapsed}</span>}
    </button>
  );
}
