//! FilmScript tools (dev-plan/52, Tier 3) — the deterministic rail the
//! Film Studio shell drives via `tools.invoke`, and the model via the
//! agent's `filmscript` skill. All five are gated behind `filmscript`:
//! dormant, invisible, zero prompt cost until the movie-maker-2 agent
//! (skill or shell on-ramp) opens the gate.
//!
//! Consent model: `FilmGenerate.budgetUsd` is REQUIRED — hosted
//! multiuser forces auto-approve, so the approval modal cannot be the
//! consent for spending money; the shell's cost-preview click (which
//! produces the arg) is. `requires_approval` stays on as desktop
//! belt-and-braces.

use super::Tool;
use crate::error::{Error, Result};
use crate::filmscript::{self, harness, AssetRequest};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub fn register(reg: &mut super::ToolRegistry) {
    reg.register(std::sync::Arc::new(FilmCompileTool));
    reg.register(std::sync::Arc::new(FilmGenerateTool));
    reg.register(std::sync::Arc::new(FilmJobStatusTool));
    reg.register(std::sync::Arc::new(FilmJobCancelTool));
    reg.register(std::sync::Arc::new(FilmAssetImportTool));
}

pub const GATE: &str = "filmscript";
const IMPORT_CAP_BYTES: usize = 30 * 1024 * 1024;

/// Compile + annotate for the UI: per-shot blocks, asset requests with
/// `exists` (fs check lives here — the compiler core stays pure), voice
/// ids validated against the registry, cost estimate, assembly plan.
pub struct FilmCompileTool;

#[async_trait]
impl Tool for FilmCompileTool {
    fn name(&self) -> &'static str {
        "FilmCompile"
    }

    fn description(&self) -> &'static str {
        "Compile a FilmScript (.film) source: validation errors (LLM-repairable), per-shot payload preview, asset requests annotated with file existence, T0-calibrated cost estimate, and the assembly plan. Pure and instant — safe on every editor change."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "script": { "type": "string", "description": "The .film source text." }
            },
            "required": ["script"]
        })
    }

    fn requires_gate(&self) -> Option<&'static str> {
        Some(GATE)
    }

    async fn call(&self, input: Value) -> Result<String> {
        let script = super::req_str(&input, "script")?;
        Ok(compile_report(script).to_string())
    }
}

fn compile_report(script: &str) -> Value {
    let p1 = filmscript::compile_phase1(script);
    let mut errors = serde_json::to_value(&p1.errors).unwrap_or(Value::Null);

    let registry = harness::tts::load_registry();
    let mut extra: Vec<Value> = Vec::new();
    let requests: Vec<Value> = p1
        .asset_requests
        .iter()
        .map(|r| {
            let mut v = serde_json::to_value(r).unwrap_or(Value::Null);
            if let AssetRequest::File { path, .. } = r {
                v["exists"] = json!(Path::new(path).exists());
            }
            if let AssetRequest::Tts { voice, .. } = r {
                if !registry.contains_key(voice) {
                    extra.push(json!({
                        "code": "E_VOICE_UNKNOWN",
                        "severity": "error",
                        "message": format!(
                            "voice '{voice}' ไม่อยู่ใน voices.json — ที่มี: {}",
                            registry.keys().cloned().collect::<Vec<_>>().join(", ")
                        ),
                    }));
                }
            }
            v
        })
        .collect();
    if let Some(arr) = errors.as_array_mut() {
        arr.extend(extra);
    }

    let shots: Vec<Value> = p1
        .shots
        .iter()
        .map(|s| {
            json!({
                "id": s.id(),
                "depends_on": s.depends_on(),
                "entities": s.entities,
                "needs_tts": s.tts_asset.is_some(),
                "needs_ref_video": s.video_asset.is_some(),
                "needs_first_frame": s.frame_asset.is_some(),
            })
        })
        .collect();

    json!({
        "errors": errors,
        "shots": shots,
        "asset_requests": requests,
        "assembly_plan": p1.assembly_plan,
        "cost": p1.cost,
        "job_id": harness::job::job_id_for_script(script),
    })
}

