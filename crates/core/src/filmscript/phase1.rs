//! Phase 1: parse → resolve → validate, then emit the shot skeletons,
//! every [`AssetRequest`] the harness must fulfil before phase 2, the
//! declarative [`AssemblyPlan`] (assembly is ffmpeg work, never prompt
//! work), and a T0-calibrated [`CostEstimate`] so the UI can show the
//! bill before anything is generated. Asset ids are content-addressed
//! (hash of the meaningful inputs) so the harness cache survives
//! recompiles.

use super::ast::*;
use super::resolve::{mode_of, resolve, Mode, ResolvedShot, TransitionKind};
use super::validate::validate;
use super::{content_id, parser, CompileError, RefVideoConstraints};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AssetRequest {
    /// Workspace file → public URL (image assets).
    File {
        id: String,
        media_type: String,
        path: String,
    },
    /// Synthesize speech; the harness must return `duration_ms` with
    /// the fulfilled asset (E_AUDIO_OVERRUN depends on it) and pad
    /// output shorter than 2s (Kie audio-ref minimum).
    Tts {
        id: String,
        text: String,
        voice: String,
        lang: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        tone_hint: Option<String>,
    },
    /// Previous shot's clip, trimmed/downscaled to `constraints`, as a
    /// continuation reference.
    Video {
        id: String,
        source_shot: String,
        constraints: RefVideoConstraints,
    },
    /// Last frame of an earlier shot's clip (`@match_cut` →
    /// `first_frame_url`). `time_sec: -1` = final frame.
    Frame {
        id: String,
        source_shot: String,
        time_sec: i32,
    },
}

impl AssetRequest {
    pub fn id(&self) -> &str {
        match self {
            AssetRequest::File { id, .. }
            | AssetRequest::Tts { id, .. }
            | AssetRequest::Video { id, .. }
            | AssetRequest::Frame { id, .. } => id,
        }
    }
}

/// What phase 2 needs to know about one entity used by a shot —
/// self-contained so `Phase1Result` alone (plus resolved assets)
/// finishes compilation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShotEntity {
    /// Base handle (without any `#tag`).
    pub handle: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    pub kind: EntityKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub desc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time: Option<String>,
    pub asset_id: String,
    /// 1-based `@ImageN` slot.
    pub image_n: u32,
}

#[derive(Debug, Clone)]
pub struct PartialShot {
    pub(crate) resolved: ResolvedShot,
    pub entities: Vec<ShotEntity>,
    pub tts_asset: Option<String>,
    pub video_asset: Option<String>,
    pub frame_asset: Option<String>,
}

