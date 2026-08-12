import { useEffect, useState } from "react";
import { send, subscribe } from "../hooks/useIPC";

type EditorState = {
  mode: "new" | "edit";
  name: string;
  frontmatter: string;
  body: string;
};

type Status =
  | { kind: "idle" }
  | { kind: "saving" }
  | { kind: "ok"; path: string }
  | { kind: "error"; message: string };

// Split a `.md` agent def into its YAML frontmatter and the system-
// prompt body. Tolerant of a missing frontmatter block (everything
// becomes the body).
function splitMd(md: string): { frontmatter: string; body: string } {
  const m = md.match(/^---\n([\s\S]*?)\n---\n?/);
  if (!m) return { frontmatter: "", body: md };
  return { frontmatter: m[1], body: md.slice(m[0].length).replace(/^\n+/, "") };
}

// Recombine the two panes into a single `.md` body for the backend.
function joinMd(frontmatter: string, body: string): string {
  const fm = frontmatter.trim();
  const b = body.replace(/^\n+/, "");
  const tail = b.endsWith("\n") ? "" : "\n";
  return `---\n${fm}\n---\n\n${b}${tail}`;
}

const inputStyle = {
  background: "var(--bg-secondary, var(--bg-primary))",
  borderColor: "var(--border)",
  color: "var(--text-primary)",
} as const;

/**
 * Agent-definition editor, opened by `/agent new <name>` or
 * `/agent edit <name>` from the GUI Chat tab. Subscribes to
 * `agent_editor_open` (which carries the full `.md` body) and submits
 * via `agent_save`. The backend writes `.thclaws/agents/<name>.md` and
 * dispatches `agent_save_result` with `{ok, error?, path?}`.
 *
 * Mirrors ScheduleAddModal: self-contained, mounted once in App.tsx,
 * open/close state driven by IPC events.
 */
export function AgentEditorModal() {
  const [editor, setEditor] = useState<EditorState | null>(null);
  const [status, setStatus] = useState<Status>({ kind: "idle" });

  useEffect(() => {
    const unsub = subscribe((msg) => {
      if (msg.type === "agent_editor_open") {
        const body = String(msg.body ?? "");
        const { frontmatter, body: prompt } = splitMd(body);
        setEditor({
          mode: msg.mode === "edit" ? "edit" : "new",
          name: String(msg.name ?? ""),
          frontmatter,
          body: prompt,
        });
        setStatus({ kind: "idle" });
      } else if (msg.type === "agent_save_result") {
        if (msg.ok) {
          setStatus({ kind: "ok", path: String(msg.path ?? "") });
          // Brief green confirm, then close.
          setTimeout(() => {
            setEditor(null);
            setStatus({ kind: "idle" });
          }, 800);
        } else {
          setStatus({
            kind: "error",
            message: String(msg.error ?? "unknown error"),
          });
        }
      }
    });
    return unsub;
  }, []);

  if (!editor) return null;

  const onClose = () => {
    if (status.kind === "saving") return;
    setEditor(null);
    setStatus({ kind: "idle" });
  };

  const onSave = () => {
    if (!editor || status.kind === "saving") return;
    setStatus({ kind: "saving" });
    send({
      type: "agent_save",
      name: editor.name,
      body: joinMd(editor.frontmatter, editor.body),
    });
  };

  const title = editor.mode === "new" ? "New agent" : "Edit agent";

  return (
    <div
      className="fixed inset-0 z-[60] flex items-center justify-center"
      style={{ background: "var(--modal-backdrop, rgba(0,0,0,0.55))" }}
      onClick={onClose}
    >
      <div
        className="rounded-lg border shadow-xl w-[720px] max-w-[94vw] max-h-[92vh] overflow-auto"
        style={{
          background: "var(--bg-primary)",
          borderColor: "var(--border)",
          color: "var(--text-primary)",
        }}
        onClick={(e) => e.stopPropagation()}
      >
        <div
          className="px-4 py-2 border-b text-sm font-semibold flex items-center gap-2"
          style={{ borderColor: "var(--border)" }}
        >
          <span style={{ color: "var(--accent)" }}>●</span>
          <span>{title}</span>
          <span className="font-mono text-xs" style={{ color: "var(--text-secondary)" }}>
            .thclaws/agents/{editor.name || "<name>"}.md
          </span>
        </div>

        <div className="px-4 py-3 space-y-3 text-xs">
          <div>
            <label
              className="block mb-1 font-medium"
              style={{ color: "var(--text-secondary)" }}
            >
              YAML frontmatter
            </label>
            <textarea
              value={editor.frontmatter}
              spellCheck={false}
              onChange={(e) => {
                setEditor({ ...editor, frontmatter: e.target.value });
                if (status.kind === "error") setStatus({ kind: "idle" });
              }}
              className="w-full px-2 py-1.5 rounded border font-mono text-xs"
              style={{ ...inputStyle, minHeight: "150px", resize: "vertical" }}
              placeholder={"name: …\ndescription: …\ntools: Read, Glob, Grep\npermissionMode: ask"}
            />
          </div>

          <div>
            <label
              className="block mb-1 font-medium"
              style={{ color: "var(--text-secondary)" }}
            >
              System prompt
            </label>
            <textarea
              value={editor.body}
              spellCheck={false}
              onChange={(e) => {
                setEditor({ ...editor, body: e.target.value });
                if (status.kind === "error") setStatus({ kind: "idle" });
              }}
              className="w-full px-2 py-1.5 rounded border text-xs"
              style={{ ...inputStyle, minHeight: "260px", resize: "vertical" }}
              placeholder="Describe what this agent should do, what to return, and what to avoid."
            />
          </div>

          {status.kind === "error" && (
            <div style={{ color: "var(--danger, #e57373)" }}>{status.message}</div>
          )}
          {status.kind === "ok" && (
            <div style={{ color: "var(--accent)" }}>✓ saved {status.path}</div>
          )}
        </div>

        <div
          className="px-4 py-2 border-t flex items-center justify-end gap-2"
          style={{ borderColor: "var(--border)" }}
        >
          <button
            type="button"
            onClick={onClose}
            disabled={status.kind === "saving"}
            className="px-3 py-1.5 rounded border text-xs"
            style={{ borderColor: "var(--border)", color: "var(--text-secondary)" }}
          >
            Cancel
          </button>
          <button
            type="button"
            onClick={onSave}
            disabled={status.kind === "saving" || !editor.name}
            className="px-3 py-1.5 rounded text-xs font-medium"
            style={{ background: "var(--accent)", color: "var(--accent-fg, #06231a)" }}
          >
            {status.kind === "saving" ? "Saving…" : "Save"}
          </button>
        </div>
      </div>
    </div>
  );
}