/// Start (or resume) the generation job. Spends real money.
pub struct FilmGenerateTool;

#[async_trait]
impl Tool for FilmGenerateTool {
    fn name(&self) -> &'static str {
        "FilmGenerate"
    }

    fn description(&self) -> &'static str {
        "Start the film generation job for a compiled .film script: TTS, uploads, per-shot Seedance tasks in dependency order, clips into .thclaws/film/<jobId>/out/. budgetUsd is REQUIRED (the confirmed cost-preview amount — hard gate). One active job per workspace; pass resume:true to continue the same script's interrupted job without double-spending."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "script": { "type": "string", "description": "The .film source text." },
                "budgetUsd": { "type": "number", "description": "Hard budget in USD the user confirmed after seeing FilmCompile's estimate. The job refuses to start if the estimate exceeds it." },
                "resume": { "type": "boolean", "description": "Continue this script's existing job (completed shots are skipped via the result cache)." }
            },
            "required": ["script", "budgetUsd"]
        })
    }

    fn requires_gate(&self) -> Option<&'static str> {
        Some(GATE)
    }

    fn requires_approval(&self, _input: &Value) -> bool {
        true
    }

    async fn call(&self, input: Value) -> Result<String> {
        let script = super::req_str(&input, "script")?;
        let budget = input
            .get("budgetUsd")
            .and_then(Value::as_f64)
            .ok_or_else(|| Error::Tool("budgetUsd is required (a number in USD)".into()))?;
        let resume = input
            .get("resume")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let job_id = harness::job::start(script, budget, resume)?;
        Ok(json!({ "jobId": job_id, "status": "started" }).to_string())
    }
}

pub struct FilmJobStatusTool;

#[async_trait]
impl Tool for FilmJobStatusTool {
    fn name(&self) -> &'static str {
        "FilmJobStatus"
    }

    fn description(&self) -> &'static str {
        "Snapshot of a film job: per-shot state (pending|assets|generating|polling|done|failed), spent credits, warnings, clip paths, per-job disk usage. Read-only; poll every few seconds while a job runs."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "jobId": { "type": "string", "description": "Job id returned by FilmGenerate." }
            },
            "required": ["jobId"]
        })
    }

    fn requires_gate(&self) -> Option<&'static str> {
        Some(GATE)
    }

    async fn call(&self, input: Value) -> Result<String> {
        let job_id = super::req_str(&input, "jobId")?;
        let state = harness::job::JobState::load(job_id)?;
        let mut v = serde_json::to_value(&state)?;
        v["disk_bytes"] = json!(dir_size(&harness_job_dir(job_id)));
        v["active_in_process"] = json!(harness::job::active_job_id().as_deref() == Some(job_id));
        Ok(v.to_string())
    }
}

fn harness_job_dir(job_id: &str) -> PathBuf {
    Path::new(".thclaws").join("film").join(job_id)
}

fn dir_size(path: &Path) -> u64 {
    let mut total = 0u64;
    if let Ok(entries) = std::fs::read_dir(path) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                total += dir_size(&p);
            } else if let Ok(m) = e.metadata() {
                total += m.len();
            }
        }
    }
    total
}

pub struct FilmJobCancelTool;

#[async_trait]
impl Tool for FilmJobCancelTool {
    fn name(&self) -> &'static str {
        "FilmJobCancel"
    }

    fn description(&self) -> &'static str {
        "Stop a running film job. Kie tasks already submitted keep their (already-debited) cost; completed shots stay in the result cache, so a later resume never re-pays them."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "jobId": { "type": "string", "description": "Job id to cancel." }
            },
            "required": ["jobId"]
        })
    }

    fn requires_gate(&self) -> Option<&'static str> {
        Some(GATE)
    }

    async fn call(&self, input: Value) -> Result<String> {
        let job_id = super::req_str(&input, "jobId")?;
        let stopped = harness::job::cancel(job_id)?;
        Ok(json!({ "jobId": job_id, "cancelled": stopped,
                   "note": if stopped { "worker will stop at the next step" }
                           else { "no such job running in this process" } })
        .to_string())
    }
}

