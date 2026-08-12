//! FilmScript DSL compiler (dev-plan/52, Tiers 1–2 — the film level).
//!
//! A pure, two-phase compiler that turns `.film` scripts into
//! single-shot Seedance payloads for the Kie.ai jobs API, plus a
//! declarative [`AssemblyPlan`] (ffmpeg work the harness executes) and
//! a T0-calibrated [`CostEstimate`]. Purity is the contract: no
//! filesystem, no network, no clock — phase 1 emits [`AssetRequest`]s
//! describing every side effect it needs (file → URL, TTS synthesis,
//! reference-video prep, frame capture), the *harness* fulfils them,
//! and phase 2 consumes the [`ResolvedAsset`]s to finish codegen. The
//! two-phase signature is what enforces purity; keep it that way.
//!
//! Language semantics are owned by `docs/filmscript-dsl-design.md`
//! (canonical spec); prompt-assembly rules were validated live against
//! Seedance 2.0 in the T0 spike (`dev-plan/52-t0-spike/FINDINGS.md`) —
//! the golden tests freeze that verified behaviour, so a diff there is
//! a semantic change, not noise. v1 defers, with clear errors: `post`
//! dialogue-sync, `@takes>1`, `@hold`, voice cloning.
//!
//! Every `say` line is TTS-first (spec v2.2): the video model never
//! generates speech; dialogue audio arrives as a reference clip and
//! the prompt asks for lip-sync to it.

pub mod ast;
pub mod backend;
pub mod harness;
mod lexer;
mod parser;
mod phase1;
mod phase2;
mod resolve;
mod validate;

pub use ast::*;
pub use backend::{BackendId, ContinuationMode, IdentityMode, VideoCaps};
pub use phase1::{
    compile_phase1, AssemblyPlan, AssetRequest, CostEstimate, MusicSpan, PartialShot, Phase1Result,
    SfxSpec, ShotCost, ShotEntity, SubtitleEntry, TransitionSpec,
};
pub use phase2::{compile_phase2, Phase2Result, ResolvedAsset, ShotPayload};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

/// Compile diagnostics are LLM repair input, not logs: `code` is stable
/// and machine-matchable, `message` mirrors the script's language (Thai
/// scripts get Thai messages) and tells the author the exact fix.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompileError {
    pub code: &'static str,
    pub severity: Severity,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shot: Option<String>,
    pub message: String,
}

impl CompileError {
    pub fn error(code: &'static str, shot: Option<&str>, message: String) -> Self {
        Self {
            code,
            severity: Severity::Error,
            shot: shot.map(str::to_string),
            message,
        }
    }
    pub fn warning(code: &'static str, shot: Option<&str>, message: String) -> Self {
        Self {
            code,
            severity: Severity::Warning,
            shot: shot.map(str::to_string),
            message,
        }
    }
}

/// Pick the diagnostic language once per script: any Thai codepoint in
/// the source → Thai messages.
pub(crate) fn msg(thai: bool, th: &str, en: &str) -> String {
    if thai {
        th.to_string()
    } else {
        en.to_string()
    }
}

pub(crate) fn is_thai_text(s: &str) -> bool {
    s.chars().any(|c| ('\u{0E00}'..='\u{0E7F}').contains(&c))
}

/// Reference-video constraints for `@continue_from` prep, from the Kie
/// API contract + T0 (shot1.mp4 at 1280×720/24fps/6s passed whole).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefVideoConstraints {
    pub max_duration_s: u32,
    pub max_bytes: u64,
    pub min_total_pixels: u64,
    pub max_total_pixels: u64,
    pub min_fps: u32,
    pub max_fps: u32,
}

impl Default for RefVideoConstraints {
    fn default() -> Self {
        Self {
            max_duration_s: 15,
            max_bytes: 50 * 1024 * 1024,
            min_total_pixels: 409_600,
            max_total_pixels: 927_408,
            min_fps: 24,
            max_fps: 60,
        }
    }
}

pub(crate) fn content_id(kind: &str, parts: &[&str]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(kind.as_bytes());
    for p in parts {
        h.update([0u8]);
        h.update(p.as_bytes());
    }
    let d = h.finalize();
    let mut s = String::with_capacity(16);
    for b in &d[..8] {
        s.push_str(&format!("{b:02x}"));
    }
    format!("{kind}-{s}")
}
