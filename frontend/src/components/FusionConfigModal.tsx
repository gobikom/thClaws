import { useEffect, useRef, useState } from "react";
import { send, subscribe } from "../hooks/useIPC";

/// Wire shape of the `openrouterFusion` config block (camelCase, matches
/// settings.json + the Rust `FusionConfig` serde). Only fields the user
/// set are sent back; empty/blank ones are stripped so the engine falls
/// through to OpenRouter's defaults.
type FusionConfig = {
  outerModel?: string;
  analysisModels?: string[];
  judgeModel?: string | null;
  maxToolCalls?: number | null;
  maxCompletionTokens?: number | null;
  temperature?: number | null;
  reasoning?: { effort?: string; max_tokens?: number } | null;
  toolChoice?: string;
};

type FormState = {
  outerModel: string;
  analysisModels: string[];
  judgeModel: string;
  maxToolCalls: string;
  maxCompletionTokens: string;
  temperature: string;
  reasoningEffort: string; // "" | low | medium | high
  toolChoice: string; // auto | required
};

const EMPTY_FORM: FormState = {
  outerModel: "openrouter/openai/gpt-4.1",
  analysisModels: [""],
  judgeModel: "",
  maxToolCalls: "",
  maxCompletionTokens: "",
  temperature: "",
  reasoningEffort: "",
  toolChoice: "auto",
};

function toForm(c: FusionConfig): FormState {
  const models = (c.analysisModels ?? []).filter((m) => m.trim());
  return {
    outerModel: c.outerModel ?? EMPTY_FORM.outerModel,
    analysisModels: models.length ? models : [""],
    judgeModel: c.judgeModel ?? "",
    maxToolCalls: c.maxToolCalls != null ? String(c.maxToolCalls) : "",
    maxCompletionTokens:
      c.maxCompletionTokens != null ? String(c.maxCompletionTokens) : "",
    temperature: c.temperature != null ? String(c.temperature) : "",
    reasoningEffort: c.reasoning?.effort ?? "",
    toolChoice: c.toolChoice === "required" ? "required" : "auto",
  };
}

function toConfig(f: FormState): FusionConfig {
  const cfg: FusionConfig = {
    outerModel: f.outerModel.trim() || EMPTY_FORM.outerModel,
    analysisModels: f.analysisModels.map((m) => m.trim()).filter(Boolean),
    toolChoice: f.toolChoice,
  };
  const judge = f.judgeModel.trim();
  if (judge) cfg.judgeModel = judge;
  const mtc = parseInt(f.maxToolCalls, 10);
  if (!Number.isNaN(mtc) && mtc > 0) cfg.maxToolCalls = mtc;
  const mct = parseInt(f.maxCompletionTokens, 10);
  if (!Number.isNaN(mct) && mct > 0) cfg.maxCompletionTokens = mct;
  const temp = parseFloat(f.temperature);
  if (!Number.isNaN(temp)) cfg.temperature = temp;
  if (f.reasoningEffort) cfg.reasoning = { effort: f.reasoningEffort };
  return cfg;
}