/// Path-jailed asset import for the Film Studio shell (the bridge has
/// no workspace-write path for user files). Real-face reference images
/// are blocked by ByteDance policy — use generated character images.
pub struct FilmAssetImportTool;

#[async_trait]
impl Tool for FilmAssetImportTool {
    fn name(&self) -> &'static str {
        "FilmAssetImport"
    }

    fn description(&self) -> &'static str {
        "Save a user-supplied asset (base64) into .thclaws/film/assets/<relpath> for use as @./ paths in .film scripts. 30 MB cap. Reminder: Seedance rejects real-face reference uploads — character refs must be generated images."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "relpath": { "type": "string", "description": "Relative file name/path under .thclaws/film/assets/ (no '..', no absolute paths)." },
                "contentBase64": { "type": "string", "description": "File bytes, base64-encoded." },
                "mediaType": { "type": "string", "description": "MIME type hint (image/png, audio/mpeg, ...)." }
            },
            "required": ["relpath", "contentBase64"]
        })
    }

    fn requires_gate(&self) -> Option<&'static str> {
        Some(GATE)
    }

    fn requires_approval(&self, _input: &Value) -> bool {
        // Writes up to 30 MB to the workspace — gate it (desktop shows the
        // prompt; multiuser auto-approves within the per-user sandbox).
        true
    }

    async fn call(&self, input: Value) -> Result<String> {
        let rel = super::req_str(&input, "relpath")?;
        let b64 = super::req_str(&input, "contentBase64")?;

        let p = Path::new(rel);
        if p.is_absolute()
            || rel.starts_with(['/', '\\'])
            || p.components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(Error::Tool(format!(
                "relpath '{rel}' must stay under .thclaws/film/assets/ (no absolute paths or '..')"
            )));
        }

        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64.trim())
            .map_err(|e| Error::Tool(format!("contentBase64: {e}")))?;
        if bytes.len() > IMPORT_CAP_BYTES {
            return Err(Error::Tool(format!(
                "asset is {} bytes — the cap is 30 MB (Seedance's own per-image limit)",
                bytes.len()
            )));
        }

        let dest = Path::new(".thclaws").join("film").join("assets").join(p);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&dest, &bytes)?;
        let script_path = format!("./{}", dest.display());
        Ok(json!({ "path": script_path, "bytes": bytes.len() }).to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compile_report_annotates_existence_and_voices() {
        // overlay mode emits a TTS asset, so the unknown-voice check runs
        // (native — the default — synthesizes no TTS).
        let report = compile_report(
            "film \"t\" {\nbackend: seedance\ndialogue_sync: overlay\n}\n\
             char $a = @./definitely-missing-9f2.png voice:no-such-voice desc:\"x\"\n\
             shot 1 (dialogue) {\n$a say \"hello\"\n}\n",
        );
        let reqs = report["asset_requests"].as_array().unwrap();
        let file = reqs.iter().find(|r| r["kind"] == "file").unwrap();
        assert_eq!(file["exists"], false);
        let errors = report["errors"].as_array().unwrap();
        assert!(
            errors.iter().any(|e| e["code"] == "E_VOICE_UNKNOWN"),
            "{errors:?}"
        );
        assert!(report["job_id"].as_str().unwrap().starts_with("film-"));
    }

    #[tokio::test]
    async fn asset_import_is_jailed_and_capped() {
        let jail = FilmAssetImportTool
            .call(json!({ "relpath": "../escape.png", "contentBase64": "aGk=" }))
            .await;
        assert!(jail.is_err());
        let abs = FilmAssetImportTool
            .call(json!({ "relpath": "/tmp/x.png", "contentBase64": "aGk=" }))
            .await;
        assert!(abs.is_err());
    }

    #[tokio::test]
    async fn generate_requires_budget() {
        let r = FilmGenerateTool
            .call(json!({ "script": "shot 1 {\nx\n}\n" }))
            .await;
        assert!(r.unwrap_err().to_string().contains("budgetUsd"));
    }
}