impl PartialShot {
    pub fn id(&self) -> &str {
        &self.resolved.id
    }
    /// Earlier shot whose *clip* this one needs (continuation or
    /// match-cut frame capture) — the harness DAG edge.
    pub fn depends_on(&self) -> Option<&str> {
        self.resolved
            .continue_from
            .as_deref()
            .or(self.resolved.match_cut.as_deref())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssemblyPlan {
    pub order: Vec<String>,
    /// Optical transitions only (non-cut) — generation-level
    /// continuity lives in `@continue_from`/`@match_cut`.
    pub transitions: Vec<TransitionSpec>,
    pub music: Vec<MusicSpan>,
    pub sfx: Vec<SfxSpec>,
    pub subtitles: Vec<SubtitleEntry>,
    /// Hardcode captions into the picture (vs the default toggle-able
    /// soft-muxed SRT track). From `subtitle_burn:` in the film header.
    pub subtitle_burn: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransitionSpec {
    pub after_shot: String,
    pub transition: String,
    pub sec: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MusicSpan {
    pub handle: String,
    pub path: String,
    pub from_shot: String,
    pub to_shot: String,
    pub volume: f32,
    /// Spec default; execution is deferred in v1 (constant-volume bed)
    /// but the plan carries the instruction for the harness.
    pub duck_on_dialogue: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SfxSpec {
    pub shot_id: String,
    pub handle: String,
    pub path: String,
    pub at_sec: f32,
}

/// Timestamps are the harness's job (final clip durations aren't known
/// until generation) — the compiler emits text + order for free.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubtitleEntry {
    pub shot_id: String,
    pub speaker: String,
    pub text: String,
}

/// Per-shot pricing, calibrated live in T0: fresh shot = duration ×
/// the "no video" rate; continuation = (source + added duration) × the
/// "with video" rate (Kie bills the reference seconds too — a 6s
/// continuation costs MORE than a fresh 6s shot). `generate_audio`
/// does not change the rate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostEstimate {
    pub per_shot: Vec<ShotCost>,
    pub total_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShotCost {
    pub shot_id: String,
    pub billed_seconds: u32,
    pub usd: f64,
    pub note: String,
}

fn rate_usd_per_s(fast: bool, resolution: &str, with_video: bool) -> Option<f64> {
    Some(match (fast, resolution, with_video) {
        (false, "480p", true) => 0.0575,
        (false, "480p", false) => 0.095,
        (false, "720p", true) => 0.125,
        (false, "720p", false) => 0.205,
        (false, "1080p", true) => 0.31,
        (false, "1080p", false) => 0.51,
        (false, "4k", true) => 0.64,
        (false, "4k", false) => 1.04,
        (true, "480p", true) => 0.045,
        (true, "480p", false) => 0.0775,
        (true, "720p", true) => 0.10,
        (true, "720p", false) => 0.165,
        _ => return None,
    })
}

#[derive(Debug)]
pub struct Phase1Result {
    pub shots: Vec<PartialShot>,
    pub asset_requests: Vec<AssetRequest>,
    pub assembly_plan: AssemblyPlan,
    pub cost: CostEstimate,
    pub errors: Vec<CompileError>,
    pub thai: bool,
}

impl Phase1Result {
    pub fn has_errors(&self) -> bool {
        self.errors
            .iter()
            .any(|e| e.severity == super::Severity::Error)
    }
}

pub fn compile_phase1(source: &str) -> Phase1Result {
    let (program, mut errors) = parser::parse(source);
    let (res, mut resolve_errors) = resolve(&program);
    errors.append(&mut resolve_errors);
    validate(&program, &res, &mut errors);

    let mut requests: Vec<AssetRequest> = Vec::new();
    let push_request = |req: AssetRequest, requests: &mut Vec<AssetRequest>| {
        if !requests.iter().any(|r| r.id() == req.id()) {
            requests.push(req);
        }
    };

    let mut shots = Vec::new();
    for shot in &res.shots {
        let mut entities = Vec::new();
        for (pos, u) in shot.used.iter().enumerate() {
            let d = &res.entities[u.entity].decl;
            let asset_id = content_id("file", &[&u.image_path]);
            push_request(
                AssetRequest::File {
                    id: asset_id.clone(),
                    media_type: "image".into(),
                    path: u.image_path.clone(),
                },
                &mut requests,
            );
            entities.push(ShotEntity {
                handle: d.handle.clone(),
                tag: u.tag.clone(),
                kind: d.kind,
                desc: d.desc.clone(),
                alias: d.alias.clone(),
                time: d.time.clone(),
                asset_id,
                image_n: pos as u32 + 1,
            });
        }

        // Dialogue routing (dev-plan/52 part C): DEFAULT is native — every
        // backend generates its own dialogue audio from the line in the prompt
        // (fine for short/casual lines). A TTS track is synthesized only when
        // the film opts into a post-process mode: `overlay` (onset-swap) or
        // `lipsync` (TTS + mouth re-sync). Both need the real voice track.
        let tts_asset = (shot.dialogue_overlay || shot.dialogue_lipsync)
            .then(|| {
                shot.dialogue.as_ref().and_then(|d| {
                    let voice = res
                        .entities
                        .iter()
                        .find(|e| e.decl.handle == d.speaker)
                        .and_then(|e| match &e.decl.voice {
                            Some(VoiceSpec::Registry(v)) => Some(v.clone()),
                            _ => None,
                        })?;
                    let id = content_id(
                        "tts",
                        &[&voice, &d.text, d.tone_hint.as_deref().unwrap_or("")],
                    );
                    push_request(
                        AssetRequest::Tts {
                            id: id.clone(),
                            text: d.text.clone(),
                            voice,
                            lang: d.lang.to_string(),
                            tone_hint: d.tone_hint.clone(),
                        },
                        &mut requests,
                    );
                    Some(id)
                })
            })
            .flatten();

        let video_asset = shot.continue_from.as_ref().map(|src| {
            let id = content_id("video", &[src]);
            push_request(
                AssetRequest::Video {
                    id: id.clone(),
                    source_shot: src.clone(),
                    constraints: RefVideoConstraints::default(),
                },
                &mut requests,
            );
            id
        });

        let frame_asset = shot.match_cut.as_ref().map(|src| {
            let id = content_id("frame", &[src]);
            push_request(
                AssetRequest::Frame {
                    id: id.clone(),
                    source_shot: src.clone(),
                    time_sec: -1,
                },
                &mut requests,
            );
            id
        });

        debug_assert!(
            mode_of(shot) != Mode::TextOnly || entities.is_empty(),
            "text_only shots carry no refs"
        );
        shots.push(PartialShot {
            resolved: shot.clone(),
            entities,
            tts_asset,
            video_asset,
            frame_asset,
        });
    }

    let assembly_plan = build_assembly_plan(&program, &res);
    let cost = estimate_cost(&res.shots);

    Phase1Result {
        shots,
        asset_requests: requests,
        assembly_plan,
        cost,
        errors,
        thai: program.thai,
    }
}

fn build_assembly_plan(program: &Program, res: &super::resolve::Resolution) -> AssemblyPlan {
    let order: Vec<String> = res.shots.iter().map(|s| s.id.clone()).collect();

    let transitions = res
        .shots
        .iter()
        .filter(|s| s.transition_out.0 != TransitionKind::Cut)
        .map(|s| TransitionSpec {
            after_shot: s.id.clone(),
            transition: s.transition_out.0.as_str().to_string(),
            sec: s.transition_out.1,
        })
        .collect();

    let mut music = Vec::new();
    for (seq_idx, seq) in program.sequences.iter().enumerate() {
        if let (Some(cue), Some(path)) = (
            &seq.music,
            seq.music
                .as_ref()
                .and_then(|c| res.audio.music.get(&c.handle)),
        ) {
            let seq_shots: Vec<&ResolvedShot> = res
                .shots
                .iter()
                .filter(|s| s.sequence_index == seq_idx)
                .collect();
            if let (Some(first), Some(last)) = (seq_shots.first(), seq_shots.last()) {
                music.push(MusicSpan {
                    handle: cue.handle.clone(),
                    path: path.clone(),
                    from_shot: first.id.clone(),
                    to_shot: last.id.clone(),
                    volume: cue.volume,
                    duck_on_dialogue: true,
                });
            }
        }
    }

    let sfx = res
        .shots
        .iter()
        .flat_map(|s| {
            s.sfx.iter().filter_map(|c| {
                res.audio.sfx.get(&c.handle).map(|path| SfxSpec {
                    shot_id: s.id.clone(),
                    handle: c.handle.clone(),
                    path: path.clone(),
                    at_sec: c.at_sec,
                })
            })
        })
        .collect();

    let subtitles = res
        .shots
        .iter()
        .filter(|s| s.subtitle)
        .filter_map(|s| {
            s.dialogue.as_ref().map(|d| SubtitleEntry {
                shot_id: s.id.clone(),
                speaker: d.speaker.clone(),
                text: d.text.clone(),
            })
        })
        .collect();

    AssemblyPlan {
        order,
        transitions,
        music,
        sfx,
        subtitles,
        subtitle_burn: program.header.subtitle_burn.unwrap_or(false),
    }
}

fn estimate_cost(shots: &[ResolvedShot]) -> CostEstimate {
    let mut per_shot = Vec::new();
    let mut total = 0.0;
    for shot in shots {
        let (billed, with_video, note) = match &shot.continue_from {
            Some(src) => {
                let src_dur = shots
                    .iter()
                    .find(|s| &s.id == src)
                    .map(|s| s.duration)
                    .unwrap_or(0);
                (
                    src_dur.saturating_add(shot.duration),
                    true,
                    format!(
                        "continuation: ({src_dur}s ref + {}s new) × with-video rate",
                        shot.duration
                    ),
                )
            }
            None => (
                shot.duration,
                false,
                "fresh: duration × no-video rate".to_string(),
            ),
        };
        let (usd, note) = match rate_usd_per_s(shot.fast_model, &shot.resolution, with_video) {
            Some(rate) => (((rate * billed as f64) * 10_000.0).round() / 10_000.0, note),
            None => (
                0.0,
                format!(
                    "unpriced: no rate for {}{} — verify before generating",
                    shot.resolution,
                    if shot.fast_model { " (fast)" } else { "" }
                ),
            ),
        };
        total += usd;
        per_shot.push(ShotCost {
            shot_id: shot.id.clone(),
            billed_seconds: billed,
            usd,
            note,
        });
    }
    CostEstimate {
        per_shot,
        total_usd: (total * 10_000.0).round() / 10_000.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "char $a = @./a.png voice:th-f desc:\"woman\"\n\
                       char $b = @./b.png voice:th-m desc:\"man\"\n\
                       scene $room = @./room.png time:day\n\
                       shot 1 (dialogue) {\n$a นั่งกับ $b ใน $room\n$a say \"สวัสดี\"\n@duration: 6\n}\n\
                       shot 2 {\n$b ลุกออกไป\n@continue_from: 1\n@duration: 6\n}\n";

    #[test]
    fn emits_deduped_requests_and_bindings() {
        let r = compile_phase1(SRC);
        assert!(!r.has_errors(), "{:?}", r.errors);
        let files = r
            .asset_requests
            .iter()
            .filter(|a| matches!(a, AssetRequest::File { .. }))
            .count();
        assert_eq!(files, 3);
        assert!(matches!(
            r.asset_requests.iter().find(|a| matches!(a, AssetRequest::Video { .. })),
            Some(AssetRequest::Video { source_shot, .. }) if source_shot == "1"
        ));
        assert_eq!(r.shots[0].entities.len(), 3);
        assert_eq!(r.shots[1].depends_on(), Some("1"));
    }

    #[test]
    fn cost_uses_t0_calibration() {
        let r = compile_phase1(SRC);
        // shot 1 fresh 720p: 6 × 0.205 = 1.23 (the T0-verified number)
        assert_eq!(r.cost.per_shot[0].usd, 1.23);
        // shot 2 continuation: (6+6) × 0.125 = 1.50 (T0: 300 credits)
        assert_eq!(r.cost.per_shot[1].billed_seconds, 12);
        assert_eq!(r.cost.per_shot[1].usd, 1.5);
        assert_eq!(r.cost.total_usd, 2.73);
    }

    #[test]
    fn variant_image_swaps_at_base_slot() {
        let src = "char $a = @./a.png desc:\"woman\"\nvariant $a#wet = @./a_wet.png\n\
                   shot 1 {\n$a#wet ยืนกลางฝน\n}\n";
        let r = compile_phase1(src);
        assert!(!r.has_errors(), "{:?}", r.errors);
        let e = &r.shots[0].entities[0];
        assert_eq!(e.tag.as_deref(), Some("wet"));
        assert_eq!(e.desc.as_deref(), Some("woman"));
        assert!(r
            .asset_requests
            .iter()
            .any(|a| matches!(a, AssetRequest::File { path, .. } if path == "./a_wet.png")));
    }

    #[test]
    fn match_cut_requests_frame() {
        let r = compile_phase1("shot 1 {\nx\n}\nshot 2 {\n@match_cut: 1\nempty hallway\n}\n");
        assert!(!r.has_errors(), "{:?}", r.errors);
        assert!(matches!(
            r.asset_requests.iter().find(|a| matches!(a, AssetRequest::Frame { .. })),
            Some(AssetRequest::Frame { source_shot, time_sec: -1, .. }) if source_shot == "1"
        ));
        assert_eq!(r.shots[1].depends_on(), Some("1"));
    }

    #[test]
    fn assembly_plan_covers_music_sfx_subtitles_transitions() {
        let src = "char $a = @./a.png voice:v desc:\"x\"\nmusic $m = @./m.mp3\nsfx $door = @./door.wav\n\
                   sequence \"s\" {\nmusic: $m volume:0.5\n\
                   shot 1 (dialogue) {\n$a say \"สวัสดี\"\nsfx: $door at 2s\n@transition: to_black 1.0\n}\n\
                   shot 2 {\n$a เดินออก\n@subtitle: off\n}\n}\n";
        let r = compile_phase1(src);
        assert!(!r.has_errors(), "{:?}", r.errors);
        let p = &r.assembly_plan;
        assert_eq!(p.order, vec!["1", "2"]);
        assert_eq!(p.transitions.len(), 1);
        assert_eq!(p.transitions[0].transition, "to_black");
        assert_eq!(p.music[0].from_shot, "1");
        assert_eq!(p.music[0].to_shot, "2");
        assert_eq!(p.sfx[0].at_sec, 2.0);
        assert_eq!(p.subtitles.len(), 1);
        assert_eq!(p.subtitles[0].text, "สวัสดี");
        assert!(!p.subtitle_burn, "burn defaults off (soft-mux)");
    }

    #[test]
    fn subtitle_burn_flag_flows_from_header() {
        let src = "film \"t\" {\nsubtitle_burn: on\n}\n\
                   char $a = @./a.png voice:v desc:\"x\"\n\
                   shot 1 (dialogue) {\n$a say \"สวัสดี\"\n}\n";
        let r = compile_phase1(src);
        assert!(!r.has_errors(), "{:?}", r.errors);
        assert!(r.assembly_plan.subtitle_burn, "subtitle_burn: on → burn");
    }

    #[test]
    fn same_inputs_same_asset_ids() {
        let a = compile_phase1(SRC);
        let b = compile_phase1(SRC);
        let ids_a: Vec<_> = a
            .asset_requests
            .iter()
            .map(|r| r.id().to_string())
            .collect();
        let ids_b: Vec<_> = b
            .asset_requests
            .iter()
            .map(|r| r.id().to_string())
            .collect();
        assert_eq!(ids_a, ids_b);
    }

    #[test]
    fn tone_hint_differentiates_tts_ids() {
        // TTS is only emitted in overlay mode (native is the default).
        let base = "film \"t\" {\nbackend: seedance\ndialogue_sync: overlay\n}\nchar $a = @./a.png voice:v desc:\"x\"\n";
        let plain = format!("{base}shot 1 (dialogue) {{\n$a say \"hi\"\n}}\n");
        let toned = format!("{base}shot 1 (dialogue) {{\nvoice_tone: angry\n$a say \"hi\"\n}}\n");
        let id = |src: &str| {
            compile_phase1(src)
                .asset_requests
                .iter()
                .find_map(|r| match r {
                    AssetRequest::Tts { id, .. } => Some(id.clone()),
                    _ => None,
                })
                .unwrap()
        };
        assert_ne!(id(&plain), id(&toned));
    }

    #[test]
    fn no_tts_request_without_voice() {
        let r = compile_phase1("char $a = @./a.png\nshot 1 {\n$a say \"x\"\n}\n");
        assert!(r.has_errors());
        assert!(!r
            .asset_requests
            .iter()
            .any(|a| matches!(a, AssetRequest::Tts { .. })));
    }
}