/// Config modal for the `openrouter/fusion+` pseudo-model. Opened from the
/// model picker when the user selects fusion+. Loads the current
/// `openrouterFusion` block, lets the user tune the deliberation panel,
/// then on Save persists it (`fusion_config_set`) and switches the active
/// model to `openrouter/fusion+` (`model_set`).
export function FusionConfigModal({
  onApplied,
  onCancel,
}: {
  onApplied: () => void;
  onCancel: () => void;
}) {
  const [form, setForm] = useState<FormState>(EMPTY_FORM);
  const [loaded, setLoaded] = useState(false);
  const [status, setStatus] = useState<
    | { kind: "idle" }
    | { kind: "saving" }
    | { kind: "error"; message: string }
  >({ kind: "idle" });
  const firstRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    const unsub = subscribe((msg) => {
      if (msg.type === "fusion_config") {
        setForm(toForm((msg.config as FusionConfig) ?? {}));
        setLoaded(true);
      } else if (msg.type === "fusion_config_result") {
        if (msg.ok) {
          // Config saved — now actually switch the active model.
          send({ type: "model_set", model: "openrouter/fusion+" });
          onApplied();
        } else {
          setStatus({
            kind: "error",
            message: String(msg.error ?? "save failed"),
          });
        }
      }
    });
    send({ type: "fusion_config_get" });
    return unsub;
  }, [onApplied]);

  useEffect(() => {
    if (loaded) firstRef.current?.focus();
  }, [loaded]);

  const set = <K extends keyof FormState>(key: K, value: FormState[K]) => {
    setForm((p) => ({ ...p, [key]: value }));
    if (status.kind === "error") setStatus({ kind: "idle" });
  };

  const setModel = (i: number, value: string) =>
    setForm((p) => {
      const next = [...p.analysisModels];
      next[i] = value;
      return { ...p, analysisModels: next };
    });
  const addModel = () =>
    setForm((p) =>
      p.analysisModels.length >= 8
        ? p
        : { ...p, analysisModels: [...p.analysisModels, ""] },
    );
  const removeModel = (i: number) =>
    setForm((p) => {
      const next = p.analysisModels.filter((_, idx) => idx !== i);
      return { ...p, analysisModels: next.length ? next : [""] };
    });

  const onSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    setStatus({ kind: "saving" });
    send({ type: "fusion_config_set", config: toConfig(form) });
  };

  const saving = status.kind === "saving";

  return (
    <div
      className="fixed inset-0 z-[70] flex items-center justify-center"
      style={{ background: "var(--modal-backdrop, rgba(0,0,0,0.55))" }}
      onMouseDown={onCancel}
    >
      <form
        className="rounded-lg border shadow-xl w-[560px] max-w-[92vw] max-h-[90vh] overflow-auto"
        style={{
          background: "var(--bg-primary)",
          borderColor: "var(--border)",
          color: "var(--text-primary)",
        }}
        onMouseDown={(e) => e.stopPropagation()}
        onSubmit={onSubmit}
      >
        <div
          className="px-4 py-2 border-b text-sm font-semibold flex items-center gap-2"
          style={{ borderColor: "var(--border)" }}
        >
          <span style={{ color: "var(--accent)" }}>◇</span>
          <span>Configure OpenRouter Fusion</span>
          <span
            className="ml-auto font-mono text-[10px]"
            style={{ color: "var(--text-secondary)" }}
          >
            openrouter/fusion+
          </span>
        </div>

        <div className="px-4 py-3 space-y-3 text-xs">
          {!loaded ? (
            <div
              className="py-6 text-center text-xs"
              style={{ color: "var(--text-secondary)" }}
            >
              Loading…
            </div>
          ) : (
            <>
              <Field
                label="Outer model"
                hint="The orchestrator call (thClaws id, e.g. openrouter/openai/gpt-4.1). Also the default judge."
              >
                <input
                  ref={firstRef}
                  type="text"
                  value={form.outerModel}
                  onChange={(e) => set("outerModel", e.target.value)}
                  autoCorrect="off"
                  autoCapitalize="off"
                  spellCheck={false}
                  className="w-full px-2 py-1.5 rounded border font-mono text-xs"
                  style={inputStyle}
                />
              </Field>

              <Field
                label="Analysis models (panel)"
                hint="OpenRouter ids (e.g. anthropic/claude-opus-4.8). 1–8. Leave blank to use Fusion's default panel (Opus + GPT + Gemini)."
              >
                <div className="space-y-1.5">
                  {form.analysisModels.map((m, i) => (
                    <div key={i} className="flex items-center gap-1.5">
                      <input
                        type="text"
                        value={m}
                        onChange={(e) => setModel(i, e.target.value)}
                        autoCorrect="off"
                        autoCapitalize="off"
                        spellCheck={false}
                        placeholder="anthropic/claude-opus-4.8"
                        className="flex-1 px-2 py-1.5 rounded border font-mono text-xs"
                        style={inputStyle}
                      />
                      <button
                        type="button"
                        onClick={() => removeModel(i)}
                        title="Remove"
                        className="px-2 py-1 rounded text-xs"
                        style={{ color: "var(--text-secondary)" }}
                      >
                        ✕
                      </button>
                    </div>
                  ))}
                  {form.analysisModels.length < 8 && (
                    <button
                      type="button"
                      onClick={addModel}
                      className="text-[11px] px-2 py-1 rounded border"
                      style={{
                        borderColor: "var(--border)",
                        color: "var(--text-secondary)",
                        background: "var(--bg-secondary)",
                      }}
                    >
                      + Add panel model
                    </button>
                  )}
                </div>
              </Field>

              <Field
                label="Judge model"
                hint="Synthesizes the final answer. Blank = same as outer model."
              >
                <input
                  type="text"
                  value={form.judgeModel}
                  onChange={(e) => set("judgeModel", e.target.value)}
                  autoCorrect="off"
                  autoCapitalize="off"
                  spellCheck={false}
                  placeholder="(outer model)"
                  className="w-full px-2 py-1.5 rounded border font-mono text-xs"
                  style={inputStyle}
                />
              </Field>

              <div className="grid grid-cols-3 gap-3">
                <Field label="Max tool calls" hint="1–16 · blank = 8">
                  <input
                    type="number"
                    min={1}
                    max={16}
                    value={form.maxToolCalls}
                    onChange={(e) => set("maxToolCalls", e.target.value)}
                    placeholder="8"
                    className="w-full px-2 py-1.5 rounded border font-mono text-xs"
                    style={inputStyle}
                  />
                </Field>
                <Field label="Max out tokens" hint="per inner call">
                  <input
                    type="number"
                    min={1}
                    value={form.maxCompletionTokens}
                    onChange={(e) =>
                      set("maxCompletionTokens", e.target.value)
                    }
                    placeholder="(default)"
                    className="w-full px-2 py-1.5 rounded border font-mono text-xs"
                    style={inputStyle}
                  />
                </Field>
                <Field label="Temperature" hint="0–2 · blank = default">
                  <input
                    type="number"
                    step="0.1"
                    min={0}
                    max={2}
                    value={form.temperature}
                    onChange={(e) => set("temperature", e.target.value)}
                    placeholder="(default)"
                    className="w-full px-2 py-1.5 rounded border font-mono text-xs"
                    style={inputStyle}
                  />
                </Field>
              </div>

              <div className="grid grid-cols-2 gap-3">
                <Field label="Reasoning effort" hint="forwarded to panel + judge">
                  <select
                    value={form.reasoningEffort}
                    onChange={(e) => set("reasoningEffort", e.target.value)}
                    className="w-full px-2 py-1.5 rounded border text-xs"
                    style={inputStyle}
                  >
                    <option value="">(provider default)</option>
                    <option value="low">low</option>
                    <option value="medium">medium</option>
                    <option value="high">high</option>
                  </select>
                </Field>
                <Field
                  label="Tool choice"
                  hint="auto = fusion + agent tools coexist · required = always deliberate"
                >
                  <select
                    value={form.toolChoice}
                    onChange={(e) => set("toolChoice", e.target.value)}
                    className="w-full px-2 py-1.5 rounded border text-xs"
                    style={inputStyle}
                  >
                    <option value="auto">auto</option>
                    <option value="required">required</option>
                  </select>
                </Field>
              </div>

              {status.kind === "error" && (
                <div
                  className="px-2 py-1.5 rounded border text-xs"
                  style={{
                    borderColor: "var(--accent-error, #c33)",
                    color: "var(--accent-error, #c33)",
                    background: "rgba(204,51,51,0.06)",
                  }}
                >
                  {status.message}
                </div>
              )}
            </>
          )}
        </div>

        <div
          className="px-4 py-3 border-t flex items-center justify-end gap-2"
          style={{ borderColor: "var(--border)" }}
        >
          <button
            type="button"
            onClick={onCancel}
            className="text-xs px-3 py-1.5 rounded hover:bg-white/5"
            style={{ color: "var(--text-secondary)" }}
            disabled={saving}
          >
            Cancel
          </button>
          <button
            type="submit"
            className="text-xs px-3 py-1.5 rounded"
            style={{
              background: "var(--accent)",
              color: "var(--accent-fg, #ffffff)",
              opacity: saving || !loaded ? 0.6 : 1,
            }}
            disabled={saving || !loaded}
          >
            {saving ? "Saving…" : "Save & use"}
          </button>
        </div>
      </form>
    </div>
  );
}

const inputStyle: React.CSSProperties = {
  background: "var(--bg-secondary)",
  borderColor: "var(--border)",
  color: "var(--text-primary)",
};

function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: React.ReactNode;
}) {
  return (
    <label className="block">
      <span
        className="block mb-1 text-[11px] uppercase tracking-wide"
        style={{ color: "var(--text-secondary)" }}
      >
        {label}
      </span>
      {children}
      {hint && (
        <span
          className="block mt-1 text-[10px]"
          style={{ color: "var(--text-secondary)" }}
        >
          {hint}
        </span>
      )}
    </label>
  );
}
